# Referral rewards

`referral_rewards` pays a fixed token bonus to a single referrer after an SME's **first minted invoice** reaches `Repaid`. It uses the canonical invoice status written by `financing_pool` through `invoice_nft.set_repaid`; partial repayments and later invoices cannot qualify. A permissionless caller invokes `claim_referral_reward(sme)` after full repayment. Anyone watching the existing `invoice_repaid` event may submit that transaction. The claim returns `false` if no reward is due, including when the allocation is exhausted. Repayment never calls the rewards contract, so incentives cannot interrupt settlement.

## Mint and attribution

Call `mint_invoice_with_referrer(sme, debtor_hash, amount, currency, due_date, ipfs_cid, risk_score, notes, referrer)` with the existing mint fields plus `Option<Address>`. `mint_invoice` remains available for existing clients and records an unattributed first mint. A batch mint also records its first invoice as the SME's first mint without attribution. The first invoice ID and optional referrer are stored separately from the invoice schema, so withdrawals do not permit a new first mint or a changed referrer. Any attempt to supply a referrer after the first mint fails. Self-referrals, contract-self referrals, and cycles through existing referrals are rejected; traversal is bounded at 32 links and deeper chains are rejected. No downstream rewards are paid.

### View Functions

- `get_sme_referrer(sme) -> Option<Address>`: returns the first-mint referrer for an SME, or None if not set.
- `get_first_mint(sme) -> Option<u64>`: returns the invoice ID of an SME's first mint, or None if none minted.

## Funding and claiming

Deploy with `initialize(admin, invoice_nft, token, bonus, allocation)` where `bonus` and `allocation` are positive token base units and `bonus <= allocation`. The treasury admin should approve a dedicated withdrawal using the existing treasury process, then call `deposit(admin, amount)` to transfer those tokens to this contract. Total deposits cannot exceed the immutable allocation. The reward contract spends only its own token balance, and cumulative payouts cannot exceed funded tokens or allocation.

After the first invoice is fully repaid, call `claim_referral_reward(sme)`. It checks the immutable referral, original invoice ID, SME ownership, canonical `Repaid` status, unclaimed marker, remaining allocation, and actual token balance. It writes the claim marker before the transfer; a failed token transfer rolls the transaction back. It emits `referral` with `(sme)` as a topic and `(referrer, invoice_id, bonus)` as data. `has_claimed` and `paid_total` expose settlement state.

For an unattended payout, run an indexer or keeper that reacts to `invoice_repaid` and sends the permissionless claim transaction. Claims are explicit transactions because Soroban events cannot execute another contract by themselves.

Storage entries use the repository's roughly 30-day persistent TTL convention. Operators must maintain/extend contract and storage lifetimes on long-lived deployments; expired attribution or claim storage cannot be reconstructed from this contract alone. Existing deployments also need a backfill/migration policy for SMEs that minted before this feature, since first-mint attribution cannot safely be retroactively asserted.

## Integration test

`contracts/tests/referral_rewards_e2e.rs` (issue #1089) drives the whole multi-contract path with real entrypoints instead of status shortcuts. `set_repaid` is only ever reached by the pool's `repay`, so sequencing bugs across `invoice_nft`, `financing_pool` and `referral_rewards` surface here. The harness reuses the shared `TestWorld` fixture and deploys `AccessControl`, `InvoiceNft`, `FinancingPool` and `ReferralRewards` over a single Stellar Asset Contract whose symbol is also the invoice currency, so the pool's currency conversion is a no-op.

Covered scenarios:

- **Happy path** — mint first invoice with a referrer, list + fund via the pool, repay in full, then a permissionless `claim_referral_reward(sme)` pays the fixed bonus to the referrer exactly once, sets `has_claimed`, moves `paid_total`, and emits the `referral` event.
- **Unrepaid rejection** — a claim while the invoice is only `Funded` returns `false` and pays nothing; the same claim succeeds once the pool repays.
- **Double claim** — a second claim returns `false` and leaves both the referrer and contract balances unchanged.
- **Premature claim** — an unlisted/unfunded first invoice is never eligible.
- **Self-referral** — `mint_invoice_with_referrer` rejects a self-referral with `InvoiceNftError::InvalidReferrer`, so no first mint and no reward accrue.
- **Paused protocol** — while the shared `AccessControl` pause is active the `Funded -> Repaid` transition is rejected with `InvoiceNftError::ProtocolPaused` and the reward stays ineligible; unpausing restores the flow.

Run it with:

```
cargo test -p kora-tests --test referral_rewards_e2e
```
