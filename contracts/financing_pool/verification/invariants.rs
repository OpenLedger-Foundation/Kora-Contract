// contracts/financing_pool/verification/invariants.rs
//!
//! Bounded exhaustive enumeration driver.
//!
//! This is the entry point for the formal checks.  It enumerates scenarios
//! systematically across the configured parameter ranges and calls
//! `model::run_scenario` / `model::run_partial_repay_scenario` on each.
//! Any invariant violation returns `Err` with the concrete counterexample.
//!
//! The enumeration is parameterised so CI can run it quickly (small bounds)
//! while a longer local run uses larger bounds for deeper assurance.

use alloc::{format, string::String, string::ToString, vec, vec::Vec};
use super::model::{
    run_partial_repay_scenario, run_scenario, run_multi_asset_scenario,
    BPS_DENOM, MAX_FACE_VALUE, MAX_INVESTORS, MIN_FACE_VALUE, TOKEN_DECIMALS, ALT_TOKEN_DECIMALS,
    MultiAssetScenario, PartialRepayScenario, Scenario, bps_of_normalized,
};

/// Enumeration bounds — tunable via env vars in tests.
pub struct Bounds {
    /// Number of distinct face values to sample.
    pub face_value_samples: usize,
    /// Max number of investors per scenario.
    pub max_investors: usize,
    /// Number of distinct contribution splits to sample per investor count.
    pub contribution_splits: usize,
    /// Number of distinct repayment multiples to sample.
    pub repayment_samples: usize,
    /// Number of distinct partial-repayment tranche sequences to sample.
    pub tranche_sequences: usize,
}

impl Bounds {
    /// Fast bounds for CI (completes in < 5 s).
    pub fn ci() -> Self {
        Bounds {
            face_value_samples: 12,
            max_investors: 4,
            contribution_splits: 8,
            repayment_samples: 6,
            tranche_sequences: 4,
        }
    }

    /// Thorough bounds for local / pre-merge runs (~30 s).
    pub fn thorough() -> Self {
        Bounds {
            face_value_samples: 40,
            max_investors: MAX_INVESTORS,
            contribution_splits: 20,
            repayment_samples: 12,
            tranche_sequences: 10,
        }
    }
}

/// Sample `n` face values log-uniformly between MIN_FACE_VALUE and MAX_FACE_VALUE.
fn sample_face_values(n: usize) -> Vec<i128> {
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![MIN_FACE_VALUE];
    }
    let lo = (MIN_FACE_VALUE as f64).ln();
    let hi = (MAX_FACE_VALUE as f64).ln();
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let v = (lo + t * (hi - lo)).exp() as i128;
            // Snap to a round number to exercise integer-boundary rounding.
            (v / MIN_FACE_VALUE) * MIN_FACE_VALUE
        })
        .collect()
}

/// Generate `n` ways to split `total` into `k` positive parts using a
/// uniform-ish lattice traversal.  Each part is >= 1.
fn split_total(total: i128, k: usize, n: usize) -> Vec<Vec<i128>> {
    if k == 0 || total <= 0 {
        return vec![];
    }
    if k == 1 {
        return vec![vec![total]];
    }
    let mut results = Vec::new();

    // Always include the equal split.
    let equal = total / k as i128;
    if equal > 0 {
        let mut v = vec![equal; k];
        // Assign remainder to the first element.
        v[0] += total - equal * k as i128;
        results.push(v);
    }

    // Extreme split: one investor holds maximum, others hold 1.
    let mut extreme = vec![1i128; k];
    extreme[0] = total - (k as i128 - 1);
    if extreme[0] > 0 {
        results.push(extreme);
    }

    // Uniform random-ish splits via a simple stride.
    let stride = (total / (n as i128 + 1)).max(1);
    let mut first = stride;
    while results.len() < n && first < total - (k as i128 - 1) {
        let mut v = vec![1i128; k];
        v[0] = first;
        let remaining = total - first;
        let per_rest = remaining / (k as i128 - 1);
        for j in 1..k {
            v[j] = if j == k - 1 {
                remaining - per_rest * (k as i128 - 2)
            } else {
                per_rest
            };
        }
        if v.iter().all(|&x| x > 0) {
            results.push(v);
        }
        first += stride;
    }

    results
}

