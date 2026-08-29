# Invoice NFT Contract

## Overview

The `invoice_nft` contract is the source of truth for every invoice in the Kora protocol. It mints invoice NFTs, owns the invoice lifecycle state machine, and is the sole authority that may advance or block status transitions.

## Status State Machine

```
Created → Listed → Funded → Repaid
                          ↘ Defaulted
```

| Transition   | Function      | Caller           |
|--------------|---------------|------------------|
| → Listed     | `set_listed`  | Marketplace      |
| → Funded     | `set_funded`  | Financing Pool   |
| → Repaid     | `set_repaid`  | Financing Pool   |
| → Defaulted  | `set_defaulted` | Admin          |

## Freeze Mechanism

### Design

Freeze enforcement is **owned internally by `invoice_nft`**, not delegated to callers. Every status-mutating function (`set_listed`, `set_funded`, `set_repaid`) calls the private `require_not_frozen` guard before executing. This provides defense-in-depth: no caller — current or future — can advance a frozen invoice's state by forgetting an external pre-check.

This is intentional and important. Earlier designs relied on external callers (e.g., `marketplace.fund_invoice`) to call `is_invoice_frozen` themselves before invoking invoice transitions. That approach is fragile: a single missed call site anywhere in the protocol silently defeats the freeze. The current design closes that class of bypass entirely.

### Admin Operations

| Function           | Who can call | Effect                                      |
|--------------------|--------------|---------------------------------------------|
| `freeze_invoice`   | Admin only   | Blocks all status transitions on the invoice |
| `unfreeze_invoice` | Admin only   | Removes the freeze; transitions resume       |
| `is_invoice_frozen`| Anyone       | Returns `true` if the invoice is frozen      |

### Error

A frozen invoice returns `KoraError::InvoiceFrozen (17)` on any attempted transition.

### Use Cases

- KYC / AML dispute on the SME or debtor
- Regulatory hold pending investigation
- Emergency administrative block

### Storage

Freeze state is stored as a `persistent` boolean under `DataKey::FrozenInvoice(invoice_id)`. The key is removed (not set to false) on unfreeze to reclaim storage.

## Error Codes

| Code | Variant                | Meaning                                  |
|------|------------------------|------------------------------------------|
| 10   | `InvoiceNotFound`      | No invoice exists for the given ID       |
| 11   | `InvoiceAlreadyExists` | Duplicate invoice ID in marketplace      |
| 12   | `InvalidInvoiceStatus` | Transition not allowed from current state |
| 13   | `InvoiceExpired`       | Invoice past due date                    |
| 14   | `InvalidAmount`        | Zero or negative amount                  |
| 15   | `InvalidDueDate`       | Due date not in the future               |
| 16   | `InvalidRiskScore`     | Risk score out of 0–100 range            |
| 17   | `InvoiceFrozen`        | Invoice is administratively frozen       |

The `invoice_nft` contract is the canonical source of truth for all invoice state in the Kora Protocol. Each invoice is represented as an immutable NFT with a unique ID, capturing all financial and metadata details of the underlying invoice.

## Invoice NFT Data Model

### Invoice Structure

```rust
pub struct Invoice {
    pub id: u64,                        // Unique invoice ID
    pub sme: Address,                   // SME (seller/borrower) address
    pub debtor_hash: Bytes,             // SHA-256 hash of debtor PII (never stored plaintext)
    pub amount: i128,                   // Invoice amount in base units
    pub currency: Symbol,               // Token symbol (e.g., "USDC", "EURC")
    pub due_date: u64,                  // Unix timestamp when invoice is due
    pub ipfs_cid: String,               // IPFS content hash for full invoice metadata
    pub metadata_hash: Bytes,           // SHA-256 content commitment of the off-chain document (empty until committed)
    pub risk_score: u32,                // Risk score 0–100 (assigned by verifiers)
    pub risk_tier: RiskTier,            // Risk tier (AAA, AA, A, B, C) derived from score
    pub status: InvoiceStatus,          // Current status in the state machine
    pub created_at: u64,                // Unix timestamp when invoice was minted
    pub funded_at: Option<u64>,         // Unix timestamp when fully funded (None until funded)
    pub repaid_at: Option<u64>,         // Unix timestamp when fully repaid (None until repaid)
}
```

