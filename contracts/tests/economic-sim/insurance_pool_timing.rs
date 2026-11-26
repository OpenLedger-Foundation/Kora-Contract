//! Insurance pool claim-timing manipulation attack simulation
//!
//! ## Attack Vector
//!
//! An attacker with knowledge of impending defaults attempts to:
//! 1. Time insurance claims to maximize payout before pool insolvency
//! 2. Manipulate pool reserves through coordinated claim timing
//! 3. Front-run legitimate claimants by observing pending defaults
//! 4. Drain pool reserves through strategic default timing if they control debtor
//! 5. Claim one ledger before the grace period ends (boundary block times)
//! 6. Split or replay claims to harvest rounding dust
//!
//! ## Expected Defenses
//!
//! - Fair claim ordering: claims are ranked by default timestamp, never by
//!   transaction submission order, and an oversubscribed batch is paid pro-rata
//! - Grace periods: a claim is eligible only once `now >= due_date + grace`,
//!   the same boundary `financing_pool::mark_default` enforces
//! - Solvency checks: total payouts never exceed the reserve
//!   (`treasury::disburse_from_reserve` rejects `InsufficientReserveBalance`)
//! - Coverage is capped below forfeited debtor collateral, so a self-dealing
//!   default never pays
//!
//! ## Invariants
//!
//! Every scenario runs through [`InsurancePool`], whose
//! [`InsurancePool::check_invariants`] panics with a descriptive message when:
//!
//! - **I1 solvency:** the reserve goes negative
//! - **I2 conservation:** `paid + reserve != initial_reserve`
//! - **I3 coverage cap:** a claimant is paid more than its capped entitlement
//! - **I4 grace boundary:** a claim submitted before `due_date + grace` is paid
//! - **I5 no replay:** a claim id is paid twice
//!
//! Order independence (**I6**) is checked by replaying the same claim set in
//! every submission order and requiring identical payouts. The
//! `invariant_checker_*` tests feed deliberately broken models to the checker
//! to prove that a regression fails the suite instead of passing silently.
//!
//! The boundary scenarios at the end of this file run against the real
//! `financing_pool` and `treasury` contracts.

#![cfg(test)]

use std::collections::BTreeMap;

use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, Bytes, Env, String, Symbol,
};

use crate::economic_sim::{estimate_gas_cost, AttackMetrics};

// ── Policy model ──────────────────────────────────────────────────────────────

/// Grace period used by the model and the contract boundary tests (1 day).
const GRACE_SECS: u64 = 86_400;
/// Share of an insured position the reserve reimburses on default.
const COVERAGE_BPS: i128 = 5_000;
/// Debtor collateral forfeited on default. Must be >= COVERAGE_BPS
/// (see `test_coverage_never_exceeds_forfeited_bond`).
const BOND_BPS: i128 = 6_000;
const BPS: i128 = 10_000;
const T0: u64 = 1_700_000_000;

#[derive(Clone, Debug)]
struct Claim {
    id: u32,
    claimant: u32,
    insured: i128,
    due_ts: u64,
    /// When the default was recorded on-chain (ranking key).
    default_ts: u64,
    /// When the claim transaction lands (must not affect ranking).
    submit_ts: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ordering {
    /// Rank by default timestamp, pro-rata when oversubscribed (protocol).
    FairProRata,
    /// Pay in submission order until empty (vulnerable reference model).
    SubmissionOrder,
}

#[derive(Debug, PartialEq, Eq)]
enum Rejection {
    GracePeriodActive,
    AlreadyClaimed,
    InvalidAmount,
}

struct InsurancePool {
    initial_reserve: i128,
    reserve: i128,
    ordering: Ordering,
    paid: BTreeMap<u32, i128>,
    entitlement: BTreeMap<u32, i128>,
    eligible_at: BTreeMap<u32, (u64, u64)>,
    /// Coverage multiplier override used only by the checker self-tests.
    coverage_bps: i128,
}

impl InsurancePool {
    fn new(reserve: i128, ordering: Ordering) -> Self {
        Self {
            initial_reserve: reserve,
            reserve,
            ordering,
            paid: BTreeMap::new(),
            entitlement: BTreeMap::new(),
            eligible_at: BTreeMap::new(),
            coverage_bps: COVERAGE_BPS,
        }
    }

