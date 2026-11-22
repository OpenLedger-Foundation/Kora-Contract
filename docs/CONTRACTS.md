# Kora Protocol — Contract Reference

Complete reference for all public contract functions, their parameters, return values, and failure modes.

All amounts are in **stroops** (1 XLM = 10,000,000 stroops). For stablecoins like USDC on Stellar, 1 USDC = 10,000,000 units.

---

## Contract Deployment Dependency Graph (issue #1083)

The following order is enforced by `scripts/deploy.sh` and verified by CI
(`check_deploy_order.sh`). A contract must be deployed **and initialized**
before its dependents attempt to call it.

```
1. access_control
   └─► 2. parameter_registry     ← FIRST dependent; marketplace/pool read it at init
       └─► 3. invoice_nft
       └─► 3. risk_registry
       └─► 3. dispute_resolution
       └─► 3. debtor_attestation
           └─► 4. financing_pool  ← reads registry at runtime (LatePenaltyBps, MaxPositionBps …)
               └─► 5. marketplace ← reads registry at list_invoice / fund_invoice
   └─► (parallel) treasury
   └─► (parallel) vesting
   └─► (parallel) price_oracle
   └─► (parallel) governance_voting
   └─► (optional) dispute_escrow
```

**Why `parameter_registry` must come before `marketplace` and `financing_pool`:**
Both contracts call `get_value` on the registry during `list_invoice` and
`fund_invoice` to resolve governance-enacted fee and risk limits. If the
registry is not deployed when those contracts initialize, the cross-contract
call fails with a host-level contract-not-found error, leaving the contracts
in an unusable state without a redeployment. `scripts/deploy.sh` exits
non-zero if `PARAMETER_REGISTRY_ID` is empty after deployment.

**Redeploy-only path:** If the registry is already deployed and you are only
redeploying marketplace or financing_pool, verify the registry address first:

```bash
REGISTRY_ID=$(jq -r '.contracts.parameter_registry.address' deployments/<network>.json)
[ -n "$REGISTRY_ID" ] || { echo "ERROR: registry address missing from manifest"; exit 1; }
```

---


## `invoice_nft`

### `initialize(admin, access_control)`

One-time setup. Sets the admin address and the access control contract address.

| Param | Type | Description |
|-------|------|-------------|
| `admin` | `Address` | Protocol admin |
| `access_control` | `Address` | Deployed access_control contract |

Fails with `AlreadyInitialized` if called more than once.

---

### `mint_invoice(sme, debtor_hash, amount, currency, due_date, ipfs_cid, risk_score) → u64`

Mints a new invoice NFT. Returns the assigned invoice ID.

| Param | Type | Description |
|-------|------|-------------|
| `sme` | `Address` | SME wallet. Must sign. |
| `debtor_hash` | `Bytes` | SHA-256 of debtor PII. Must be 32 bytes. |
| `amount` | `i128` | Face value in stroops. Must be > 0. |
| `currency` | `Symbol` | Token symbol (e.g. `USDC`). |
| `due_date` | `u64` | Unix timestamp. Must be in the future. |
| `ipfs_cid` | `String` | IPFS CID of full invoice metadata. |
| `risk_score` | `u32` | 0–100. Assigned by verifier off-chain. |

Errors: `InvalidAmount`, `InvalidDueDate`, `InvalidRiskScore`, `EmptyString`, `ProtocolPaused`

---

### `set_listed(caller, invoice_id)`

Transitions invoice from `Created` to `Listed`. Called by the marketplace contract.

Errors: `InvoiceNotFound`, `InvalidInvoiceStatus`, `ProtocolPaused`

---

### `set_funded(caller, invoice_id)`

Transitions invoice from `Listed` to `Funded`. Called by the financing pool.

Errors: `InvoiceNotFound`, `InvalidInvoiceStatus`

---

### `set_repaid(caller, invoice_id)`

Transitions invoice from `Funded` to `Repaid`. Called by the financing pool on full repayment.

Errors: `InvoiceNotFound`, `InvalidInvoiceStatus`

---

### `set_defaulted(caller, invoice_id)`

Transitions invoice from `Funded` to `Defaulted`. Admin only. Requires `ledger.timestamp > due_date`.

