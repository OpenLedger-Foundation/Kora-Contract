// contracts/financing_pool/verification/model.rs
//!
//! Formal model of the `financing_pool` contract's core invariants.
//!
//! This module implements a **bounded exhaustive state-space model** of the
//! pool's state transitions.  It is not a mock or unit test — it enumerates
//! every reachable pool state within the configured bounds and asserts the
//! five core invariants on every state.
//!
//! ## Invariants
//!
//! - **I1 (Solvency):** `pool.balance >= sum(positions[i].contributed)` for
//!   all open pools; and `pool.balance >= 0` for all closed pools (the pool
//!   contract never goes negative).
//!
//! - **I2 (No double-payout):** For a closed pool, the total tokens disbursed
//!   to investors across the single `distribute_yield` call equals exactly
//!   `pool.repaid_amount`.  No investor receives yield a second time because
//!   the pool is marked `is_closed = true` before `distribute_yield` runs and
//!   the `RepaymentLock` prevents concurrent calls.
//!
//! - **I3 (Proportional yield-share):** Each investor's payout is
//!   `bps_of_normalized(repaid_amount, share_bps, TOKEN_DECIMALS)`.
//!   The sum of all payouts equals `repaid_amount` (possibly with dust
//!   credited to the first investor; see FINDING-V1 in README.md).
//!
//! - **I4 (Partial-repayment solvency):** After every partial repayment step
//!   the pool balance must remain >= `total_owed - repaid_amount` (the contract
//!   still holds enough to pay back what it owes).  Equivalently, each partial
//!   payout must be <= the remaining outstanding balance.
//!
//! - **I5 (No partial-repayment overpayment):** The cumulative `repaid_amount`
//!   across all partial repayment steps must never exceed `total_owed`.
//!   Equivalently, a partial repayment that would push `repaid_amount` above
//!   `total_owed` must be rejected, not silently capped.
//!
//! ## Simplifying assumptions
//!
//! See `README.md` §"Simplifying Assumptions" for the full list.  The most
//! important ones for reading this code:
//!
//! - Token decimals are fixed at 7 (USDC/EURC standard on Stellar).
//!   The multi-asset extension (I4_MULTI, I5_MULTI) adds a second decimal
//!   configuration to test the boundary where decimals differ across tokens.
//! - At most `MAX_INVESTORS` investors per pool.
//! - No cross-invoice netting, no installment schedules, no late penalties.
//! - `share_bps` values are computed from `contributed * 10_000 / total_pool`
//!   exactly as the contract does.

#![allow(dead_code)]

use alloc::{format, string::String, string::ToString, vec, vec::Vec};

/// Maximum number of investors per pool in the model.
/// Covers all realistic tiered allocations (max_position_bps >= 1250 bps → max 8).
pub const MAX_INVESTORS: usize = 8;

/// Maximum number of positions per pool in the main contract, mirrored here for consistency.
pub const MAX_POSITIONS_PER_POOL: usize = 100;

/// Fixed token decimals (USDC/EURC on Stellar).
pub const TOKEN_DECIMALS: u32 = 7;

/// Secondary token decimals used by the multi-asset model extension.
/// Represents a hypothetical 6-decimal stablecoin (e.g. USDC on EVM chains).
pub const ALT_TOKEN_DECIMALS: u32 = 6;

/// Minimum sensible pool face value in the model (1 USDC = 10^7 stroops).
pub const MIN_FACE_VALUE: i128 = 10_000_000;

/// Maximum pool face value in the model.  Set to 100 000 USDC to keep the
/// enumeration tractable while covering all realistic invoice amounts.
pub const MAX_FACE_VALUE: i128 = 1_000_000_000_000; // 100 000 USDC

/// Basis points denominator.
pub const BPS_DENOM: i128 = 10_000;

// ── Model types ───────────────────────────────────────────────────────────────

