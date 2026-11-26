# Kora Protocol Event Naming Convention

This document defines the standardized event topic naming convention across all Kora contracts, ensuring consistent event identification for indexers, dashboards, and monitoring systems.

---

## Schema Versioning (#583)

All Kora on-chain events carry an explicit schema version so off-chain indexers can
detect contract upgrades that alter event shapes without silently breaking.

### Topic layout

Every event is published with **three** Soroban topics:

```
topics: ("SCHEMA_V", <event_topic>, <version: u32>)
data:   <event payload tuple>
```

The first topic is always the literal `SCHEMA_V` sentinel. The second is the
per-event topic symbol (e.g. `INV_CRT`). The third is the schema version number
as a `u32`.

The current schema version is defined in `contracts/shared/src/events.rs`:

```rust
pub const EVENT_SCHEMA_VERSION: u32 = 1;
```

### Versioning policy

| Change type | Action |
|---|---|
| **Additive** — new optional field appended to the end of an existing event tuple | Bump `EVENT_SCHEMA_VERSION` (minor intent); document in this file |
| **Breaking** — field removed, type changed, ordering altered, or topic symbol renamed | Bump `EVENT_SCHEMA_VERSION`; add a migration note below; update all payload tables |

Out of scope: migrating historical events that were already emitted on-chain before versioning
was introduced. Those events carry no `SCHEMA_V` topic and indexers should treat their
absence as "pre-v1".

### Changelog

| Version | Change | Related issue |
|---|---|---|
| 1 | Initial versioned release — `SCHEMA_V` topic added to every event | #583 |
| 1 | Added `PAR_BOOT` (`parameter_registry_bootstrap`) — emitted once per `bootstrap_from_live_contracts` call with seeded count and checksum | governance-sync |
| 1 | Added `DA_POLICY` (`debtor_attestation_policy_decision`) — emitted on every attestation-gate evaluation in `marketplace` with tier ordinal and `required` flag | governance-sync |

### Indexer integration

```rust
// Filter events that belong to the current schema version
env.events().get_events()
    .iter()
    .filter(|e| {
        e.topics.get(0) == Some(Symbol::new(env, "SCHEMA_V")) &&
        e.topics.get(2) == Some(Val::from(kora_shared::events::EVENT_SCHEMA_VERSION))
    })
    .for_each(|e| { /* process e.topics[1] for event type */ });
```

Pre-v1 events (no `SCHEMA_V` topic) can be identified by their single-element topic tuple.

---

## Naming Pattern

All event topic symbols follow the pattern:

```
<CONTRACT>_<ACTION>
```

Where:
- `<CONTRACT>`: 3-letter contract identifier (INV, POOL, MKTPL, TREAS, RISK, AC, VEST)
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
| `MKTPL_EXPIRED` | listing_expired | (seller, invoice_id, timestamp) | Funding deadline passed |
| `DA_POLICY` | debtor_attestation_policy_decision | (invoice_id, tier_ordinal, required, timestamp) | Attestation gate decision per tier (0=AAA…4=C). `required=true` means a valid debtor attestation is mandatory for this listing/funding attempt. Governance-sync dashboards index this to audit policy changes live. |

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

#### Counter increment events (#1079, AUDIT_LOG RR-01)

Every counter that risk_registry increments emits exactly **one** event in the same
call, so indexers can rebuild risk aggregates from the event log. The on-chain topic
is the second `SCHEMA_V` topic for shared events, and the first topic for raw events.

| Counter | Entrypoint | On-chain topic | Payload |
|---|---|---|---|
| `SmeProfile.total_invoices` | `increment_invoice_count` | `SME_INV` | (sme, new_total, timestamp) |
| `SmeProfile.defaults` and `DefaultCountTier(tier)` | `record_default` | `SME_DFT` | (admin, sme, new_total_defaults, timestamp) |
| `TotalScoredCountTier(tier)` | `register_sme` | `SME_REG` | (verifier, sme, risk_score, timestamp); the tier is derived from `risk_score` |
| `DisputeCount(verifier)` | `raise_dispute` | `DSP_RAIS` (raw) | topics `(DSP_RAIS, verifier, subject)`, data `challenger` |

