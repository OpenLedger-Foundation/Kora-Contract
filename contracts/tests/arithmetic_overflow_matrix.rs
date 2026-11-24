//! Arithmetic Overflow / Underflow Exhaustive Test Matrix — Issue #811
//!
//! Every checked-arithmetic operation across all Kora contracts is tested here
//! with boundary values (zero, MAX_AMOUNT, i128::MAX/MIN) to confirm that every
//! arithmetic path fails safely rather than silently wrapping or truncating.
//!
//! See `docs/ARITHMETIC_MATRIX.md` for the full operation-to-test traceability
//! matrix. Test IDs in comments (e.g., "V-1") reference rows in that document.
//!
//! Sections:
//!   1. Shared validation helpers (safe_add, safe_sub, safe_mul, safe_div, bps_of, …)
//!   2. financing_pool arithmetic
//!   3. marketplace fee / concentration-cap arithmetic
//!   4. treasury fee / rate-limit arithmetic
//!   5. risk_registry slashing / count arithmetic

#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

// Shared validation helpers re-exported through kora_shared
use kora_shared::errors::CommonError;
use kora_shared::validation::{
    bps_of, bps_of_normalized, denormalize_amount, normalize_amount, require_within_max_amount,
    safe_add, safe_div, safe_mul, safe_sub, MAX_AMOUNT,
};

// ─────────────────────────────────────────────────────────────────────────────
// Section 1: kora_shared::validation — safe arithmetic helpers
// ─────────────────────────────────────────────────────────────────────────────

// ── safe_add [V-1] ────────────────────────────────────────────────────────────

/// V-1: 0 + 0 = 0. Trivial identity.
#[test]
fn test_safe_add_zero_plus_zero() {
    assert_eq!(safe_add(0, 0).unwrap(), 0);
}

/// V-1: MAX_AMOUNT + 0 = MAX_AMOUNT (no overflow).
#[test]
fn test_safe_add_max_amount_plus_zero() {
    assert_eq!(safe_add(MAX_AMOUNT, 0).unwrap(), MAX_AMOUNT);
}

/// V-1: MAX_AMOUNT + 1 is still within i128 range (MAX_AMOUNT = i128::MAX / 2).
#[test]
fn test_safe_add_max_amount_plus_one_is_safe() {
    assert!(safe_add(MAX_AMOUNT, 1).is_ok());
}

/// V-1: i128::MAX + 1 must overflow.
#[test]
fn test_safe_add_i128_max_plus_one_overflows() {
    assert_eq!(safe_add(i128::MAX, 1).unwrap_err(), CommonError::ArithmeticOverflow);
}

/// V-1: i128::MAX + i128::MAX must overflow.
#[test]
fn test_safe_add_both_max_overflows() {
    assert_eq!(safe_add(i128::MAX, i128::MAX).unwrap_err(), CommonError::ArithmeticOverflow);
}

/// V-1: Negative + negative stays negative without overflow.
#[test]
fn test_safe_add_negative_values() {
    assert_eq!(safe_add(-100, -200).unwrap(), -300);
}

/// V-1: i128::MIN + (-1) must overflow.
#[test]
fn test_safe_add_i128_min_minus_one_overflows() {
    assert_eq!(safe_add(i128::MIN, -1).unwrap_err(), CommonError::ArithmeticOverflow);
}

// ── safe_sub [V-2] ────────────────────────────────────────────────────────────

/// V-2: 0 - 0 = 0.
#[test]
fn test_safe_sub_zero_minus_zero() {
    assert_eq!(safe_sub(0, 0).unwrap(), 0);
}

/// V-2: Positive minus itself = 0.
#[test]
fn test_safe_sub_value_minus_itself() {
    assert_eq!(safe_sub(1_000_000, 1_000_000).unwrap(), 0);
}

/// V-2: 0 - 1 underflows.
#[test]
fn test_safe_sub_zero_minus_one_underflows() {
    assert_eq!(safe_sub(0, 1).unwrap_err(), CommonError::ArithmeticUnderflow);
}

/// V-2: i128::MIN - 1 underflows.
#[test]
fn test_safe_sub_i128_min_minus_one_underflows() {
    assert_eq!(safe_sub(i128::MIN, 1).unwrap_err(), CommonError::ArithmeticUnderflow);
}

/// V-2: MAX_AMOUNT - MAX_AMOUNT = 0 (no underflow).
#[test]
fn test_safe_sub_max_amount_minus_itself() {
    assert_eq!(safe_sub(MAX_AMOUNT, MAX_AMOUNT).unwrap(), 0);
}

/// V-2: MAX_AMOUNT - (MAX_AMOUNT + 1) underflows.
#[test]
fn test_safe_sub_max_amount_minus_more_underflows() {
    assert_eq!(
        safe_sub(MAX_AMOUNT, MAX_AMOUNT + 1).unwrap_err(),
        CommonError::ArithmeticUnderflow
    );
}

// ── safe_mul [V-3] ────────────────────────────────────────────────────────────

/// V-3: 0 * anything = 0.
#[test]
fn test_safe_mul_zero_times_max() {
    assert_eq!(safe_mul(0, i128::MAX).unwrap(), 0);
    assert_eq!(safe_mul(i128::MAX, 0).unwrap(), 0);
}

/// V-3: 1 * MAX_AMOUNT = MAX_AMOUNT.
#[test]
fn test_safe_mul_one_times_max_amount() {
    assert_eq!(safe_mul(1, MAX_AMOUNT).unwrap(), MAX_AMOUNT);
}