/// Repayment amounts to test: below cost (partial default), at cost,
/// and several multiples representing yield.
fn repayment_amounts(total_funded: i128, n: usize) -> Vec<i128> {
    let mut amounts = vec![
        1,                          // extreme partial
        total_funded / 2,           // 50% recovery
        total_funded - 1,           // just under principal
        total_funded,               // exact principal (0 yield)
        total_funded * 105 / 100,   // 5% yield
        total_funded * 110 / 100,   // 10% yield
        total_funded * 120 / 100,   // 20% yield
        total_funded * 200 / 100,   // 100% yield (2×)
    ];
    // Add boundary values that stress the decimal normalisation.
    amounts.push(total_funded + 1);
    amounts.push(total_funded + 7); // 7 = 10^0 boundary for 7-decimal token
    amounts.push(total_funded + 10_000_000); // +1 USDC
    amounts.retain(|&x| x > 0);
    amounts.sort_unstable();
    amounts.dedup();
    amounts.truncate(n);
    amounts
}

/// Generate `n` ways to split `total` into `k` ordered tranches, each > 0
/// and summing to at most `total`.
fn split_into_tranches(total: i128, k: usize, n: usize) -> Vec<Vec<i128>> {
    if k == 0 || total <= 0 {
        return vec![];
    }
    if k == 1 {
        // Single tranche: try several partial fractions.
        let mut v = vec![
            1i128,
            total / 4,
            total / 2,
            total * 3 / 4,
            total - 1,
            total,
        ];
        v.retain(|&x| x > 0 && x <= total);
        v.dedup();
        return v.into_iter().map(|x| vec![x]).collect();
    }

    let mut results = Vec::new();

    // Equal tranches.
    let equal = total / k as i128;
    if equal > 0 {
        let mut v = vec![equal; k];
        v[k - 1] = total - equal * (k as i128 - 1); // last tranche absorbs remainder
        if v.iter().all(|&x| x > 0) {
            results.push(v);
        }
    }

    // Small first tranche, large final.
    let small = (total / 10).max(1);
    if small < total {
        let remaining = total - small;
        let mid = remaining / (k as i128 - 1);
        if mid > 0 {
            let mut v = vec![mid; k];
            v[0] = small;
            v[k - 1] = remaining - mid * (k as i128 - 2);
            if v.iter().all(|&x| x > 0) {
                results.push(v);
            }
        }
    }

    // Decreasing tranches: 50%, 30%, 20% (for k=3).
    if k == 3 {
        let a = total * 50 / 100;
        let b = total * 30 / 100;
        let c = total - a - b;
        if a > 0 && b > 0 && c > 0 {
            results.push(vec![a, b, c]);
        }
    }

    // Stride-based enumeration.
    let stride = (total / (n as i128 * k as i128 + 1)).max(1);
    let mut first = stride;
    while results.len() < n && first < total {
        let rest = total - first;
        let per_rest = rest / (k as i128 - 1);
        if per_rest > 0 {
            let mut v = vec![per_rest; k];
            v[0] = first;
            v[k - 1] = rest - per_rest * (k as i128 - 2);
            if v.iter().all(|&x| x > 0) {
                results.push(v.clone());
                // Also test partial sums < total_owed (the pool stays open).
                let partial_total: i128 = v.iter().sum::<i128>() * 80 / 100;
                if partial_total > 0 && partial_total < total {
                    let scale = partial_total as f64 / v.iter().sum::<i128>() as f64;
                    let scaled: Vec<i128> = v.iter().map(|&x| ((x as f64 * scale) as i128).max(1)).collect();
                    let sum: i128 = scaled.iter().sum();
                    if sum <= total && scaled.iter().all(|&x| x > 0) {
                        results.push(scaled);
                    }
                }
            }
        }
        first += stride;
    }

    results
}