If an increment would overflow, the call fails with `ArithmeticOverflow`. Soroban
discards the events and storage writes of a failed invocation, so no event
dangles. Before this change `raise_dispute` panicked on overflow; it now returns
the typed error. All of this is pinned by
`contracts/tests/risk_registry_counter_events.rs`, which fails if any of these
emissions is removed.

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

### Parameter Registry Contract (PAR_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `REG_INIT` | registry_param_initialized | (admin, timestamp) | Registry initialized; bootstrap defaults written |
| `PAR_PROP` | param_update_proposed | (proposer, key_discriminant, new_value, timestamp) | Governance proposal opened for a parameter change |
| `PAR_APRV` | param_update_approved | (approver, proposal_id, approval_count, timestamp) | A signer approved a pending proposal |
| `PAR_EXEC` | param_updated | (executor, key_discriminant, old_value, new_value, timestamp) | Proposal executed; value is now live on-chain |
| `PAR_BOOT` | parameter_registry_bootstrap | (admin, seeded_count, checksum, timestamp) | `bootstrap_from_live_contracts` completed; `seeded_count` keys written, `checksum` is big-endian concatenation of their values for indexer verification |

**Key discriminant mapping** (stable `u32` used in `PAR_PROP`/`PAR_EXEC` payloads):

| Discriminant | Key |
|---|---|
| 1 | `MarketplaceFeeBps` |
| 2 | `ReferrerSplitBps` |
| 3 | `MinContributionAmount` |
| 4–8 | `TierFeeAaa`…`TierFeeC` |
| 9 | `LatePenaltyBps` |
| 10 | `LatePenaltyTreasurySplitBps` |
| 11 | `MaxPositionBps` |
| 12 | `GracePeriodSecs` |
| 13 | `MinimumVerifierStake` |
| 14 | `SlashPercentageBps` |
| 15 | `MinimumDebtorScore` |
| 16 | `TreasuryFeeBps` |
| 17 | `ReserveAllocationBps` |
| 18 | `MinReserveRatioBps` |
| 19–23 | `DebtorAttestationAaa`…`DebtorAttestationC` |

### Vesting Contract (VEST_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `VEST_INIT` | vesting_initialized | (admin, token, timestamp) | Vesting contract initialized |
| `VEST_GRNT` | vesting_grant_created | (admin, beneficiary, total_amount, start_time, cliff_duration, vesting_duration, timestamp) | Vesting grant created |
| `VEST_REL` | vesting_tokens_released | (beneficiary, amount, timestamp) | Vested tokens released |
| `VEST_REV` | vesting_grant_revoked | (admin, beneficiary, revoked_at, timestamp) | Vesting grant revoked |
| `UPG_PROP` | upgrade_proposed | (admin, wasm_hash, timestamp) | Upgrade proposed |
| `UPG_EXEC` | upgrade_executed | (admin, wasm_hash, timestamp) | Upgrade executed |

### Secondary Market Contract (SM_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `SM_INIT` | secondary_market_initialized | (admin, financing_pool, fee_bps, timestamp) | Secondary market initialized |
| `SM_PAUS` | secondary_market_paused | (timestamp) | Secondary market paused |
| `SM_UNP` | secondary_market_unpaused | (timestamp) | Secondary market unpaused |
| `SM_FEE` | secondary_market_fee_updated | (fee_bps, timestamp) | Protocol fee rate updated |
| `SM_R_ADD` | secondary_market_fee_recipient_added | (admin, recipient, timestamp) | Address added to the fee-recipient allowlist |
| `SM_R_REM` | secondary_market_fee_recipient_removed | (admin, recipient, timestamp) | Address removed from the fee-recipient allowlist |

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

## Generated On-Chain Event Table

Generated from `contracts/shared/src/events.rs`; this table is authoritative for
the topic symbols emitted on-chain by the shared helpers. Do not edit by hand.
Regenerate with `cargo run -p kora-xtask --bin gen_events`.

