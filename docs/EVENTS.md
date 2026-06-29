# Kora Protocol Event Naming Convention

This document defines the standardized event topic naming convention across all Kora contracts, ensuring consistent event identification for indexers, dashboards, and monitoring systems.

## Naming Pattern

All event topic symbols follow the pattern:

```
<CONTRACT>_<ACTION>
```

Where:
- `<CONTRACT>`: 3-letter contract identifier (INV, POOL, MKTPL, TREAS, RISK, AC)
- `<ACTION>`: 3-6 letter action verb (CREATED, LISTED, FUNDED, UPDATED, etc.)

**Limit:** Soroban `symbol_short!()` supports up to 32 characters per topic. All our symbols stay well under this limit (8-14 chars).

## Event Topic Registry

### Invoice NFT Contract (INV_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `INV_CREATED` | invoice_created | (invoice_id, sme, amount, timestamp) | Invoice minted |
| `INV_LISTED` | invoice_listed | (seller, invoice_id, asking_price, timestamp) | Listing created |
| `INV_FUNDED` | invoice_funded | (investor, invoice_id, funded_amount, timestamp) | Funding received |
| `INV_REPAID` | invoice_repaid | (invoice_id, sme, amount, timestamp) | Repayment made |
| `INV_DEFAULTED` | invoice_defaulted | (invoice_id, sme, timestamp) | Invoice defaulted |

### Financing Pool Contract (POOL_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `POOL_OPENED` | pool_opened | (marketplace, invoice_id, token, face_value, timestamp) | Pool initialized |
| `POS_RECORDED` | position_recorded | (admin, invoice_id, investor, contributed, share_bps, timestamp) | Position allocated |

### Marketplace Contract (MKTPL_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `MKTPL_CANCELLED` | listing_cancelled | (invoice_id, seller, timestamp) | Listing cancelled |
| `MKTPL_EXPIRED` | listing_expired | (invoice_id, seller, timestamp) | Funding deadline passed |

### Treasury Contract (TREAS_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `TREAS_INITIALIZED` | treasury_initialized | (admin, fee_bps) | Contract initialized |
| `TREAS_FEE_COLLECTED` | fee_collected | (invoice_id, fee_amount, token, timestamp) | Fee accrued |
| `TREAS_FEE_WITHDRAWN` | fee_withdrawn | (token, amount) | Fee withdrawn |
| `TREAS_EMERGENCY_WTH` | emergency_withdrawn | (by, token, amount) | Emergency drain |
| `TREAS_FEE_UPDATED` | fee_rate_updated | (by, old_bps, new_bps) | Fee rate changed |

### Risk Registry Contract (RISK_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `RISK_VERIFIER_ADDED` | verifier_added | (admin, verifier, timestamp) | Verifier whitelisted |
| `RISK_VERIFIER_REMOVED` | verifier_removed | (admin, verifier, timestamp) | Verifier revoked |
| `RISK_SME_REGISTERED` | sme_registered | (verifier, sme, risk_score, timestamp) | SME profile created |
| `RISK_SME_SCORE_UPDATED` | sme_score_updated | (verifier, sme, new_score, timestamp) | Risk score changed |
| `RISK_SME_DEFAULT_REC` | sme_default_recorded | (admin, sme, total_defaults, timestamp) | Default recorded |
| `RISK_SME_INV_COUNT` | sme_invoice_count_incremented | (sme, new_total, timestamp) | Invoice count updated |
| `RISK_DEBTOR_SCORE_SET` | debtor_score_set | (verifier, debtor_hash, score, timestamp) | Debtor score set |
| `RISK_REGISTRY_INIT` | registry_initialized | (admin, invoice_nft) | Contract initialized |

### Access Control Contract (AC_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `AC_PAUSED` | protocol_paused | (by, timestamp) | Protocol paused |
| `AC_UNPAUSED` | protocol_unpaused | (by, timestamp) | Protocol unpaused |
| `AC_ADMIN_TRANSFERRED` | admin_transferred | (new_admin) | Admin changed |
| `AC_ROLE_GRANTED` | role_granted | (admin, target) | Role assigned |
| `AC_ROLE_REVOKED` | role_revoked | (admin, target) | Role revoked |
| `AC_TOKEN_WHITELISTED` | token_whitelisted | (token) | Token approved |
| `AC_UPGRADE_PROPOSED` | upgrade_proposed | (admin, wasm_hash, timestamp) | Upgrade proposed |
| `AC_UPGRADE_EXECUTED` | upgrade_executed | (admin, wasm_hash, timestamp) | Upgrade executed |
| `AC_MULTISIG_CFG` | multisig_configured | (threshold, signer_count, timestamp) | Multisig config set |
| `AC_ACTION_PROPOSED` | action_proposed | (proposal_id, proposer, timestamp) | Multisig action proposed |
| `AC_ACTION_APPROVED` | action_approved | (proposal_id, approver, approval_count, timestamp) | Multisig approval |
| `AC_ACTION_EXECUTED` | action_executed | (proposal_id, executor, timestamp) | Multisig action executed |