/// Minimal pool state — matches the fields relevant to I1–I5.
#[derive(Debug, Clone)]
pub struct ModelPool {
    pub face_value: i128,
    pub total_funded: i128,
    /// Cumulative repaid amount across all repay/repay_partial calls.
    pub repaid_amount: i128,
    /// Total amount owed by the SME (face_value + any late penalties).
    /// Invariant I5 checks repaid_amount <= total_owed at all times.
    pub total_owed: i128,
    pub is_closed: bool,
}

/// Minimal investor position.
#[derive(Debug, Clone)]
pub struct ModelPosition {
    pub contributed: i128,
    /// share_bps = contributed * 10_000 / total_funded (integer division, as in the contract)
    pub share_bps: u32,
    /// Total yield claimed so far (across partial repayment distributions).
    /// Used to verify I4: each partial payout is <= remaining outstanding.
    pub yield_claimed: i128,
}

/// Freeze overlay mirroring `invoice_nft::is_invoice_frozen` semantics.
/// When `Frozen`, fund (`record_position`) and repay/position transitions are
/// blocked while read-only views remain legal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeOverlay {
    Active,
    Frozen,
}

/// Complete model state for one pool.
#[derive(Debug, Clone)]
pub struct PoolState {
    pub pool: ModelPool,
    pub positions: Vec<ModelPosition>,
    /// Running balance of the pool's token account.
    /// Starts at 0, increases on funding and each repayment, decreases on distribution.
    pub balance: i128,
    /// Freeze overlay flag. `Frozen` blocks `record_position` and
    /// `repay_partial` transitions (mirrors the runtime
    /// `is_invoice_frozen` guard in `financing_pool`).
    pub freeze: FreezeOverlay,
    /// Token decimals for this pool (fixed to TOKEN_DECIMALS in the primary model;
    /// ALT_TOKEN_DECIMALS in the multi-asset extension).
    pub token_decimals: u32,
}

// ── Arithmetic helpers (mirror the contract's kora_shared::validation) ────────

/// `bps_of(amount, bps)` — same checked arithmetic as the contract.
/// Returns None on overflow.
pub fn bps_of(amount: i128, bps: u32) -> Option<i128> {
    amount.checked_mul(bps as i128)?.checked_div(BPS_DENOM)
}

/// `bps_of_normalized(amount, bps, decimals)` — mirrors the contract's
/// normalization path that rounds payout amounts to the token's precision.
///
/// The contract divides by `10^decimals` and multiplies by `10^decimals`
/// to eliminate sub-stroop dust from each individual payout.  This is the
/// source of FINDING-V1: the normalization can lose up to 1 stroop per
/// investor per distribution call.
pub fn bps_of_normalized(amount: i128, bps: u32, decimals: u32) -> Option<i128> {
    let scale = 10i128.pow(decimals);
    // Compute payout at full precision, then truncate to token precision.
    let full = amount.checked_mul(bps as i128)?;
    let truncated = full.checked_div(BPS_DENOM)?;
    // Round down to nearest whole token unit (stroop).
    let normalized = truncated.checked_div(scale)?.checked_mul(scale)?;
    Some(normalized)
}

/// Compute `share_bps` for an investor with `contributed` out of `total_funded`.
/// Returns None if total_funded is 0 or contributed > total_funded.
pub fn compute_share_bps(contributed: i128, total_funded: i128) -> Option<u32> {
    if total_funded <= 0 || contributed <= 0 || contributed > total_funded {
        return None;
    }
    let bps = contributed.checked_mul(BPS_DENOM)?.checked_div(total_funded)?;
    Some(bps as u32)
}

// ── State transitions ─────────────────────────────────────────────────────────

impl PoolState {
    /// Create a new pool with no investors and no funding, using a specific
    /// token decimal precision.  The `token_decimals` parameter lets the
    /// multi-asset extension instantiate pools with different precisions.
    pub fn new_with_decimals(face_value: i128, token_decimals: u32) -> Self {
        PoolState {
            pool: ModelPool {
                face_value,
                total_funded: 0,
                repaid_amount: 0,
                total_owed: face_value,
                is_closed: false,
            },
            positions: Vec::new(),
            balance: 0,
            token_decimals,
            freeze: FreezeOverlay::Active,
        }
    }

