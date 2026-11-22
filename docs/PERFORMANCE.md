# Kora Protocol — Performance & WASM Size Budgets

This document outlines WASM size budgets, optimization guidelines, and the approval process for budget increases.

## Per-Contract WASM Size Budgets

Soroban enforces strict upper bounds on deployed WASM size. To avoid late-stage rework, Kora enforces committed per-contract size budgets in CI (`.github/workflows/wasm-size.yml` & `wasm-budgets.json`):

| Contract | Size Budget | Purpose / Description |
|---|---|---|
| `access_control` | 120 KB (122,880 B) | Role & multisig admin governance |
| `invoice_nft` | 120 KB (122,880 B) | NFT minting & lifecycle state |
| `risk_registry` | 120 KB (122,880 B) | SME risk profiles & verifier registry |
| `treasury` | 120 KB (122,880 B) | Protocol fees & reserve management |
| `financing_pool` | 150 KB (153,600 B) | Funding & yield distribution math |
| `marketplace` | 150 KB (153,600 B) | Two-phase listing & order matching |
| `price_oracle` | 100 KB (102,400 B) | FX & valuation feed aggregator |
| `tranche` | 100 KB (102,400 B) | Tranche tokenization |
| `dispute_resolution` | 120 KB (122,880 B) | Escrow dispute arbitration |
| `dispute_escrow` | 120 KB (122,880 B) | Custody of disputed repayments |
| `governance` | 120 KB (122,880 B) | Protocol upgrade proposals |
| `governance_voting` | 120 KB (122,880 B) | Governance ballots and quorum |
| `parameter_registry` | 120 KB (122,880 B) | Governed protocol parameters |
| `debtor_attestation` | 120 KB (122,880 B) | On-chain debtor identity attestation |
| `referral_rewards` | 120 KB (122,880 B) | Referrer fee tracking & distribution |
| `verifier_nomination` | 120 KB (122,880 B) | Community verifier nomination flow |
| `fractionalizer` | 120 KB (122,880 B) | Position fractionalisation & wrapping |
| `secondary_market` | 120 KB (122,880 B) | Secondary position sale offers |
| `vesting` | 120 KB (122,880 B) | Token vesting schedules & claims |

The JSON keys omit the `kora_` artifact prefix: `dispute_escrow` gates
`kora_dispute_escrow.optimized.wasm`, falling back to the release `.wasm`.
The escrow and governance limits start at the existing 120 KiB default to
leave room for SDK overhead and contract growth. They are provisional caps,
not measured headroom: no builds or measurements were run for #911/#912.
Record optimized release sizes from CI before ratcheting these caps downward.
The other limits reproduce the existing documented budgets above.

CI sets `REQUIRE_ALL_WASMS=1` for `scripts/check-wasm-size.sh` so a missing
artifact fails independently of budget overrides. Local callers may leave it
unset to inspect a partial build. `SIZE_THRESHOLD_BYTES` retains the separate
main-branch growth threshold in the workflow.

---

## Size Budget CI Enforcement & Override Process

1. **CI Verification**: Every PR triggers `.github/workflows/wasm-size.yml` which executes `scripts/check-wasm-size.sh`.
2. **Budget Breach**: If a compiled WASM exceeds its budget in `wasm-budgets.json`, CI fails with an overage report.
3. **Budget Increase Approval Process**:
   - If a feature legitimately requires increasing a contract's budget, apply the `wasm-size-increase-approved` label to the PR.
   - Update `wasm-budgets.json` in the PR with the new limit and document the justification in the PR description.