    /// First timestamp at which a claim on `due_ts` may be paid. Mirrors
    /// `financing_pool::mark_default`: rejected while `now < due + grace`.
    /// Overflow means the claim is never eligible.
    fn eligible_from(due_ts: u64) -> Option<u64> {
        due_ts.checked_add(GRACE_SECS)
    }

    fn validate(&self, c: &Claim) -> Result<(), Rejection> {
        if c.insured <= 0 {
            return Err(Rejection::InvalidAmount);
        }
        if self.paid.contains_key(&c.id) {
            return Err(Rejection::AlreadyClaimed);
        }
        match Self::eligible_from(c.due_ts) {
            Some(t) if c.submit_ts >= t => Ok(()),
            _ => Err(Rejection::GracePeriodActive),
        }
    }

    /// Process one settlement batch. Returns rejected claims with reasons.
    fn settle_batch(&mut self, claims: &[Claim]) -> Vec<(u32, Rejection)> {
        let mut rejected = Vec::new();
        let mut accepted: Vec<Claim> = Vec::new();
        for c in claims {
            match self.validate(c) {
                Ok(()) if accepted.iter().all(|a| a.id != c.id) => accepted.push(c.clone()),
                Ok(()) => rejected.push((c.id, Rejection::AlreadyClaimed)),
                Err(e) => rejected.push((c.id, e)),
            }
        }
        if self.ordering == Ordering::FairProRata {
            accepted.sort_by_key(|c| (c.default_ts, c.id));
        }

        let owed: Vec<i128> = accepted
            .iter()
            .map(|c| c.insured * self.coverage_bps / BPS)
            .collect();
        let total_owed: i128 = owed.iter().sum();
        // Pro-rata shares are computed against the reserve as it stood before
        // this batch paid anyone, so being processed first gains nothing.
        let batch_reserve = self.reserve;

        for (c, due) in accepted.iter().zip(owed.iter()) {
            let pay = match self.ordering {
                Ordering::FairProRata if total_owed > batch_reserve => {
                    // Floor division: dust stays in the reserve, never paid out.
                    due * batch_reserve / total_owed
                }
                Ordering::FairProRata => *due,
                Ordering::SubmissionOrder => (*due).min(self.reserve),
            };
            self.reserve -= pay;
            self.paid.insert(c.id, pay);
            self.entitlement
                .insert(c.id, c.insured * COVERAGE_BPS / BPS);
            self.eligible_at.insert(c.id, (c.submit_ts, c.due_ts));
        }
        self.check_invariants();
        rejected
    }

    fn total_paid(&self) -> i128 {
        self.paid.values().sum()
    }

    fn paid_to(&self, claims: &[Claim], claimant: u32) -> i128 {
        claims
            .iter()
            .filter(|c| c.claimant == claimant)
            .filter_map(|c| self.paid.get(&c.id))
            .sum()
    }

