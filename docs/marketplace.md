# Marketplace Contract

## Overview

The Marketplace contract manages the listing lifecycle and investor funding flow for invoice-backed securities. It orchestrates the flow of funds from investors through the protocol: investors contribute capital, fees are deducted to the treasury, and net funds are held in the financing pool until the SME fulfills the invoice.

## Role in Kora Protocol

The Marketplace acts as the intermediary between SMEs (Small/Medium Enterprises) seeking financing and investors providing capital. It:

1. **Manages Listings** — SMEs create listings for specific invoices, specifying asking price and face value
2. **Collects Investor Funding** — Investors fund listings in shares, with fees automatically deducted
3. **Orchestrates Transitions** — Notifies Invoice NFT contract when status changes (Created → Listed → Funded)
4. **Releases Funds** — Triggers the Financing Pool to move funds to the SME when fully funded

## Public API Surface

### initialization

```
initialize(
  env: Env,
  admin: Address,
  invoice_nft: Address,
  financing_pool: Address,
  treasury: Address,
  fee_bps: u32
) -> Result<(), KoraError>
```

One-time initialization. Sets up the marketplace with pointers to other contracts and the protocol fee rate.

**Parameters:**
- `admin` — Protocol admin address (can pause, whitelist tokens, set fees)
- `invoice_nft` — Address of the Invoice NFT contract (for status transitions)
- `financing_pool` — Address of the Financing Pool (holds investor funds)
- `treasury` — Address of the Treasury (accumulates fees)
- `fee_bps` — Fee rate in basis points (0–10,000; e.g., 50 = 0.5%)

**Errors:**
- `AlreadyInitialized` — contract already initialized
- `InvalidAmount` — fee_bps > 10,000

### list_invoice

```
list_invoice(
  env: Env,
  seller: Address,
  invoice_id: u64,
  asking_price: i128,
  face_value: i128,
  token: Address,
  funding_deadline: u64
) -> Result<(), KoraError>
```

SME lists an invoice NFT for financing. Requires `seller` authentication. The asking price must be less than face value by at least the configured minimum discount (a meaningful discount must exist for investor yield).

**Parameters:**
- `seller` — SME address (must authenticate)
- `invoice_id` — ID of the invoice (from invoice_nft contract)
- `asking_price` — Target funding amount in tokens
- `face_value` — Nominal value of the underlying invoice
- `token` — Whitelisted stablecoin address
- `funding_deadline` — Unix timestamp; must be in the future
- `referrer` — Optional referrer address credited a share of the collected fee

**State Changes:**
- Persists a Listing struct with status `active` and `funded_amount = 0`
- Calls `invoice_nft.set_listed()` to transition invoice status

**Errors:**
- `InvalidAmount` — asking_price or face_value <= 0, asking_price >= face_value, or the discount (face_value - asking_price) is below the minimum discount requirement
- `InvalidDueDate` — funding_deadline in the past
- `TokenNotWhitelisted` — token not whitelisted by admin
- `InvoiceAlreadyExists` — listing already exists for this invoice_id

**Minimum Discount:**

To keep listings economically meaningful for investors (and to prevent fee-arbitrage or spam listings priced a stroop below face value), `list_invoice` enforces a minimum discount, expressed in basis points of `face_value`:

```
min_discount = face_value × min_discount_bps / 10_000
required: face_value - asking_price >= min_discount
```

- Defaults to `10` bps (0.1%) if never configured.
- Configurable by the admin via `set_min_discount_bps(admin, min_discount_bps)` (0–10,000 bps; validated with the same range as `fee_bps`).
- Read the current value with `get_min_discount_bps()`.

### fund_invoice

```
fund_invoice(
  env: Env,
  investor: Address,
  invoice_id: u64,
  amount: i128
) -> Result<(), KoraError>
```

Investor funds a share of an invoice. Marketplace fee is deducted and sent to treasury; net amount is transferred to the financing pool. When the listing reaches or exceeds asking_price, it is closed and the pool releases funds to the SME.