<!-- BEGIN GENERATED EVENTS -->
| Topic | Function | Parameters | Payload |
|---|---|---|---|
| `INV_CRT` | `invoice_created` | `invoice_id: u64, sme: &Address, amount: i128, currency: Symbol` | `(sme, invoice_id, amount, currency, timestamp)` |
| `INV_LIST` | `invoice_listed` | `invoice_id: u64, seller: &Address, asking_price: i128, currency: Symbol` | `(seller, invoice_id, asking_price, currency, timestamp)` |
| `INV_FUND` | `invoice_funded` | `invoice_id: u64, investor: &Address, amount: i128, currency: Symbol` | `(investor, invoice_id, amount, currency, timestamp)` |
| `INV_RPD` | `invoice_repaid` | `invoice_id: u64, sme: &Address, amount: i128, currency: Symbol` | `(sme, invoice_id, amount, currency, timestamp)` |
| `INV_DFT` | `invoice_defaulted` | `invoice_id: u64, actor: &Address, amount: i128, currency: Symbol` | `(actor, invoice_id, amount, currency, timestamp)` |
| `INV_AMD` | `invoice_amended` | `invoice_id: u64, sme: &Address, currency: Symbol` | `(invoice_id, sme, currency, timestamp)` |
| `INV_WTH` | `invoice_withdrawn` | `invoice_id: u64, sme: &Address, currency: Symbol` | `(invoice_id, sme, currency, timestamp)` |
| `INV_BATCH` | `invoice_batch_minted` | `batch_id: u64, sme: &Address, invoice_ids: &Vec<u64>` | `(sme, batch_id, invoice_ids, timestamp)` |
| `RISK_RFSH` | `risk_score_refreshed` | `invoice_id: u64, caller: &Address, old_score: u32, new_score: u32, old_tier: &RiskTier, new_tier: &RiskTier` | `(caller, invoice_id, old_score, new_score, old_tier, new_tier, timestamp)` |
| `INV_ARC` | `invoice_archived` | `invoice_id: u64, sme: &Address, amount: i128, _status: InvoiceStatus` | `(invoice_id, sme, amount, timestamp)` |
| `REPAY` | `repayment_made` | `invoice_id: u64, payer: &Address, amount: i128` | `(payer, invoice_id, amount, timestamp)` |
| `REP_APPR` | `repayment_approved` | `invoice_id: u64, payer: &Address, amount: i128` | `(payer, invoice_id, amount, timestamp)` |
| `INSTMT_PD` | `installment_paid` | `invoice_id: u64, payer: &Address, index: u32, amount: i128` | `(payer, invoice_id, index, amount, timestamp)` |
| `YIELD` | `yield_distributed` | `invoice_id: u64, investor: &Address, yield_amount: i128` | `(investor, invoice_id, yield_amount, timestamp)` |
| `LATE_PEN` | `late_penalty_applied` | `invoice_id: u64, penalty_amount: i128, total_owed: i128` | `(invoice_id, penalty_amount, total_owed, timestamp)` |
| `PEN_SPLIT` | `late_penalty_split` | `invoice_id: u64, total_penalty: i128, treasury_cut: i128, investor_cut: i128` | `(invoice_id, total_penalty, treasury_cut, investor_cut, timestamp)` |
| `PEN_SPL_U` | `late_penalty_split_updated` | `split_bps: u32` | `(split_bps, timestamp)` |
| `LST_CXL` | `listing_cancelled` | `invoice_id: u64, seller: &Address` | `(seller, invoice_id, timestamp)` |
| `MKTPL_EXPIRED` | `listing_expired` | `invoice_id: u64, seller: &Address` | `(seller, invoice_id, timestamp)` |
| `DA_VERI` | `debtor_attestation_verified` | `invoice_id: u64` | `(invoice_id, timestamp)` |
| `DA_CFG` | `debtor_attestation_configured` | `admin: &Address, addr: Option<&Address>` | `(admin, addr, timestamp)` |
| `FEE_COL` | `fee_collected` | `investor: &Address, invoice_id: u64, fee_amount: i128, token: &Address` | `(investor, invoice_id, fee_amount, token, timestamp)` |
| `FEE_WTH` | `fee_withdrawn` | `actor: &Address, token: &Address, amount: i128` | `(actor, token, amount, timestamp)` |
| `FEE_REV` | `fee_reversed` | `marketplace: &Address, invoice_id: u64, investor: &Address, amount: i128, token: &Address` | `(marketplace, invoice_id, investor, amount, token, timestamp)` |
| `EMRG_WTH` | `emergency_withdrawn` | `by: &Address, token: &Address, amount: i128` | `(by, token, amount, timestamp)` |
| `FEE_UPD` | `fee_rate_updated` | `by: &Address, old_bps: u32, new_bps: u32` | `(by, old_bps, new_bps, timestamp)` |
| `REF_FEE` | `referral_fee_paid` | `invoice_id: u64, referrer: &Address, referral_fee: i128` | `(invoice_id, referrer, referral_fee, timestamp)` |
| `TRES_INI` | `treasury_initialized` | `admin: &Address, fee_bps: u32` | `(admin, fee_bps, timestamp)` |
| `AC_PAUSED` | `protocol_paused` | `by: &Address` | `(by, timestamp)` |
| `UNPAUSED` | `protocol_unpaused` | `by: &Address` | `(by, timestamp)` |
| `TOK_WL` | `token_whitelisted` | `actor: &Address, token: &Address` | `(actor, token, timestamp)` |
| `TOK_UNWL` | `token_whitelist_removed` | `actor: &Address, token: &Address` | `(actor, token, timestamp)` |
| `ADM_TRF` | `admin_transferred` | `actor: &Address, new_admin: &Address` | `(actor, new_admin, timestamp)` |
| `ADM_PRP` | `admin_proposed` | `proposer: &Address, proposed: &Address` | `(proposer, proposed, timestamp)` |
| `ADM_CXL` | `admin_proposal_cancelled` | `admin: &Address` | `(admin, timestamp)` |
| `ADM_ROT` | `admin_rotated` | `executor: &Address, old_admin: &Address, new_admin: &Address` | `(executor, old_admin, new_admin, timestamp)` |
| `ROL_GRT` | `role_granted` | `admin: &Address, target: &Address` | `(admin, target, timestamp)` |
| `ROL_RVK` | `role_revoked` | `admin: &Address, target: &Address` | `(admin, target, timestamp)` |
| `POOL_OPN` | `pool_opened` | `marketplace: &Address, invoice_id: u64, token: &Address, face_value: i128` | `(marketplace, invoice_id, token, face_value, timestamp)` |
| `POS_RECRD` | `position_recorded` | `admin: &Address, invoice_id: u64, investor: &Address, contributed: i128, share_bps: u32` | `(admin, invoice_id, investor, contributed, share_bps, timestamp)` |
| `NET_SETTL` | `net_settled` | `payer: &Address, invoice_ids: &Vec<u64>, total_amount: i128` | `(payer, invoice_ids, total_amount, timestamp)` |
| `VRF_ADD` | `verifier_added` | `admin: &Address, verifier: &Address` | `(admin, verifier, timestamp)` |
| `VRF_REM` | `verifier_removed` | `admin: &Address, verifier: &Address` | `(admin, verifier, timestamp)` |
| `SME_REG` | `sme_registered` | `verifier: &Address, sme: &Address, risk_score: u32` | `(verifier, sme, risk_score, timestamp)` |
| `SME_UPD` | `sme_score_updated` | `verifier: &Address, sme: &Address, new_score: u32` | `(verifier, sme, new_score, timestamp)` |
| `SME_DFT` | `sme_default_recorded` | `admin: &Address, sme: &Address, total_defaults: u32` | `(admin, sme, total_defaults, timestamp)` |
| `SME_INV` | `sme_invoice_count_incremented` | `sme: &Address, new_total: u32` | `(sme, new_total, timestamp)` |
| `DBT_SCORE` | `debtor_score_set` | `verifier: &Address, debtor_hash: &Bytes, score: u32` | `(verifier, debtor_hash, score, timestamp)` |
| `REG_INI` | `registry_initialized` | `admin: &Address, invoice_nft: &Address` | `(admin, invoice_nft, timestamp)` |
| `SUB_ADD` | `sub_account_added` | `primary: &Address, sub_account: &Address` | `(primary, sub_account, timestamp)` |
| `SUB_RMV` | `sub_account_removed` | `primary: &Address, sub_account: &Address` | `(primary, sub_account, timestamp)` |
| `UPG_PROP` | `upgrade_proposed` | `admin: &Address, wasm_hash: &soroban_sdk::BytesN<32>` | `(admin, wasm_hash, timestamp)` |
| `UPG_EXEC` | `upgrade_executed` | `admin: &Address, wasm_hash: &soroban_sdk::BytesN<32>` | `(admin, wasm_hash, timestamp)` |
| `MS_CFG` | `multisig_configured` | `threshold: u32, signer_count: u32` | `(threshold, signer_count, timestamp)` |
| `MS_PROP` | `action_proposed` | `proposal_id: u64, proposer: &Address` | `(proposal_id, proposer, timestamp)` |
| `MS_APPR` | `action_approved` | `proposal_id: u64, approver: &Address, approval_count: u32` | `(proposal_id, approver, approval_count, timestamp)` |
| `MS_EXEC` | `action_executed` | `proposal_id: u64, executor: &Address` | `(proposal_id, executor, timestamp)` |
| `MS_STALE` | `stale_votes_discarded` | `proposal_id: u64, executor: &Address, discarded_count: u32, valid_count: u32` | `(proposal_id, executor, discarded_count, valid_count, timestamp)` |
| `REFUND` | `refund_claimed` | `invoice_id: u64, investor: &Address, amount: i128` | `(investor, invoice_id, amount, timestamp)` |
| `POS_SALE` | `position_listed_for_sale` | `invoice_id: u64, seller: &Address, price: i128` | `(invoice_id, seller, price, timestamp)` |
| `POS_SOLD` | `position_sold` | `invoice_id: u64, seller: &Address, buyer: &Address, price: i128` | `(invoice_id, seller, buyer, price, timestamp)` |
| `POS_XFER` | `position_transferred` | `invoice_id: u64, from: &Address, to: &Address` | `(invoice_id, from, to, timestamp)` |
| `SM_INIT` | `secondary_market_initialized` | `admin: &Address, financing_pool: &Address, fee_bps: u32` | `(admin, financing_pool, fee_bps, timestamp)` |
| `SM_PAUS` | `secondary_market_paused` | `` | `(timestamp)` |
| `SM_UNP` | `secondary_market_unpaused` | `` | `(timestamp)` |
| `SM_FEE` | `secondary_market_fee_updated` | `fee_bps: u32` | `(fee_bps, timestamp)` |
| `WTH_CAP_P` | `withdrawal_cap_proposed` | `admin: &Address, token: &Address, new_cap: i128` | `(admin, token, new_cap, timestamp)` |
| `WTH_CAP_U` | `withdrawal_cap_updated` | `admin: &Address, token: &Address, old_cap: i128, new_cap: i128` | `(admin, token, old_cap, new_cap, timestamp)` |
| `EMRG_DECL` | `emergency_declared` | `admin: &Address` | `(admin, timestamp)` |
| `EMRG_REVK` | `emergency_revoked` | `admin: &Address` | `(admin, timestamp)` |
| `AC_SET` | `access_control_updated` | `admin: &Address, access_control: &Address` | `(admin, access_control, timestamp)` |
| `RCP_PROP` | `recipient_proposed` | `admin: &Address, recipient: &Address` | `(admin, recipient, timestamp)` |
| `RCP_ALLOW` | `recipient_allowed` | `admin: &Address, recipient: &Address` | `(admin, recipient, timestamp)` |
| `RSRV_DISB` | `reserve_disbursed` | `caller: &Address, token: &Address, recipient: &Address, amount: i128` | `(caller, token, recipient, amount, timestamp)` |
| `SME_CL` | `sme_credit_limit_set` | `verifier: &Address, sme: &Address, credit_limit: i128` | `(verifier, sme, credit_limit, timestamp)` |
| `SME_REAS` | `sme_verifier_reassigned` | `admin: &Address, sme: &Address, new_verifier: &Address` | `(admin, sme, new_verifier, timestamp)` |
| `INV_FRZ` | `invoice_frozen` | `invoice_id: u64, admin: &Address` | `(admin, invoice_id, timestamp)` |
| `INV_UFRZ` | `invoice_unfrozen` | `invoice_id: u64, admin: &Address` | `(admin, invoice_id, timestamp)` |
| `CXL_REQ` | `cancellation_requested` | `invoice_id: u64, caller: &Address` | `(caller, invoice_id, timestamp)` |
| `MTD_DISP` | `metadata_mismatch_flagged` | `invoice_id: u64, challenger: &Address` | `(challenger, invoice_id, timestamp)` |
| `MTD_RES` | `metadata_dispute_resolved` | `invoice_id: u64, admin: &Address, upheld: bool` | `(admin, invoice_id, upheld, timestamp)` |
| `MTD_CORR` | `metadata_hash_corrected` | `invoice_id: u64, admin: &Address, old_hash: &Bytes, new_hash: &Bytes` | `(admin, invoice_id, old_hash, new_hash, timestamp)` |
| `MTD_CID` | `metadata_cid_updated` | `invoice_id: u64, sme: &Address, new_cid: &String` | `(sme, invoice_id, new_cid, timestamp)` |
| `DECAY_SET` | `decay_schedule_set` | `invoice_id: u64, seller: &Address, floor_price: i128, decay_end_ts: u64` | `(seller, invoice_id, floor_price, decay_end_ts, timestamp)` |
| `BID_SUBM` | `bid_submitted` | `invoice_id: u64, investor: &Address, bid_price: i128, amount: i128` | `(investor, invoice_id, bid_price, amount, timestamp)` |
| `BID_ACCP` | `bid_accepted` | `invoice_id: u64, seller: &Address, investor: &Address, bid_price: i128, amount: i128` | `(seller, invoice_id, investor, bid_price, amount, timestamp)` |
| `UPG_CNCL` | `upgrade_cancelled` | `admin: &Address` | `(admin, timestamp)` |
| `ADM_AUDIT` | `admin_action_audited` | `entry: &AdminAuditEntry` | `(entry.sequence, entry.actor, entry.action, entry.source, entry.timestamp)` |
| `ADM_AUDT` | `adm_audit` | `sequence: u64, action: soroban_sdk::String, actor: &Address, timestamp: u64` | `(sequence, action, actor, timestamp)` |
| `AUDT_CHK` | `audit_checkpoint` | `total_entries: u64, checksum: soroban_sdk::BytesN<32>, discarded_action: soroban_sdk::String, discarded_actor: &Address, discarded_timestamp: u64, discarded_sequence: u64` | `(total_entries, checksum, discarded_action, discarded_actor, discarded_timestamp, discarded_sequence)` |
| `MKT_SET` | `marketplace_configured` | `admin: &Address, marketplace: &Address` | `(admin, marketplace, timestamp)` |
| `SHARE_CRT` | `position_share_created` | `invoice_id: u64, original_investor: &Address, share_index: u32, amount: i128, new_owner: &Address` | `(invoice_id, original_investor, share_index, amount, new_owner, timestamp)` |
| `SHARE_TRF` | `position_share_transferred` | `invoice_id: u64, original_investor: &Address, share_index: u32, from: &Address, to: &Address` | `(invoice_id, original_investor, share_index, from, to, timestamp)` |
| `SHARE_SALE` | `share_listed_for_sale` | `invoice_id: u64, share_index: u32, price: i128` | `(invoice_id, share_index, price, timestamp)` |
| `SHARE_SOLD` | `share_sold` | `invoice_id: u64, share_index: u32, buyer: &Address, price: i128` | `(invoice_id, share_index, buyer, price, timestamp)` |
| `DISP_OPEN` | `dispute_opened` | `invoice_id: u64, challenger: &Address` | `(challenger, invoice_id, timestamp)` |
| `DISP_EVID` | `dispute_evidence_submitted` | `invoice_id: u64, evidence_cid: &String` | `(invoice_id, evidence_cid, timestamp)` |
| `DISP_RES` | `dispute_resolved` | `invoice_id: u64, resolver: &Address, upheld: bool` | `(resolver, invoice_id, upheld, timestamp)` |
| `DISP_FUND` | `dispute_funded` | `invoice_id: u64, amount: i128` | `(invoice_id, amount, timestamp)` |
| `DISP_PAY` | `dispute_payout` | `invoice_id: u64, amount: i128` | `(invoice_id, amount, timestamp)` |
| `SWP_CFG` | `sweep_splits_configured` | `admin: &Address, split_count: u32, timestamp: u64` | `(admin, split_count, timestamp)` |
| `SWP_EXEC` | `sweep_executed` | `token: &Address, destination: &Address, amount: i128, timestamp: u64` | `(token, destination, amount, timestamp)` |
| `TRN_CRTD` | `tranche_created` | `id: u64, creator: &Address, total_face_value: i128` | `(id, creator, total_face_value)` |
| `TRN_FNDD` | `tranche_funded` | `tranche_id: u64, investor: &Address, amount: i128` | `(tranche_id, investor, amount)` |
| `TOK_WLP` | `token_whitelist_proposed` | `actor: &Address, token: &Address` | `(actor, token, timestamp)` |
| `DEP_PROP` | `dependency_update_proposed` | `actor: &Address, field_tag: u32, new_address: &Address` | `(actor, field_tag, new_address, timestamp)` |
| `DEP_EXEC` | `dependency_updated` | `actor: &Address, field_tag: u32, old_address: &Address, new_address: &Address` | `(actor, field_tag, old_address, new_address, timestamp)` |
| `INV_CONC` | `investor_concentration_exceeded` | `invoice_id: u64, investor: &Address, prospective: i128, cap_bps: u32` | `(investor, invoice_id, prospective, cap_bps, timestamp)` |
| `LST_AMND` | `listing_amended` | `invoice_id: u64, caller: &Address, old_asking_price: i128, new_asking_price: i128, old_deadline: u64, new_deadline: u64` | `(caller, invoice_id, old_asking_price, new_asking_price, old_deadline, new_deadline, timestamp)` |
| `REG_INIT` | `registry_param_initialized` | `admin: &Address` | `(admin, timestamp)` |
| `PAR_PROP` | `param_update_proposed` | `proposer: &Address, key: u32, value: i128` | `(proposer, key, value, timestamp)` |
| `PAR_APRV` | `param_update_approved` | `approver: &Address, proposal_id: u64, approvals: u32` | `(approver, proposal_id, approvals, timestamp)` |
| `PAR_EXEC` | `param_updated` | `executor: &Address, key: u32, old_value: i128, new_value: i128` | `(executor, key, old_value, new_value, timestamp)` |
| `VEST_INIT` | `vesting_initialized` | `admin: &Address, token: &Address` | `(admin, token, timestamp)` |
| `VEST_GRANT` | `vesting_grant_created` | `admin: &Address, beneficiary: &Address, total_amount: i128, start_time: u64, cliff_duration: u64, vesting_duration: u64` | `(admin, beneficiary, total_amount, start_time, cliff_duration, vesting_duration, timestamp)` |
| `VEST_REL` | `vesting_tokens_released` | `beneficiary: &Address, amount: i128` | `(beneficiary, amount, timestamp)` |
| `VEST_REV` | `vesting_grant_revoked` | `admin: &Address, beneficiary: &Address, revoked_at: u64` | `(admin, beneficiary, revoked_at, timestamp)` |
<!-- END GENERATED EVENTS -->