    fn check_invariants(&self) {
        assert!(
            self.reserve >= 0,
            "I1 solvency broken: reserve went negative ({})",
            self.reserve
        );
        assert_eq!(
            self.total_paid() + self.reserve,
            self.initial_reserve,
            "I2 conservation broken: paid {} + reserve {} != initial {}",
            self.total_paid(),
            self.reserve,
            self.initial_reserve
        );
        for (id, paid) in &self.paid {
            let cap = self.entitlement[id];
            assert!(
                *paid <= cap,
                "I3 coverage cap broken: claim {} paid {} > entitlement {}",
                id,
                paid,
                cap
            );
            let (submit, due) = self.eligible_at[id];
            assert!(
                matches!(Self::eligible_from(due), Some(t) if submit >= t),
                "I4 grace boundary broken: claim {} paid at {} before due {} + grace {}",
                id,
                submit,
                due,
                GRACE_SECS
            );
        }
    }
}

/// Run `claims` through a fresh pool once per submission order and assert that
/// every claimant receives the same payout regardless of order (I6).
fn assert_order_independent(reserve: i128, ordering: Ordering, claims: &[Claim]) -> InsurancePool {
    let mut reference: Option<BTreeMap<u32, i128>> = None;
    let mut last = None;
    let n = claims.len();
    for rot in 0..n {
        for reverse in [false, true] {
            let mut order: Vec<Claim> = claims.iter().cycle().skip(rot).take(n).cloned().collect();
            if reverse {
                order.reverse();
            }
            let mut pool = InsurancePool::new(reserve, ordering);
            pool.settle_batch(&order);
            let payouts = pool.paid.clone();
            match &reference {
                None => reference = Some(payouts),
                Some(r) => assert_eq!(
                    r, &payouts,
                    "I6 order independence broken: payouts depend on submission order"
                ),
            }
            last = Some(pool);
        }
    }
    last.expect("at least one claim")
}

fn claim(id: u32, claimant: u32, insured: i128, default_ts: u64, submit_ts: u64) -> Claim {
    Claim {
        id,
        claimant,
        insured,
        due_ts: T0,
        default_ts,
        submit_ts,
    }
}

fn setup_insurance_env() -> (
    Env,
    Address, // admin
    Address, // attacker_investor
    Address, // attacker_debtor
    i128,    // pool_reserves
    i128,    // total_insured
) {
    let env = Env::default();
    env.mock_all_auths();

    env.ledger().set(LedgerInfo {
        timestamp: T0,
        protocol_version: 21,
        sequence_number: 10_000,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1000,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 600_000,
    });

    let admin = Address::generate(&env);
    let attacker_investor = Address::generate(&env);
    let attacker_debtor = Address::generate(&env);

    // Pool is under-reserved (typical risk condition)
    let pool_reserves = 2_000_000_000_000i128; // 2M reserves
    let total_insured = 10_000_000_000_000i128; // 10M total insured value

    (
        env,
        admin,
        attacker_investor,
        attacker_debtor,
        pool_reserves,
        total_insured,
    )
}

// ── Model scenarios ───────────────────────────────────────────────────────────

#[test]
fn test_front_running_insurance_claims() {
    let (_env, _admin, _attacker, _debtor, pool_reserves, _total_insured) = setup_insurance_env();
    let mut metrics = AttackMetrics::new();

    // Attacker (claimant 0) sees 6M of honest defaults coming and races its
    // own 100K claim to land first, hoping to be paid in full before the
    // oversubscribed reserve runs dry. All defaults were recorded at the same
    // time; only the submission order differs.
    let ready = T0 + GRACE_SECS;
    let attacker_position = 100_000_000_000i128;
    let mut claims = vec![claim(0, 0, attacker_position, ready, ready)];
    for i in 1..=6 {
        claims.push(claim(i, i, 1_000_000_000_000, ready, ready + 1));
    }

    let fair = assert_order_independent(pool_reserves, Ordering::FairProRata, &claims);
    let attacker_paid = fair.paid_to(&claims, 0);
    let total_owed: i128 = claims.iter().map(|c| c.insured * COVERAGE_BPS / BPS).sum();
    assert!(
        total_owed > pool_reserves,
        "scenario must be oversubscribed"
    );
    let fair_share = attacker_position * COVERAGE_BPS / BPS * pool_reserves / total_owed;

    metrics.capital_deployed = attacker_position;
    metrics.tx_count = 1;
    metrics.estimated_gas_cost = estimate_gas_cost(30) * 10; // 10x priority fee
    metrics.net_profit = (attacker_paid - fair_share) - metrics.estimated_gas_cost * 1_000;
    metrics.outcome = format!(
        "Front-run paid {} vs pro-rata share {}; ordering is by default time, so racing gains nothing",
        attacker_paid, fair_share
    );
    println!("Insurance Claim Front-Running: {}", metrics.outcome);

    assert_eq!(
        attacker_paid, fair_share,
        "front-runner must receive exactly its pro-rata share"
    );
    assert!(
        !metrics.is_exploitable(),
        "front-running exploit: {:?}",
        metrics
    );

    // Reference: the vulnerable model really does let the racer win, which is
    // why the protocol must not pay in submission order.
    let mut naive = InsurancePool::new(pool_reserves, Ordering::SubmissionOrder);
    naive.settle_batch(&claims);
    assert!(naive.paid_to(&claims, 0) > fair_share);
}

#[test]
fn test_coverage_never_exceeds_forfeited_bond() {
    assert!(
        COVERAGE_BPS <= BOND_BPS,
        "policy broken: coverage {}bps exceeds forfeited bond {}bps, self-dealing defaults pay",
        COVERAGE_BPS,
        BOND_BPS
    );
}

#[test]
fn test_strategic_default_timing() {
    let (_env, _admin, _attacker, _attacker_debtor, pool_reserves, _total_insured) =
        setup_insurance_env();
    let mut metrics = AttackMetrics::new();

    // Attacker controls both debtor and investor. It funds its own 500K
    // invoice, keeps the borrowed 500K, defaults when reserves are full and
    // claims on the investor side. Borrowed and invested amounts cancel, so
    // the trade is: insurance received vs bond forfeited.
    let invested = 500_000_000_000i128;
    let bond = invested * BOND_BPS / BPS;

    // Try every default time across a 30-day window: reserve level and claim
    // timing must never make it profitable.
    let mut best = i128::MIN;
    for day in 0..30u64 {
        let ts = T0 + GRACE_SECS + day * 86_400;
        let mut pool = InsurancePool::new(pool_reserves, Ordering::FairProRata);
        pool.settle_batch(&[claim(1, 0, invested, ts, ts)]);
        let net = pool.total_paid() - bond;
        best = best.max(net);
    }

    metrics.capital_deployed = invested;
    metrics.tx_count = 2;
    metrics.estimated_gas_cost = estimate_gas_cost(40) * 2;
    metrics.net_profit = best - metrics.estimated_gas_cost * 1_000;
    metrics.outcome = format!(
        "Best self-dealing default nets {} (coverage {}bps vs bond {}bps)",
        metrics.net_profit, COVERAGE_BPS, BOND_BPS
    );
    println!("Strategic Default Timing: {}", metrics.outcome);

    assert!(
        metrics.net_profit <= 0,
        "strategic default profitable at best timing: {:?}",
        metrics
    );
    assert!(
        !metrics.is_exploitable(),
        "strategic default exploit: {:?}",
        metrics
    );
}

#[test]
fn test_pool_insolvency_cascade() {
    let (_env, _admin, _attacker, _debtor, pool_reserves, _total_insured) = setup_insurance_env();
    let mut metrics = AttackMetrics::new();

    // 10 colluding defaulters plus 5 honest claimants hit the pool in one batch.
    let ts = T0 + GRACE_SECS;
    let position = 300_000_000_000i128;
    let mut claims = Vec::new();
    for i in 0..10 {
        claims.push(claim(i, 0, position, ts, ts));
    }
    for i in 10..15 {
        claims.push(claim(i, 1, position, ts, ts));
    }
    let pool = assert_order_independent(pool_reserves, Ordering::FairProRata, &claims);

    let attackers_paid = pool.paid_to(&claims, 0);
    let honest_paid = pool.paid_to(&claims, 1);
    let bonds_lost = position * BOND_BPS / BPS * 10;

    metrics.capital_deployed = position * 10;
    metrics.tx_count = 20;
    metrics.estimated_gas_cost = estimate_gas_cost(40) * 20;
    metrics.net_profit = attackers_paid - bonds_lost - metrics.estimated_gas_cost * 1_000;
    metrics.outcome = format!(
        "Cascade: reserve {} -> {}, attackers paid {}, honest paid {}, attackers net {}",
        pool_reserves, pool.reserve, attackers_paid, honest_paid, metrics.net_profit
    );
    println!("Pool Insolvency Cascade: {}", metrics.outcome);

    // Honest claimants keep a share proportional to their exposure (1/3).
    let honest_floor = (attackers_paid + honest_paid) / 3 - 1;
    assert!(
        honest_paid >= honest_floor,
        "honest claimants squeezed below pro-rata: {} < {}",
        honest_paid,
        honest_floor
    );
    assert!(metrics.net_profit <= 0, "cascade profitable: {:?}", metrics);
    assert!(!metrics.is_exploitable(), "cascade exploit: {:?}", metrics);
}

#[test]
fn test_grace_period_gaming() {
    let (_env, _admin, _attacker, _debtor, pool_reserves, _total_insured) = setup_insurance_env();

    // Strategy A: claim the instant the grace period ends.
    // Strategy B: wait until the end of the batch window to see if the debtor
    // repays. With ranking by default time both must pay the same.
    let position = 200_000_000_000i128;
    let open = T0 + GRACE_SECS;
    let others: Vec<Claim> = (1..5)
        .map(|i| claim(i, i, 900_000_000_000, open, open + 10))
        .collect();

    let mut early = others.clone();
    early.push(claim(0, 0, position, open, open));
    let mut late = others.clone();
    late.push(claim(0, 0, position, open, open + 17_000));

    let a =
        assert_order_independent(pool_reserves, Ordering::FairProRata, &early).paid_to(&early, 0);
    let b = assert_order_independent(pool_reserves, Ordering::FairProRata, &late).paid_to(&late, 0);
    let advantage = (a - b).abs();
    println!(
        "Grace Period Gaming: immediate {} vs wait {} (advantage {})",
        a, b, advantage
    );

    assert_eq!(
        advantage, 0,
        "claim timing inside the window must not change payout"
    );
}

#[test]
fn test_boundary_block_times() {
    // due + grace - 1 is rejected, due + grace is accepted (inclusive).
    let mut pool = InsurancePool::new(1_000_000, Ordering::FairProRata);
    let due = T0;
    let edge = due + GRACE_SECS;
    let rejected = pool.settle_batch(&[Claim {
        id: 1,
        claimant: 0,
        insured: 100_000,
        due_ts: due,
        default_ts: edge - 1,
        submit_ts: edge - 1,
    }]);
    assert_eq!(rejected, vec![(1, Rejection::GracePeriodActive)]);
    assert_eq!(pool.total_paid(), 0, "nothing paid one second early");

    let rejected = pool.settle_batch(&[Claim {
        id: 1,
        claimant: 0,
        insured: 100_000,
        due_ts: due,
        default_ts: edge,
        submit_ts: edge,
    }]);
    assert!(
        rejected.is_empty(),
        "claim at exact boundary must be accepted"
    );
    assert_eq!(pool.total_paid(), 100_000 * COVERAGE_BPS / BPS);

    // A due date so far out that due + grace overflows is never claimable.
    let rejected = pool.settle_batch(&[Claim {
        id: 2,
        claimant: 0,
        insured: 100_000,
        due_ts: u64::MAX - 10,
        default_ts: u64::MAX,
        submit_ts: u64::MAX,
    }]);
    assert_eq!(rejected, vec![(2, Rejection::GracePeriodActive)]);
}

#[test]
fn test_replay_and_zero_claims_rejected() {
    let ts = T0 + GRACE_SECS;
    let mut pool = InsurancePool::new(10_000_000, Ordering::FairProRata);
    // Same claim twice in one batch, then again in a later batch.
    let c = claim(7, 0, 1_000_000, ts, ts);
    let rejected = pool.settle_batch(&[c.clone(), c.clone()]);
    assert_eq!(rejected, vec![(7, Rejection::AlreadyClaimed)]);
    let rejected = pool.settle_batch(&[c]);
    assert_eq!(rejected, vec![(7, Rejection::AlreadyClaimed)]);
    assert_eq!(pool.total_paid(), 1_000_000 * COVERAGE_BPS / BPS);

    let rejected = pool.settle_batch(&[claim(8, 0, 0, ts, ts), claim(9, 0, -5, ts, ts)]);
    assert_eq!(
        rejected,
        vec![(8, Rejection::InvalidAmount), (9, Rejection::InvalidAmount)]
    );
}

#[test]
fn test_claim_splitting_harvests_no_dust() {
    // Oversubscribed pool with an awkward reserve so pro-rata leaves dust.
    let ts = T0 + GRACE_SECS;
    let reserve = 1_000_003i128;
    let honest = claim(100, 1, 3_000_000, ts, ts);

    let whole = vec![honest.clone(), claim(0, 0, 999_999, ts, ts)];
    let whole_paid =
        assert_order_independent(reserve, Ordering::FairProRata, &whole).paid_to(&whole, 0);

    let mut split = vec![honest];
    for i in 0..9 {
        split.push(claim(i, 0, 111_111, ts, ts));
    }
    let split_paid =
        assert_order_independent(reserve, Ordering::FairProRata, &split).paid_to(&split, 0);

    println!(
        "Claim splitting: whole {} vs split into 9 {}",
        whole_paid, split_paid
    );
    assert!(
        split_paid <= whole_paid,
        "splitting a claim must not harvest rounding dust ({} > {})",
        split_paid,
        whole_paid
    );
}

#[test]
fn test_staggered_batches_respect_conservation() {
    // Claims trickle in over several batches while the reserve drains.
    let mut pool = InsurancePool::new(1_500_000, Ordering::FairProRata);
    let ts = T0 + GRACE_SECS;
    let mut id = 0;
    for batch in 0..5u64 {
        let claims: Vec<Claim> = (0..4)
            .map(|_| {
                id += 1;
                claim(id, id % 3, 400_000, ts + batch, ts + batch * 600)
            })
            .collect();
        pool.settle_batch(&claims);
    }
    assert_eq!(pool.total_paid() + pool.reserve, 1_500_000);
    assert!(pool.reserve >= 0);
}

// ── Invariant checker self-tests ──────────────────────────────────────────────

#[test]
#[should_panic(expected = "I6 order independence broken")]
fn invariant_checker_catches_submission_order_payouts() {
    let ts = T0 + GRACE_SECS;
    let claims: Vec<Claim> = (0..4).map(|i| claim(i, i, 1_000_000, ts, ts)).collect();
    assert_order_independent(1_000_000, Ordering::SubmissionOrder, &claims);
}

#[test]
#[should_panic(expected = "I3 coverage cap broken")]
fn invariant_checker_catches_overpaid_claim() {
    let ts = T0 + GRACE_SECS;
    let mut pool = InsurancePool::new(10_000_000, Ordering::FairProRata);
    pool.coverage_bps = BPS; // pays 100% while entitlement is capped
    pool.settle_batch(&[claim(1, 0, 1_000_000, ts, ts)]);
}

#[test]
#[should_panic(expected = "I2 conservation broken")]
fn invariant_checker_catches_leaked_reserve() {
    let ts = T0 + GRACE_SECS;
    let mut pool = InsurancePool::new(10_000_000, Ordering::FairProRata);
    pool.settle_batch(&[claim(1, 0, 1_000_000, ts, ts)]);
    pool.reserve -= 1; // simulate value leaving without a recorded payout
    pool.check_invariants();
}

#[test]
#[should_panic(expected = "I4 grace boundary broken")]
fn invariant_checker_catches_early_payout() {
    let mut pool = InsurancePool::new(10_000_000, Ordering::FairProRata);
    pool.paid.insert(1, 10);
    pool.entitlement.insert(1, 10);
    pool.eligible_at.insert(1, (T0 + GRACE_SECS - 1, T0));
    pool.reserve -= 10;
    pool.check_invariants();
}

// ── Contract boundary scenarios ───────────────────────────────────────────────

struct PoolEnv {
    env: Env,
    admin: Address,
    token: Address,
    marketplace: Address,
    sme: Address,
    nft: kora_invoice_nft::InvoiceNftContractClient<'static>,
    pool: kora_financing_pool::FinancingPoolContractClient<'static>,
}

fn setup_pool(grace: u64) -> PoolEnv {
    let (env, admin, _a, _d, _r, _t) = setup_insurance_env();
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let marketplace = Address::generate(&env);
    let sme = Address::generate(&env);

    let ac = env.register_contract(None, kora_access_control::AccessControlContract);
    kora_access_control::AccessControlContractClient::new(&env, &ac).initialize(&admin);
    let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
    let nft = kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_id);
    nft.initialize(&admin, &ac);
    let pool_id = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
    let pool = kora_financing_pool::FinancingPoolContractClient::new(&env, &pool_id);
    pool.initialize(
        &admin,
        &nft_id,
        &Address::generate(&env),
        &Address::generate(&env),
        &ac,
        &200u32,
        &Address::generate(&env),
        &10_000u32,
        &grace,
        &Address::generate(&env),
    );
    nft.set_authorized_callers(&admin, &marketplace, &pool_id);
    PoolEnv {
        env,
        admin,
        token,
        marketplace,
        sme,
        nft,
        pool,
    }
}

