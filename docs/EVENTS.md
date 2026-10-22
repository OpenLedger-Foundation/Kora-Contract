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
| `MKTPL_EXPIRED` | listing_expired | (seller, invoice_id, timestamp) | Funding deadline passed |

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
