# Schema Migration Runbook

This document describes the process for safely evolving `#[contracttype]` struct
schemas in the Kora Protocol. Read this before adding, removing, or reordering
fields on `Invoice`, `Pool`, `Position`, `SmeProfile`, `Listing`, or any other
persisted type in `kora-shared/src/types.rs`.

---

## Why schema changes are binary-incompatible

Every `#[contracttype]` struct is encoded with Soroban's XDR codec.  The codec
uses **positional field encoding** — field N in the struct corresponds to field N
in the wire format, with no field names embedded.

Consequences:
- Adding a field (even at the end) changes the total field count.  Existing
  records encoded without that field will panic when deserialized under the new
  struct definition.
- Removing a field shifts every field that came after it.
- Reordering fields produces silent data corruption (values decode into the wrong
  fields without error).

There is no built-in versioning or schema evolution.  Every change requires an
explicit migration that reads old records with the old struct definition and
rewrites them with the new one.

---

## The migration pattern

### Step 1 — define the legacy struct

Before changing the live struct in `kora-shared/src/types.rs`, copy its current
definition into the contract that owns the data (e.g. `invoice_nft/src/lib.rs`)
under a versioned name such as `InvoiceV1`.  Annotate it with `#[contracttype]`
so it uses the same XDR codec as the original.

```rust
// contracts/invoice_nft/src/lib.rs
#[contracttype]
#[derive(Clone)]
pub struct InvoiceV1 {
    pub id: u64,
    pub sme: Address,
    // ... all fields as they were BEFORE the change ...
    pub repaid_at: Option<u64>,
    // no `notes` field here
}
```

### Step 2 — update the live struct

Add (or remove, or reorder) the field in `kora-shared/src/types.rs`.  Update the
schema-version comment on the struct.

```rust
// kora-shared/src/types.rs
/// Schema version: 2
#[contracttype]
pub struct Invoice {
    // ... all previous fields ...
    pub repaid_at: Option<u64>,
    /// Added in schema v2.
    pub notes: Option<String>,
}
```

### Step 3 — update all construction sites

Every place that constructs the struct must now supply the new field.  For new
mints, the value is usually the caller-supplied argument or a sensible default.

```rust
// contracts/invoice_nft/src/lib.rs — mint_invoice
let invoice = Invoice {
    // ... existing fields ...
    repaid_at: None,
    notes,           // new parameter propagated from the function signature
};
```

### Step 4 — implement the migration in `migrate()`

Gate the migration on the stored `MigrationVersion` so it is idempotent:

```rust
// Version 1 -> 2: Invoice gained `notes: Option<String>`.
if current_version < 2 {
    let next_id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
    let mut id = 1u64;
    while id < next_id {
        let key = DataKey::Invoice(id);
        if let Some(old) = env.storage().persistent().get::<DataKey, InvoiceV1>(&key) {
            let upgraded = Invoice {
                id: old.id,
                sme: old.sme,
                // ... copy every field ...
                repaid_at: old.repaid_at,
                notes: None,   // backfill with the default for old records
            };
            env.storage().persistent().set(&key, &upgraded);
        }
        id += 1;
    }
    env.storage().instance().set(&DataKey::MigrationVersion, &2u32);
}
```

### Step 5 — deploy and run the migration

**Order of operations is critical:**

1. Prepare and test the new WASM locally.
2. Upgrade the contract on-chain (`upgrade_wasm`).
3. Call `migrate()` **before any other user interaction**.
   - Until `migrate()` runs, any read of an old record as the new struct will
     panic and abort the transaction.
   - Any new write (e.g. a new `mint_invoice`) will produce a v2 record that
     cannot be read by the migration's `InvoiceV1` decoder.
4. Verify by calling `get_invoice()` on a few known IDs and confirming the new
   field is present with its default value.
5. Once all records have been upgraded, the `InvoiceV{N}` legacy struct can be
   removed in the next upgrade cycle.

### Step 6 — clean up

After all nodes have called `migrate()` and you are satisfied that no old records
remain, remove the legacy struct in your next PR.

---

## Worked example: adding `notes: Option<String>` to `Invoice`

This is the concrete change shipped as schema v2.

| | v1 (original) | v2 (current) |
|---|---|---|
| struct | `Invoice` (no `notes`) | `Invoice` with `notes: Option<String>` |
| legacy | — | `InvoiceV1` (copy of v1, kept for migration) |
| `MigrationVersion` after `migrate()` | 1 | 2 |
| backfill value | n/a | `notes = None` |

**Files changed:**
- `contracts/shared/src/types.rs` — added `notes` field, updated doc comment
- `contracts/invoice_nft/src/lib.rs` — added `InvoiceV1`, updated `migrate()`,
  updated `mint_invoice` signature (new `notes: Option<String>` parameter)

**Tests:**
- `test_migrate_v1_to_v2_backfills_notes_field` — writes a raw `InvoiceV1`
  record to persistent storage, resets `MigrationVersion` to 1, calls
  `migrate()`, then asserts `get_invoice()` returns a valid v2 record with
  `notes = None`.

---

## Checklist for every schema migration

- [ ] Legacy struct `Foo_V{N}` added with `#[contracttype]` BEFORE changing `Foo`
- [ ] New field added to `Foo` in `kora-shared/src/types.rs`
- [ ] All construction sites updated
- [ ] `migrate()` gate added: `if current_version < N+1 { ... }`
- [ ] `MigrationVersion` bumped to `N+1` at the end of the gate
- [ ] Test added that writes a raw `FooV{N}` record and asserts readable post-migration
- [ ] Upgrade → `migrate()` ordering documented in the deployment PR
- [ ] Legacy struct removal scheduled for the following upgrade cycle

---

## Version history

| Version | Change | Migration gate | File |
|---------|--------|---------------|------|
| 1 | Baseline — all original fields | v0 → v1 (no-op, sets version) | `invoice_nft::migrate` |
| 2 | `Invoice.notes: Option<String>` added | v1 → v2, backfills `notes = None` | `invoice_nft::migrate` |

---

## Contracts with mutable on-chain state

All structs below are `#[contracttype]` and stored live on-chain.  Any field
change requires a migration following the pattern above.

| Struct | Contract | Storage key |
|--------|----------|-------------|
| `Invoice` | `invoice_nft` | `DataKey::Invoice(u64)` (persistent) |
| `Pool` | `financing_pool` | `DataKey::Pool(u64)` (persistent) |
| `Position` | `financing_pool` | `DataKey::Positions(u64)` (persistent, inside `Map`) |
| `SmeProfile` | `risk_registry` | `DataKey::SmeProfile(Address)` (persistent) |
| `Listing` | `marketplace` | `DataKey::Listing(u64)` (persistent) |
| `Proposal` | `access_control` | `DataKey::Proposal(u64)` (persistent) |
| `MultisigConfig` | `access_control` | `DataKey::MultisigConfig` (instance) |