/// Open and fully fund a pool; returns (invoice_id, due_date).
fn open_pool(p: &PoolEnv, investor: &Address) -> (u64, u64) {
    let due = p.env.ledger().timestamp() + 30 * 86_400;
    let id = p.nft.mint_invoice(
        &p.sme,
        &Bytes::from_slice(&p.env, &[3u8; 32]),
        &1_000_000i128,
        &Symbol::new(&p.env, "USDC"),
        &due,
        &String::from_str(&p.env, "ipfs://ins"),
        &50u32,
        &None,
    );
    p.nft.set_listed(&p.marketplace, &id);
    p.pool.release_funds(&p.marketplace, &id, &p.token);
    p.pool
        .record_position(&p.admin, &id, investor, &1_000_000i128, &1_000_000i128);
    (id, due)
}

#[test]
fn test_contract_mark_default_grace_boundary() {
    let p = setup_pool(GRACE_SECS);
    let investor = Address::generate(&p.env);
    let (id, due) = open_pool(&p, &investor);

    // One second before due + grace: GracePeriodActive.
    p.env
        .ledger()
        .with_mut(|l| l.timestamp = due + GRACE_SECS - 1);
    assert_eq!(
        p.pool.try_mark_default(&p.admin, &id, &p.token),
        Err(Ok(
            kora_financing_pool::FinancingPoolError::GracePeriodActive
        )),
        "default one second before the grace boundary must be rejected"
    );
    assert!(!p.pool.get_pool(&id).is_closed);

    // Exactly at due + grace: accepted.
    p.env.ledger().with_mut(|l| l.timestamp = due + GRACE_SECS);
    p.pool.mark_default(&p.admin, &id, &p.token);
    assert!(p.pool.get_pool(&id).is_closed);

    // Racing a second default in the same ledger cannot double-trigger claims.
    assert_eq!(
        p.pool.try_mark_default(&p.admin, &id, &p.token),
        Err(Ok(
            kora_financing_pool::FinancingPoolError::PoolAlreadyClosed
        ))
    );
}

