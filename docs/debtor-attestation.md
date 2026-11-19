# Tier-gated debtor attestation

Debtor attestation is governed per invoice risk tier through `parameter_registry`. Values are `0` for optional and `1` for required. Defaults are AAA optional, AA optional, A optional, B required, and C required.

Marketplace reads the policy at `list_invoice` and every `fund_invoice` path. Governance changes therefore take effect without redeploying marketplace WASM, and a tier upgrade cannot bypass a newly mandatory requirement. If policy resolution fails, marketplace fails closed and treats the requirement as mandatory. Optional policy skips the attestation client call.

The `DBT_POL` event records invoice ID, tier, decision, and timestamp. `DBT_ATST` records successful verification. Missing attestation and confirmed-but-mismatched amounts retain distinct errors (`DebtorNotAttested` and `AttestationAmountMismatch`).

| Tier | Default | Registry key |
|---|---|---|
| AAA | Optional | `DebtorAttestationAaa` |
| AA | Optional | `DebtorAttestationAa` |
| A | Optional | `DebtorAttestationA` |
| B | Required | `DebtorAttestationB` |
| C | Required | `DebtorAttestationC` |

Governance may override each key only within `0..=1`. A paused, missing, or unreadable registry is treated as required.