## Migration Notes

Events use Soroban's `symbol_short!()` macro, which is more efficient than `Symbol::new()` for constants. All topic symbols are 8-14 characters and fit comfortably within the 32-character limit.

For backwards compatibility with deployed contracts, the internal event topic names remain unchanged—only new deployments follow this convention.

---

## Analytics Data Warehouse (#757)

The `services/analytics/` pipeline consumes the indexer database and writes
time-bucketed summary tables. This section documents the exact computation
formula for each metric so the warehouse output is reproducible and auditable.

### Source events

All metrics are derived from the events in the registry above. The pipeline
reads the indexer's normalized event table (one row per emitted event, keyed by
`(ledger, tx_hash, event_index)`) and never mutates it.

### Time bucketing

Every summary table is keyed by `(bucket_start, bucket_size)` where
`bucket_size` is one of `hour`, `day`, `week`. Buckets are aligned to UTC
epoch boundaries. A metric value for a bucket is computed from **all** events
whose `timestamp` falls in `[bucket_start, bucket_start + bucket_size)`.

### Metric definitions

| Metric | Table | Formula |
|---|---|---|
| **TVL over time** | `analytics_tvl` | `SUM(funded_amount)` over `INV_FUNDED` events in the bucket, minus `SUM(amount)` over `INV_REPAID` and `PROTOCOL_REFUND_CLAIMED` events in the bucket, plus the previous bucket's closing TVL. Defaulted invoices (`INV_DEFAULTED`) are removed from TVL at the bucket in which the default is recorded. |
| **Default rate by risk tier** | `analytics_default_rate` | For each risk tier `t` and bucket `b`: `defaults(t, b) / funded(t, b)`, where `defaults` counts distinct `invoice_id` in `INV_DEFAULTED` and `funded` counts distinct `invoice_id` in `INV_FUNDED`. Risk tier is taken from the most recent `RISK_SME_SCORE_UPDATED` / `RISK_SME_REGISTERED` event for the invoice's SME at or before the bucket end. |
| **Funding-to-repayment cycle time** | `analytics_cycle_time` | For each invoice repaid in the bucket: `INV_REPAID.timestamp - INV_FUNDED.timestamp` (seconds). The bucket value is the mean over all such invoices; `p50` and `p95` are also stored. |
| **Average yield** | `analytics_yield` | `SUM(PROTOCOL_YIELD_DIST.yield_amount) / SUM(INV_FUNDED.funded_amount)` for invoices funded in the bucket, annualized by `365 / mean_cycle_days`. |
| **SME retention / repeat-usage rate** | `analytics_sme_retention` | `distinct SMEs with >= 2 INV_CREATED events in the trailing 90 days / distinct SMEs with >= 1 INV_CREATED event in the trailing 90 days`, evaluated at each bucket end. |