### Shared / Cross-Contract Events (PROTOCOL_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `PROTOCOL_YIELD_DIST` | yield_distributed | (invoice_id, investor, yield_amount, timestamp) | Yield paid |
| `PROTOCOL_LATE_PEN` | late_penalty_applied | (invoice_id, penalty_amount, total_owed, timestamp) | Late fee applied |
| `PROTOCOL_REPAYMENT` | repayment_made | (invoice_id, payer, amount, timestamp) | Repayment made |
| `PROTOCOL_REFUND_CLAIMED` | refund_claimed | (invoice_id, investor, amount, timestamp) | Refund processed |

## Indexer Integration

When subscribing to events via Soroban event streams:

```rust
// Match events by topic symbol
let topic = env.events().last_published_topic();

// Example matchers (for off-chain indexers)
match topic {
    "INV_CREATED" => { /* handle invoice creation */ },
    "POOL_OPENED" => { /* handle pool opening */ },
    "RISK_SME_REGISTERED" => { /* handle new SME */ },
    _ => { /* unrecognized event */ },
}
```

## Migration Notes

Events use Soroban's `symbol_short!()` macro, which is more efficient than `Symbol::new()` for constants. All topic symbols are 8-14 characters and fit comfortably within the 32-character limit.

For backwards compatibility with deployed contracts, the internal event topic names remain unchanged—only new deployments follow this convention.
# Kora Protocol — Event Schema Reference

This document is the canonical reference for every on-chain event published by the
Kora protocol contracts. Indexer authors and off-chain reconciliation tooling should
treat this file as the source of truth.

---

## Canonical Schema Convention

Every event follows the ordering convention:

```
(actor: Address, subject: ..., amount: i128, ledger_timestamp: u64)
```

| Position | Field | Description |
|----------|-------|-------------|
| 1 | **actor** | The `Address` initiating the action (SME, investor, admin, contract) |
| 2 | **subject** | What is being acted on — typically an `invoice_id: u64` or `token: Address` |
| 3 | **amount / data** | Monetary value in stroops, or relevant scalar data (0 when not applicable) |
| last | **timestamp** | `env.ledger().timestamp()` — always present for deterministic indexing |

Events with more than three data fields extend the tuple while preserving actor-first,
timestamp-last ordering.

System events (where there is no single initiating actor — e.g. `late_penalty_applied`,
`multisig_configured`) omit the actor and start with the relevant subject or scalar.

---

## Event Catalog

### Invoice Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `INV_CRT` | `invoice_created` | `(sme, invoice_id, amount, timestamp)` | `invoice_nft` |
| `INV_LST` | `invoice_listed` | `(seller, invoice_id, asking_price, timestamp)` | `marketplace`, `invoice_nft` |
| `INV_FND` | `invoice_funded` | `(investor, invoice_id, funded_amount, timestamp)` | `marketplace`, `invoice_nft` |
| `INV_RPD` | `invoice_repaid` | `(sme, invoice_id, amount, timestamp)` | `invoice_nft` |
| `INV_DFT` | `invoice_defaulted` | `(actor, invoice_id, timestamp)` | `invoice_nft`, `financing_pool` |

> **Note on `INV_DFT`:** The `actor` field is the admin address that triggered the
> default marking — it is not the SME. In `invoice_nft`, the caller is validated as
> the contract admin; in `financing_pool`, it is the admin address passed to `mark_default`.

---

### Repayment Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `REPAY` | `repayment_made` | `(payer, invoice_id, amount, timestamp)` | `financing_pool` |
| `YIELD` | `yield_distributed` | `(investor, invoice_id, yield_amount, timestamp)` | `financing_pool` |
| `LATE_PEN` | `late_penalty_applied` | `(invoice_id, penalty_amount, total_owed, timestamp)` | `financing_pool` |

> **`LATE_PEN`** is a system event with no actor. `invoice_id` identifies which pool
> the penalty applies to, `penalty_amount` is the incremental penalty, and `total_owed`
> is the new total the SME owes.

---

### Marketplace Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `LST_CXL` | `listing_cancelled` | `(seller, invoice_id, timestamp)` | `marketplace` |
| `LST_EXP` | `listing_expired` | `(seller, invoice_id, timestamp)` | `marketplace` |
| `REFUND` | `refund_claimed` | `(investor, invoice_id, amount, timestamp)` | `marketplace` |