Errors: `NotAdmin`, `InvoiceNotFound`, `InvalidInvoiceStatus`

---

### `get_invoice(invoice_id) → Invoice`

Returns the full invoice struct.

Errors: `InvoiceNotFound`

---

### `next_id() → u64`

Returns the next invoice ID that will be assigned.

---

## `marketplace`

### `initialize(admin, invoice_nft, financing_pool, treasury, fee_bps)`

One-time setup.

| Param | Type | Description |
|-------|------|-------------|
| `fee_bps` | `u32` | Protocol fee in basis points. Max 10,000 (100%). |

---

### `list_invoice(seller, invoice_id, asking_price, face_value, token, funding_deadline)`

Lists an invoice for financing.

| Param | Type | Description |
|-------|------|-------------|
| `seller` | `Address` | SME wallet. Must sign. |
| `asking_price` | `i128` | Discounted price investors pay. Must be < `face_value`. |
| `face_value` | `i128` | Full repayment amount. |
| `token` | `Address` | Whitelisted stablecoin contract. |
| `funding_deadline` | `u64` | Unix timestamp. Must be in the future. |

Errors: `InvalidAmount`, `InvalidDueDate`, `TokenNotWhitelisted`, `InvoiceAlreadyExists`

---

### `fund_invoice(investor, invoice_id, amount)`

Investor funds a share of the invoice.

| Param | Type | Description |
|-------|------|-------------|
| `investor` | `Address` | Investor wallet. Must sign. |
| `amount` | `i128` | Amount to contribute. Must not exceed remaining unfunded amount. |

Fee is deducted from `amount` and sent to treasury. Net is sent to financing pool.

Errors: `ListingNotFound`, `ListingAlreadyCancelled`, `FundingDeadlinePassed`, `ExceedsFundingTarget`, `InvalidAmount`

---

### `cancel_listing(caller, invoice_id)`

Cancels an active listing. Caller must be the seller or admin.

Errors: `ListingNotFound`, `ListingAlreadyCancelled`, `Unauthorized`

---

### `whitelist_token(admin, token)`

Adds a stablecoin to the whitelist. Admin only.

---

### `get_listing(invoice_id) → Listing`

Returns the listing struct.

Errors: `ListingNotFound`

---

## `financing_pool`

### `initialize(admin, invoice_nft, treasury, access_control, late_penalty_bps, price_oracle)`

One-time setup.

| Param | Type | Description |
|-------|------|-------------|
| `admin` | `Address` | Protocol admin |
| `invoice_nft` | `Address` | Deployed invoice_nft contract |
| `treasury` | `Address` | Deployed treasury contract |
| `access_control` | `Address` | Deployed access_control contract |
| `late_penalty_bps` | `u32` | Late penalty in basis points. Max 10,000 (100%). |
| `price_oracle` | `Address` | Deployed price_oracle contract for FX conversion |

---

### `release_funds(marketplace, invoice_id)`

Called by marketplace when an invoice is fully funded. Creates the pool record and transitions the NFT to `Funded`.

Errors: `PoolAlreadyClosed`, `InvoiceNotFound`

---

### `record_position(caller, invoice_id, investor, contributed, total_pool)`

Records an investor's position in the pool. Admin only (called internally).

---

### `repay(payer, invoice_id, token, amount)`

SME repays the invoice. If fully repaid, distributes yield to all investors and marks NFT as `Repaid`.

**Late penalty model:** On the first repayment call where `ledger.timestamp > invoice.due_date`, a one-time flat penalty of `bps_of(face_value, late_penalty_bps)` is added to `total_owed`. Subsequent repayments (partial or full) are tracked against `total_owed` so the penalty is never double-counted. Uses the same bps conventions as marketplace `fee_bps`.

**Yield distribution precision:** When `is_closed && should_close`, yield is distributed proportionally to each investor based on their `share_bps`. Due to integer division in basis point calculations, rounding loss is bounded to **≤ position count × 1 stroop**. For up to 50 positions, drift is ≤ 50 stroops (negligible relative to typical invoice amounts). See [PERFORMANCE.md](PERFORMANCE.md) for detailed bounds.

