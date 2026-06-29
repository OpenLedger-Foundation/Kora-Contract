# Kora Protocol — Contract Reference

Complete reference for all public contract functions, their parameters, return values, and failure modes.

All amounts are in **stroops** (1 XLM = 10,000,000 stroops). For stablecoins like USDC on Stellar, 1 USDC = 10,000,000 units.

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

Stores a risk score for a debtor (keyed by hash). Verifier only.

---

### `get_sme_profile(sme) → SmeProfile`

Returns the full SME profile.

Errors: `SMENotRegistered`

---

### `is_verified_sme(sme) → bool`

Returns `true` if the SME is registered and verified.

---

### `is_verifier(verifier) → bool`

Returns `true` if the address is a whitelisted verifier.

---

## `access_control`

### `initialize(admin)`

One-time setup. Grants `Role::Admin` to the admin address.

---

### `pause(admin)` / `unpause(admin)`

Toggle the protocol pause state. Admin only.

---

### `grant_role(admin, target, role)` / `revoke_role(admin, target)`

Manage roles. Admin only.

Roles: `Admin`, `Operator`, `Verifier`, `None`

---

### `transfer_admin(current_admin, new_admin)`

Transfers admin rights. Current admin must sign.

---

### `is_paused() → bool`

Returns the current pause state.

---

### `get_role(address) → Role`

Returns the role assigned to an address.

---

### `get_admin() → Address`

Returns the current admin address.

Errors: `NotInitialized`

---

## `price_oracle`

Mock/testnet-compatible price oracle for cross-currency conversion. Prices are stored as stroops-scaled values (1e7 = 1.0). The oracle rejects reads of prices older than 3600 seconds (1 hour) to prevent stale-price exploits.

### `initialize(admin)`

One-time setup.

---

### `set_price(admin, base, quote, price)`

Set the exchange rate for a currency pair. Admin only. Price is `base` per 1 `quote`, scaled by 1e7.

| Param | Type | Description |
|-------|------|-------------|
| `base` | `Symbol` | Base currency symbol (e.g. `EURC`) |
| `quote` | `Symbol` | Quote currency symbol (e.g. `USDC`) |
| `price` | `i128` | Exchange rate scaled by 1e7 |

Errors: `NotAdmin`, `InvalidAmount`

---

### `get_price(base, quote) → PriceData`

Returns the price and its timestamp. Fails if the price is stale (> 1 hour) or missing.

Errors: `InvalidAmount` (missing), `InvoiceExpired` (stale)

---

### `convert(amount, from, to) → i128`

Convert an amount between currencies. Returns the same amount if `from == to`. Uses `get_price` internally, so it inherits staleness checks.

Errors: `InvalidAmount`, `InvoiceExpired`, `ArithmeticOverflow`

---

## Error Code Reference

All `KoraError` variants are `#[repr(u32)]`. The numeric values are stable on-chain and must not be changed without a documented migration.

| Code | Variant | Description |
|------|---------|-------------|
| 1 | `Unauthorized` | Caller lacks permission for this action |
| 2 | `NotAdmin` | Caller is not the protocol admin |
| 3 | `NotVerifier` | Caller is not a whitelisted verifier |
| 4 | `ProtocolPaused` | Operation blocked while protocol is paused |
| 5 | `AlreadyPaused` | Protocol is already in paused state |
| 6 | `NotPaused` | Protocol is not currently paused |
| 7 | `RoleNotAssigned` | No role is assigned to the given address |
| 10 | `InvoiceNotFound` | Invoice ID does not exist |
| 11 | `InvoiceAlreadyExists` | Invoice ID is already in use |
| 12 | `InvalidInvoiceStatus` | Invoice is in the wrong state for this transition |
| 13 | `InvoiceExpired` | Invoice due date has passed |
| 14 | `InvalidAmount` | Amount is zero, negative, or out of range |
| 15 | `InvalidDueDate` | Due date is not strictly in the future |
| 16 | `InvalidRiskScore` | Risk score is outside the 0–100 range |
| 20 | `ListingNotFound` | Listing for the given invoice ID does not exist |
| 21 | `ListingAlreadyCancelled` | Listing has already been cancelled |
| 22 | `ListingExpired` | Listing funding deadline has passed |
| 23 | `FundingDeadlinePassed` | Funding deadline has elapsed |
| 24 | `InsufficientFunds` | Caller does not have enough balance |
| 25 | `ExceedsFundingTarget` | Contribution would exceed the asking price |
| 26 | `AlreadyFullyFunded` | Invoice has already reached its funding target |
| 30 | `PoolNotFound` | Financing pool for the given invoice ID does not exist |
| 31 | `PoolAlreadyClosed` | Pool has already been closed (repaid or defaulted) |
| 32 | `RepaymentAlreadyMade` | A repayment is already in progress (reentrancy lock) |
| 33 | `InsufficientPoolBalance` | Pool balance is too low for the requested withdrawal |
| 40 | `InvalidFeeRate` | Fee rate exceeds 10 000 bps or violates range constraint |
| 41 | `WithdrawalFailed` | Token transfer during withdrawal failed |
| 42 | `TokenNotWhitelisted` | Token is not on the treasury whitelist |
| 50 | `SMENotRegistered` | SME address has not been registered in the risk registry |
| 51 | `DebtorNotRegistered` | Debtor hash has no recorded score |
| 52 | `RiskScoreOutOfRange` | Score is outside 0–100 |
| 90 | `ArithmeticOverflow` | A checked arithmetic operation overflowed |
| 91 | `ArithmeticUnderflow` | A checked subtraction produced a negative result |
| 92 | `InvalidAddress` | Address is self-referential or must be distinct from another |
| 93 | `EmptyString` | A required string field is empty |
| 94 | `AlreadyInitialized` | Contract has already been initialized |
| 96 | `NotInitialized` | Contract has not been initialized yet |
| 97 | `EmptyBytes` | A required bytes field is empty |
| 98 | `Reentrancy` | Reentrancy guard triggered — concurrent call detected |
| 100 | `NoUpgradeProposed` | No upgrade has been proposed |
| 101 | `UpgradeTimelockNotElapsed` | Upgrade timelock period has not yet elapsed |
