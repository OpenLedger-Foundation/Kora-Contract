# Security: Protocol Pause Enforcement Matrix

When the admin calls `access_control.pause()`, state-mutating entrypoints across the
protocol are gated by a cross-contract call to `AccessControlContract.is_paused()`.
Any blocked call returns `KoraError::ProtocolPaused`.

## Pause-Semantics Enforcement Matrix

This matrix documents **every state-mutating entrypoint** across all seven Kora contracts. Verified against implementation in each contract's `require_not_paused()` checks.

### Core Protocol Contracts

| Contract          | Entrypoint            | Blocked | Rationale                                                    |
|-------------------|-----------------------|---------|--------------------------------------------------------------|
| **invoice_nft**   | `mint_invoice`        | ✓ YES   | No new invoices during emergency pause                       |
| **invoice_nft**   | `set_listed`          | ✓ YES   | Paused: state transitions during minting blocked             |
| **invoice_nft**   | `set_funded`          | ✓ YES   | Paused: state transitions during funding blocked             |
| **invoice_nft**   | `set_repaid`          | ✗ NO    | EXEMPT: repayment settlement must complete; blocking harms investors |
| **invoice_nft**   | `set_defaulted`       | ✗ NO    | EXEMPT: admin must mark defaults even during pause           |
| **invoice_nft**   | `migrate`             | ✗ NO    | Not pause-guarded; reserved for contract upgrades            |
| **marketplace**   | `list_invoice`        | ✓ YES   | No new listings during emergency pause                       |
| **marketplace**   | `fund_invoice`        | ✓ YES   | No new capital inflows during emergency pause                |
| **marketplace**   | `cancel_listing`      | ✗ NO    | EXEMPT: sellers must retain ability to withdraw invoices     |
| **marketplace**   | `claim_refund`        | ✗ NO    | Not pause-guarded; investors must recover expired funds      |
| **marketplace**   | `set_fee_bps`         | ✗ NO    | Admin function; fee config changes are not pause-guarded     |
| **marketplace**   | `whitelist_token`     | ✗ NO    | Admin function; token management operates independently      |
| **marketplace**   | `remove_token_whitelist` | ✗ NO  | Admin function; token management operates independently      |
| **financing_pool**| `release_funds`       | ✓ YES   | No new pool positions during emergency pause                 |
| **financing_pool**| `record_position`     | ✓ YES   | No new investor positions during emergency pause             |
| **financing_pool**| `repay`               | ✗ NO    | EXEMPT: SMEs must always be able to repay; blocking punishes SMEs and investors |
| **financing_pool**| `mark_default`        | ✓ YES   | Default processing deferred until admin review post-pause     |

### Administrative Contracts (Non-Pauseable)

| Contract          | Entrypoint               | Blocked | Rationale                                            |
|-------------------|--------------------------|---------|------------------------------------------------------|
| **access_control**| `pause` / `unpause`      | ✗ NO    | The pause mechanism itself is never paused           |
| **access_control**| `grant_role` / `revoke_role` | ✗ NO  | Role management is independent of protocol pause     |
| **access_control**| `transfer_admin`         | ✗ NO    | Admin change must be executable even during pause    |
| **access_control**| `configure_multisig`     | ✗ NO    | Governance config changes operate independently      |
| **access_control**| `propose_action` / `approve_action` / `execute_action` | ✗ NO | Multisig governance must not be blocked by pause |
| **risk_registry** | `register_sme`           | ✗ NO    | SME registration independent of protocol pause       |
| **risk_registry** | `update_sme_score`       | ✗ NO    | Risk scoring operations are always available         |
| **risk_registry** | `increment_invoice_count`| ✗ NO    | Risk tracking operations are always available        |
| **risk_registry** | `record_default`         | ✗ NO    | Called by pool during defaults; not directly paused  |
| **risk_registry** | `add_verifier` / `remove_verifier` | ✗ NO | Verifier management independent of pause |
| **risk_registry** | `set_debtor_score`       | ✗ NO    | Debtor scoring independent of protocol pause         |
| **treasury**      | `set_fee_bps`            | ✗ NO    | Treasury fee config operates independently            |
| **treasury**      | `whitelist_token`        | ✗ NO    | Treasury token management independent of pause       |
| **treasury**      | `collect_fee`            | ✗ NO    | Fee collection (passive) always available            |
| **treasury**      | `withdraw`               | ✗ NO    | Admin withdrawal not paused; treasury operates independently |
| **treasury**      | `emergency_withdraw`     | ✗ NO    | Emergency recovery always available                   |
| **price_oracle**  | `set_price`              | ✗ NO    | Price oracle updates independent of protocol pause    |
| **price_oracle**  | `convert`                | ✗ NO    | Price queries (passive) never blocked                |

