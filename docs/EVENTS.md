# Kora Protocol Event Naming Convention

This document defines the standardized event topic naming convention across all Kora contracts, ensuring consistent event identification for indexers, dashboards, and monitoring systems.

---

## Schema Versioning (#583, #1152)

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

### Consumer compatibility contract (#1152)

Indexers must **fail loudly** rather than silently mis-index when they encounter an
event they cannot interpret. The shared event helpers expose a single classification
point so every consumer behaves identically:

```rust
use kora_shared::events::{classify_event, EventCompat, EVENT_SCHEMA_VERSION};

match classify_event(&topics) {
    EventCompat::Current { topic } => { /* parse payload with current schema */ }
    EventCompat::Legacy { topic } => { /* pre-v1: no SCHEMA_V topic */ }
    EventCompat::Unsupported { topic, version } => {
        // version > EVENT_SCHEMA_VERSION: halt indexing for this stream and alert.
        // Do NOT attempt to parse the payload — its shape is unknown.
        panic!("unsupported event schema v{version} for {topic:?}");
    }
    EventCompat::Malformed { reason } => {
        // SCHEMA_V present but version topic missing/not a u32: fail closed.
        panic!("malformed versioned event: {reason}");
    }
}
```

Compatibility rules:

| Observed topics | Classification | Consumer action |
|---|---|---|
| `(SCHEMA_V, topic, v)` with `v == EVENT_SCHEMA_VERSION` | `Current` | Parse payload normally |
| `(SCHEMA_V, topic, v)` with `v < EVENT_SCHEMA_VERSION` | `Unsupported` | Fail loudly — no downgrade parser is shipped |
| `(SCHEMA_V, topic, v)` with `v > EVENT_SCHEMA_VERSION` | `Unsupported` | Fail loudly — upgrade the indexer |
| `(SCHEMA_V, topic)` — version topic missing or not `u32` | `Malformed` | Fail closed with a typed error |
| `(topic)` — no `SCHEMA_V` sentinel | `Legacy` | Treat as pre-v1; parse only if a legacy parser is configured |

These rules are pinned by `contracts/shared/src/events.rs` unit tests
(`classify_event_*`) and by the indexer compatibility tests in
`services/indexer/`, which assert that an unknown version aborts the batch instead
of writing partial state.

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

### Vesting Contract (VEST_*)

| Topic | Function | Payload | Description |
|-------|----------|---------|-------------|
| `VEST_INIT` | vesting_initialized | (admin, token, timestamp) | Vesting c

/* … truncated 5316 chars — edit only what you need near the top … */