/// Run the full bounded exhaustive enumeration (I1–I3).
/// Returns `Ok(count)` (number of scenarios checked) or `Err(counterexample)`.
pub fn run_exhaustive_check(bounds: &Bounds) -> Result<usize, String> {
    let face_values = sample_face_values(bounds.face_value_samples);
    let mut checked = 0usize;

    for fv in &face_values {
        let fv = *fv;

        for k in 1..=bounds.max_investors {
            let splits = split_total(fv, k, bounds.contribution_splits);

            for contributions in &splits {
                let total_funded: i128 = contributions.iter().sum();
                assert!(total_funded <= fv, "split exceeds face_value");

                let repayments = repayment_amounts(total_funded, bounds.repayment_samples);
                for repaid in repayments {
                    let scenario = Scenario {
                        face_value: fv,
                        contributions: contributions.clone(),
                        repaid_amount: repaid,
                    };
                    run_scenario(&scenario).map_err(|e| {
                        format!(
                            "COUNTEREXAMPLE:\n  scenario={:?}\n  violation={}",
                            scenario, e
                        )
                    })?;
                    checked += 1;
                }
            }
        }
    }

    Ok(checked)
}

/// Run the bounded exhaustive enumeration for I4 and I5 (partial repayment).
///
/// For each (face_value, contributions, tranche_sequence) triple, runs
/// `run_partial_repay_scenario` and asserts I4 and I5 on every intermediate
/// state.
pub fn run_partial_repay_check(bounds: &Bounds) -> Result<usize, String> {
    let face_values = sample_face_values(bounds.face_value_samples);
    let mut checked = 0usize;

    for fv in &face_values {
        let fv = *fv;

        for k in 1..=bounds.max_investors.min(3) {
            // Limit investor count for partial scenarios to keep run time tractable.
            let splits = split_total(fv, k, bounds.contribution_splits.min(6));

            for contributions in &splits {
                let total_funded: i128 = contributions.iter().sum();

                for num_tranches in 1usize..=3 {
                    // Generate tranche sequences that sum to <= total_funded * 120 / 100
                    // (allowing up to 20% yield on top of principal).
                    let max_repay = total_funded * 120 / 100;
                    let tranche_lists = split_into_tranches(
                        max_repay,
                        num_tranches,
                        bounds.tranche_sequences,
                    );

                    for tranches in tranche_lists {
                        let tranche_sum: i128 = tranches.iter().sum();
                        if tranche_sum <= 0 { continue; }

                        let scenario = PartialRepayScenario {
                            face_value: fv,
                            contributions: contributions.clone(),
                            tranches,
                            final_repayment: None,
                            token_decimals: TOKEN_DECIMALS,
                        };
                        run_partial_repay_scenario(&scenario).map_err(|e| {
                            format!(
                                "COUNTEREXAMPLE (I4/I5):\n  scenario={:?}\n  violation={}",
                                scenario, e
                            )
                        })?;
                        checked += 1;
                    }
                }
            }
        }
    }

    Ok(checked)
}

/// Run the multi-asset extension checks: I4 and I5 across pools with
/// different token decimal precisions.
pub fn run_multi_asset_check(bounds: &Bounds) -> Result<usize, String> {
    let face_values = sample_face_values(bounds.face_value_samples.min(6));
    let mut checked = 0usize;

    for fv in &face_values {
        let fv = *fv;

        // Use 2-investor pools to keep the cross-product manageable.
        let splits_7 = split_total(fv, 2, bounds.contribution_splits.min(4));
        let splits_6 = split_total(fv, 2, bounds.contribution_splits.min(4));

        for contributions_a in &splits_7 {
            for contributions_b in &splits_6 {
                let total_a: i128 = contributions_a.iter().sum();
                let total_b: i128 = contributions_b.iter().sum();

                let tranches_a = vec![total_a * 110 / 100]; // 10% yield
                let tranches_b = vec![total_b, total_b * 10 / 100]; // principal + 10%

                let scenario = MultiAssetScenario {
                    pool_a: PartialRepayScenario {
                        face_value: fv,
                        contributions: contributions_a.clone(),
                        tranches: tranches_a,
                        final_repayment: None,
                        token_decimals: TOKEN_DECIMALS,
                    },
                    pool_b: PartialRepayScenario {
                        face_value: fv,
                        contributions: contributions_b.clone(),
                        tranches: tranches_b,
                        final_repayment: None,
                        token_decimals: ALT_TOKEN_DECIMALS,
                    },
                };
                run_multi_asset_scenario(&scenario).map_err(|e| {
                    format!("COUNTEREXAMPLE (multi-asset):\n  violation={}", e)
                })?;
                checked += 1;
            }
        }
    }

    Ok(checked)
}