    /// Freeze the pool: blocks fund and repay transitions (read-only stays legal).
    /// Idempotent: freezing twice keeps the overlay `Frozen`.
    pub fn freeze(&mut self) {
        self.freeze = FreezeOverlay::Frozen;
    }

    /// Unfreeze the pool, restoring previously legal transitions. Idempotent.
    pub fn unfreeze(&mut self) {
        self.freeze = FreezeOverlay::Active;
    }

    /// Read-only view: legal regardless of the freeze overlay.
    pub fn total_owed_view(&self) -> i128 {
        self.pool.total_owed
    }

    /// Create a new pool with no investors and no funding (standard 7-decimal token).
    pub fn new(face_value: i128) -> Self {
        Self::new_with_decimals(face_value, TOKEN_DECIMALS)
    }

    /// Record an investor position.  Mirrors `record_position` in the contract.
    /// `contributed` must be > 0 and <= remaining unfunded amount.
    /// Returns Err if the position is invalid.
    /// Blocked while the freeze overlay is active (mirrors `is_invoice_frozen`).
    pub fn record_position(&mut self, contributed: i128) -> Result<(), &'static str> {
        if self.freeze == FreezeOverlay::Frozen {
            return Err("pool frozen: funding blocked");
        }
        if contributed <= 0 {
            return Err("contributed must be > 0");
        }
        let new_total = self.pool.total_funded + contributed;
        if new_total > self.pool.face_value {
            return Err("exceeds face value");
        }
        if self.positions.len() >= MAX_INVESTORS {
            return Err("max investors reached");
        }
        let share_bps = compute_share_bps(contributed, new_total)
            .ok_or("share_bps overflow")?;

        // Recompute all existing positions' share_bps with the new total,
        // mirroring the contract's behaviour where the last record_position
        // call fixes the final share_bps for each investor.
        for pos in &mut self.positions {
            pos.share_bps = compute_share_bps(pos.contributed, new_total)
                .ok_or("share_bps recompute overflow")?;
        }

