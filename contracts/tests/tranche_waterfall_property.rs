//! Tranche waterfall distribution property tests (issue #1102).
//!
//! Exercises `kora_tranche::compute_waterfall` and `kora_tranche::split_pro_rata`
//! across random repayment amounts and holder sets. Invariants asserted
//! (documented in `contracts/tranche/src/lib.rs`):
//!
//! - sum(payouts) <= available, with the remainder reported as residual/dust
//! - senior is paid in full before junior receives anything
//! - per-class caps (`senior <= senior_owed`, `junior <= junior_owed`)
//! - zero-junior tranches route everything past senior to residual
//! - monotonic payouts as `available` grows
//! - pro-rata dust is `0 <= dust < holders` and is never paid out

use kora_tranche::{compute_waterfall, split_pro_rata, TrancheError, WaterfallPayout};
use proptest::prelude::*;

/// Upper bound keeps `amount * weight` inside i128 for the pro-rata split.
const MAX_AMOUNT: i128 = 1_000_000_000_000_000; // 1e18 stroops
const MAX_WEIGHT: i128 = 1_000_000_000_000; // 1e12

fn amount() -> impl Strategy<Value = i128> {
    prop_oneof![
        Just(0i128),
        Just(1i128),
        0i128..1_000,
        0i128..=MAX_AMOUNT,
    ]
}

fn assert_class_invariants(available: i128, senior_owed: i128, junior_owed: i128, p: WaterfallPayout) {
    assert!(p.senior >= 0 && p.junior >= 0 && p.residual >= 0);
    assert_eq!(p.senior + p.junior + p.residual, available, "conservation");
    assert!(p.senior + p.junior <= available, "over-distribution");
    assert!(p.senior <= senior_owed);
    assert!(p.junior <= junior_owed);
    if p.junior > 0 {
        assert_eq!(p.senior, senior_owed, "junior paid before senior was whole");
    }
    if p.residual > 0 {
        assert_eq!(p.senior, senior_owed);
        assert_eq!(p.junior, junior_owed);
    }
    if junior_owed == 0 {
        assert_eq!(p.junior, 0);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5_000))]

    #[test]
    fn waterfall_never_over_distributes(
        available in amount(),
        senior_owed in amount(),
        junior_owed in amount(),
    ) {
        let p = compute_waterfall(available, senior_owed, junior_owed).unwrap();
        assert_class_invariants(available, senior_owed, junior_owed, p);
    }

    #[test]
    fn waterfall_zero_junior(available in amount(), senior_owed in amount()) {
        let p = compute_waterfall(available, senior_owed, 0).unwrap();
        assert_class_invariants(available, senior_owed, 0, p);
        prop_assert_eq!(p.senior, available.min(senior_owed));
        prop_assert_eq!(p.residual, available - p.senior);
    }

    #[test]
    fn waterfall_is_monotonic_in_available(
        available in 0i128..MAX_AMOUNT,
        extra in 0i128..1_000_000,
        senior_owed in amount(),
        junior_owed in amount(),
    ) {
        let lo = compute_waterfall(available, senior_owed, junior_owed).unwrap();
        let hi = compute_waterfall(available + extra, senior_owed, junior_owed).unwrap();
        prop_assert!(hi.senior >= lo.senior);
        prop_assert!(hi.junior >= lo.junior);
        prop_assert!(hi.residual >= lo.residual);
    }

    /// Many sequential repayments: cumulative payouts never exceed cumulative
    /// repayments and senior is fully settled before any junior cashflow.
    #[test]
    fn sequential_repayments_preserve_priority(
        senior_owed in 0i128..10_000_000,
        junior_owed in 0i128..10_000_000,
        repayments in prop::collection::vec(0i128..3_000_000, 1..20),
    ) {
        let (mut senior_left, mut junior_left) = (senior_owed, junior_owed);
        let (mut paid_senior, mut paid_junior, mut repaid) = (0i128, 0i128, 0i128);
        for r in repayments {
            repaid += r;
            let p = compute_waterfall(r, senior_left, junior_left).unwrap();
            assert_class_invariants(r, senior_left, junior_left, p);
            if p.junior > 0 {
                prop_assert_eq!(paid_senior + p.senior, senior_owed);
            }
            senior_left -= p.senior;
            junior_left -= p.junior;
            paid_senior += p.senior;
            paid_junior += p.junior;
            prop_assert!(paid_senior + paid_junior <= repaid);
            prop_assert!(paid_senior <= senior_owed && paid_junior <= junior_owed);
        }
        // Path independence: the class totals equal a single lump-sum waterfall.
        let lump = compute_waterfall(repaid, senior_owed, junior_owed).unwrap();
        prop_assert_eq!(lump.senior, paid_senior);
        prop_assert_eq!(lump.junior, paid_junior);
    }

    #[test]
    fn pro_rata_split_never_over_distributes(
        class_payout in amount(),
        weights in prop::collection::vec(0i128..=MAX_WEIGHT, 1..32),
    ) {
        let mut out = vec![0i128; weights.len()];
        let dust = split_pro_rata(class_payout, &weights, &mut out).unwrap();
        let paid: i128 = out.iter().sum();
        prop_assert!(dust >= 0);
        prop_assert_eq!(paid + dust, class_payout);
        prop_assert!(paid <= class_payout);
        let total_weight: i128 = weights.iter().sum();
        if total_weight > 0 {
            prop_assert!(dust < weights.len() as i128, "dust {} >= holders {}", dust, weights.len());
            for (w, o) in weights.iter().zip(out.iter()) {
                prop_assert!(*o >= 0);
                // floor(payout * w / total) — never above the exact share.
                prop_assert!(*o * total_weight <= class_payout * *w);
            }
        } else {
            prop_assert_eq!(dust, class_payout);
        }
    }

    /// End-to-end: class waterfall then holder split; total holder payouts
    /// across both classes never exceed the repayment.
    #[test]
    fn full_distribution_bounded_by_available(
        available in 0i128..MAX_AMOUNT,
        senior_owed in 0i128..MAX_AMOUNT,
        junior_owed in 0i128..MAX_AMOUNT,
        senior_weights in prop::collection::vec(1i128..=MAX_WEIGHT, 1..16),
        junior_weights in prop::collection::vec(1i128..=MAX_WEIGHT, 0..16),
    ) {
        let junior_owed = if junior_weights.is_empty() { 0 } else { junior_owed };
        let p = compute_waterfall(available, senior_owed, junior_owed).unwrap();

        let mut s_out = vec![0i128; senior_weights.len()];
        let s_dust = split_pro_rata(p.senior, &senior_weights, &mut s_out).unwrap();
        let mut j_out = vec![0i128; junior_weights.len()];
        let j_dust = split_pro_rata(p.junior, &junior_weights, &mut j_out).unwrap();

        let holders_paid: i128 = s_out.iter().sum::<i128>() + j_out.iter().sum::<i128>();
        prop_assert!(holders_paid <= available);
        prop_assert_eq!(holders_paid + s_dust + j_dust + p.residual, available);
    }
}