### Invoice Status Lifecycle

```
             ┌─────────┐
             │ Created │  ← mint_invoice()
             └────┬────┘
                  │ set_listed() [marketplace auth]
             ┌────▼────┐
             │ Listed  │
             └────┬────┘
                  │ set_funded() [financing_pool auth]
             ┌────▼────┐
             │ Funded  │
             └────┬────┘
      ┌──────────┴──────────┐
      │ set_repaid()        │ set_defaulted()
      │ [pool auth]         │ [admin auth + past due_date]
 ┌────▼────┐          ┌────▼──────┐
 │ Repaid  │          │ Defaulted │
 └─────────┘          └───────────┘
```

**Key Invariants:**
- Invoices can only move forward through the state machine (no backward transitions)
- Status changes are strictly ordered and enforced by authorization checks
- Only specific callers can trigger each transition (marketplace, financing pool, admin)
- `Repaid` and `Defaulted` are terminal states

### Risk Tiers

Risk tiers are derived from the risk score (0–100) assigned by verifiers:

| Risk Score Range | Tier | Interpretation |
|------------------|------|----------------|
| 0–20 | AAA | Lowest risk, highest credit quality |
| 21–40 | AA | High credit quality |
| 41–60 | A | Good credit quality |
| 61–80 | B | Adequate credit quality |
| 81–100 | C | Speculative / higher risk |

## Public API Surface

### Initialization

```rust
pub fn initialize(env: Env, admin: Address, access_control: Address) -> Result<(), KoraError>
```

**Purpose:** One-time initialization of the contract.

**Parameters:**
- `env` — Soroban environment
- `admin` — Address to designate as the contract admin
- `access_control` — Address of the access control contract (for pause checks)

**Returns:** `Ok(())` on success, or `KoraError::AlreadyInitialized` if already initialized.

**Authorization:** None required (one-time setup).

**Storage Initialization:**
- `Admin` is set
- `AccessControl` contract address is stored
- `NextId` is initialized to 1
- `InvoiceCount` is initialized to 0

---

### Minting

```rust
pub fn mint_invoice(
    env: Env,
    sme: Address,
    debtor_hash: Bytes,
    amount: i128,
    currency: Symbol,
    due_date: u64,
    ipfs_cid: String,
    risk_score: u32,
) -> Result<u64, KoraError>
```

**Purpose:** Create a new invoice NFT.

**Parameters:**
- `env` — Soroban environment
- `sme` — Address of the SME (seller/borrower)
- `debtor_hash` — SHA-256 hash of debtor PII (32 bytes, never plaintext)
- `amount` — Invoice amount in base units (e.g., cents for USDC)
- `currency` — Token symbol for the invoice (e.g., "USDC")
- `due_date` — Unix timestamp when payment is due (must be in the future)
- `ipfs_cid` — IPFS content hash for full invoice metadata (encrypted, access-controlled by SME)
- `risk_score` — Risk assessment score (0–100) from a verifier

**Returns:** The newly allocated invoice ID, or an error.