        self.positions.push(ModelPosition {
            contributed,
            share_bps,
            yield_claimed: 0,
        });
        self.pool.total_funded = new_total;
        // Funding increases the pool balance.
        self.balance += contributed;
        Ok(())
    }

    /// Make a partial repayment of `amount`.
    ///
    /// Mirrors the `repay_partial` path in the contract.  Unlike `repay`,
    /// this does NOT close the pool — it applies a proportional distribution
    /// of the partial amount to investors immediately.
    ///
    /// ## I4 check
    /// After distribution, every investor's cumulative `yield_claimed` must
    /// be <= their pro-rata share of the amount owed so far.
    ///
    /// ## I5 check
    /// `new_repaid_amount <= total_owed` must hold; exceeding it is rejected.
    pub fn repay_partial(&mut self, amount: i128) -> Result<Vec<i128>, String> {
        if self.freeze == FreezeOverlay::Frozen {
            return Err("pool frozen: repayment blocked".to_string());
        }
        if amount <= 0 {
            return Err("partial repayment amount must be > 0".to_string());
        }
        if self.pool.is_closed {
            return Err("pool already closed".to_string());
        }

        // I5 pre-check: reject if this would exceed total_owed.
        let new_repaid = self.pool.repaid_amount
            .checked_add(amount)
            .ok_or_else(|| "arithmetic overflow in repaid_amount".to_string())?;
        if new_repaid > self.pool.total_owed {
            return Err(format!(
                "I5 VIOLATED (pre-check): repaid_amount {} + amount {} = {} > total_owed {}",
                self.pool.repaid_amount, amount, new_repaid, self.pool.total_owed
            ));
        }

        self.pool.repaid_amount = new_repaid;
        // Repayment increases the balance.
        self.balance += amount;

        // Distribute this tranche proportionally.
        let payouts = self.distribute_partial(amount)?;

        Ok(payouts)
    }

    /// Distribute a `tranche` of tokens proportionally among all investors.
    ///
    /// Used internally by `repay_partial`.  Returns each investor's payout
    /// for this tranche.  Updates `yield_claimed` on each position.
    ///
    /// Post-condition: sum of payouts == tranche (I4, via same dust-credit logic).
    fn distribute_partial(&mut self, tranche: i128) -> Result<Vec<i128>, String> {
        if tranche <= 0 {
            return Ok(vec![0; self.positions.len()]);
        }

        let mut payouts: Vec<i128> = Vec::with_capacity(self.positions.len());
        let mut distributed: i128 = 0;

        for pos in &self.positions {
            let payout = bps_of_normalized(tranche, pos.share_bps, self.token_decimals)
                .ok_or("payout overflow in distribute_partial")?;
            payouts.push(payout);
            distributed = distributed
                .checked_add(payout)
                .ok_or("distributed overflow")?;
        }

        // Credit dust to the first investor (same FINDING-V1 remediation).
        let dust = tranche - distributed;
        if dust > 0 && !payouts.is_empty() {
            payouts[0] += dust;
            distributed += dust;
        }

        // Update yield_claimed for each investor.
        for (pos, &payout) in self.positions.iter_mut().zip(payouts.iter()) {
            pos.yield_claimed = pos.yield_claimed
                .checked_add(payout)
                .ok_or("yield_claimed overflow")?;
        }

        self.balance -= distributed;

        Ok(payouts)
    }

    /// Repay the pool in full.  Mirrors `repay` (lump-sum path) in the contract.
    /// Sets is_closed = true then calls distribute_yield.
    pub fn repay(&mut self, amount: i128) -> Result<Vec<i128>, &'static str> {
        if amount <= 0 {
            return Err("amount must be > 0");
        }
        if self.pool.is_closed {
            return Err("pool already closed");
        }
        self.pool.repaid_amount = amount;
        self.pool.is_closed = true;
        // Repayment increases the balance (payer transfers tokens in).
        self.balance += amount;
        let payouts = self.distribute_yield()?;
        Ok(payouts)
    }

    /// Distribute yield to all investors.  Mirrors `distribute_yield` in the contract.
    ///
    /// Returns the vector of individual payouts (index matches `positions`).
    ///
    /// Post-condition: sum of payouts == repaid_amount (I3).  Any dust from
    /// integer division is credited to the first investor (FINDING-V1 fix).
    pub fn distribute_yield(&mut self) -> Result<Vec<i128>, &'static str> {
        let total = self.pool.repaid_amount;
        if total <= 0 {
            return Ok(vec![0; self.positions.len()]);
        }

        let mut payouts: Vec<i128> = Vec::with_capacity(self.positions.len());
        let mut distributed: i128 = 0;

        for pos in &self.positions {
            let payout = bps_of_normalized(total, pos.share_bps, self.token_decimals)
                .ok_or("payout overflow")?;
            payouts.push(payout);
            distributed = distributed.checked_add(payout).ok_or("distributed overflow")?;
        }

        // Credit dust to the first investor (remediation for FINDING-V1).
        let dust = total - distributed;
        if dust > 0 && !payouts.is_empty() {
            payouts[0] += dust;
            distributed += dust;
        }

        // Yield distribution decreases the balance.
        self.balance -= distributed;

        Ok(payouts)
    }
}

// ── Invariant checkers ────────────────────────────────────────────────────────

/// I1: Solvency — balance never goes below 0, and for open pools balance
/// is always >= sum of contributed amounts (the pool holds at least what
/// investors put in).
pub fn check_solvency(state: &PoolState) -> Result<(), String> {
    if state.balance < 0 {
        return Err(format!(
            "I1 VIOLATED (solvency): balance {} < 0\n  pool={:?}\n  positions={:?}",
            state.balance, state.pool, state.positions
        ));
    }
    if !state.pool.is_closed {
        let sum_contributed: i128 = state.positions.iter().map(|p| p.contributed).sum();
        if state.balance < sum_contributed {
            return Err(format!(
                "I1 VIOLATED (solvency): balance {} < sum_contributed {}\n  pool={:?}\n  positions={:?}",
                state.balance, sum_contributed, state.pool, state.positions
            ));
        }
    }
    Ok(())
}

