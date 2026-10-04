# TODO - Standardize events in financing pool contracts

## Plan (from inspection)
- [x] Replace local event emitters in `contracts/financing_pool/src/lib.rs` with calls to `kora_shared::events::*` so financing pool events are standardized.

- [ ] Remove unused local emit_* helpers and any duplicate/special event constants that conflict with shared event schema.
- [ ] Ensure every relevant financing pool state transition emits exactly one standardized event.
- [ ] Add tests for event emission for: pool creation, position recording, repayment, yield distribution, default marking.
- [ ] Cover edge cases: authorization failures, invalid bounds, arithmetic overflow paths.
- [ ] Ensure safe arithmetic usage remains intact (no silent overflows).
- [ ] Run `make fmt`.
- [ ] Run `make test`.