// ── Targeted adversarial scenarios ───────────────────────────────────────────

/// Additional hand-crafted scenarios targeting specific boundary conditions
/// that systematic sampling might miss.
pub fn run_adversarial_scenarios() -> Result<usize, String> {
    let mut checked = 0;

    // --- Boundary: single investor, 100% share ---
    run_scenario(&Scenario {
        face_value: 10_000_000_000,
        contributions: vec![10_000_000_000],
        repaid_amount: 12_000_000_000,
    })?;
    checked += 1;

    // --- Boundary: MAX_INVESTORS all equal share ---
    run_scenario(&Scenario {
        face_value: MAX_INVESTORS as i128 * MIN_FACE_VALUE,
        contributions: vec![MIN_FACE_VALUE; MAX_INVESTORS],
        repaid_amount: MAX_INVESTORS as i128 * MIN_FACE_VALUE + 7,
    })?;
    checked += 1;

    // --- Boundary: tiny repayment (1 stroop) ---
    run_scenario(&Scenario {
        face_value: 10_000_000_000,
        contributions: vec![5_000_000_000, 5_000_000_000],
        repaid_amount: 1,
    })?;
    checked += 1;

    // --- Boundary: repayment amount is not divisible by 10_000 (bps denom) ---
    run_scenario(&Scenario {
        face_value: 10_000_000_000,
        contributions: vec![3_333_333_333, 3_333_333_334, 3_333_333_333],
        repaid_amount: 10_000_000_007,
    })?;
    checked += 1;

    // --- Boundary: very small contributions that produce share_bps = 0 due to truncation ---
    run_scenario(&Scenario {
        face_value: 1_000_000_000_000,
        contributions: vec![1, 999_999_999_999],
        repaid_amount: 1_100_000_000_000,
    })?;
    checked += 1;

    // --- Boundary: single stroop face value (dust pool) ---
    run_scenario(&Scenario {
        face_value: MIN_FACE_VALUE,
        contributions: vec![MIN_FACE_VALUE / 3, MIN_FACE_VALUE / 3, MIN_FACE_VALUE - 2 * (MIN_FACE_VALUE / 3)],
        repaid_amount: MIN_FACE_VALUE + 1,
    })?;
    checked += 1;

    // --- FINDING-V1 counterexample: prime-ish repayment with 3 equal-share investors ---
    run_scenario(&Scenario {
        face_value: 10_000_000_000,
        contributions: vec![3_333_333_333, 3_333_333_333, 3_333_333_334],
        repaid_amount: 10_000_007,
    })?;
    checked += 1;

    // --- All investors hold exactly equal shares (4 × 2500 bps) ---
    run_scenario(&Scenario {
        face_value: 400_000_000,
        contributions: vec![100_000_000, 100_000_000, 100_000_000, 100_000_000],
        repaid_amount: 440_000_000,
    })?;
    checked += 1;

    // --- I4/I5 adversarial: many micro-tranches (10 × 1/10 of face value) ---
    {
        let fv = 10_000_000_000i128;
        let tranche = fv / 10; // 1 000 000 000 per tranche
        run_partial_repay_scenario(&PartialRepayScenario {
            face_value: fv,
            contributions: vec![5_000_000_000, 5_000_000_000],
            tranches: vec![tranche; 10],
            final_repayment: None,
            token_decimals: TOKEN_DECIMALS,
        })?;
        checked += 1;
    }

    // --- I5: single tranche that exactly equals total_owed ---
    {
        let fv = 50_000_000_000i128;
        run_partial_repay_scenario(&PartialRepayScenario {
            face_value: fv,
            contributions: vec![25_000_000_000, 25_000_000_000],
            tranches: vec![fv], // repay in full via repay_partial
            final_repayment: None,
            token_decimals: TOKEN_DECIMALS,
        })?;
        checked += 1;
    }

    // --- I5: attempt to overpay (should be caught as Err, not a panic) ---
    {
        let fv = 10_000_000_000i128;
        let result = run_partial_repay_scenario(&PartialRepayScenario {
            face_value: fv,
            contributions: vec![5_000_000_000, 5_000_000_000],
            tranches: vec![fv, 1_000_000], // second tranche would exceed total_owed
            final_repayment: None,
            token_decimals: TOKEN_DECIMALS,
        });
        // This must return Err (I5 pre-check), not panic.
        assert!(
            result.is_err(),
            "I5 boundary: overpayment attempt must return Err, not Ok"
        );
        checked += 1;
    }

    // --- Multi-asset: two pools with different decimal precisions, 3 investors each ---
    {
        run_multi_asset_scenario(&MultiAssetScenario {
            pool_a: PartialRepayScenario {
                face_value: 10_000_000_000,
                contributions: vec![3_333_333_333, 3_333_333_333, 3_333_333_334],
                tranches: vec![5_000_000_000, 6_500_000_007],
                final_repayment: None,
                token_decimals: TOKEN_DECIMALS, // 7 decimals
            },
            pool_b: PartialRepayScenario {
                face_value: 10_000_000_000,
                contributions: vec![3_333_333_333, 3_333_333_333, 3_333_333_334],
                tranches: vec![4_000_000_000, 7_000_000_000],
                final_repayment: None,
                token_decimals: ALT_TOKEN_DECIMALS, // 6 decimals
            },
        })?;
        checked += 1;
    }

    // --- I4 boundary: single stroop tranche ---
    {
        run_partial_repay_scenario(&PartialRepayScenario {
            face_value: 10_000_000_000,
            contributions: vec![5_000_000_000, 5_000_000_000],
            tranches: vec![1], // 1 stroop — minimum possible tranche
            final_repayment: None,
            token_decimals: TOKEN_DECIMALS,
        })?;
        checked += 1;
    }

    // --- I4 boundary: tranche that would leave 0 balance (full repayment via partial) ---
    {
        let fv = 10_000_000_000i128;
        let yield_amount = fv * 15 / 100; // 15% yield
        run_partial_repay_scenario(&PartialRepayScenario {
            face_value: fv,
            contributions: vec![5_000_000_000, 5_000_000_000],
            tranches: vec![fv / 2, fv / 2 + yield_amount],
            final_repayment: None,
            token_decimals: TOKEN_DECIMALS,
        })?;
        checked += 1;
    }

    Ok(checked)
}