/// I2: No double-payout — once is_closed=true, the pool balance must be
/// exactly 0 after distribute_yield completes (all tokens have left the
/// contract) and must not have gone negative at any point.
///
/// Because the model runs distribute_yield once as part of `repay`, this
/// checks the post-distribution balance.
pub fn check_no_double_payout(state: &PoolState, post_payouts: &[i128]) -> Result<(), String> {
    if !state.pool.is_closed {
        return Ok(()); // only relevant for closed pools
    }

    let sum_payouts: i128 = post_payouts.iter().sum();

    // The sum of all payouts must equal repaid_amount (I3 as a precondition
    // for I2 — if I3 holds, then distributing exactly once zeros the balance).
    if sum_payouts != state.pool.repaid_amount {
        return Err(format!(
            "I2/I3 VIOLATED: sum_payouts {} != repaid_amount {}\n  payouts={:?}\n  pool={:?}",
            sum_payouts, state.pool.repaid_amount, post_payouts, state.pool
        ));
    }

    if state.balance < 0 {
        return Err(format!(
            "I2 VIOLATED (no double-payout): balance {} < 0 after distribution\n  pool={:?}",
            state.balance, state.pool
        ));
    }

    Ok(())
}

/// I3: Proportional yield-share — each payout is `bps_of_normalized(repaid,
/// share_bps, token_decimals)` and the sum equals `repaid_amount`.
pub fn check_proportional_yield(
    state: &PoolState,
    payouts: &[i128],
) -> Result<(), String> {
    if !state.pool.is_closed || payouts.is_empty() {
        return Ok(());
    }

    let total = state.pool.repaid_amount;

    // Verify each individual payout matches the formula.
    // The first payout may include dust (FINDING-V1 remediation).
    let mut sum: i128 = 0;
    for (i, (pos, &payout)) in state.positions.iter().zip(payouts.iter()).enumerate() {
        let expected_base = bps_of_normalized(total, pos.share_bps, state.token_decimals)
            .ok_or("overflow in expected payout")?;

        // The first investor's payout may be larger by the dust amount.
        if i == 0 {
            if payout < expected_base {
                return Err(format!(
                    "I3 VIOLATED: investor[0] payout {} < base {} (share_bps={}, total={})",
                    payout, expected_base, pos.share_bps, total
                ));
            }
        } else if payout != expected_base {
            return Err(format!(
                "I3 VIOLATED: investor[{}] payout {} != expected {} (share_bps={}, total={})",
                i, payout, expected_base, pos.share_bps, total
            ));
        }
        sum += payout;
    }

    if sum != total {
        return Err(format!(
            "I3 VIOLATED: sum_payouts {} != repaid_amount {}\n  payouts={:?}",
            sum, total, payouts
        ));
    }

    Ok(())
}

/// I4: Partial-repayment solvency — after each partial repayment tranche,
/// the pool balance must be >= the remaining outstanding (total_owed - repaid_amount).
///
/// This prevents a scenario where partial distributions leave the pool unable
/// to cover future distributions — each tranche reduces the balance by exactly
/// the tranche amount, so the invariant reduces to:
///   `balance_after_tranche >= total_owed - repaid_amount_after_tranche`
///
/// For a pool whose balance is funded exactly at `total_funded`, after
/// receiving a partial repayment of `amount` and distributing it:
///   balance = total_funded + amount - distributed_amount
///   where distributed_amount == amount (I4 is equivalent to I1 for partial paths).
pub fn check_partial_repayment_solvency(
    state: &PoolState,
    tranche_payouts: &[i128],
) -> Result<(), String> {
    // The pool balance after distributing this tranche must be non-negative.
    if state.balance < 0 {
        return Err(format!(
            "I4 VIOLATED (partial solvency): balance {} < 0 after partial distribution\n\
             tranche_payouts={:?}\n  pool={:?}",
            state.balance, tranche_payouts, state.pool
        ));
    }

    // The distributed tranche must equal the repaid increment.
    let sum_tranche: i128 = tranche_payouts.iter().sum();
    if sum_tranche < 0 {
        return Err(format!(
            "I4 VIOLATED (partial solvency): sum_tranche_payouts {} < 0\n  pool={:?}",
            sum_tranche, state.pool
        ));
    }

    // The remaining balance is at least total_funded (investors' principal intact).
    let sum_contributed: i128 = state.positions.iter().map(|p| p.contributed).sum();
    if state.balance < sum_contributed {
        return Err(format!(
            "I4 VIOLATED (partial solvency): balance {} < sum_contributed {} after partial payout\n\
             tranche_payouts={:?}\n  pool={:?}",
            state.balance, sum_contributed, tranche_payouts, state.pool
        ));
    }

    Ok(())
}