### Late-arriving and backfilled data

Indexer data may arrive out of order (backfills, reorgs). The pipeline must
**recompute** affected buckets rather than append:

1. Each indexer event row carries the `ledger` at which it was observed.
2. The pipeline tracks a high-water mark per source table.
3. When a backfill inserts events with `ledger` below the high-water mark, the
   pipeline determines the earliest affected `bucket_start` and recomputes
   every bucket from there forward (idempotent upsert keyed by
   `(metric, bucket_start, bucket_size, dimensions)`).
4. Gaps in event history are tolerated: a bucket with no events yields a
   `NULL`/absent row rather than a zero, so downstream consumers can
distinguish "no activity" from "zero activity".

### Query API

The aggregated tables are exposed through an internal read-only query API
(`GET /internal/analytics/{metric}?from=&to=&bucket=&tier=`) consumed by the
frontend analytics dashboard (tracked separately). The API performs no
aggregation itself; it only filters and returns pre-computed rows.


---

## Subscription Authorization (#769)

The off-chain subscription service (`services/api/subscriptions/`) streams the
invoice events above to clients over Server-Sent Events. To prevent leaking
counterparty commercial data, **every stream is authorized against a verified
wallet/session before any event is delivered**.

### Authorization gate

| Step | Check | Failure response |
|---|---|---|
| 1 | Valid, non-expired session token (HMAC-signed, bound to a wallet address) | HTTP 401 `UNAUTHORIZED` |
| 2 | Requested topic is permitted and, for `position-status-change`, scoped to the authenticated wallet | HTTP 403 `FORBIDDEN` |
| 3 | Per-wallet new-connection rate limit not exceeded | HTTP 429 `RATE_LIMITED` + `Retry-After` |
| 4 | Mid-stream: session must not expire while the stream is open | `event: session_expired` + close |

The service never trusts a caller-supplied address. The wallet address is read
from the verified session token and passed to the in-process EventBus as the
subscriber identity, which enforces per-investor scoping for
`position-status-change`.

### Permitted subscription topics

The subscription service exposes three topics, each mapped from the on-chain
events in the registry above:

| Subscription topic | On-chain source | Scoping |
|---|---|---|
| `listing-funding-progress` | `INV_FUNDED` | Public funding progress per invoice |
| `position-status-change` | `POS_RECORDED`, `PROTOCOL_YIELD_DIST` | Scoped to the authenticated investor only |
| `repayment` | `PROTOCOL_REPAYMENT` | Repayment events per invoice |

A subscriber authenticated as wallet `A` may **not** request
`position-status-change` with `investorAddress=B`; the server auto-scopes the
stream to the authenticated wallet.

### Configuration

| Env var | Default | Description |
|---|---|---|
| `SESSION_SECRET` | _(required)_ | HMAC secret validating session tokens (≥16 chars; fail-closed) |
| `SUBSCRIPTION_RATE_LIMIT_PER_MIN` | `30` | Max new subscriptions per wallet per minute |

See `services/api/subscriptions/README.md` for the integration contract and
the mid-stream session-expiry eviction behavior.
