# Governance bootstrap

`parameter_registry.bootstrap_from_live_contracts` is an admin-only, idempotent cutover operation. It reads the effective marketplace fee, financing-pool penalty/cap/grace values, and treasury fee from deployed peers. Missing registry keys are written; existing governance values are preserved unless the authorized caller passes `force=true`.

The deployment script calls bootstrap only after all peer contracts are initialized and wired to the registry. The contract emits `REG_BOOT` with the number of keys written and a checksum over the imported values. A second call without force is a no-op for already populated keys.