/// I5: No partial-repayment overpayment — `repaid_amount` must never exceed
/// `total_owed`.  The contract enforces this with `PartialRepayInvalid` — this
/// check verifies the model correctly propagates that constraint.
///
/// Also verifies that each investor's cumulative `yield_claimed` does not
/// exceed their pro-rata share of `total_owed`:
///   `yield_claimed[i] <= bps_of_normalized(total_owed, share_bps[i], decimals)`
pub fn check_no_overpayment(state: &PoolState) -> Result<(), String> {
    // Pool-level: repaid must not exceed owed.
    if state.pool.repaid_amount > state.pool.total_owed {
        return Err(format!(
            "I5 VIOLATED (no overpayment): repaid_amount {} > total_owed {}\n  pool={:?}",
            state.pool.repaid_amount, state.pool.total_owed, state.pool
        ));
    }

    // Per-investor: cumulative yield_claimed must not exceed fair share of total_owed.
    for (i, pos) in state.positions.iter().enumerate() {
        let max_claimable = bps_of_normalized(
            state.pool.total_owed,
            pos.share_bps,
            state.token_decimals,
        )
        .ok_or_else(|| format!("I5: overflow computing max_claimable for investor {}", i))?;

        // Allow up to 1 unit of dust tolerance (first investor gets dust credit).
        let scale = 10i128.pow(state.token_decimals);
        if pos.yield_claimed > max_claimable + scale {
            return Err(format!(
                "I5 VIOLATED (no overpayment): investor[{}] yield_claimed {} > max_claimable {} \
                 (share_bps={}, total_owed={}, tolerance={})\n  pool={:?}",
                i,
                pos.yield_claimed,
                max_claimable,
                pos.share_bps,
                state.pool.total_owed,
                scale,
                state.pool
            ));
        }
    }

    Ok(())
}

// ── Model execution / scenario builder ───────────────────────────────────────

/// A concrete scenario to check: a pool with given contributions and a
/// lump-sum repayment.  Used by I1–I3.
#[derive(Debug, Clone)]
pub struct Scenario {
    pub face_value: i128,
    /// Investor contributions (must sum to <= face_value).
    pub contributions: Vec<i128>,
    /// Repayment amount (>= total_funded for a profitable pool; may be less for a partial default).
    pub repaid_amount: i128,
}

/// A scenario that exercises partial repayment across multiple tranches.
/// Used by I4 and I5.
#[derive(Debug, Clone)]
pub struct PartialRepayScenario {
    pub face_value: i128,
    pub contributions: Vec<i128>,
    /// Sequence of partial repayment amounts.  Each must be > 0 and the sum
    /// must not exceed `face_value` (the model's `total_owed`).
    pub tranches: Vec<i128>,
    /// Optional lump-sum final repayment (simulates a closing repay() call
    /// after several repay_partial() calls).
    pub final_repayment: Option<i128>,
    /// Token decimals for this scenario (allows testing alt-decimal tokens).
    pub token_decimals: u32,
}