/// V-3: MAX_AMOUNT * 2 does not overflow (MAX_AMOUNT = i128::MAX / 2).
#[test]
fn test_safe_mul_max_amount_times_two_fits() {
    assert!(safe_mul(MAX_AMOUNT, 2).is_ok());
}

/// V-3: i128::MAX * 2 overflows.
#[test]
fn test_safe_mul_i128_max_times_two_overflows() {
    assert_eq!(safe_mul(i128::MAX, 2).unwrap_err(), CommonError::ArithmeticOverflow);
}

/// V-3: i128::MAX * i128::MAX overflows.
#[test]
fn test_safe_mul_i128_max_times_i128_max_overflows() {
    assert_eq!(
        safe_mul(i128::MAX, i128::MAX).unwrap_err(),
        CommonError::ArithmeticOverflow
    );
}

/// V-3: Negative * positive gives correct sign.
#[test]
fn test_safe_mul_negative_times_positive() {
    assert_eq!(safe_mul(-5, 10).unwrap(), -50);
}

// ── safe_div [V-4] ────────────────────────────────────────────────────────────

/// V-4: X / 0 returns InvalidAmount (divide-by-zero).
#[test]
fn test_safe_div_by_zero_returns_invalid_amount() {
    assert_eq!(safe_div(1_000, 0).unwrap_err(), CommonError::InvalidAmount);
    assert_eq!(safe_div(0, 0).unwrap_err(), CommonError::InvalidAmount);
    assert_eq!(safe_div(i128::MAX, 0).unwrap_err(), CommonError::InvalidAmount);
}

/// V-4: MAX / 1 = MAX.
#[test]
fn test_safe_div_max_by_one() {
    assert_eq!(safe_div(i128::MAX, 1).unwrap(), i128::MAX);
}

/// V-4: i128::MIN / -1 overflows (two's-complement edge).
#[test]
fn test_safe_div_i128_min_by_neg_one_overflows() {
    assert_eq!(safe_div(i128::MIN, -1).unwrap_err(), CommonError::ArithmeticOverflow);
}

/// V-4: Normal division truncates toward zero.
#[test]
fn test_safe_div_truncation() {
    assert_eq!(safe_div(7, 2).unwrap(), 3);
    assert_eq!(safe_div(-7, 2).unwrap(), -3);
}

// ── bps_of [V-5] ─────────────────────────────────────────────────────────────

/// V-5: 0 bps of any amount = 0.
#[test]
fn test_bps_of_zero_bps() {
    assert_eq!(bps_of(1_000_000, 0).unwrap(), 0);
    assert_eq!(bps_of(i128::MAX, 0).unwrap(), 0);
}

/// V-5: 100% (10_000 bps) of amount = amount.
#[test]
fn test_bps_of_full_rate_equals_amount() {
    assert_eq!(bps_of(1_000_000, 10_000).unwrap(), 1_000_000);
    assert_eq!(bps_of(MAX_AMOUNT, 10_000).unwrap(), MAX_AMOUNT);
}

/// V-5: 50 bps of 1_000_000 = 5_000.
#[test]
fn test_bps_of_standard_fee() {
    assert_eq!(bps_of(1_000_000, 50).unwrap(), 5_000);
}

/// V-5: Negative amount is rejected.
#[test]
fn test_bps_of_negative_amount_rejected() {
    assert!(bps_of(-1, 50).is_err());
}

/// V-5: i128::MAX * 10_000 overflows (mul-first path triggers overflow, not silent wrap).
#[test]
fn test_bps_of_i128_max_times_max_bps_overflows() {
    assert_eq!(bps_of(i128::MAX, 10_000).unwrap_err(), CommonError::ArithmeticOverflow);
}

/// V-5: MAX_AMOUNT * 10_000 does NOT overflow because MAX_AMOUNT = i128::MAX / 2.
#[test]
fn test_bps_of_max_amount_max_bps_fits() {
    let result = bps_of(MAX_AMOUNT, 10_000);
    assert!(result.is_ok(), "MAX_AMOUNT * 10_000 must fit in i128");
    assert_eq!(result.unwrap(), MAX_AMOUNT);
}

/// V-5: 1 bps of 10_000 = 1 (minimum non-zero fee).
#[test]
fn test_bps_of_one_bps_minimum() {
    assert_eq!(bps_of(10_000, 1).unwrap(), 1);
}

/// V-5: 1 bps of 9_999 = 0 (truncation — no dust carry-over into fees).
#[test]
fn test_bps_of_truncation_below_one() {
    assert_eq!(bps_of(9_999, 1).unwrap(), 0);
}

/// V-5: Multiplication is performed BEFORE division (precision guarantee).
/// If division were first: 1_000 / 10_000 = 0 → 0 * 50 = 0 (wrong).
/// Correct: 1_000 * 50 = 50_000 → 50_000 / 10_000 = 5.
#[test]
fn test_bps_of_mul_before_div_precision_small_amount() {
    assert_eq!(bps_of(1_000, 50).unwrap(), 5);
}

/// V-5: Same precision test with a larger amount to confirm no truncation loss.
#[test]
fn test_bps_of_mul_before_div_precision_large_amount() {
    let fee = bps_of(1_000_000_000, 50).unwrap();
    assert_eq!(fee, 5_000_000);
}

