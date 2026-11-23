# Tier-gated debtor attestation (#1084)

Debtor attestation is governed per invoice risk tier through `parameter_registry`. Values are `0` for optional and `1` for required. Defaults are AAA optional, AA optional, A optional, B required, and C required.

Marketplace reads the policy at `list_invoice` and every `fund_invoice` path. Governance changes therefore take effect without redeploying marketplace WASM, and a tier upgrade cannot bypass a newly mandatory requirement. If policy resolution fails (paused, missing, or unreadable registry), marketplace fails closed and treats the requirement as mandatory. Optional policy skips the attestation client call entirely.

The `DBT_POL` event records invoice ID, tier, decision, and timestamp. `DBT_ATST` records successful verification. Missing attestation and confirmed-but-mismatched amounts retain distinct errors (`DebtorNotAttested` and `AttestationAmountMismatch`).

| Tier | Default | Registry key |
|---|---|---|
| AAA | Optional | `DebtorAttestationAaa` |
| AA | Optional | `DebtorAttestationAa` |
| A | Optional | `DebtorAttestationA` |
| B | Required | `DebtorAttestationB` |
| C | Required | `DebtorAttestationC` |

Governance may override each key only within `0..=1`. A paused, missing, or unreadable registry is treated as required.

## Implementation (issue #1084)

### Tier comparison

The gate is implemented in `marketplace::require_debtor_attestation_if_needed`.
It maps the invoice's `RiskTier` to the corresponding `RegistryParamKey`, reads
the value from `parameter_registry`, and:

- `value == 0` → returns `Ok(())` immediately (optional; no cross-contract call).
- `value == 1` → calls `debtor_attestation.is_confirmed(invoice_id, face_value)`.
  `is_confirmed` returns `true` only when the record is `Confirmed`, not expired,
  and the amount matches exactly.
- Read failure → treated as required (fails closed).

If no `debtor_attestation` contract is wired (staging) the gate is a no-op for
all tiers, regardless of the registry value. This prevents hardcoding a live
attestation address in environments that don't have one deployed.

### Error types

| Condition | Error |
|---|---|
| Attestation absent or pending/rejected/expired | `KoraError::DebtorNotAttested` |
| Attestation confirmed but `amount != face_value` | `KoraError::AttestationAmountMismatch` |

Both are typed contract errors — no host traps, no panics.

### Tests (contracts/marketplace/src/lib.rs)

| Test | Coverage |
|---|---|
| `tier_below_threshold_lists_without_attestation` | A-tier (optional) with no attestation contract → not blocked |
| `b_tier_without_attestation_fails_typed_error` | B-tier (required) without attestation → `DebtorNotAttested` |
| `b_tier_with_valid_attestation_lists_successfully` | B-tier with valid attestation → allowed |
| `tier_boundary_equality_b_requires_attestation` | Exact B-tier boundary score, no attestation → `DebtorNotAttested` |
| `governance_can_make_a_tier_optional` | Registry B-key set to 0 at runtime → B-tier no longer blocked |

### Attestation expiry

An expired attestation (`now > expires_at`) returns `Expired` from
`attestation_status` and `false` from `is_confirmed`. Marketplace then returns
`DebtorNotAttested`, not `AttestationAmountMismatch`. The debtor must re-attest
with a new expiry before the invoice can be listed again.

## Expiry sweep job (issue #1150)

Expired attestations otherwise linger silently and confuse tier gates. The
keeper runs a periodic **attestation expiry sweep** that walks the existing
`debtor_attestation` records and reports which ones have lapsed, so operators
can alert and prompt debtors to re-attest. The sweep is read-only against the
contract — it never mutates attestation state and never bypasses the
marketplace gate.

### Behavior

- Uses the existing keeper job pattern in `services/keeper/` (same scheduler,
  config loading, and metrics plumbing as other keeper jobs). No parallel
  subsystem is introduced.
- Reads attestations through the existing `contracts/debtor_attestation/`
  module (`attestation_status` / `is_confirmed`); an attestation is considered
  expired when `now > expires_at`.
- Emits metrics on each run:
  - `attestation_sweep_expired_total` — count of expired attestations found.
  - `attestation_sweep_failures_total` — count of records that could not be
    read (RPC/contract errors).
  - `attestation_sweep_last_success_timestamp` — last successful sweep time.
- Emits an alert when `attestation_sweep_expired_total > 0` or when
  `attestation_sweep_failures_total > 0`, so lapsed attestations and degraded
  reads are both surfaced.

### Edge cases

- **Paused / degraded dependency:** if the attestation contract or its RPC is
  paused or unreachable, the sweep records the failure in
  `attestation_sweep_failures_total`, emits an alert, and does not advance
  `attestation_sweep_last_success_timestamp`. It never treats an unreadable
  record as valid.
- **Empty or missing config:** a missing attestation contract address or sweep
  interval fails closed with a typed config error at startup; the job does not
  run with partial configuration.
- **Idempotent re-entry:** the sweep is read-only and keyed by attestation ID,
  so re-running it (retries, overlapping schedules) produces the same counts
  and does not double-count or mutate state.

### Ops / env

The sweep reuses the keeper's existing attestation contract address and RPC
configuration; no new required env vars are introduced. Operators should wire
`attestation_sweep_expired_total` and `attestation_sweep_failures_total` into
alerting so expired attestations are actioned promptly.
