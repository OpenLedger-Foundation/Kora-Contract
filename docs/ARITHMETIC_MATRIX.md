# Kora Protocol — Arithmetic Overflow/Underflow Exhaustive Test Matrix

**Issue:** #811  
**Status:** Complete  
**Last updated:** 2026-09-29

This document is the durable, reviewable artifact required by issue #811. It
enumerates every arithmetic operation site across all contracts, records the
boundary values tested, and tracks coverage status. The companion test file is
`contracts/tests/arithmetic_overflow_matrix.rs`.

---

## Methodology

For each arithmetic operation site we test three boundary classes:

| Class | Values tested |
|-------|---------------|
| **Zero** | `0`, `0 op X`, `X op 0` |
| **MAX_AMOUNT** | `MAX_AMOUNT` (= `i128::MAX / 2`), `MAX_AMOUNT + 1` |
| **i128 extremes** | `i128::MAX`, `i128::MIN`, `i128::MAX - 1`, `i128::MIN + 1` |
| **Mul-before-div** | `amount * bps / 10_000` ordering — checks that multiplication is performed first, then division, to minimise precision loss |

Operations are sourced from `contracts/shared/src/validation.rs` (shared
helpers used by every contract) and from per-contract arithmetic in
`financing_pool`, `marketplace`, `treasury`, and `risk_registry`.

---

## Matrix

### 1. `kora_shared::validation` — safe arithmetic helpers

| # | Operation | Function | Zero case | MAX_AMOUNT case | i128 extreme | Mul-before-div | Test name | Status |
|---|-----------|----------|-----------|----------------|-------------|----------------|-----------|--------|
| V-1 | `a + b` | `safe_add` | ✅ `0 + 0 = 0` | ✅ `MAX + 1 → overflow` | ✅ `i128::MAX + 1 → overflow` | N/A | `test_safe_add_*` | ✅ Covered |
| V-2 | `a - b` | `safe_sub` | ✅ `0 - 0 = 0` | ✅ `0 - 1 → underflow` | ✅ `i128::MIN - 1 → underflow` | N/A | `test_safe_sub_*` | ✅ Covered |
| V-3 | `a * b` | `safe_mul` | ✅ `0 * MAX = 0` | ✅ `MAX * 2 → overflow` | ✅ `i128::MAX * 2 → overflow` | N/A | `test_safe_mul_*` | ✅ Covered |
| V-4 | `a / b` | `safe_div` | ✅ `X / 0 → err` | ✅ `MAX / 1 = MAX` | ✅ `i128::MIN / -1 → overflow` | N/A | `test_safe_div_*` | ✅ Covered |
| V-5 | `amount * bps / 10_000` | `bps_of` | ✅ `0 bps` and `0 amount` | ✅ `MAX_AMOUNT * 10_000` safe | ✅ `i128::MAX * bps → overflow` | ✅ mul-first order verified | `test_bps_of_*` | ✅ Covered |
| V-5b | `part * 10_000 > cap_bps * whole` | `exceeds_bps_share` | ✅ `0 part`, `0 cap` | ✅ exact cap boundary | ✅ `i128::MAX * 10_000 → overflow` | ✅ division-free compare | `test_exceeds_bps_share_*` | ✅ Covered |
| V-5c | `bps > 10_000` rejected | `bps_of`, `bps_of_normalized`, `exceeds_bps_share` | ✅ `10_000` accepted | — | — | — | `test_bps_of_rejects_rate_above_100_percent` | ✅ Covered |
| V-6 | `amount * bps / 10_000` (normalised) | `bps_of_normalized` | ✅ zero amount | ✅ large amount | ✅ overflow path | ✅ decimal scale then bps | `test_bps_of_normalized_*` | ✅ Covered |
| V-7 | `10^(scale)` | `normalize_amount` | ✅ 0 decimals diff = noop | ✅ large scale | ✅ overflow on extreme scale | N/A | `test_normalize_amount_*` | ✅ Covered |
| V-8 | `amount > MAX_AMOUNT` | `require_within_max_amount` | ✅ `0 ok` | ✅ `MAX_AMOUNT ok`, `MAX_AMOUNT+1 err` | ✅ `i128::MAX err` | N/A | `test_require_within_max_amount_*` | ✅ Covered |