## Design Decisions

**Core Principle:** The pause mechanism blocks *inbound activity* (new invoices, new funding, new positions) but never blocks *outbound settlements* (repayment, refunds, withdrawals). This protects existing investors while preventing new exposure during an emergency.

**Exempt Entrypoints (9 total):**

1. **`financing_pool.repay()`** — Blocking repayment harms both SMEs (who must settle debts) and investors (who need to recover capital). Any emergency requiring pause should not prevent debt settlement.

2. **`invoice_nft.set_repaid()`** — Called by pool as part of settlement flow; must complete regardless of pause state.

3. **`invoice_nft.set_defaulted()`** — Admin must be able to mark defaults even during pause. Delaying default marking would hide protocol damage.

4. **`marketplace.cancel_listing()`** — Sellers must retain the ability to withdraw invoices from listing at any time, pause or not. Blocking cancellation traps sellers.

5. **`marketplace.claim_refund()`** — Investors must be able to claim refunds on expired listings. Not paused.

6. **Administrative contracts** (`access_control`, `risk_registry`, `treasury`) — These operate independently of the protocol pause. The pause mechanism itself cannot be paused; role changes, risk updates, and treasury operations continue.

**Rationale:** The pause is a *circuit-breaker for new economic activity*, not a freeze of all state. If pause prevented existing obligations from settling, it would become a punitive mechanism rather than a safety tool.

**Non-paused note:** Some functions like `set_fee_bps`, `whitelist_token`, `set_price`, etc. are admin-only and do not require pause guards because they are synchronized with the admin's pause decision.

## Verification & Testing

**Matrix Accuracy:** This pause-semantics matrix has been **verified against the actual implementation** by inspecting `require_not_paused()` calls in all 7 contract source files. The matrix reflects the current deployed behavior as of commit [see git log for merge of A15–A17, B11].

**Integration Test Coverage:** The enforcement matrix is validated by the test suite:

- `test_pause_enforcement_matrix` in `contracts/tests/src/lib.rs` — Calls each blocked entrypoint while paused and confirms `ProtocolPaused` is returned
- Confirms exempt entrypoints (like `repay`, `claim_refund`, `cancel_listing`) execute successfully during pause
- Verifies unpause restores normal operation

**How to Verify:** To confirm the matrix matches current code, run:

```bash
cargo test -p kora-marketplace test_pause_enforcement_matrix -- --nocapture
cargo test -p kora-financing-pool --test \* -- --nocapture  
```

If any function's pause status has been changed post-merge, update this matrix immediately and open an issue flagging the discrepancy.

---

## Cross-Contract Authorization

See [ARCHITECTURE.md § Cross-Contract Authorization Matrix](ARCHITECTURE.md#cross-contract-authorization-matrix) for a detailed table of all cross-contract calls in the protocol, including the authorization required for each call.

Key insight: authorization is transitive. When a user calls `marketplace.fund_invoice()` with their signature, the marketplace contract calls `financing_pool.release_funds()` with its own address (`env.current_contract_address()`), which then calls `invoice_nft.set_funded()` also with the pool's address. This three-level call chain is secure because each step is verified by the callee (via `require_auth()`).
