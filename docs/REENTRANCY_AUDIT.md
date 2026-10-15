# Kora Protocol — Reentrancy & CEI Audit Matrix

**Issue:** #808  
**Status:** Complete  
**Last updated:** 2026-09-29

This document systematically audits every cross-contract call site across all
Kora contracts for Checks-Effects-Interactions (CEI) ordering violations and
reentrancy risk. It serves as a durable artifact — updating it is required
whenever a new cross-contract call site is introduced.

---

## Background

Soroban's execution model is synchronous and deterministic. True re-entrant
callback loops (contract A → contract B → contract A mid-transaction) are
possible when contract B is adversarial or compromised. The standard defense is:

1. **CEI ordering** — complete all state writes _before_ any token transfer or
   cross-contract call.
2. **Storage-backed reentrancy guard** — `kora_shared::reentrancy::ReentrancyGuard`
   acquires an instance-storage lock on construction and releases it on drop,
   preventing any re-entrant call to the same contract while the guard is live.

Both defenses are complementary: CEI prevents exploitation even if the guard is
absent; the guard catches subtle indirect re-entrance (e.g., a callback-style
pattern through a whitelisted token contract) that pure CEI cannot.

---

## Audit Matrix

Each row records one cross-contract call site, whether it follows CEI ordering,
whether a reentrancy guard is present, and the fix applied (if any).

### `contracts/financing_pool/src/lib.rs`

| # | Function | Call site (line region) | Callee | CEI order? | Guard present? | Assessment | Fix applied |
|---|----------|------------------------|--------|-----------|---------------|------------|-------------|
| FP-1 | `repay_internal` | token `transfer` (payer→pool) | Token contract | ✅ Yes — `pool.repaid_amount` and `pool.is_closed` written before transfer | ✅ `RepaymentLock` persistent guard | Safe | FP-02/03 (prior audit) |
| FP-2 | `repay_internal` | `nft_client.set_repaid` | `invoice_nft` | ✅ Yes — pool closed state written before NFT call | ✅ `RepaymentLock` | Safe | — |
| FP-3 | `repay_internal` | `distribute_yield` (internal) | Same contract (private fn) | ✅ Yes — pool state fully committed before yield distribution | ✅ `RepaymentLock` | Safe | — |
| FP-4 | `distribute_yield` | token `transfer` (pool→investor) | Token contract | ✅ Yes — all accounting updated before transfers | ✅ Inherited from caller's `RepaymentLock` | Safe | — |
| FP-5 | `distribute_yield` | `treasury_client.try_collect_fee` | `treasury` | ✅ Yes — fee recorded then forwarded | ✅ Inherited lock | Safe | — |
| FP-6 | `mark_default` | `distribute_yield` (partial) | Same contract | ✅ Yes — pool closed before distribution | ✅ `RepaymentLock` checked before entry | Safe | — |
| FP-7 | `mark_default` | `nft_client.set_defaulted` | `invoice_nft` | ✅ Yes — distribution complete before NFT call | ✅ `RepaymentLock` | Safe | — |
| FP-8 | `mark_default` | `rr_client.try_record_default` | `risk_registry` | ✅ Yes — best-effort call after all state changes | ✅ `RepaymentLock` | Safe | — |
| FP-9 | `release_funds` | `nft_client.get_invoice` (read) | `invoice_nft` | N/A — read only | ✅ Pause check + `PoolAlreadyClosed` guard | Safe | — |
| FP-10 | `release_funds` | `nft_client.set_funded` | `invoice_nft` | ✅ Yes — pool record written before NFT transition | None (no token transfer in this fn) | Safe | — |
| FP-11 | `repay_partial` | token `transfer` | Token contract | ✅ Yes — `pool.repaid_amount` written before transfer | ✅ `RepaymentLock` | Safe | — |
| FP-12 | `net_settle` | token `transfer` (single batch transfer) | Token contract | ✅ Yes — all pool state + locks acquired before the single transfer | ✅ All `RepaymentLock`s set before transfer | Safe | — |
| FP-13 | `buy_position` | token `transfer` (buyer→seller) | Token contract | ✅ Yes — sale offer removed and position transferred before token call | None needed (no contract-wide state at risk) | Safe | — |
| FP-14 | `accept_early_settlement` | token `transfer` via `distribute_yield` | Token contract | ✅ Yes — `RepaymentLock` set, pool closed, offer removed before distribution | ✅ `RepaymentLock` | Safe | — |
| FP-15 | `propose_early_settlement` | token `transfer` (sme→pool escrow) | Token contract | ✅ Yes — offer stored before token transfer | None (escrow; no re-entrance risk) | Safe | — |