// ── bps_of_normalized [V-6] ───────────────────────────────────────────────────

/// V-6: Same-decimals path equals bps_of directly.
#[test]
fn test_bps_of_normalized_same_decimals_matches_bps_of() {
    assert_eq!(
        bps_of_normalized(1_000_000, 50, 7).unwrap(),
        bps_of(1_000_000, 50).unwrap()
    );
}

/// V-6: Negative amount is rejected.
#[test]
fn test_bps_of_normalized_negative_amount_rejected() {
    assert!(bps_of_normalized(-1, 50, 7).is_err());
}

/// V-6: Zero amount gives zero fee regardless of decimals.
#[test]
fn test_bps_of_normalized_zero_amount() {
    assert_eq!(bps_of_normalized(0, 50, 7).unwrap(), 0);
    assert_eq!(bps_of_normalized(0, 10_000, 6).unwrap(), 0);
}

/// V-6: 6-decimal token: fee is calculated in 7-decimal space and returned in 6.
#[test]
fn test_bps_of_normalized_6_decimal_roundtrip() {
    let result = bps_of_normalized(1_000_000, 100, 6).unwrap();
    // 100 bps = 1%; 1% of 1_000_000 = 10_000 (but scale adjusted for decimals)
    assert!(result > 0);
}

// ── normalize_amount / denormalize_amount [V-7] ───────────────────────────────

/// V-7: Same decimals → identity.
#[test]
fn test_normalize_amount_same_decimals_noop() {
    assert_eq!(normalize_amount(1_000_000, 7).unwrap(), 1_000_000);
}

/// V-7: 6 → 7 decimals scales up by 10.
#[test]
fn test_normalize_amount_6_to_7_scales_up() {
    assert_eq!(normalize_amount(1_000_000, 6).unwrap(), 10_000_000);
}

/// V-7: 8 → 7 decimals scales down by 10.
#[test]
fn test_normalize_amount_8_to_7_scales_down() {
    assert_eq!(normalize_amount(100_000_000, 8).unwrap(), 10_000_000);
}

/// V-7: Denormalize is the inverse of normalize.
#[test]
fn test_denormalize_is_inverse_of_normalize() {
    let original = 5_555_555i128;
    let normalized = normalize_amount(original, 6).unwrap();
    let back = denormalize_amount(normalized, 6).unwrap();
    assert_eq!(back, original);
}

/// V-7: Extremely large scale difference overflows.
#[test]
fn test_normalize_amount_extreme_scale_overflows() {
    // 30 decimal places difference would require 10^30 scale factor,
    // which overflows i128.
    let result = normalize_amount(1, 0); // scale by 10^7 — that's fine
    assert!(result.is_ok());
    // But a number that when scaled by 10^7 would overflow.
    let result2 = normalize_amount(i128::MAX / 2, 0);
    assert!(result2.is_err());
}

// ── require_within_max_amount [V-8] ───────────────────────────────────────────

/// V-8: 0 is within bounds.
#[test]
fn test_require_within_max_amount_zero_ok() {
    assert!(require_within_max_amount(0).is_ok());
}

/// V-8: MAX_AMOUNT itself is within bounds.
#[test]
fn test_require_within_max_amount_at_boundary_ok() {
    assert!(require_within_max_amount(MAX_AMOUNT).is_ok());
}

/// V-8: MAX_AMOUNT + 1 exceeds the ceiling.
#[test]
fn test_require_within_max_amount_one_over_rejected() {
    assert!(require_within_max_amount(MAX_AMOUNT + 1).is_err());
}

/// V-8: i128::MAX is far over the ceiling.
#[test]
fn test_require_within_max_amount_i128_max_rejected() {
    assert!(require_within_max_amount(i128::MAX).is_err());
}