### 2. `contracts/financing_pool/src/lib.rs`

| # | Operation | Function | Boundary tested | Mul-before-div | Test name | Status |
|---|-----------|----------|----------------|----------------|-----------|--------|
| FP-A | `contributed * 10_000 / total_pool` (share_bps) | `record_position` | Zero contributed, zero total, contributed > total, MAX_AMOUNT inputs | ✅ mul first | `test_fp_share_bps_*` | ✅ Covered |
| FP-B | `pool.repaid_amount + effective_amount` | `repay_internal` | repaid = 0, repaid near MAX, overflow path | N/A | `test_fp_repay_overflow_*` | ✅ Covered |
| FP-C | `pool.total_owed + penalty` | `repay_internal` (late penalty) | penalty = 0, penalty = MAX | N/A | `test_fp_penalty_overflow_*` | ✅ Covered |
| FP-D | `prev_agg ± contributed` (aggregate funded) | `record_position` | agg = 0, agg near MAX | N/A | `test_fp_aggregate_funded_*` | ✅ Covered |
| FP-E | `pool.face_value * late_penalty_bps / 10_000` | `repay_internal` | face = 0, face = MAX_AMOUNT, bps = 0, bps = 10_000 | ✅ | `test_fp_late_penalty_bps_of_*` | ✅ Covered |
| FP-F | `payout = bps_of_normalized(repaid, share_bps, decimals)` | `distribute_yield` | share_bps = 0, 10_000; repaid = 0, MAX | ✅ | `test_fp_yield_payout_*` | ✅ Covered |
| FP-G | `alloc * remaining / total_remaining` (net_settle) | `net_settle` | single invoice, equal pools, remainder distribution | ✅ | `test_fp_net_settle_alloc_*` | ✅ Covered |
| FP-H | `offer.accepted_bps + position.share_bps` | `accept_early_settlement` | bps sum = 10_000, overflow | N/A | `test_fp_early_settlement_bps_*` | ✅ Covered |

### 3. `contracts/marketplace/src/lib.rs`

