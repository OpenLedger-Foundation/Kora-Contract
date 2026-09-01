# Event Snapshots / Golden Files

This directory contains golden files for event schemas emitted by Kora Protocol contracts.
These files protect downstream consumers (SDKs, indexers, analytics) from unintended schema changes.

## Purpose

Golden file testing (snapshot testing) captures the serialized structure of events into committed files.
Tests fail when an event's structure changes unexpectedly, preventing silent schema drift that breaks
downstream integrations.

## File Organization

- `{contract}_{EventType}.json` — Golden file for a specific event type
  - Example: `invoice_nft_InvoiceMinted.json`
  - Example: `financing_pool_PoolCreated.json`
  - Example: `treasury_FeeCollected.json`

## File Format

Golden files are JSON documents containing the event schema (fields and types).
Non-deterministic fields like timestamps are normalized away.

```json
{
  "contract": "invoice_nft",
  "event": "InvoiceMinted",
  "fields": {
    "invoice_id": "u64",
    "sme": "Address",
    "amount": "i128",
    "currency": "Symbol",
    "due_date": "u64",
    "risk_score": "u32",
    "ipfs_cid": "String",
    "debtor_hash": "Bytes"
  }
}
```

## Test Behavior

The snapshot testing process:

1. **Emit** an event from the contract
2. **Serialize** the event structure to JSON
3. **Normalize** non-deterministic fields (timestamps, ledger info)
4. **Compare** against the golden file
5. **FAIL** if mismatch (schema changed unexpectedly)
6. **PASS** if match (event schema is stable)

## Updating Golden Files

To intentionally update golden files after a schema change:

```bash
UPDATE_GOLDEN_FILES=1 cargo test
```

This updates all golden files to match current event schemas.

### Update Workflow

1. Make intentional schema change in contract code
2. Run: `UPDATE_GOLDEN_FILES=1 cargo test`
3. Review changes: `git diff contracts/tests/event_snapshots/`
4. Verify only intentional changes are present
5. Commit separately: `git commit -m "refactor: Update event schemas"`

## Normalized Fields

The following fields are normalized away before snapshot comparison
(they are non-deterministic across test runs):

- `timestamp`
- `created_at`
- `updated_at`
- `funded_at`
- `repaid_at`
- `ledger_sequence`
- `block_height`
- Addresses (normalized to `Address(placeholder)`)

## Deployment Changes

When updating event schemas:

1. **Before**: Event version 1 (old schema)
2. **During**: Emit events with both old and new fields (if possible)
3. **After**: Event version 2 (new schema)

Backwards compatibility allows downstream consumers time to upgrade.

## Consumer Protection

Snapshot testing provides protection against:

- Accidental field renames
- Accidental field removal
- Accidental field type changes
- Unintended schema drift

This ensures SDKs and indexers don't break unexpectedly due to silent schema changes.

## Adding New Events

When adding a new event type:

1. Emit the event from the contract
2. Add golden file: `contracts/tests/event_snapshots/{contract}_{EventType}.json`
3. Add snapshot test in `contracts/tests/issue_682_event_snapshot_testing.rs`
4. Commit both together

Example:

```bash
# After adding InvoiceFrozen event to invoice_nft
git add contracts/tests/event_snapshots/invoice_nft_InvoiceFrozen.json
git commit -m "feat(invoice-nft): Add InvoiceFrozen event and snapshot"
```