---

### Fee Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `FEE_COL` | `fee_collected` | `(investor, invoice_id, fee_amount, token, timestamp)` | `marketplace`, `treasury` |
| `FEE_WTH` | `fee_withdrawn` | `(admin, token, amount, timestamp)` | `treasury` |
| `EMRG_WTH` | `emergency_withdrawn` | `(admin, token, amount, timestamp)` | `treasury` |
| `FEE_UPD` | `fee_rate_updated` | `(admin, old_bps, new_bps, timestamp)` | `treasury`, `marketplace` |
| `TRES_INI` | `treasury_initialized` | `(admin, fee_bps, timestamp)` | `treasury` |

> **`FEE_COL` from treasury:** When `treasury.collect_fee` emits this event, the
> `investor` field is set to the treasury contract address (a sentinel indicating
> a protocol-internal accounting deposit rather than a direct investor action).
> Off-chain indexers can distinguish by checking whether `investor == treasury_contract`.

---

### Protocol / Admin Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `PAUSED` | `protocol_paused` | `(admin, timestamp)` | `access_control` |
| `UNPAUSED` | `protocol_unpaused` | `(admin, timestamp)` | `access_control` |
| `TOK_WL` | `token_whitelisted` | `(admin, token, timestamp)` | `marketplace`, `treasury` |
| `ADM_TRF` | `admin_transferred` | `(current_admin, new_admin, timestamp)` | `access_control`, `risk_registry` |
| `ROL_GRT` | `role_granted` | `(admin, target, timestamp)` | `access_control` |
| `ROL_RVK` | `role_revoked` | `(admin, target, timestamp)` | `access_control` |

---

### Financing Pool Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `PLOP` | `pool_opened` | `(marketplace, invoice_id, token, face_value, timestamp)` | `financing_pool` |
| `POSR` | `position_recorded` | `(admin, invoice_id, investor, contributed, share_bps, timestamp)` | `financing_pool` |

---

### Risk Registry Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `REG_INI` | `registry_initialized` | `(admin, invoice_nft, timestamp)` | `risk_registry` |
| `VRF_ADD` | `verifier_added` | `(admin, verifier, timestamp)` | `risk_registry` |
| `VRF_REM` | `verifier_removed` | `(admin, verifier, timestamp)` | `risk_registry` |
| `SME_REG` | `sme_registered` | `(verifier, sme, risk_score, timestamp)` | `risk_registry` |
| `SME_UPD` | `sme_score_updated` | `(verifier, sme, new_score, timestamp)` | `risk_registry` |
| `SME_DFT` | `sme_default_recorded` | `(admin, sme, total_defaults, timestamp)` | `risk_registry` |
| `SME_INV` | `sme_invoice_count_incremented` | `(sme, new_total_invoices, timestamp)` | `risk_registry` |
| `DBT_SCR` | `debtor_score_set` | `(verifier, debtor_hash, score, timestamp)` | `risk_registry` |

---

### Upgrade Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `UPG_PROP` | `upgrade_proposed` | `(admin, wasm_hash, timestamp)` | all contracts |
| `UPG_EXEC` | `upgrade_executed` | `(admin, wasm_hash, timestamp)` | all contracts |

---

### Multisig Events

| Topic Symbol | Function | Payload | Emitter |
|---|---|---|---|
| `MS_CFG` | `multisig_configured` | `(threshold, signer_count, timestamp)` | `access_control` |
| `MS_PROP` | `action_proposed` | `(proposal_id, proposer, timestamp)` | `access_control` |
| `MS_APPR` | `action_approved` | `(proposal_id, approver, approval_count, timestamp)` | `access_control` |
| `MS_EXEC` | `action_executed` | `(proposal_id, executor, timestamp)` | `access_control` |

---

## Indexing Notes

- All topics are published as a single-element tuple: `(topic_symbol,)`.
- `ledger_timestamp` is a `u64` Unix timestamp in seconds.
- `invoice_id` is a `u64` auto-incrementing integer starting at 1.
- `share_bps` is a `u32` in basis points (10 000 = 100 %).
- `fee_bps`, `old_bps`, `new_bps` are `u32` basis-point values (max 10 000).
- `risk_score` / `new_score` are `u32` values in the range 0–100.
- `debtor_hash` is `Bytes` (SHA-256 of off-chain PII — the raw bytes, not hex-encoded).
- `wasm_hash` is `BytesN<32>`.
- All monetary amounts (`amount`, `face_value`, `fee_amount`, etc.) are `i128` in stroops
  (7 decimal places for USDC/EURC on Stellar).