| # | Operation | Function | Boundary tested | Mul-before-div | Test name | Status |
|---|-----------|----------|----------------|----------------|-----------|--------|
| MP-A | `bps_of_normalized(listing_amount, fee_bps, decimals)` | `fund_invoice_internal` | fee_bps = 0, 50, 10_000; amount = 1, MAX_AMOUNT | ✅ | `test_mp_fee_bps_of_*` | ✅ Covered |
| MP-B | `listing_amount - fee` (net) | `fund_invoice_internal` | fee = amount (100%), fee = 0 | N/A | `test_mp_net_after_fee_*` | ✅ Covered |
| MP-C | `listing.funded_amount + listing_amount` | `fund_invoice_internal` | funded = 0, funded + amount = asking_price, overflow | N/A | `test_mp_funded_amount_overflow_*` | ✅ Covered |
| MP-D | `prospective * 10_000 > cap_bps * asking_price` (concentration cap) | `fund_invoice_internal` | prev_gross = 0, at cap, over cap | ✅ mul-before-compare | `test_mp_concentration_cap_*` | ✅ Covered |
| MP-E | `cap_bps * asking_price` overflow guard | `fund_invoice_internal` | asking_price = MAX_AMOUNT, cap_bps = 10_000 | ✅ | `test_mp_concentration_cap_overflow_guard` | ✅ Covered |
| MP-F | `bps_of(fee, referrer_split_bps)` (was local `fee * split / 10_000`, #1188) | `fund_invoice_internal` | split = 0, 2_000, 10_000 | ✅ | `test_referral*` | ✅ Covered |
| MP-G | `exceeds_bps_share(prospective, asking_price, cap_bps)` (was local checked compare, #1188) | `fund_invoice_internal` | at cap, cap + 1 | ✅ | `test_exceeds_bps_share_*` | ✅ Covered |

### 4. `contracts/treasury/src/lib.rs`

| # | Operation | Function | Boundary tested | Mul-before-div | Test name | Status |
|---|-----------|----------|----------------|----------------|-----------|--------|
| TR-A | `current + amount` (collected ledger) | `collect_fee` | amount = 1, near MAX, overflow | N/A | `test_tr_collect_fee_overflow_*` | ✅ Covered |
| TR-B | `reserve_cut = bps_of(amount, reserve_bps)` | `collect_fee` | reserve_bps = 0, 5_000, 10_000 | ✅ | `test_tr_reserve_allocation_bps_*` | ✅ Covered |
| TR-C | `epoch_withdrawn + amount` (rate limit) | `enforce_rate_limit` | amount = cap, amount = cap + 1 | N/A | `test_tr_rate_limit_boundary_*` | ✅ Covered |
| TR-D | `amount.checked_mul(bps_i128) / 10_000` (treasury `bps_of`) | `claim_share` | bps = 0, 5_000, 10_000; amount = 0, MAX | ✅ | `test_tr_bps_of_boundary_*` | ✅ Covered |

### 5. `contracts/risk_registry/src/lib.rs`

| # | Operation | Function | Boundary tested | Mul-before-div | Test name | Status |
|---|-----------|----------|----------------|----------------|-----------|--------|
| RR-A | `current_stake + additional` | `top_up_stake` | additional = 0 (err), near MAX, overflow | N/A | `test_rr_top_up_stake_overflow_*` | ✅ Covered |
| RR-B | `(stake * slash_bps / 10_000)` (slash) | `record_default` | slash = 0 bps, 5_000 bps, 10_000 bps; stake = 0, 1 | ✅ | `test_rr_slash_bps_of_*` | ✅ Covered |
| RR-C | `profile.total_invoices + 1` | `increment_invoice_count` | count at u32::MAX - 1, at u32::MAX | N/A | `test_rr_invoice_count_overflow_*` | ✅ Covered |
| RR-D | `profile.defaults + 1` | `record_default` | defaults at u32::MAX - 1 | N/A | `test_rr_defaults_overflow_*` | ✅ Covered |
| RR-E | `total_score + score` (aggregate average) | `get_debtor_score` | 50 verifiers each scoring 100 (max sum = 5_000) | N/A | `test_rr_aggregate_score_overflow_*` | ✅ Covered |

---

## Coverage Summary

| Module | Operations enumerated | Covered | Gaps |
|--------|----------------------|---------|------|
| `validation.rs` shared helpers | 8 | 8 | 0 |
| `financing_pool` | 8 | 8 | 0 |
| `marketplace` | 5 | 5 | 0 |
| `treasury` | 4 | 4 | 0 |
| `risk_registry` | 5 | 5 | 0 |
| **Total** | **30** | **30** | **0** |

---

## Multiplication-Before-Division Ordering

The protocol uses `amount * bps / 10_000` throughout. Performing the division
first (`amount / 10_000 * bps`) would lose precision on small amounts. All
`bps_of` variants multiply first:

```rust
// Correct (multiply first):
amount.checked_mul(bps as i128)
      .and_then(|v| v.checked_div(10_000))

// Wrong (would truncate small amounts):
amount.checked_div(10_000)
      .and_then(|v| v.checked_mul(bps as i128))
```

Tests `test_bps_of_mul_before_div_precision_*` verify that the correct ordering
is maintained by checking known small-amount results.

---

## How to Extend

When adding a new arithmetic operation to any contract:

1. Add a row to the relevant section above.
2. Identify the three boundary classes (Zero, MAX_AMOUNT, i128 extremes).
3. Note whether mul-before-div ordering applies.
4. Add at least one test per boundary class in
   `contracts/tests/arithmetic_overflow_matrix.rs`.
5. Mark the row ✅ Covered.

### Shared bps helpers (#1188)

All basis-point math goes through `kora_shared::validation`
(`BPS_DENOMINATOR`, `bps_of`, `bps_of_normalized`, `exceeds_bps_share`).
Contracts must not inline `* bps / 10_000`; the helpers reject negative
amounts and rates above 100 % and always round toward zero, so fee rounding
cannot diverge between `marketplace`, `secondary_market` and `financing_pool`.
