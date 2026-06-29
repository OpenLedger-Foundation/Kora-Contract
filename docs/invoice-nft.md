# Invoice NFT Contract

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

## Minting Rules

1. **Who can mint?** Any address can call `mint_invoice()`, but **must sign the transaction** (via `sme.require_auth()`)
   - Typically, this is the SME themself or a trusted agent with their signing key

2. **What are the constraints?**
   - Amount must be > 0
   - Amount must not exceed i128::MAX / 2 (to prevent arithmetic overflow in fees/yields)
   - Due date must be in the future (> current block timestamp)
   - Risk score must be 0–100 (typically assigned by a verifier)
   - Debtor hash must be non-empty (32-byte SHA-256 hash)
   - IPFS CID must be non-empty (pointer to encrypted invoice metadata)

3. **NFT Immutability**
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

### TTL Management
- Invoice NFT storage entries expire if TTL is not extended
- Protocol operator or keeper bot must periodically extend TTL
- Failure to do so could result in invoice data loss

### No Signature Delegation
- Only the SME can mint their own invoices (no delegation mechanism)
- Future versions may support signed delegation for agents
