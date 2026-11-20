# Security: Protocol Pause Enforcement Matrix

When the admin calls `access_control.pause()`, state-mutating entrypoints across the
protocol are gated by a cross-contract call to `AccessControlContract.is_paused()`.
Any blocked call returns `KoraError::ProtocolPaused`.

## Pause-Semantics Enforcement Matrix

This matrix covers **every contract in the workspace** (19 contracts, #1087). A
**✓ YES** row means the entrypoint calls `require_not_paused()`, directly or through
a private helper, and returns `ProtocolPaused` while `access_control.is_paused()` is
true. Contracts with no pause guard are marked **N/A**. **✗ NO** rows record critical
mutators that are intentionally *not* guarded.

The ✓ rows are generated from the source and enforced in CI by
`cargo run -p kora-xtask --bin check_pause_matrix`. The check fails if a guarded
entrypoint is missing here, a ✓/✗ row disagrees with the code, or a contract is not
listed. For incident procedure, see [EMERGENCY_PAUSE_DRILL.md](EMERGENCY_PAUSE_DRILL.md).

**Summary:** 64 pause-guarded entrypoints across 9 contracts (`dispute_resolution`,
`financing_pool`, `fractionalizer`, `invoice_nft`, `marketplace`, `price_oracle`,
`risk_registry`, `secondary_market`, `treasury`). 10 contracts are N/A.

| Contract | Entrypoint | Paused? | Notes |
|---|---|---|---|
| **access_control** | — | N/A | Hosts the pause itself (`pause` / `unpause` / `emergency_pause_by_guardian`). Role and multisig management must keep working during an incident |
| **debtor_attestation** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |
| **dispute_escrow** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |
| **dispute_resolution** | `open_dispute` | ✓ YES |  |
| **dispute_resolution** | `resolve_dispute` / `submit_evidence` | ✗ NO | Open disputes can still be progressed and closed |
| **financing_pool** | `accept_early_settlement` | ✓ YES |  |
| **financing_pool** | `buy_position` | ✓ YES |  |
| **financing_pool** | `buy_share` | ✓ YES |  |
| **financing_pool** | `list_position_for_sale` | ✓ YES |  |
| **financing_pool** | `list_share_for_sale` | ✓ YES |  |
| **financing_pool** | `mark_default` | ✓ YES |  |
| **financing_pool** | `net_settle` | ✓ YES |  |
| **financing_pool** | `propose_early_settlement` | ✓ YES |  |
| **financing_pool** | `record_position` | ✓ YES |  |
| **financing_pool** | `release_funds` | ✓ YES | No new pool positions |
| **financing_pool** | `set_installment_schedule` | ✓ YES |  |
| **financing_pool** | `split_position` | ✓ YES |  |
| **financing_pool** | `transfer_position` | ✓ YES |  |
| **financing_pool** | `transfer_share` | ✓ YES |  |
| **financing_pool** | `repay` / `repay_partial` / `execute_repayment` | ✗ NO | Settlement path, not guarded itself. **⚠ It calls `invoice_nft.set_repaid`, which *is* pause-guarded, so repayment that closes an invoice is effectively blocked while paused.** See the note below. |
| **financing_pool** | `cancel_early_settlement` | ✗ NO | Unwinds a pending proposal; must stay available |
| **fractionalizer** | `unwrap` | ✓ YES |  |
| **fractionalizer** | `wrap` | ✓ YES |  |
| **fractionalizer** | `redeem` / `settle` / `transfer_shares` | ✗ NO | Outbound / settlement paths |
| **governance** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |
| **governance_voting** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |
| **invoice_nft** | `amend_invoice` | ✓ YES |  |
| **invoice_nft** | `commit_metadata_hash` | ✓ YES |  |
| **invoice_nft** | `flag_metadata_mismatch` | ✓ YES |  |
| **invoice_nft** | `mint_invoice` | ✓ YES | No new invoices |
| **invoice_nft** | `mint_invoice_with_referrer` | ✓ YES |  |
| **invoice_nft** | `mint_invoices_batch` | ✓ YES |  |
| **invoice_nft** | `refresh_risk_score` | ✓ YES |  |
| **invoice_nft** | `set_created` | ✓ YES |  |
| **invoice_nft** | `set_funded` | ✓ YES |  |
| **invoice_nft** | `set_listed` | ✓ YES |  |
| **invoice_nft** | `set_repaid` | ✓ YES | ⚠ Blocks `financing_pool` repayment flows (see note) |
| **invoice_nft** | `update_metadata_cid` | ✓ YES |  |
| **invoice_nft** | `withdraw_invoice` | ✓ YES |  |
| **invoice_nft** | `set_defaulted` | ✗ NO | Admin must be able to record defaults during a pause |
| **invoice_nft** | `freeze_invoice` / `unfreeze_invoice` / `freeze_sme_invoices` / `unfreeze_sme_invoices` | ✗ NO | Incident-response tools; must work while paused |
| **invoice_nft** | `migrate` | ✗ NO | Reserved for upgrades |
| **marketplace** | `amend_listing` | ✓ YES |  |
| **marketplace** | `fund_invoice` | ✓ YES | No new capital inflows |
| **marketplace** | `fund_invoices_batch` | ✓ YES |  |
| **marketplace** | `list_invoice` | ✓ YES | No new listings |
| **marketplace** | `cancel_listing` / `withdraw_listing` / `request_cancellation` | ✗ NO | Sellers can always withdraw a listing |
| **marketplace** | `claim_refund` | ✗ NO | Investors can always recover funds from expired listings |
| **marketplace** | `set_fee_bps` / `update_fee_bps` | ✗ NO | Admin configuration |
| **parameter_registry** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |
| **price_oracle** | `set_price` | ✓ YES | No price updates during a pause |
| **price_oracle** | `add_feeder` / `remove_feeder` / `clear_peg_flag` | ✗ NO | Admin feed management; removing a compromised feeder must work while paused |
| **referral_rewards** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |
| **risk_registry** | `add_sub_account` | ✓ YES |  |
| **risk_registry** | `add_verifier` | ✓ YES |  |
| **risk_registry** | `cancel_verifier_removal` | ✓ YES |  |
| **risk_registry** | `clear_circuit_breaker_debtor` | ✓ YES |  |
| **risk_registry** | `clear_circuit_breaker_tier` | ✓ YES |  |
| **risk_registry** | `configure_circuit_breaker` | ✓ YES |  |
| **risk_registry** | `configure_slashing` | ✓ YES |  |
| **risk_registry** | `finalize_verifier_removal` | ✓ YES |  |
| **risk_registry** | `raise_dispute` | ✓ YES |  |
| **risk_registry** | `reassign_sme_verifier` | ✓ YES |  |
| **risk_registry** | `record_default` | ✓ YES | Deferred until post-pause review |
| **risk_registry** | `record_successful_repayment` | ✓ YES |  |
| **risk_registry** | `register_sme` | ✓ YES | No new SME onboarding |
| **risk_registry** | `reinstate_verifier` | ✓ YES |  |
| **risk_registry** | `remove_sub_account` | ✓ YES |  |
| **risk_registry** | `remove_verifier` | ✓ YES |  |
| **risk_registry** | `request_verifier_removal` | ✓ YES |  |
| **risk_registry** | `resolve_dispute` | ✓ YES |  |
| **risk_registry** | `set_credit_limit` | ✓ YES |  |
| **risk_registry** | `set_debtor_score` | ✓ YES |  |
| **risk_registry** | `set_min_debtor_score` | ✓ YES |  |
| **risk_registry** | `suspend_verifier` | ✓ YES |  |
| **risk_registry** | `top_up_stake` | ✓ YES |  |
| **risk_registry** | `update_risk_tier_definition` | ✓ YES |  |
| **risk_registry** | `update_sme_score` | ✓ YES |  |
| **risk_registry** | `increment_invoice_count` | ✗ NO | Called by `invoice_nft.mint_invoice`, which is itself pause-guarded |
| **secondary_market** | `buy_position` | ✓ YES |  |
| **secondary_market** | `list_position` | ✓ YES |  |
| **secondary_market** | `cancel_listing` | ✗ NO | Sellers can always withdraw |
| **secondary_market** | `pause` / `unpause` | ✗ NO | Contract-local pause controls |
| **tranche** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |
| **treasury** | `execute_treasury_action` | ✓ YES |  |
| **treasury** | `withdraw` | ✓ YES | Admin treasury withdrawal blocked |
| **treasury** | `collect_fee` | ✗ NO | Inbound fee accrual from settlement flows; not blocked |
| **treasury** | `emergency_withdraw` / `declare_emergency` | ✗ NO | Emergency tooling must work during a pause |
| **verifier_nomination** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |
| **vesting** | — | N/A | No `require_not_paused` in this contract; it is outside the protocol pause scope. Review before adding fund-moving entrypoints |

> `secondary_market` gates `list_position` / `buy_position` with `require_not_paused`,
> and also exposes its own `pause` / `unpause` controls.

### ⚠ Known gap: repayment during a pause

The principle below says settlements must not be blocked. `financing_pool.repay`,
`repay_partial` and `execute_repayment` are not pause-guarded themselves. But when a
repayment closes an invoice they call `invoice_nft.set_repaid`, which **is**
pause-guarded, so the whole call reverts with `ProtocolPaused`. In practice, final
repayment is blocked while the protocol is paused. Treat this as a decision for the
maintainers: either exempt `set_repaid` when the caller is `financing_pool`, or update
the principle. This matrix documents the current behaviour and does not change it.

## Pause Exemptions — Rationale

**Core Principle:** The pause blocks *inbound activity* (new invoices, new funding,
new positions, new risk inputs, price updates) and is meant to leave *outbound
settlement* (refunds, cancellations, emergency withdrawals) available. This protects
existing participants while preventing new exposure during an emergency. Note the
repayment gap above.

Intentional exemptions:

1. **`marketplace.cancel_listing` / `withdraw_listing` / `claim_refund`.** Sellers can always withdraw, and investors can always recover funds from expired listings.
2. **`invoice_nft.set_defaulted` and the freeze tools.** The admin must be able to record defaults and freeze invoices during an incident.
3. **`treasury.emergency_withdraw` / `declare_emergency`.** Emergency tooling must work while paused. Note that ordinary `treasury.withdraw` **is** paused.
4. **`price_oracle` feeder management.** A compromised feeder can be removed while paused, even though `set_price` itself is paused.
5. **`access_control`.** The pause mechanism, role management and multisig governance are never paused.

**Rationale:** The pause is a *circuit-breaker for new economic activity*, not a
freeze of all state.

**Changed from the previous matrix (7 contracts):** `risk_registry` mutators (`register_sme`, `set_debtor_score`, `record_default`, verifier
management, …), `price_oracle.set_price` and `treasury.withdraw` are now pause-guarded
(issues #906 and later). The previous table listed them as not paused.

## Verification & Testing

**Matrix Accuracy:** `cargo run -p kora-xtask --bin check_pause_matrix` checks this matrix against the source of every contract under `contracts/*/src` on every CI run. It parses the code; it does not trust this file. An entrypoint counts as pause-guarded if it calls `require_not_paused` (or checks `is_paused` and returns a `*Paused` error), directly or through a private helper.

**Integration Test Coverage:** The enforcement matrix is validated by the test suite:

- `test_pause_enforcement_matrix` in `contracts/tests/src/lib.rs` — Calls each blocked entrypoint while paused and confirms `ProtocolPaused` is returned
- Confirms exempt entrypoints (like `repay`, `claim_refund`, `cancel_listing`) execute successfully during pause
- Verifies unpause restores normal operation

**How to Verify:** To confirm the matrix matches current code, run:

```bash
cargo test -p kora-marketplace test_pause_enforcement_matrix -- --nocapture
cargo test -p kora-financing-pool --test \* -- --nocapture  
```

If a function's pause status changes, `check_pause_matrix` fails until this matrix is updated.

---

## Cross-Contract Authorization

See [ARCHITECTURE.md § Cross-Contract Authorization Matrix](ARCHITECTURE.md#cross-contract-authorization-matrix) for a detailed table of all cross-contract calls in the protocol, including the authorization required for each call.

Key insight: authorization is transitive. When a user calls `marketplace.fund_invoice()` with their signature, the marketplace contract calls `financing_pool.release_funds()` with its own address (`env.current_contract_address()`), which then calls `invoice_nft.set_funded()` also with the pool's address. This three-level call chain is secure because each step is verified by the callee (via `require_auth()`).