### `contracts/marketplace/src/lib.rs`

| # | Function | Call site | Callee | CEI order? | Guard present? | Assessment | Fix applied |
|---|----------|-----------|--------|-----------|---------------|------------|-------------|
| MP-1 | `fund_invoice_internal` | token `transfer` (investor→treasury fee) | Token contract | ✅ Yes — all local accounting written before transfers | ✅ `ReentrancyGuard` (from `fund_invoice`) | Safe | — |
| MP-2 | `fund_invoice_internal` | token `transfer` (investor→pool net) | Token contract | ✅ Yes — state committed; second transfer after first | ✅ `ReentrancyGuard` | Safe | — |
| MP-3 | `fund_invoice_internal` | `treasury_client.collect_fee` | `treasury` | ✅ Yes — after token transfer; purely additive accounting | ✅ `ReentrancyGuard` | Safe | — |
| MP-4 | `fund_invoice_internal` | `pool_client.release_funds` | `financing_pool` | ✅ Yes — called only after full funding; listing already deactivated | ✅ `ReentrancyGuard` | Safe | — |
| MP-5 | `fund_invoice_internal` | `nft_client.get_invoice` (read), `nft_client.is_invoice_frozen` (read) | `invoice_nft` | N/A — reads only, before any state change | ✅ `ReentrancyGuard` | Safe | — |
| MP-6 | `list_invoice` | `nft_client.get_invoice` (read) | `invoice_nft` | N/A — read before state writes | ✅ `ReentrancyGuard` (in `list_invoice`) | Safe | — |
| MP-7 | `list_invoice` | `nft_client.set_listed` | `invoice_nft` | ✅ Yes — listing record written before NFT call | ✅ `ReentrancyGuard` | Safe | — |
| MP-8 | `claim_refund` | `token_client.transfer` (pool→investor) | Token contract | ✅ Yes — `RefundClaimed` flag set to `true` before transfer | None (single-actor claim; guard not needed) | Safe | — |
| MP-9 | `claim_refund` | `treasury_client.refund_fee` | `treasury` | ✅ Yes — called after investor refund flag set | None | Safe | — |
| MP-10 | `withdraw_listing` | `nft_client.set_created` | `invoice_nft` | ✅ Yes — listing record removed before NFT call | None (no token transfer) | Safe | — |
| MP-11 | `convert_via_oracle` | `oracle_client.convert` | `price_oracle` | ✅ Yes — called during currency conversion before token transfers | ✅ `ReentrancyGuard` (from `fund_invoice`) | Safe | Verified in `reentrancy_callback_price_oracle.rs` (#922) |

### `contracts/treasury/src/lib.rs`

| # | Function | Call site | Callee | CEI order? | Guard present? | Assessment | Fix applied |
|---|----------|-----------|--------|-----------|---------------|------------|-------------|
| TR-1 | `withdraw` | token `transfer` | Token contract | ✅ Yes — collected ledger decremented and rate-limit recorded before transfer | ✅ `ReentrancyGuard` (RAII) | Safe | — |
| TR-2 | `do_emergency_withdraw` | token `transfer` | Token contract | ✅ Yes — reserve balance reduced before transfer | ✅ `ReentrancyGuard` | Safe | — |
| TR-3 | `disburse_from_reserve` | token `transfer` | Token contract | ✅ Yes — `reserve_key` balance written before transfer | ✅ `ReentrancyGuard` | Safe | — |
| TR-4 | `refund_fee` | token `transfer` (treasury→recipient) | Token contract | ✅ Yes — collected ledger decremented before transfer | None (marketplace-restricted path; single auth check) | Safe | — |
| TR-5 | `withdraw_batch` | token `transfer` (per entry) | Token contract | ✅ Yes — all-or-nothing checks pass before any transfer; per-entry ledger update before each transfer | ✅ `ReentrancyGuard` (batch-level) | Safe | — |
| TR-6 | `claim_share` | token `transfer` | Token contract | ✅ Yes — `StakeholderClaim` key set before transfer | None (stakeholder-restricted) | ⚠️ No guard — see note | No guard added; CEI is sufficient here |
| TR-7 | `get_total_collected_value` | `oracle_client.try_convert_with_decimals` | `price_oracle` | N/A — read-only fee valuation view | ✅ Target functions (`withdraw`) protected by `ReentrancyGuard` & admin auth | Safe | Verified in `reentrancy_callback_price_oracle.rs` (#922) |

> **TR-6 note:** `claim_share` has no `ReentrancyGuard`. The function is not
> gated behind an admin key; any registered stakeholder can call it. However,
> CEI is correctly observed (the claim record is written before the token
> transfer), so an adversarial token cannot re-enter and double-claim. A guard
> would provide defence-in-depth but is not strictly required. Tracked as a
> follow-up hardening item.

### `contracts/risk_registry/src/lib.rs`

| # | Function | Call site | Callee | CEI order? | Guard present? | Assessment | Fix applied |
|---|----------|-----------|--------|-----------|---------------|------------|-------------|
| RR-1 | `add_verifier` | `token_client.transfer` (verifier→contract) | Token contract | ✅ Yes — transfer happens after all validation; verifier flag/stake written after transfer | None (token transfer is inbound; reentrancy window is benign) | Safe | — |
| RR-2 | `remove_verifier` | `token_client.transfer` (contract→verifier) | Token contract | ⚠️ Partial — verifier flag and stake removed _before_ token transfer ✅; but `VerifierStatus::Removed` marker written _after_. The verifier flag removal is the effective guard. | None | Safe (flag removal before transfer is the CEI anchor) | Document flag-first ordering as the explicit CEI contract |
| RR-3 | `finalize_verifier_removal` | `token_client.transfer` (contract→verifier) | Token contract | ✅ Yes — verifier flag, stake, and reputation removed before stake return | None | Safe | — |
| RR-4 | `record_default` | (no cross-contract call — slashing is storage-only) | — | N/A | ✅ `ReentrancyGuard` | Safe | — |
| RR-5 | `update_sme_score` | (no cross-contract call) | — | N/A | ✅ `ReentrancyGuard` | Safe | — |
| RR-6 | `top_up_stake` | `token_client.transfer` (verifier→contract) | Token contract | ✅ Yes — current stake read, new stake value computed, then transfer, then storage write | None (inbound transfer; re-entrance benign) | ⚠️ Effects written _after_ interaction — see note | Added inline note; refactor tracked |

> **RR-6 note:** `top_up_stake` reads the current stake, performs the token
> transfer (interaction), then writes the new stake (effect). This is an
> inbound transfer (attacker sends tokens _to_ the contract) so exploitation
> requires the attacker to control the staking token, which requires admin
> approval. The risk is low but the ordering violates the CEI principle. A
> regression test `test_top_up_stake_cei_ordering` documents the current
> behaviour and will catch any future refactor that accidentally enables
> re-entrance.

### `contracts/invoice_nft/src/lib.rs`

| # | Function | Call site | Callee | CEI order? | Guard present? | Assessment | Fix applied |
|---|----------|-----------|--------|-----------|---------------|------------|-------------|
| NFT-1 | `mint_invoice` | `rr.try_get_sme_profile` (read) | `risk_registry` | N/A — read before any state change | ✅ `ReentrancyGuard` | Safe | — |
| NFT-2 | `refresh_risk_score` | `rr.try_get_sme_profile` (read) | `risk_registry` | N/A — read before state write | ✅ `ReentrancyGuard` | Safe | — |
| NFT-3 | `set_listed` / `set_funded` / `set_repaid` / `set_defaulted` | No external token transfers; these _are_ called _by_ marketplace/pool | `access_control` (pause check only) | ✅ Yes | ✅ `ReentrancyGuard` on write transitions | Safe | — |

### `contracts/access_control/src/lib.rs`

| # | Function | Call site | Callee | CEI order? | Guard present? | Assessment |
|---|----------|-----------|--------|-----------|---------------|------------|
| AC-1 | `pause` / `unpause` | No token transfers; storage-only | — | N/A | ✅ `ReentrancyGuard` | Safe |
| AC-2 | `execute_action` | No token transfers | — | N/A | None needed | Safe |
| AC-3 | `execute_treasury_action` (in treasury, routed from AC) | token transfers inside treasury | Token contract | ✅ Handled inside treasury | ✅ Treasury's own guard | Safe |

---

## Indirect Reentrancy via Callback Patterns

Soroban does not support unsolicited callbacks (contracts are passive), but the
following indirect patterns were considered:

1. **Whitelisted token with a malicious `transfer` hook** — A token contract
   could call back into a Kora contract during a `transfer`. All Kora contracts
   that make outbound token transfers either (a) hold a `RepaymentLock` /
   `ReentrancyGuard` that would reject the re-entrant call, or (b) have already
   completed all state writes before the transfer (CEI). Neither path allows
   double-spending or state corruption.

2. **`treasury.collect_fee` called during `distribute_yield`** — Treasury's
   `collect_fee` only adds to an accounting counter; it holds no lock. A
   re-entrant call would simply double-count fees in the ledger (an
   over-reporting issue, not a fund loss). The actual token balance limits the
   real payout. No fix required; documented here for completeness.

3. **`risk_registry.record_default` called during `mark_default`** — The
   `record_default` call in `mark_default` is best-effort (`try_` variant) and
   happens after the pool is fully closed. Even if `record_default` itself
   called back into `financing_pool`, the pool's `PoolAlreadyClosed` check would
   block any further state change.

4. **`price_oracle` callbacks during `fund_invoice` and valuation (#922)** —
   When `marketplace::fund_invoice` converts amounts across currencies via
   `convert_via_oracle`, control flows to the configured price oracle. An adversarial
   oracle attempting to reenter `marketplace::fund_invoice` or `financing_pool::repay`
   during this conversion callback is strictly rejected:
   - `fund_invoice` holds an instance-level `ReentrancyGuard`, failing any second entry closed.
   - `financing_pool` enforces `RepaymentLock` preventing state manipulation during in-flight operations.
   - Treasury valuation via `get_total_collected_value` cleanly isolates multi-asset computation.
   - Multi-hop nested callbacks (depth > 1) fail closed atomically.
   - Validated end-to-end in regression suite `contracts/tests/reentrancy_callback_price_oracle.rs`.

---

## Summary of Findings

| Severity | Count | Status |
|----------|-------|--------|
| High (prior, now fixed) | 2 (FP-02, FP-03) | ✅ Fixed in prior audit |
| Medium — CEI deviation (RR-6 `top_up_stake`) | 1 | ⚠️ Documented; low exploitability — regression test added |
| Low — no guard on `claim_share` (TR-6) | 1 | ⚠️ Documented; CEI is correct — follow-up hardening |
| Safe (no action needed) | All others | ✅ |

No new High or Critical findings were identified in this audit pass.

---

## CI Guard

The following CI rule is recommended to prevent regression (add to
`.github/workflows/ci.yml`):

```yaml
- name: Enforce single KoraError enum
  run: |
    count=$(git grep -c 'pub enum KoraError' contracts/shared/src/errors.rs)
    if [ "$count" != "1" ]; then
      echo "ERROR: multiple KoraError enums detected in errors.rs"
      exit 1
    fi
- name: Check brace balance in Rust files
  run: |
    failed=0
    for f in $(git ls-files 'contracts/**/*.rs'); do
      result=$(awk 'BEGIN{o=0}{for(i=1;i<=length($0);i++){c=substr($0,i,1);if(c=="{")o++;else if(c=="}")o--}}END{if(o!=0)print FILENAME": "o}' "$f")
      if [ -n "$result" ]; then
        echo "Unbalanced braces: $result"
        failed=1
      fi
    done
    if [ "$failed" -ne 0 ]; then exit 1; fi
```

---

## How to extend this document

When adding a new cross-contract call site:

1. Add a row to the relevant contract section above.
2. Confirm CEI ordering: all state writes must precede the call.
3. Confirm guard presence: a `ReentrancyGuard` or `RepaymentLock` is present
   when the function is callable by non-admin addresses.
4. Add a regression test in `contracts/tests/reentrancy_cei_regression.rs`
   (see that file for the naming convention).
5. Update the CI brace-balance guard if adding a new source file.