**Fee Model:**
```
gross_amount (from investor)
├── fee = gross_amount × fee_bps / 10_000  →  treasury
└── net = gross_amount - fee                →  financing_pool
```

**Parameters:**
- `investor` — Investor address (must authenticate)
- `invoice_id` — Invoice being funded
- `amount` — Gross contribution (fee calculated on this)

**State Changes:**
- Increments listing `funded_amount` by `amount`
- Transfers `fee` to treasury
- Transfers `net` to financing pool
- If fully funded (funded_amount >= asking_price):
  - Sets listing `is_active = false`
  - Calls `financing_pool.release_funds()` to move funds to SME
  - Calls `invoice_nft.set_funded()` to transition invoice status

**Errors:**
- `ListingNotFound` — no listing for this invoice_id
- `ListingAlreadyCancelled` — listing is inactive
- `FundingDeadlinePassed` — current time > funding_deadline
- `InvalidAmount` — amount <= 0
- `ExceedsFundingTarget` — amount would exceed asking_price
- `ContributionBelowMinimum` — amount is below the configured minimum contribution floor and does not exactly complete the remaining funding target (#451)
- `ArithmeticOverflow` — fee or net calculation overflowed

**Minimum contribution floor (#451):** contributions below `get_min_contribution()` (admin-configurable via `set_min_contribution`, defaults to 10_000_000 — ~1 unit for a 7-decimal stablecoin) are rejected, unless `amount` exactly equals the listing's remaining funding target. This closes off cheap dust-contribution storage-griefing of a listing: without a floor, an attacker could create unbounded distinct `Contribution(invoice_id, investor)` entries at near-zero cost by submitting many tiny contributions from many distinct addresses.

### cancel_listing

```
cancel_listing(
  env: Env,
  caller: Address,
  invoice_id: u64
) -> Result<(), KoraError>
```

Cancel a listing before it is fully funded. Caller must be the seller or the admin.

**Parameters:**
- `caller` — Address attempting to cancel (must be seller or admin)
- `invoice_id` — Listing to cancel

**State Changes:**
- Sets listing `is_active = false`

**Errors:**
- `ListingNotFound` — no listing for this invoice_id
- `ListingAlreadyCancelled` — listing already inactive
- `Unauthorized` — caller is neither seller nor admin

### whitelist_token

```
whitelist_token(
  env: Env,
  admin: Address,
  token: Address
) -> Result<(), KoraError>
```

Whitelist a stablecoin token for use in listings. Admin only. Multiple tokens can be whitelisted; all new listings must use a whitelisted token.

**Parameters:**
- `admin` — Protocol admin (must authenticate and be authorized)
- `token` — Token address to whitelist

**Errors:**
- `NotAdmin` — caller is not the admin

### get_listing

```
get_listing(
  env: Env,
  invoice_id: u64
) -> Result<Listing, KoraError>
```

Get a listing by invoice_id. Returns the full Listing struct.

**Returns:**
- `Listing` — containing invoice_id, seller, asking_price, face_value, token, funded_amount, funding_deadline, is_active

**Errors:**
- `ListingNotFound` — no listing for this invoice_id

## Typical Usage Flow

### 1. Listing (SME initiates)

```
list_invoice(
  seller=sme_address,
  invoice_id=1,
  asking_price=9_500_000_000,     // 9,500 USDC (offer a discount)
  face_value=10_000_000_000,       // 10,000 USDC (invoice amount)
  token=usdc_address,
  funding_deadline=timestamp_30_days_from_now
)
```

Result: Listing is created, invoice status becomes `Listed`.

### 2. Funding (Investor(s) contribute)

```
fund_invoice(
  investor=investor_address,
  invoice_id=1,
  amount=4_000_000_000     // first investor contributes 4,000 USDC
)
// marketplace deducts fee: 4_000_000_000 × 50 / 10_000 = 20_000_000 USDC
// net to pool: 3_980_000_000 USDC
// fee to treasury: 20_000_000 USDC
```

Multiple investors can fund in tranches.

### 3. Full Funding & Release (Automatic when target reached)

```
// Second investor completes the funding:
fund_invoice(
  investor=investor_address_2,
  invoice_id=1,
  amount=5_500_000_000     // second investor contributes 5,500 USDC
)
// Total funded: 9,500,000,000 USDC (matches asking_price)
// Marketplace automatically:
//   1. Deducts fee (27,500,000 USDC) to treasury
//   2. Transfers net (5,472,500,000 USDC) to pool
//   3. Calls financing_pool.release_funds()
//   4. SME receives funds net of marketplace fees
//   5. Invoice status transitions to Funded
```

### 4. Settlement (SME repays or defaults)

See `financing_pool` contract for repayment and yield distribution.

## Security Considerations

### Input Validation

- **Amounts:** All contributions must be > 0. Asking price must be < face value (ensures discount/yield).
- **Timestamps:** Funding deadline must be in the future.
- **Tokens:** Only whitelisted tokens are accepted. Admin controls which tokens are used.
- **Listing State:** Cannot fund a cancelled listing. Cannot exceed asking price.

### Fee Calculation

Fees use checked arithmetic (`bps_of`) to prevent overflow. Overflows return `ArithmeticOverflow` error.

### Cross-Contract Calls

Marketplace calls Invoice NFT and Financing Pool via authenticated calls. Each callee verifies the caller is the marketplace contract. No direct state mutation across contracts.

### Reentrancy

Token transfers happen after all state updates (checks-effects-interactions pattern). Financing Pool uses a reentrancy guard on withdrawal operations.

## Error Codes

| Error | Meaning |
|-------|---------|
| `InvalidAmount` | Amount <= 0 or price validation failed |
| `InvalidDueDate` | Deadline in the past |
| `TokenNotWhitelisted` | Token not whitelisted |
| `InvoiceAlreadyExists` | Listing already created for this invoice |
| `ListingNotFound` | No listing for given invoice_id |
| `ListingAlreadyCancelled` | Listing is inactive |
| `FundingDeadlinePassed` | Current time > deadline |
| `ExceedsFundingTarget` | Funding amount would exceed asking_price |
| `NotAdmin` | Caller is not the admin |
| `Unauthorized` | Caller not authorized for this operation |
| `ArithmeticOverflow` | Arithmetic operation overflowed |

## Storage Model

Listings are stored in persistent storage with listing ID as key. Each listing is a separate entry with its own TTL. Whitelisted tokens are stored as boolean flags.

## Admin Operations

- **Initialize:** Set up marketplace with pointers to other contracts
- **Whitelist Tokens:** Add tokens that SMEs and investors can use
- **Pause Protocol:** Via access_control contract (pauses state-mutating operations)
- **Set Fees:** Update the marketplace fee rate

## Authorization Model

Privileged marketplace calls are gated on the multisig signer set configured in
the `access_control` contract, not on a single admin key alone.

Affected entrypoints: `set_fee_bps` / `update_fee_bps`, `set_tier_fee_bps`,
`set_referrer_split_bps`, `whitelist_token`, `remove_token_whitelist`,
`propose_upgrade`, and `execute_upgrade`.

Two authorization paths exist:

- **Direct admin call.** Allowed only when `access_control` has no multisig
  configured, or has a 1-of-1 multisig. This is the backward-compatible path for
  low-risk chains and existing deployments. When a quorum above 1 is configured,
  these calls fail with `MultisigApprovalRequired`.
- **Multisig quorum.** A configured signer calls `propose_admin_action(proposer,
  action)` with a `MarketplaceAction`, other signers call
  `approve_admin_action(approver, proposal_id)`, and once approvals reach the
  configured threshold any signer calls `execute_admin_action(executor,
  proposal_id)`. The proposal is marked executed before dispatch, so it cannot
  be replayed.

`is_multisig_required()` reports which path is currently in force, and
`get_admin_proposal(proposal_id)` returns a proposal's action, approvals, and
executed flag.

`MarketplaceAction` variants: `SetFeeBps`, `SetReferrerSplitBps`,
`SetTierFeeBps`, `WhitelistToken`, `RemoveTokenWhitelist`, `ProposeUpgrade`,
`ExecuteUpgrade`.

Relevant errors:

| Error | Meaning |
|---|---|
| `MultisigApprovalRequired` | Direct admin call attempted while a quorum above 1 is configured |
| `MultisigNotConfigured` | Proposal flow used with no multisig configured on access_control |
| `NotMultisigSigner` | Caller is not in the configured signer set |
| `GovernanceThresholdNotMet` | Approvals are below the configured threshold |
| `AlreadyVoted` | Signer has already approved this proposal |
| `ParameterProposalNotFound` | No proposal exists with the given id |
| `ParameterProposalAlreadyExecuted` | Proposal has already been executed |

## Per-investor concentration cap (#435)

`set_max_investor_share_bps(admin, bps)` caps how much of a single listing one address may fund, in basis points of `asking_price`; `0` (default) disables it. The cap is cumulative: the marketplace keeps a running gross (pre-fee) total per `(invoice_id, investor)` in listing-token units, read with `get_gross_contribution(invoice_id, investor)`.

- Each `fund_invoice` adds its listing-token amount (after oracle conversion for cross-currency payments) to the running total. The exact cap is allowed; anything above it fails with `InvestorConcentrationExceeded` and changes no state.
- The total is recorded even while the cap is disabled, so enabling or lowering the cap later counts prior funding. Lowering it below an existing holding blocks top-ups but never claws funds back.
- The total is cleared when the investor is refunded (`cancel_listing` or `claim_refund`).
- The cap is per address and per listing. Sybil resistance comes from accreditation: every funding address must be accredited with `set_investor_accredited`.

## Protocol-wide debtor concentration limits (#748)

The marketplace tracks gross outstanding funding per debtor hash across invoices in `DebtorExposure(debtor_hash)`. This is separate from the existing per-investor share cap and per-token listing cap. Admins set fixed, nonnegative limits with `set_debtor_tier_cap(admin, tier, cap)`; `0` disables a tier limit. `get_debtor_tier_cap(tier)` and `get_debtor_exposure(debtor_hash)` expose the configured and recorded values.

The debtor's tier comes from active verifier scores in `risk_registry.get_debtor_risk_tier`, using that registry's current tier boundaries. Once any cap is enabled, a debtor without an active score cannot list or receive further funding. Listing checks projected outstanding exposure plus its full asking price. Each funding call checks again against the current tier and adds its gross listing-token amount; the exact cap is allowed and the next unit is rejected. A listed invoice can therefore stop accepting funds if another invoice uses the debtor's remaining capacity or its score worsens.

`claim_refund` subtracts the investor's gross funded amount (net contribution plus fee) once. Fully repaid or defaulted invoices release their remaining recorded exposure through the permissionless `sync_debtor_exposure(debtor_hash)` call. Listing and funding also sync terminal invoices for that debtor before checking the cap. The sync reads canonical `invoice_nft` status, so callers cannot release exposure by claiming that an invoice settled; repeated syncs have no further effect. Operators should invoke sync after a repayment or default for prompt dashboard updates. Settlement remains independent of marketplace availability.

Sync a terminal invoice **before** archiving its NFT record. Once that record is removed, the marketplace cannot independently verify its terminal status and retains its exposure until an explicit migration/recovery procedure handles the archived record.

The active invoice index is limited to 256 per debtor to bound reconciliation cost. Entries use the repository's persistent storage TTL convention; operators must maintain contract/storage lifetimes. Existing funded invoices created before the exposure ledger was deployed are not automatically backfilled. Backfill or settle those records before enabling caps on an upgraded deployment.

Caps and exposure are in gross listing-token base units. A common token denomination (or an agreed normalization policy before enabling multiple currencies) is required for economically comparable protocol-wide limits; the current implementation does not recalculate caps from live TVL or exchange rates.