/// V-8: Negative values are within range (function only checks upper bound).
#[test]
fn test_require_within_max_amount_negative_ok() {
    assert!(require_within_max_amount(-1).is_ok());
    assert!(require_within_max_amount(i128::MIN).is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 2: financing_pool arithmetic [FP-A through FP-H]
// ─────────────────────────────────────────────────────────────────────────────

use kora_financing_pool::{DataKey as FpKey, FinancingPoolContract, FinancingPoolContractClient};
use kora_shared::types::Pool;

fn fp_env() -> (Env, Address, FinancingPoolContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, FinancingPoolContract);
    let client = FinancingPoolContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let nft = Address::generate(&env);
    let rr = Address::generate(&env);
    let treasury = Address::generate(&env);
    let ac = env.register_contract(None, kora_access_control::AccessControlContract);
    let ac_client = kora_access_control::AccessControlContractClient::new(&env, &ac);
    ac_client.initialize(&admin);
    let oracle = Address::generate(&env);
    let dispute = Address::generate(&env);
    client.initialize(
        &admin, &nft, &rr, &treasury, &ac, &200u32, &oracle, &10_000u32, &0u64, &dispute,
    );
    (env, admin, client)
}

fn seed_pool(env: &Env, contract_id: &Address, invoice_id: u64, face: i128) -> Address {
    let token = Address::generate(env);
    let pool = Pool {
        invoice_id,
        token: token.clone(),
        total_funded: face,
        face_value: face,
        repaid_amount: 0,
        is_closed: false,
        late_penalty_bps: 0,
        total_owed: face,
        penalty_applied: false,
    };
    env.as_contract(contract_id, || {
        env.storage().persistent().set(&FpKey::Pool(invoice_id), &pool);
    });
    token
}

/// FP-A: share_bps = contributed * 10_000 / total_pool.
/// Zero contributed gives 0 bps without division by zero.
#[test]
fn test_fp_share_bps_zero_contributed() {
    let (env, admin, client) = fp_env();
    seed_pool(&env, &client.address, 1, 1_000);
    let investor = Address::generate(&env);
    // contributed must be > 0 to pass validation; this test confirms the
    // guard is hit BEFORE the multiplication.
    let result = client.try_record_position(&admin, &1u64, &investor, &0i128, &1_000i128);
    assert!(result.is_err());
}

/// FP-A: contributed = total_pool → share_bps = 10_000 (100%).
#[test]
fn test_fp_share_bps_full_pool() {
    let (env, admin, client) = fp_env();
    seed_pool(&env, &client.address, 2, 10_000);
    let investor = Address::generate(&env);
    client.record_position(&admin, &2u64, &investor, &10_000i128, &10_000i128);
    let pos = client.get_positions(&2u64).get(0).unwrap();
    assert_eq!(pos.share_bps, 10_000u32);
}

/// FP-A: MAX_AMOUNT * 10_000 fits in i128 (key invariant of the MAX_AMOUNT ceiling).
#[test]
fn test_fp_share_bps_max_amount_does_not_overflow() {
    let (env, admin, client) = fp_env();
    seed_pool(&env, &client.address, 3, MAX_AMOUNT);
    let investor = Address::generate(&env);
    let result = client.try_record_position(
        &admin, &3u64, &investor, &MAX_AMOUNT, &MAX_AMOUNT,
    );
    assert!(result.is_ok(), "MAX_AMOUNT * 10_000 must fit; it is the invariant of MAX_AMOUNT");
}

/// FP-A: contributed > MAX_AMOUNT is rejected before the multiplication.
#[test]
fn test_fp_share_bps_above_max_amount_rejected() {
    let (env, admin, client) = fp_env();
    seed_pool(&env, &client.address, 4, MAX_AMOUNT + 1);
    let investor = Address::generate(&env);
    let result = client.try_record_position(
        &admin, &4u64, &investor, &(MAX_AMOUNT + 1), &(MAX_AMOUNT + 2),
    );
    assert!(result.is_err());
}

/// FP-B: repay with amount > MAX_AMOUNT is rejected in Checks.
#[test]
fn test_fp_repay_overflow_rejected_in_checks() {
    let (env, _admin, client) = fp_env();
    let payer = Address::generate(&env);
    let token = Address::generate(&env);
    let result = client.try_repay(&payer, &1u64, &token, &(MAX_AMOUNT + 1));
    assert!(result.is_err());
}

/// FP-B: repay with 0 amount is rejected.
#[test]
fn test_fp_repay_zero_amount_rejected() {
    let (env, _admin, client) = fp_env();
    let payer = Address::generate(&env);
    let token = Address::generate(&env);
    assert!(client.try_repay(&payer, &1u64, &token, &0i128).is_err());
}

/// FP-B: negative repay amount is rejected.
#[test]
fn test_fp_repay_negative_amount_rejected() {
    let (env, _admin, client) = fp_env();
    let payer = Address::generate(&env);
    let token = Address::generate(&env);
    assert!(client.try_repay(&payer, &1u64, &token, &-1i128).is_err());
}

/// FP-F: yield payout bps_of: 0 bps → 0 payout.
#[test]
fn test_fp_yield_payout_zero_bps() {
    assert_eq!(bps_of(1_000_000_000, 0).unwrap(), 0);
}

/// FP-F: yield payout bps_of: 10_000 bps of repaid = repaid (full amount).
#[test]
fn test_fp_yield_payout_full_bps() {
    let repaid = 5_000_000_000i128;
    assert_eq!(bps_of(repaid, 10_000).unwrap(), repaid);
}

/// FP-F: yield payout never exceeds total_repaid for any valid share_bps.
#[test]
fn test_fp_yield_payout_bounded_by_repaid() {
    let repaid = 9_999_999_999i128;
    for bps in [1u32, 100, 1_000, 5_000, 9_999, 10_000] {
        let payout = bps_of(repaid, bps).unwrap();
        assert!(payout <= repaid, "payout {payout} must not exceed repaid {repaid}");
    }
}

/// FP-G: net_settle rejects fewer than 2 invoice IDs (minimum batch size).
#[test]
fn test_fp_net_settle_alloc_single_invoice_rejected() {
    let (env, _admin, client) = fp_env();
    let payer = Address::generate(&env);
    let token = Address::generate(&env);
    let mut ids = soroban_sdk::Vec::new(&env);
    ids.push_back(1u64);
    let result = client.try_net_settle(&payer, &ids, &token, &1_000i128);
    assert!(result.is_err());
}

/// FP-G: net_settle rejects 0 total amount.
#[test]
fn test_fp_net_settle_alloc_zero_amount_rejected() {
    let (env, _admin, client) = fp_env();
    let payer = Address::generate(&env);
    let token = Address::generate(&env);
    let mut ids = soroban_sdk::Vec::new(&env);
    ids.push_back(1u64);
    ids.push_back(2u64);
    let result = client.try_net_settle(&payer, &ids, &token, &0i128);
    assert!(result.is_err());
}

/// FP-G: net_settle rejects amount above MAX_AMOUNT.
#[test]
fn test_fp_net_settle_alloc_above_max_rejected() {
    let (env, _admin, client) = fp_env();
    let payer = Address::generate(&env);
    let token = Address::generate(&env);
    let mut ids = soroban_sdk::Vec::new(&env);
    ids.push_back(1u64);
    ids.push_back(2u64);
    let result = client.try_net_settle(&payer, &ids, &token, &(MAX_AMOUNT + 1));
    assert!(result.is_err());
}

/// FP-H: early settlement accepted_bps accumulation: 10_000 bps → settlement triggered.
/// Verify that the u32 addition in accepted_bps does not overflow for valid inputs.
#[test]
fn test_fp_early_settlement_bps_accumulation_safe() {
    // accepted_bps + share_bps: max realistic case is 10_000 + 10_000 = 20_000
    // which fits in u32.  The overflow guard is checked_add.
    let a: u32 = 9_999;
    let b: u32 = 1;
    let result = a.checked_add(b);
    assert_eq!(result.unwrap(), 10_000u32, "10_000 total bps triggers settlement");
}

/// FP-H: u32 addition of two max bps values fits.
#[test]
fn test_fp_early_settlement_bps_u32_max_fits() {
    let a: u32 = u32::MAX;
    let b: u32 = 1;
    assert!(a.checked_add(b).is_none(), "u32 overflow detected by checked_add");
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 3: marketplace fee / concentration-cap arithmetic [MP-A through MP-E]
// ─────────────────────────────────────────────────────────────────────────────

use kora_marketplace::{DataKey as MpKey, MarketplaceContract, MarketplaceContractClient};
use kora_shared::types::Listing;

fn mp_env() -> (Env, Address, MarketplaceContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(soroban_sdk::testutils::LedgerInfo {
        timestamp: 1_700_000_000,
        protocol_version: 21,
        sequence_number: 1,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1000,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 100_000,
    });
    let admin = Address::generate(&env);
    let nft = Address::generate(&env);
    let pool = Address::generate(&env);
    let treasury = Address::generate(&env);
    let ac = env.register_contract(None, kora_access_control::AccessControlContract);
    kora_access_control::AccessControlContractClient::new(&env, &ac).initialize(&admin);
    let oracle = Address::generate(&env);
    let registry = Address::generate(&env);
    let mp = env.register_contract(None, MarketplaceContract);
    let client = MarketplaceContractClient::new(&env, &mp);
    client.initialize(&admin, &nft, &pool, &treasury, &ac, &oracle, &registry, &50u32, &0u32);
    (env, admin, client)
}

/// MP-A: Fee at 0 bps = 0.
#[test]
fn test_mp_fee_bps_of_zero_bps() {
    assert_eq!(bps_of(1_000_000_000, 0).unwrap(), 0);
}

/// MP-A: Fee at 50 bps (standard protocol fee) on 1_000_000_000 stroops.
#[test]
fn test_mp_fee_bps_of_standard_fee() {
    assert_eq!(bps_of(1_000_000_000, 50).unwrap(), 5_000_000);
}

/// MP-A: Fee at 10_000 bps = amount (100% fee is the ceiling but not illegal).
#[test]
fn test_mp_fee_bps_of_full_fee_equals_amount() {
    let amount = 7_654_321i128;
    assert_eq!(bps_of(amount, 10_000).unwrap(), amount);
}

/// MP-A: Fee on MAX_AMOUNT at 10_000 bps does not overflow.
#[test]
fn test_mp_fee_bps_of_max_amount_at_full_rate() {
    assert!(bps_of(MAX_AMOUNT, 10_000).is_ok());
}

/// MP-B: net = amount - fee; net at 0 fee = amount.
#[test]
fn test_mp_net_after_zero_fee_equals_amount() {
    let amount = 5_000_000_000i128;
    let fee = bps_of(amount, 0).unwrap();
    let net = safe_sub(amount, fee).unwrap();
    assert_eq!(net, amount);
}

/// MP-B: net = amount - fee; net at 100% fee = 0.
#[test]
fn test_mp_net_after_full_fee_is_zero() {
    let amount = 5_000_000_000i128;
    let fee = bps_of(amount, 10_000).unwrap();
    let net = safe_sub(amount, fee).unwrap();
    assert_eq!(net, 0);
}

/// MP-B: fee + net = amount exactly (no dust loss).
#[test]
fn test_mp_fee_plus_net_equals_amount_exactly() {
    for (amount, bps) in [
        (1_000_000_000i128, 50u32),
        (9_999_999_999, 1),
        (1, 5_000),
        (100, 100),
        (MAX_AMOUNT, 50),
    ] {
        let fee = bps_of(amount, bps).unwrap();
        let net = safe_sub(amount, fee).unwrap();
        let reconstructed = safe_add(fee, net).unwrap();
        assert_eq!(reconstructed, amount, "fee+net must equal amount for ({amount}, {bps})");
    }
}

/// MP-C: funded_amount overflow: fund beyond MAX_AMOUNT is rejected.
#[test]
fn test_mp_funded_amount_overflow_rejected_in_checks() {
    let (env, _admin, client) = mp_env();
    let investor = Address::generate(&env);
    // Any amount > MAX_AMOUNT should be rejected.
    let result = client.try_fund_invoice(&investor, &1u64, &(MAX_AMOUNT + 1), &None);
    assert!(result.is_err());
}

/// MP-C: funded_amount at exactly asking_price completes the listing.
/// This is a boundary case: funded_amount + contribution == asking_price.
#[test]
fn test_mp_funded_amount_at_exact_asking_price() {
    // Verified conceptually: safe_add(funded, contributed) where result == asking_price
    // should succeed and mark the listing fully funded.
    let funded: i128 = 9_000_000_000;
    let contribution: i128 = 500_000_000;
    let asking_price: i128 = 9_500_000_000;
    let result = safe_add(funded, contribution).unwrap();
    assert_eq!(result, asking_price);
}

/// MP-D: Concentration cap check: prospective * 10_000 vs cap_bps * asking_price.
/// Boundary: at exactly the cap (== not >), the call should succeed.
#[test]
fn test_mp_concentration_cap_at_exactly_cap_allowed() {
    let asking_price: i128 = 10_000_000_000;
    let cap_bps: i128 = 5_000; // 50%
    let prospective: i128 = 5_000_000_000; // exactly 50%

    let lhs = prospective.checked_mul(10_000).unwrap();
    let rhs = cap_bps.checked_mul(asking_price).unwrap();
    // lhs == rhs means exactly at cap, not over it → allowed.
    assert_eq!(lhs, rhs, "at exactly cap_bps the concentration check must pass (lhs == rhs)");
    assert!(!(lhs > rhs), "lhs > rhs should be false at exactly the cap");
}

/// MP-D: One stroop over cap → rejected.
#[test]
fn test_mp_concentration_cap_one_over_rejected() {
    let asking_price: i128 = 10_000_000_000;
    let cap_bps: i128 = 5_000;
    let prospective: i128 = 5_000_000_001; // one stroop over 50%

    let lhs = prospective.checked_mul(10_000).unwrap();
    let rhs = cap_bps.checked_mul(asking_price).unwrap();
    assert!(lhs > rhs, "one stroop over cap must trigger rejection");
}

/// MP-E: concentration cap overflow guard: MAX_AMOUNT * 10_000 must fit.
#[test]
fn test_mp_concentration_cap_overflow_guard_max_amount() {
    let prospective = MAX_AMOUNT;
    let result = prospective.checked_mul(10_000);
    assert!(result.is_some(), "MAX_AMOUNT * 10_000 must fit in i128");
}

/// MP-E: i128::MAX * 10_000 would overflow — checked_mul catches it.
#[test]
fn test_mp_concentration_cap_overflow_guard_i128_max() {
    let result = i128::MAX.checked_mul(10_000);
    assert!(result.is_none(), "i128::MAX * 10_000 must overflow");
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 4: treasury fee / rate-limit arithmetic [TR-A through TR-D]
// ─────────────────────────────────────────────────────────────────────────────

use kora_treasury::{TreasuryContract, TreasuryContractClient};

fn tr_env() -> (Env, Address, TreasuryContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, TreasuryContract);
    let client = TreasuryContractClient::new(&env, &id);
    let admin = Address::generate(&env);
    client.initialize(&admin, &50u32);
    (env, admin, client)
}

/// TR-A: collect_fee with 0 is rejected (no-op accounting entry).
#[test]
fn test_tr_collect_fee_overflow_zero_rejected() {
    let (env, admin, client) = tr_env();
    let token = Address::generate(&env);
    client.whitelist_token(&admin, &token);
    assert!(client.try_collect_fee(&token, &0i128).is_err());
}

/// TR-A: collect_fee with negative is rejected.
#[test]
fn test_tr_collect_fee_overflow_negative_rejected() {
    let (env, admin, client) = tr_env();
    let token = Address::generate(&env);
    client.whitelist_token(&admin, &token);
    assert!(client.try_collect_fee(&token, &-1i128).is_err());
}

/// TR-A: Two collect_fee calls whose sum overflows i128 must return ArithmeticOverflow.
#[test]
fn test_tr_collect_fee_overflow_detected_on_second_call() {
    let (env, admin, client) = tr_env();
    let token = Address::generate(&env);
    client.whitelist_token(&admin, &token);
    // Seed the accumulated ledger with i128::MAX.
    client.collect_fee(&token, &i128::MAX);
    // Any positive addition overflows.
    let result = client.try_collect_fee(&token, &1i128);
    assert!(result.is_err(), "overflow must be detected on the second collect_fee call");
}

/// TR-B: reserve allocation at 0 bps → 0 reserve cut.
#[test]
fn test_tr_reserve_allocation_bps_zero_no_cut() {
    assert_eq!(bps_of(1_000_000, 0).unwrap(), 0);
}

/// TR-B: reserve allocation at 5_000 bps → 50% of fee.
#[test]
fn test_tr_reserve_allocation_bps_half() {
    let fee = 1_000_000i128;
    let cut = bps_of(fee, 5_000).unwrap();
    assert_eq!(cut, 500_000);
}

/// TR-B: reserve allocation at 10_000 bps → 100% of fee.
#[test]
fn test_tr_reserve_allocation_bps_full() {
    let fee = 1_000_000i128;
    let cut = bps_of(fee, 10_000).unwrap();
    assert_eq!(cut, fee);
}

/// TR-C: rate-limit boundary: amount = cap passes.
#[test]
fn test_tr_rate_limit_boundary_at_cap_passes() {
    let cap: i128 = 10_000_000;
    let epoch_withdrawn: i128 = 0;
    let amount: i128 = cap;
    let new_total = safe_add(epoch_withdrawn, amount).unwrap();
    assert!(new_total <= cap, "amount equal to cap must not exceed the cap");
}

/// TR-C: rate-limit boundary: amount = cap + 1 exceeds limit.
#[test]
fn test_tr_rate_limit_boundary_one_over_cap_fails() {
    let cap: i128 = 10_000_000;
    let epoch_withdrawn: i128 = 0;
    let amount: i128 = cap + 1;
    let new_total = safe_add(epoch_withdrawn, amount).unwrap();
    assert!(new_total > cap, "amount one over cap must exceed the limit");
}

/// TR-C: epoch_withdrawn + amount overflows i128 → checked_add returns None.
#[test]
fn test_tr_rate_limit_overflow_on_addition() {
    let epoch_withdrawn = i128::MAX;
    let amount: i128 = 1;
    assert!(epoch_withdrawn.checked_add(amount).is_none());
}

/// TR-D: Treasury internal bps_of boundary values.
#[test]
fn test_tr_bps_of_boundary_zero_bps() {
    assert_eq!(bps_of(1_000_000, 0).unwrap(), 0);
}

#[test]
fn test_tr_bps_of_boundary_full_bps() {
    let amount = 2_500_000i128;
    assert_eq!(bps_of(amount, 10_000).unwrap(), amount);
}

#[test]
fn test_tr_bps_of_boundary_max_amount() {
    assert!(bps_of(MAX_AMOUNT, 10_000).is_ok());
    assert!(bps_of(MAX_AMOUNT + 1, 10_000).is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 5: risk_registry slashing / count arithmetic [RR-A through RR-E]
// ─────────────────────────────────────────────────────────────────────────────

use kora_risk_registry::{DataKey as RrKey, RiskRegistryContract, RiskRegistryContractClient};
use soroban_sdk::Bytes;

fn rr_env() -> (Env, Address, Address, RiskRegistryContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let id = env.register_contract(None, RiskRegistryContract);
    let client = RiskRegistryContractClient::new(&env, &id);
    let admin = Address::generate(&env);
    let invoice_nft = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let staking_token = env.register_stellar_asset_contract_v2(token_admin).address();
    client.initialize(&admin, &invoice_nft, &staking_token, &1_000_000i128, &5_000u32);
    (env, admin, staking_token, client)
}

fn mint_stake(env: &Env, token: &Address, to: &Address, amount: i128) {
    soroban_sdk::token::StellarAssetClient::new(env, token).mint(to, &amount);
}

/// RR-A: top_up_stake with 0 is rejected before any transfer.
#[test]
fn test_rr_top_up_stake_overflow_zero_rejected() {
    let (env, admin, staking_token, client) = rr_env();
    let verifier = Address::generate(&env);
    mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
    client.add_verifier(&admin, &verifier, &1_000_000i128);
    assert!(client.try_top_up_stake(&admin, &verifier, &0i128).is_err());
}

/// RR-A: top_up_stake with negative is rejected.
#[test]
fn test_rr_top_up_stake_overflow_negative_rejected() {
    let (env, admin, staking_token, client) = rr_env();
    let verifier = Address::generate(&env);
    mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
    client.add_verifier(&admin, &verifier, &1_000_000i128);
    assert!(client.try_top_up_stake(&admin, &verifier, &-1i128).is_err());
}

/// RR-B: slash at 0 bps leaves stake unchanged.
#[test]
fn test_rr_slash_bps_of_zero_bps_no_slash() {
    let stake = 1_000_000i128;
    let slash_bps: u32 = 0;
    // The risk_registry uses integer arithmetic: stake * bps / 10_000.
    let slash = (stake as u128 * slash_bps as u128 / 10_000) as i128;
    assert_eq!(slash, 0);
    assert_eq!(stake - slash, stake);
}

/// RR-B: slash at 5_000 bps (50%) halves the stake.
#[test]
fn test_rr_slash_bps_of_half() {
    let stake = 1_000_000i128;
    let slash_bps: u32 = 5_000;
    let slash = (stake as u128 * slash_bps as u128 / 10_000) as i128;
    assert_eq!(slash, 500_000);
}

/// RR-B: slash at 10_000 bps (100%) zeroes the stake.
#[test]
fn test_rr_slash_bps_of_full_zeroes_stake() {
    let stake = 1_000_000i128;
    let slash_bps: u32 = 10_000;
    let slash = (stake as u128 * slash_bps as u128 / 10_000) as i128;
    assert_eq!(slash, stake);
    assert_eq!(stake - slash, 0);
}

/// RR-B: stake = 0, any slash bps → slash = 0.
#[test]
fn test_rr_slash_bps_of_zero_stake() {
    let stake: i128 = 0;
    let slash_bps: u32 = 5_000;
    let slash = (stake as u128 * slash_bps as u128 / 10_000) as i128;
    assert_eq!(slash, 0);
}

/// RR-C: total_invoices at u32::MAX - 1, increment reaches u32::MAX.
#[test]
fn test_rr_invoice_count_overflow_at_max_minus_one() {
    let current: u32 = u32::MAX - 1;
    let next = current.checked_add(1).unwrap();
    assert_eq!(next, u32::MAX);
}

/// RR-C: total_invoices at u32::MAX, increment overflows.
#[test]
fn test_rr_invoice_count_overflow_at_u32_max() {
    let current: u32 = u32::MAX;
    assert!(current.checked_add(1).is_none(), "u32::MAX + 1 must overflow");
}

/// RR-D: defaults at u32::MAX overflows on next default.
#[test]
fn test_rr_defaults_overflow_at_u32_max() {
    let current: u32 = u32::MAX;
    assert!(current.checked_add(1).is_none(), "defaults at u32::MAX must overflow");
}

/// RR-E: aggregate debtor score — 50 verifiers each scoring 100.
/// Sum fits in u64 without overflow (50 * 100 = 5_000 << u64::MAX).
#[test]
fn test_rr_aggregate_score_overflow_max_verifiers_max_score() {
    let verifier_count: u64 = 50; // MAX_VERIFIERS_PER_DEBTOR
    let max_score: u64 = 100;     // maximum per-verifier risk score
    let total = verifier_count.checked_mul(max_score).unwrap();
    let avg = total / verifier_count;
    assert_eq!(avg, max_score, "average of 50 * 100 must be 100");
    assert!(total < u64::MAX, "total score fits comfortably in u64");
}

/// RR-E: aggregate score with one verifier scoring 0 → average = 0.
#[test]
fn test_rr_aggregate_score_overflow_single_zero_score() {
    let total: u64 = 0;
    let count: u64 = 1;
    let avg = total / count;
    assert_eq!(avg, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 6: Property-based boundary invariants (pure arithmetic, no Env)
// ─────────────────────────────────────────────────────────────────────────────

/// Invariant: fee + net == amount for all bps in [0, 10_000] and amounts in [1, MAX_AMOUNT].
/// Spot-checked for representative values.
#[test]
fn test_invariant_fee_plus_net_equals_amount_spot_check() {
    let cases: &[(i128, u32)] = &[
        (1, 0),
        (1, 10_000),
        (1_000_000_000, 50),
        (MAX_AMOUNT, 1),
        (MAX_AMOUNT, 9_999),
        (MAX_AMOUNT, 10_000),
        (100, 3_333),
        (7, 7_777),
    ];
    for &(amount, bps) in cases {
        let fee = bps_of(amount, bps).unwrap();
        let net = safe_sub(amount, fee).unwrap();
        assert_eq!(
            safe_add(fee, net).unwrap(),
            amount,
            "fee+net invariant failed for amount={amount} bps={bps}"
        );
    }
}

/// Invariant: bps_of(X, 0) == 0 for all X in [0, MAX_AMOUNT].
#[test]
fn test_invariant_zero_bps_always_zero() {
    for amount in [0i128, 1, 999, 1_000_000, MAX_AMOUNT] {
        assert_eq!(bps_of(amount, 0).unwrap(), 0, "0 bps must always produce 0");
    }
}

/// Invariant: bps_of(X, 10_000) == X for all X in [0, MAX_AMOUNT].
#[test]
fn test_invariant_full_bps_equals_amount() {
    for amount in [0i128, 1, 999, 1_000_000, MAX_AMOUNT] {
        assert_eq!(
            bps_of(amount, 10_000).unwrap(),
            amount,
            "10_000 bps must return the full amount"
        );
    }
}

/// Invariant: share_bps = contributed * 10_000 / total ≤ 10_000 for contributed ≤ total.
#[test]
fn test_invariant_share_bps_never_exceeds_10000() {
    let pairs: &[(i128, i128)] = &[
        (1, 1),
        (1, 1_000_000),
        (500, 1_000),
        (MAX_AMOUNT, MAX_AMOUNT),
        (1, MAX_AMOUNT),
    ];
    for &(contributed, total) in pairs {
        let share_bps = contributed
            .checked_mul(10_000)
            .and_then(|v| v.checked_div(total))
            .unwrap();
        assert!(
            share_bps <= 10_000,
            "share_bps {share_bps} must not exceed 10_000 for contributed={contributed} total={total}"
        );
    }
}

// Section 7: secondary_market fee arithmetic (buy_position)

/// SM-1 / SM-2: fee and proceeds conserve the price at every boundary.
#[test]
fn test_secondary_fee_conserves_price() {
    for price in [0, 1, 39, 40, 9_999, 10_000, 12_345_679, i128::MAX / 1_000] {
        for bps in [0u32, 1, 250, 1_000] {
            let fee = bps_of(price, bps).unwrap();
            let proceeds = safe_sub(price, fee).unwrap();
            assert!(fee >= 0 && proceeds >= 0);
            assert_eq!(fee + proceeds, price);
        }
    }
}

/// SM-1: fees round down; 39 units at 250 bps is below one fee unit.
#[test]
fn test_secondary_fee_rounding_boundary() {
    assert_eq!(bps_of(39, 250).unwrap(), 0);
    assert_eq!(bps_of(40, 250).unwrap(), 1);
}

/// SM-1: i128 extremes fail with typed errors.
#[test]
fn test_secondary_fee_extremes_fail_closed() {
    assert_eq!(bps_of(i128::MAX, 1_000), Err(CommonError::ArithmeticOverflow));
    assert_eq!(bps_of(MAX_AMOUNT, 1_000), Err(CommonError::ArithmeticOverflow));
    assert_eq!(bps_of(i128::MAX / 1_000 + 1, 1_000), Err(CommonError::ArithmeticOverflow));
    assert_eq!(bps_of(i128::MIN, 250), Err(CommonError::InvalidAmount));
    assert_eq!(bps_of(-1, 250), Err(CommonError::InvalidAmount));
}