#[test]
fn exact_coverage_leaves_no_residual() {
    let p = compute_waterfall(150, 100, 50).unwrap();
    assert_eq!(p, WaterfallPayout { senior: 100, junior: 50, residual: 0 });
}

#[test]
fn shortfall_hits_junior_first() {
    let p = compute_waterfall(120, 100, 50).unwrap();
    assert_eq!(p, WaterfallPayout { senior: 100, junior: 20, residual: 0 });
    let p = compute_waterfall(80, 100, 50).unwrap();
    assert_eq!(p, WaterfallPayout { senior: 80, junior: 0, residual: 0 });
}

#[test]
fn zero_available_pays_nothing() {
    let p = compute_waterfall(0, 100, 50).unwrap();
    assert_eq!(p, WaterfallPayout { senior: 0, junior: 0, residual: 0 });
}

#[test]
fn negative_inputs_rejected() {
    assert_eq!(compute_waterfall(-1, 0, 0), Err(TrancheError::InvalidAmount));
    assert_eq!(compute_waterfall(0, -1, 0), Err(TrancheError::InvalidAmount));
    assert_eq!(compute_waterfall(0, 0, -1), Err(TrancheError::InvalidAmount));
    let mut out = [0i128; 1];
    assert_eq!(split_pro_rata(-1, &[1], &mut out), Err(TrancheError::InvalidAmount));
    assert_eq!(split_pro_rata(1, &[-1], &mut out), Err(TrancheError::InvalidAmount));
    assert_eq!(split_pro_rata(1, &[1, 1], &mut out), Err(TrancheError::InvalidAmount));
}

#[test]
fn dust_policy_example() {
    // 100 split three ways: 33 each, 1 stroop of dust retained by the tranche.
    let mut out = [0i128; 3];
    let dust = split_pro_rata(100, &[1, 1, 1], &mut out).unwrap();
    assert_eq!(out, [33, 33, 33]);
    assert_eq!(dust, 1);
}

#[test]
fn zero_weight_class_retains_everything() {
    let mut out = [0i128; 2];
    let dust = split_pro_rata(500, &[0, 0], &mut out).unwrap();
    assert_eq!(out, [0, 0]);
    assert_eq!(dust, 500);
}