#[test]
fn test_contract_zero_grace_defaults_at_due_date() {
    let p = setup_pool(0);
    let investor = Address::generate(&p.env);
    let (id, due) = open_pool(&p, &investor);

    p.env.ledger().with_mut(|l| l.timestamp = due - 1);
    assert_eq!(
        p.pool.try_mark_default(&p.admin, &id, &p.token),
        Err(Ok(
            kora_financing_pool::FinancingPoolError::GracePeriodActive
        ))
    );

    // Known boundary gap: financing_pool accepts `now >= due + grace`, but
    // invoice_nft::set_defaulted requires `now > due`. With grace == 0 a
    // default at exactly `due` passes the pool check and then reverts inside
    // the NFT call (InvoiceNftError::InvalidInvoiceStatus = #12, which a
    // client decodes as FinancingPoolError #12). The whole call rolls back,
    // so no claim can be triggered early; the pool stays open.
    p.env.ledger().with_mut(|l| l.timestamp = due);
    assert!(p.pool.try_mark_default(&p.admin, &id, &p.token).is_err());
    assert!(
        !p.pool.get_pool(&id).is_closed,
        "a reverted default at the boundary must leave the pool open"
    );

    p.env.ledger().with_mut(|l| l.timestamp = due + 1);
    p.pool.mark_default(&p.admin, &id, &p.token);
    assert!(p.pool.get_pool(&id).is_closed);
}