/// Run a single lump-sum scenario against all three invariants (I1–I3).
/// Returns `Ok(payouts)` if all invariants hold, or `Err(description)` with
/// the violating state.
pub fn run_scenario(s: &Scenario) -> Result<Vec<i128>, String> {
    let mut state = PoolState::new(s.face_value);

    for &c in &s.contributions {
        state.record_position(c).map_err(|e| {
            format!("record_position failed: {} — scenario={:?}", e, s)
        })?;
    }

    // I1 must hold after every record_position.
    check_solvency(&state)?;

    let payouts = state.repay(s.repaid_amount).map_err(|e| {
        format!("repay failed: {} — scenario={:?}", e, s)
    })?;

    // I1 after repay + distribute.
    check_solvency(&state)?;
    // I2 and I3.
    check_no_double_payout(&state, &payouts)?;
    check_proportional_yield(&state, &payouts)?;

    Ok(payouts)
}

/// Run a partial-repayment scenario against I4 and I5.
///
/// Each tranche is distributed immediately (mirrors `repay_partial`).  After
/// all tranches, an optional final lump-sum repayment closes the pool.
///
/// Returns `Ok(all_payouts)` — a flat list of all tranche payouts concatenated —
/// or `Err(description)` with the violating state.
pub fn run_partial_repay_scenario(s: &PartialRepayScenario) -> Result<Vec<Vec<i128>>, String> {
    let mut state = PoolState::new_with_decimals(s.face_value, s.token_decimals);

    // Seed the pool with investor positions.
    for &c in &s.contributions {
        state.record_position(c).map_err(|e| {
            format!("record_position failed: {} — partial scenario={:?}", e, s)
        })?;
    }
    check_solvency(&state)?;

    let mut all_tranche_payouts: Vec<Vec<i128>> = Vec::new();

    // Apply each partial repayment tranche.
    for (idx, &tranche) in s.tranches.iter().enumerate() {
        let tranche_payouts = state.repay_partial(tranche).map_err(|e| {
            format!("repay_partial[{}] failed: {} — partial scenario={:?}", idx, e, s)
        })?;

        // I4: solvency after this tranche.
        check_partial_repayment_solvency(&state, &tranche_payouts)?;

        // I5: no overpayment after this tranche.
        check_no_overpayment(&state)?;

        // I1: general solvency invariant still holds.
        check_solvency(&state)?;

        all_tranche_payouts.push(tranche_payouts);
    }

    // Optional final lump-sum closing repayment.
    if let Some(final_amount) = s.final_repayment {
        let remaining_owed = state.pool.total_owed - state.pool.repaid_amount;
        if final_amount <= remaining_owed && final_amount > 0 {
            state.pool.repaid_amount += final_amount;
            state.pool.is_closed = true;
            state.balance += final_amount;
            let final_payouts = state.distribute_yield().map_err(|e| e.to_string())?;
            check_solvency(&state)?;
            check_no_double_payout(&state, &final_payouts)?;
            check_proportional_yield(&state, &final_payouts)?;
            all_tranche_payouts.push(final_payouts);
        }
    }

    // Final overpayment check after all tranches.
    check_no_overpayment(&state)?;

    Ok(all_tranche_payouts)
}

/// A multi-asset scenario: two pools, each using a different token decimal
/// precision, funded by overlapping investors.
///
/// Checks that I4 and I5 hold independently per pool and that there is no
/// cross-pool leakage (each pool's balance is self-contained).
#[derive(Debug, Clone)]
pub struct MultiAssetScenario {
    /// Pool A: standard 7-decimal token.
    pub pool_a: PartialRepayScenario,
    /// Pool B: alternate decimal precision (e.g. 6-decimal).
    pub pool_b: PartialRepayScenario,
}

/// Run a multi-asset scenario, checking I4 and I5 on both pools independently.
pub fn run_multi_asset_scenario(s: &MultiAssetScenario) -> Result<(), String> {
    // Each pool is fully independent; run them sequentially.
    run_partial_repay_scenario(&s.pool_a).map_err(|e| format!("pool_a: {}", e))?;
    run_partial_repay_scenario(&s.pool_b).map_err(|e| format!("pool_b: {}", e))?;
    Ok(())
}