**Errors:**
- `KoraError::ArithmeticOverflow` if amount > i128::MAX / 2 or ID counter overflows
- `KoraError::ProtocolPaused` if the protocol is paused
- `KoraError::SMENotVerified` if a risk_registry is configured and `sme` is not verified — see [Minting Rules](#minting-rules)
- `KoraError::ComplianceNotAttested` if a risk_registry is configured and `sme` lacks a compliance attestation — see [Minting Rules](#minting-rules)
- `KoraError::InvalidInput` if:
  - `amount <= 0`
  - `due_date <= current_time` (must be in the future)
  - `risk_score > 100`
  - `debtor_hash` is empty (0 bytes)
  - `ipfs_cid` is empty

**Authorization:** Requires `sme.require_auth()`.

**Security:**
- Validates all inputs before state changes
- Uses checked arithmetic for ID allocation
- Emits `invoice_created` event with ID, SME, and amount
- Invoice is stored in persistent storage with TTL managed by the protocol operator

---

### Batch Minting

```rust
pub fn mint_invoices_batch(
    env: Env,
    sme: Address,
    invoices: Vec<BatchInvoiceInput>,
) -> Result<Vec<u64>, KoraError>
```

**Purpose:** Create multiple invoice NFTs in a single transaction, with atomic validation (all-or-nothing semantics).

**Parameters:**
- `env` — Soroban environment
- `sme` — Address of the SME (must be the same for all invoices in the batch)
- `invoices` — Vector of `BatchInvoiceInput` structs (maximum **25 invoices**)

**Batch Size Limit:**
- Maximum batch size is **`MAX_BATCH_MINT_SIZE = 25`**
- This limit is enforced before any validation or storage writes occur
- Batches exceeding 25 invoices are rejected immediately with `KoraError::BatchSizeExceeded`
- The limit is conservatively chosen based on measured resource cost per invoice:
  - ~50K CPU instructions for persistent storage write
  - ~5K CPU instructions for event emission
  - ~10K CPU instructions for TTL bump
  - Total: ~65K CPU per invoice × 25 = ~1.625M (safe margin under Soroban's ~80M CPU budget)

**Rationale:**
- Prevents transactions from exceeding Soroban's CPU, memory, and ledger-write resource limits
- Provides a stable, documented maximum that enables predictable client-side tooling
- Allows reasonable batch sizes while preserving headroom for other middleware

**Returns:** A vector of newly allocated invoice IDs (in order), or an error.

**Errors:**
- `KoraError::BatchSizeExceeded` if `invoices.len() > 25` (fast-fail, before any validation)
- `KoraError::ProtocolPaused` if the protocol is paused
- Validation errors (applied to each invoice in the batch):
  - `KoraError::InvalidAmount` if any invoice has `amount <= 0` or `amount > i128::MAX / 2`
  - `KoraError::InvalidDueDate` if any invoice has `due_date <= current_time`
  - `KoraError::InvalidRiskScore` if any invoice has `risk_score > 100`
  - `KoraError::EmptyBytes` if any invoice has an empty `debtor_hash`
  - `KoraError::FieldTooLong` if any invoice has a `debtor_hash` longer than 64 bytes or `ipfs_cid` longer than 128 bytes
  - `KoraError::EmptyString` if any invoice has an empty `ipfs_cid`

**Atomicity:**
- All inputs are validated **before** any storage writes
- If any validation fails, the entire batch is aborted (no invoices are stored)
- `next_id` is only updated after all invoices are successfully stored

**Authorization:** Requires `sme.require_auth()`.

**Security:**
- Validates all inputs before state changes
- Uses checked arithmetic for ID allocation
- Each invoice emits an `invoice_created` event with ID, SME, and amount
- All invoices are stored in persistent storage with TTL managed by the protocol operator
- Batch size limit prevents resource exhaustion

**Example:**

```javascript
// Client-side: batch up to 25 invoices
const batch = [];
for (let i = 0; i < 25; i++) {
  batch.push({
    debtor_hash: Buffer.from(...), // SHA-256 hash
    amount: 10_000_000i128,        // In stroops
    currency: "USDC",
    due_date: Math.floor(Date.now() / 1000) + 86_400 * 30,
    ipfs_cid: "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
    risk_score: 50,
    notes: "Batch invoice",
  });
}
const ids = await nftContract.mint_invoices_batch(smeMAddress, batch);
console.log(`Created ${ids.length} invoices`);
```

---

### Metadata Integrity

#### commit_metadata_hash

```rust
pub fn commit_metadata_hash(
    env: Env,
    sme: Address,
    invoice_id: u64,
    metadata_hash: Bytes,
) -> Result<(), KoraError>
```

`ipfs_cid` only commits to a content identifier, not to the bytes a gateway actually serves —
some pinning setups allow the content behind a CID to change. `commit_metadata_hash` binds the
invoice on-chain to the **SHA-256 of the canonical off-chain metadata document**, giving a
tamper-evident anchor that survives any gateway-side mutation.

**Semantics:**
- **Write-once.** The hash can only be set while it is empty and the invoice is still in
  `Created` status. After commitment it is immutable.
- Only the invoice's `sme` may commit it (`Unauthorized` otherwise).
- Empty hashes are rejected (`InvalidInput`); a second commit returns `AlreadyInitialized`;
  committing after the invoice leaves `Created` returns `InvalidInvoiceStatus`.

**Off-chain verification guidance:**

1. Compute the canonical document bytes deterministically (e.g. sorted-key JSON, UTF-8, no
   trailing whitespace) — the same canonicalization the SME used when committing.
2. Fetch the document from IPFS using `ipfs_cid`.
3. Hash the fetched bytes with SHA-256.
4. Read the on-chain invoice (`get_invoice`) and compare your digest against `metadata_hash`.
   A mismatch means the served content was tampered with and must be rejected.

```bash
# Example: verify a fetched document against the on-chain commitment
sha256sum invoice-metadata.json        # -> compare hex against invoice.metadata_hash
```

A `metadata_hash` of length 0 means no commitment was made for that invoice.

#### Dispute mechanism (`flag_metadata_mismatch` / `resolve_metadata_dispute`)

Verifying a `metadata_hash` commitment (above) is an off-chain, manual process. If a
third party — an investor, verifier, or auditor — performs that verification and finds
a mismatch, they can flag it on-chain:

```rust
pub fn flag_metadata_mismatch(
    env: Env,
    challenger: Address,
    invoice_id: u64,
    evidence_hash: Bytes,
) -> Result<(), KoraError>

pub fn resolve_metadata_dispute(
    env: Env,
    admin: Address,
    invoice_id: u64,
    upheld: bool,
) -> Result<(), KoraError>
```

- **Any address** may call `flag_metadata_mismatch`, supplying the SHA-256 they
  independently computed from the fetched document as `evidence_hash`. This requires
  the invoice to already have a committed `metadata_hash` (`InvalidInvoiceStatus`
  otherwise) and **automatically freezes the invoice** (blocking `fund_invoice`/`repay`)
  pending admin review. Emits `metadata_mismatch_flagged`.
- **Anti-griefing:** at most one dispute may ever be raised per invoice. Once an admin
  resolves it (upheld or rejected) via `resolve_metadata_dispute`, the invoice cannot be
  disputed again through this path — a false accusation cannot be repeatedly relitigated,
  and a confirmed one doesn't need to be. `AlreadyInitialized` is returned for a second
  attempt, whether the prior dispute is still open or already resolved.
- **Admin resolution:** `upheld = true` confirms the fraud and leaves the invoice frozen
  (the admin is expected to follow up via other governance/legal channels). `upheld =
  false` clears the dispute and unfreezes the invoice, emitting `invoice_unfrozen` and
  `metadata_dispute_resolved` in both cases.

#### Admin correction (`admin_correct_metadata_hash`)

Distinct from the dispute mechanism above, this addresses an SME's own **honest**
mistake (e.g. committing the hash of the wrong file version) rather than third-party
fraud detection:

```rust
pub fn admin_correct_metadata_hash(
    env: Env,
    admin: Address,
    invoice_id: u64,
    new_hash: Bytes,
) -> Result<(), KoraError>
```

Admin-only, and restricted to `status == Created` — identical to the guard on
`commit_metadata_hash` and `amend_invoice` — so no investor could possibly have relied
on the original (incorrect) commitment. Works whether or not a hash was already
committed. Emits a distinctly-named `metadata_hash_corrected` event carrying both the
old and new hash, and records a structured `AdminAuditEntry` (`AdminActionType::
CorrectMetadataHash`, `AuditSource::InvoiceNft`, readable via `get_audit_log`) — this
admin override is never conflated with the SME's own original commitment in the audit
trail, precisely because it is a deliberate exception to a stated immutability guarantee.

---

### State Transitions

#### set_listed

```rust
pub fn set_listed(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError>
```

**Purpose:** Transition invoice from `Created` → `Listed`.

**Parameters:**
- `env` — Soroban environment
- `caller` — The caller's address (must be the marketplace contract)
- `invoice_id` — ID of the invoice to list

**Returns:** `Ok(())` on success, or an error.

**Errors:**
- `KoraError::ProtocolPaused` if the protocol is paused
- `KoraError::InvoiceNotFound` if invoice does not exist
- `KoraError::InvalidInvoiceStatus` if invoice is not in `Created` status

**Authorization:** Requires `caller.require_auth()` (implicitly requires the marketplace contract).

**Security:** Only the marketplace contract (as verified at initialization) can list invoices.

---

#### set_funded

```rust
pub fn set_funded(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError>
```

**Purpose:** Transition invoice from `Listed` → `Funded`.

**Parameters:**
- `env` — Soroban environment
- `caller` — The caller's address (must be the financing pool contract)
- `invoice_id` — ID of the invoice to mark as funded

**Returns:** `Ok(())` on success, or an error.

**Errors:**
- `KoraError::ProtocolPaused` if the protocol is paused
- `KoraError::InvoiceNotFound` if invoice does not exist
- `KoraError::InvalidInvoiceStatus` if invoice is not in `Listed` status

**Authorization:** Requires `caller.require_auth()` (implicitly requires the financing pool contract).

**Side Effects:** Records the `funded_at` timestamp.

---

#### set_repaid

```rust
pub fn set_repaid(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError>
```

**Purpose:** Transition invoice from `Funded` → `Repaid`.

**Parameters:**
- `env` — Soroban environment
- `caller` — The caller's address (must be the financing pool contract)
- `invoice_id` — ID of the invoice to mark as repaid

**Returns:** `Ok(())` on success, or an error.

**Errors:**
- `KoraError::InvoiceNotFound` if invoice does not exist
- `KoraError::InvalidInvoiceStatus` if invoice is not in `Funded` status

**Authorization:** Requires `caller.require_auth()` (implicitly requires the financing pool contract).

**Side Effects:** Records the `repaid_at` timestamp. Emits `invoice_repaid` event.

**Note:** This function does NOT check the pause flag — SMEs can always repay.

---

#### set_defaulted

```rust
pub fn set_defaulted(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError>
```

**Purpose:** Transition invoice from `Funded` → `Defaulted` (used after due date passes).

**Parameters:**
- `env` — Soroban environment
- `caller` — The caller's address (must be the admin)
- `invoice_id` — ID of the invoice to mark as defaulted

**Returns:** `Ok(())` on success, or an error.

**Errors:**
- `KoraError::NotAdmin` if caller is not the admin
- `KoraError::InvoiceNotFound` if invoice does not exist
- `KoraError::InvalidInvoiceStatus` if invoice is not in `Funded` status or due date hasn't passed

**Authorization:** Requires `caller.require_auth()` (implicitly requires the admin).

**Conditions:**
- Current timestamp must be **after** the invoice's `due_date`
- Fails if called before the due date (even by admin)

**Security:** Admin-only to prevent accidental or malicious defaults.

---

### Views

```rust
pub fn get_invoice(env: Env, invoice_id: u64) -> Result<Invoice, KoraError>
```

**Purpose:** Retrieve a full invoice by ID.

**Returns:** The complete `Invoice` struct, or `KoraError::InvoiceNotFound` if not found.

**Security:** No authorization check (public view).

---

```rust
pub fn next_id(env: Env) -> u64
```

**Purpose:** Get the next invoice ID that will be allocated.

**Returns:** The ID of the next invoice to be minted (starting at 1).

**Security:** No authorization check (public view).

---

```rust
pub fn invoice_count(env: Env) -> u64
```

**Purpose:** Get the total count of invoices minted.

**Returns:** The cumulative number of invoices created on this contract.

**Security:** No authorization check (public view).

---

```rust
pub fn reconcile_outstanding_exposure(env: Env, sme: Address) -> i128
```

**Purpose:** Independently recompute an SME's aggregate exposure from the underlying
invoice set (summing `amount` across `Created`/`Listed`/`Funded` invoices), bypassing
the cached `OutstandingExposure` counter used by `get_outstanding_exposure`.

**Returns:** The recomputed total. Should always equal `get_outstanding_exposure(sme)` —
a mismatch indicates a bug in one of the five exposure-adjusting code paths (`mint_invoice`,
`mint_invoices_batch`, `amend_invoice`, `withdraw_invoice`, `set_repaid`, `set_defaulted`).

**Security:** No authorization check (public view). Scans every allocated invoice ID
(bounded by `next_id`) — there is no SME-to-invoice index yet, so cost is linear in the
total number of invoices minted on the contract, not just this SME's.

---

## Protocol Configuration (`ProtocolConfig`)

```rust
pub fn set_protocol_config(env: Env, admin: Address, config: ProtocolConfig) -> Result<(), KoraError>
pub fn get_protocol_config(env: Env) -> ProtocolConfig
```

`kora_shared::types::ProtocolConfig` is a shared struct — `fee_bps`, `late_penalty_bps`,
`max_risk_score`, `min_funding_period` — defined for protocol-wide use but historically
unused by any contract. `invoice_nft` is the first adopter: `set_protocol_config`
(admin-only) stores it, and `mint_invoice`/`mint_invoices_batch` enforce `max_risk_score`
as an additional ceiling on top of the fixed 0–100 range `require_valid_risk_score`
already checks — letting the protocol tighten its risk appetite (e.g. rejecting anything
above `risk_score: 70`) without a contract upgrade.

**Defaults:** an unconfigured contract behaves exactly as before — `max_risk_score`
defaults to 100 (no additional restriction) and `fee_bps`/`late_penalty_bps`/
`min_funding_period` default to 0.

**Out of scope:** `fee_bps`, `late_penalty_bps`, and `min_funding_period` are stored in
`ProtocolConfig` for forward-compatibility but are **not** read anywhere in this contract.
They remain owned by `treasury`/`financing_pool`'s own local parameters; wiring them into
this shared struct is follow-up work.

---

## Minting Rules

1. **Who can mint?** Any address can call `mint_invoice()`, but **must sign the transaction** (via `sme.require_auth()`)
   - Typically, this is the SME themself or a trusted agent with their signing key

2. **What are the constraints?**
   - Amount must be > 0
   - Amount must not exceed i128::MAX / 2 (to prevent arithmetic overflow in fees/yields)
   - Due date must be in the future (> current block timestamp)
   - Risk score must be 0–100 (typically assigned by a verifier)
   - Debtor hash must be non-empty (32-byte SHA-256 hash)
   - IPFS CID must be non-empty, ≤128 bytes, and **structurally valid** — either a CIDv0
     (`Qm` prefix, exactly 46 base58btc characters) or a CIDv1 (multibase-prefixed with
     `b`/`B`/`z`/`f`/`F`/`u`/`U`/`k`/`K`, ≥10 characters). A garbage string such as
     `"not-a-cid"` is rejected with `KoraError::InvalidCid` — this is enforced structurally
     on-chain (see `kora_shared::validation::require_valid_ipfs_cid`), not merely a length
     check, at `mint_invoice`, `mint_invoices_batch` (per item), and `amend_invoice`. This
     is **not retroactive**: invoices minted before this validation was wired in and whose
     CID happens not to conform are left as-is; the check only gates new mints and amends
     going forward.
   - If a `risk_registry` is wired up (`set_risk_registry`) and the SME has a non-zero
     `credit_limit` on their `SmeProfile`, the mint (or batch, or amend) is rejected with
     `KoraError::CreditLimitExceeded` whenever it would push the SME's aggregate
     `OutstandingExposure` — the sum of `amount` across all of that SME's non-terminal
     (not `Repaid`/`Defaulted`) invoices — over `credit_limit`. `OutstandingExposure` itself
     is tracked for every SME regardless of whether a registry is wired up (it is
     incremented on mint and decremented on withdraw/repay/default), and adjusted by the
     delta when `amend_invoice` changes `amount`. `mint_invoices_batch` checks the whole
     batch's cumulative amount against a single running total before writing anything, so a
     batch that individually fits per item but collectively exceeds `credit_limit` is
     rejected in its entirety (atomic-abort).

3. **Verification and compliance gating.** When `set_risk_registry()` has been
   called (admin-only, post-deployment), `mint_invoice()` and
   `mint_invoices_batch()` additionally require, *before any storage write*:
   - `sme` is `verified` in the risk_registry's `SmeProfile` — otherwise
     `KoraError::SMENotVerified`. In practice this means `sme` must have been
     registered via `risk_registry.register_sme()` by a verifier; there is
     currently no code path that registers an SME as unverified.
   - `sme.compliance_attested == true` — otherwise `KoraError::ComplianceNotAttested`.

   **This is enforced at two lifecycle stages, intentionally (defense in depth):**
   - **Mint time** (`invoice_nft.mint_invoice` / `mint_invoices_batch`) — closes the
     window where a non-compliant or unverified SME could otherwise mint an
     on-chain invoice (with real metadata, notes, and `invoice_created` events)
     that only gets rejected much later, at listing.
   - **Listing time** (`marketplace.list_invoice`, via `require_compliance_attested`)
     — kept as an independent, second gate. It protects against `invoice_nft` and
     `marketplace` being wired to *different* `risk_registry` deployments, and
     against invoices minted before `invoice_nft.set_risk_registry()` was ever
     called (mint-time gating is a no-op with no registry configured). Note
     marketplace only re-checks `compliance_attested`, not `verified` — `verified`
     is invoice_nft-only, mint-time-only enforcement.

   If no risk_registry has been configured on `invoice_nft`, the verified/compliance
   checks are skipped entirely — an explicit backward-compatibility no-op, not a
   silent bypass. Production deployments **must** call `set_risk_registry` for
   these checks to be enforced.

4. **NFT Immutability**
   - Once minted, the following fields **never change:**
     - `id`, `sme`, `debtor_hash`, `amount`, `currency`, `due_date`, `ipfs_cid`, `risk_score`, `risk_tier`, `created_at`
   - Only the following fields can change:
     - `status` (via state transitions)
     - `funded_at` (set when transitioned to `Funded`)
     - `repaid_at` (set when transitioned to `Repaid`)

---

## Transfer Rules

Invoice NFTs are **not transferable** in this version of the protocol. Each invoice is permanently associated with its SME creator. This simplification:
- Prevents fund theft through illicit NFT transfers
- Maintains a clear audit trail of who minted each invoice
- Avoids the complexity of tracking beneficial ownership vs. NFT holder

Future versions may allow transfers with strict controls (e.g., only to other SMEs in a whitelist, or only with admin approval).

---

## Cross-Contract Call Paths

### marketplace → invoice_nft

```
marketplace.list_invoice(invoice_id)
  └── invoice_nft.set_listed(marketplace_address, invoice_id)
       └─ Validates invoice exists and status is Created
       └─ Transitions to Listed
```

### financing_pool → invoice_nft

```
financing_pool.release_funds(invoice_id)
  └── invoice_nft.set_funded(pool_address, invoice_id)
       └─ Validates invoice exists and status is Listed
       └─ Sets funded_at timestamp
       └─ Transitions to Funded

financing_pool.complete_repayment(invoice_id, ...)
  └── invoice_nft.set_repaid(pool_address, invoice_id)
       └─ Validates invoice exists and status is Funded
       └─ Sets repaid_at timestamp
       └─ Transitions to Repaid
       └─ Emits invoice_repaid event
```

### admin → invoice_nft

```
admin calls invoice_nft.set_defaulted(admin_address, invoice_id)
  └─ Validates invoice exists and status is Funded
  └─ Requires current_time > due_date
  └─ Transitions to Defaulted
  └─ Emits invoice_defaulted event
```

---

## Security Considerations

### 1. Debtor Privacy
- Debtor personally identifiable information (name, address, tax ID) is **never stored on-chain**
- Only a SHA-256 hash (`debtor_hash`) is stored as a privacy-preserving identifier
- Full metadata is stored on IPFS, encrypted and access-controlled by the SME
- This keeps on-chain data minimal and protects debtor privacy

### 2. Authorization
- **Minting:** SME must sign the transaction (`sme.require_auth()`)
- **set_listed:** Marketplace contract must sign (cross-contract call verification)
- **set_funded:** Financing pool contract must sign
- **set_repaid:** Financing pool contract must sign
- **set_defaulted:** Admin must sign AND invoice must be past due date

### 3. Immutability
- Core invoice fields (amount, due date, risk score) are **immutable after creation**
- Only status and timestamps can change (via controlled state transitions)
- This prevents silent modifications that would invalidate the invoice

### 4. Pause Enforcement
- `mint_invoice()`, `set_listed()`, and `set_funded()` revert if protocol is paused
- `set_repaid()` does **NOT** check pause flag — SMEs can always repay
- `set_defaulted()` does **NOT** check pause flag — defaults can be marked even if paused

### 5. Arithmetic Safety
- Amount validation prevents overflow: `amount > i128::MAX / 2` → error
- ID counter uses `checked_add()` to detect overflow
- Invoice count uses `checked_add()` to detect overflow

### 6. State Machine Enforced
- No backward transitions (e.g., cannot go from `Funded` → `Listed`)
- Cannot skip states (e.g., cannot go directly from `Created` → `Funded`)
- All transitions are validated by the receiving contract

### 7. Re-entrancy
- Soroban's synchronous execution model prevents classic reentrancy
- All state changes happen before cross-contract calls (checks-effects-interactions)

---

## Known Limitations (v1)

### Single Admin for Defaults
- Only the admin can mark invoices as defaulted
- No automated default detection (keeper network planned for v2)
- Manual intervention required after due date

### No Secondary Market
- Invoices cannot be traded or transferred
- Investors are locked in once they fund an invoice
- Secondary market support planned for v2

### No Oracle
- Invoice amounts and due dates are self-reported by SMEs
- No on-chain verification that the underlying invoice is real
- Mitigated off-chain by the verifier network's KYC/KYB checks

### TTL Management (Fixed in v1.1)
- **[FIXED]** Invoice storage entries now have their TTL extended on all state transitions:
  - `mint_invoice()`, `mint_invoices_batch()`, `amend_invoice()`
  - `set_listed()`, `set_funded()`, `set_repaid()`, `set_defaulted()`
  - `commit_metadata_hash()`
- Previously, `set_repaid()` and `commit_metadata_hash()` did not refresh the TTL, leaving repaid invoices (terminal state, rarely touched) vulnerable to expiry
- TTL extension now uses unified shared constants (`DEFAULT_TTL_THRESHOLD`, `DEFAULT_TTL_BUMP`) from `kora_shared::validation::extend_persistent_ttl`
- Protocol operator should still monitor persistent storage to ensure TTL stays healthy

### No Signature Delegation
- Only the SME can mint their own invoices (no delegation mechanism)
- Future versions may support signed delegation for agents