// ── Share-bps sum invariant ────────────────────────────────────────────────────

/// I-extra: The sum of all `share_bps` values in a fully-funded pool must
/// equal 10_000 (100%).  Partial pools are allowed to have a smaller sum.
pub fn check_share_bps_sum(contributions: &[i128]) -> Result<(), String> {
    if contributions.is_empty() {
        return Ok(());
    }
    let total: i128 = contributions.iter().sum();
    if total <= 0 {
        return Ok(());
    }
    let sum_bps: i128 = contributions
        .iter()
        .map(|&c| c * BPS_DENOM / total)
        .sum();

    // Due to truncation the sum may be slightly below 10_000; it must not exceed it.
    if sum_bps > BPS_DENOM {
        return Err(format!(
            "share_bps sum {} > 10_000 for contributions={:?}",
            sum_bps, contributions
        ));
    }
    Ok(())
}

/// Enumerate share_bps sum invariant across all contribution splits.
pub fn run_share_bps_sum_check(bounds: &Bounds) -> Result<usize, String> {
    let face_values = sample_face_values(bounds.face_value_samples);
    let mut checked = 0;

    for fv in &face_values {
        for k in 1..=bounds.max_investors {
            for contributions in split_total(*fv, k, bounds.contribution_splits) {
                check_share_bps_sum(&contributions)?;
                checked += 1;
            }
        }
    }

    Ok(checked)
}