| Param | Type | Description |
|-------|------|-------------|
| `payer` | `Address` | Must sign. |
| `amount` | `i128` | Repayment amount in stroops. |

Errors: `PoolNotFound`, `RepaymentAlreadyMade`, `ArithmeticOverflow`

---

### `mark_default(admin, invoice_id, token)`

Admin marks an invoice as defaulted. Distributes any partial recovery to investors.

Errors: `NotAdmin`, `PoolNotFound`, `PoolAlreadyClosed`

---

### `get_pool(invoice_id) → Pool`

Returns the pool struct.

Errors: `PoolNotFound`

---

### `get_positions(invoice_id) → Vec<Position>`

Returns all investor positions for an invoice.

---

## `price_oracle`

### `initialize(admin, access_control)`

One-time setup. Sets the admin address and the access control contract address.

| Param | Type | Description |
|-------|------|-------------|
| `admin` | `Address` | Protocol admin |
| `access_control` | `Address` | Deployed access_control contract |

Fails with `AlreadyInitialized` if called more than once.

---

### `set_rate_curve(caller, points)`

Replaces the tenor-based discount-rate curve. Caller must be admin or a whitelisted verifier.

| Param | Type | Description |
|-------|------|-------------|
| `points` | `Vec<(u32, u32)>` | `(tenor_days, rate_bps)` pairs. Must be non-empty, strictly increasing in `tenor_days`, and at most `MAX_CURVE_POINTS` (16) entries. |

Stores the curve in a bounded, sorted vector and records the update timestamp for staleness checks. Rejects out-of-order or non-monotonic tenor inputs with `InvalidCurve`. Rejects curves exceeding the point bound with `TooManyCurvePoints`.

Errors: `Unauthorized`, `InvalidCurve`, `TooManyCurvePoints`, `ProtocolPaused`

---

### `rate_for_tenor(days) → u32`

Returns the discount rate in basis points for a given tenor in days.

| Param | Type | Description |
|-------|------|-------------|
| `days` | `u32` | Days to maturity. |

Uses deterministic integer linear interpolation between the two surrounding curve points. Flat extrapolation is applied at both edges: tenors at or below the first point return the first rate, and tenors at or above the last point return the last rate. No floating-point math is used.

Errors: `CurveNotSet`

---

### `get_rate_curve() → Vec<(u32, u32)>`

Returns the stored `(tenor_days, rate_bps)` curve points.

Errors: `CurveNotSet`

---

### `get_curve_updated_at() → u64`

Returns the ledger timestamp of the last curve update, for staleness checks.

---

## `treasury`

### `initialize(admin, fee_bps)`

One-time setup.

---

### `set_fee_bps(admin, fee_bps)`

Updates the protocol fee. Admin only. Max 10,000 bps.

Errors: `NotAdmin`, `InvalidFeeRate`

---

### `withdraw(admin, token, recipient, amount)`

Withdraws accumulated fees. Admin only.

Errors: `NotAdmin`, `InvalidAmount`, `InsufficientPoolBalance`

---

### `emergency_withdraw(admin, token, recipient)`

Withdraws entire token balance. Admin only.

---

### `get_fee_bps() → u32`

Returns current fee in basis points.

---

### `get_balance(token) → i128`

Returns treasury balance for a given token.

---

## `risk_registry`

### `initialize(admin)`

One-time setup.

---

### `add_verifier(admin, verifier)` / `remove_verifier(admin, verifier)`

Manage the verifier whitelist. Admin only.

---

### `register_sme(verifier, sme, risk_score)`

Verifier registers and scores an SME.

| Param | Type | Description |
|-------|------|-------------|
| `risk_score` | `u32` | 0–100. |

Errors: `NotVerifier`, `InvalidRiskScore`

---

### `update_sme_score(verifier, sme, new_score)`

Updates an existing SME's risk score. Verifier only.

Errors: `NotVerifier`, `SMENotRegistered`, `InvalidRiskScore`

---

### `record_default(admin, sme)`

Increments the default counter for an SME. Admin only.

---

### `set_debtor_score(verifier, debtor_hash, score)`

Stores a risk score for a debtor (keyed by ha

/* … truncated 5307 chars — edit only what you need near the top … */