#[test]
fn test_contract_reserve_drain_race() {
    let (env, admin, _a, _d, _r, _t) = setup_insurance_env();
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let treasury_id = env.register_contract(None, kora_treasury::TreasuryContract);
    let treasury = kora_treasury::TreasuryContractClient::new(&env, &treasury_id);
    treasury.initialize(&admin, &100u32);
    treasury.whitelist_token(&admin, &token);
    treasury.set_reserve_allocation_bps(&admin, &10_000u32);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&treasury_id, &1_000_000i128);
    treasury.collect_fee(&token, &1_000_000i128);
    let initial = treasury.get_reserve_balance(&token);
    assert_eq!(initial, 1_000_000);

    let pool_caller = Address::generate(&env);
    let stranger = Address::generate(&env);
    treasury.set_reserve_caller(&admin, &pool_caller, &true);

    // Unauthorized front-runner is rejected before touching the reserve.
    let thief = Address::generate(&env);
    assert_eq!(
        treasury.try_disburse_from_reserve(&stranger, &token, &1i128, &thief),
        Err(Ok(
            kora_shared::errors::KoraError::ReserveCallerNotAuthorized
        ))
    );

    // Three claimants race for 400K each against a 1M reserve: the third must
    // be rejected, not partially paid from someone else's funds.
    let claimants: Vec<Address> = (0..3).map(|_| Address::generate(&env)).collect();
    let mut paid = 0i128;
    for (i, c) in claimants.iter().enumerate() {
        let r = treasury.try_disburse_from_reserve(&pool_caller, &token, &400_000i128, c);
        if i < 2 {
            assert!(r.is_ok(), "claim {} within reserve must succeed", i);
            paid += 400_000;
        } else {
            assert_eq!(
                r,
                Err(Ok(
                    kora_shared::errors::KoraError::InsufficientReserveBalance
                )),
                "claim exceeding remaining reserve must be rejected"
            );
        }
        let reserve = treasury.get_reserve_balance(&token);
        assert!(reserve >= 0, "I1 solvency broken on-chain");
        assert_eq!(paid + reserve, initial, "I2 conservation broken on-chain");
    }

    // The remaining 200K is still claimable exactly, and not a stroop more.
    treasury.disburse_from_reserve(&pool_caller, &token, &200_000i128, &claimants[2]);
    assert_eq!(treasury.get_reserve_balance(&token), 0);
    assert_eq!(
        treasury.try_disburse_from_reserve(&pool_caller, &token, &1i128, &claimants[2]),
        Err(Ok(
            kora_shared::errors::KoraError::InsufficientReserveBalance
        ))
    );

    // Revoking the caller mid-incident stops further draws immediately.
    treasury.set_reserve_caller(&admin, &pool_caller, &false);
    assert_eq!(
        treasury.try_disburse_from_reserve(&pool_caller, &token, &1i128, &claimants[0]),
        Err(Ok(
            kora_shared::errors::KoraError::ReserveCallerNotAuthorized
        ))
    );
}
