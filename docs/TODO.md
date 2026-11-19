# TODO - Standardize events in financing pool contracts

## Plan (from inspection)
- [x] Replace local event emitters in `contracts/financing_pool/src/lib.rs` with calls to `kora_shared::events::*` so financing pool events are standardized.

- [x] Remove unused local emit_* helpers and any duplicate/special event constants that conflict with shared event schema.
  - `set_marketplace` now calls `events::marketplace_configured` instead of the bare `env.events().publish(("MKT_SET",), ...)`.
- [x] Ensure every relevant financing pool state transition emits exactly one standardized event.
- [ ] Add tests for event emission for: pool creation, position recording, repayment, yield distribution, default marking.
- [ ] Cover edge cases: authorization failures, invalid bounds, arithmetic overflow paths.
- [ ] Ensure safe arithmetic usage remains intact (no silent overflows).
- [ ] Run `make fmt`.
- [ ] Run `make test`.

## v2 Upgrade path (ARCHITECTURE.md)
- [x] `propose_upgrade` and `execute_upgrade` on `financing_pool` now check whether a
  multisig is configured on the linked `AccessControl` contract and reject the
  bare-admin path when one is found (closes #1076).

## Property tests (IMPROVEMENTS.md)
- [x] Table-driven and proptest coverage for fee + net identity and fund_invoice
  CEI ordering added in `contracts/tests/marketplace_fee_property.rs` (closes #1075).
