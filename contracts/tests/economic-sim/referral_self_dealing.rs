//! Referral self-dealing attack simulation
//!
//! ## Attack Vector
//!
//! An attacker attempts to extract referral rewards by:
//! 1. Creating multiple SME identities (Sybil attack)
//! 2. Cross-referrring between controlled addresses
//! 3. Creating and immediately repaying invoices to trigger referral bonuses
//! 4. Extracting rewards while minimizing legitimate economic activity
//!
//! ## Expected Defenses
//!
//! - Referral rewards only trigger on FIRST invoice repayment per SME
//! - Fixed bonus amount limits total extractable value
//! - Allocation cap prevents unbounded drain
//! - Upfront fees on invoice creation + funding make cycling expensive

#![cfg(test)]

use kora_access_control::AccessControlContract;
use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient, InvoiceNftError};
use kora_referral_rewards::{ReferralRewards, ReferralRewardsClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    token::StellarAssetClient,
    Address, Bytes, Env, String, Symbol,
};

use crate::economic_sim::{AttackMetrics, estimate_gas_cost};

fn setup_referral_env() -> (
    Env,
    Address, // admin
    Address, // attacker_sme_1
    Address, // attacker_sme_2
    Address, // attacker_referrer
    i128,    // bonus_amount
    i128,    // allocation
) {
    let env = Env::default();
    env.mock_all_auths();
    
    env.ledger().set(LedgerInfo {
        timestamp: 1_700_000_000,
        protocol_version: 21,
        sequence_number: 1000,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1000,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 600_000,
    });

    let admin = Address::generate(&env);
    let attacker_sme_1 = Address::generate(&env);
    let attacker_sme_2 = Address::generate(&env);
    let attacker_referrer = Address::generate(&env);
    
    let bonus_amount = 100_000_000i128; // 100 USDC bonus
    let allocation = 10_000_000_000i128; // 10K USDC total allocation

    (env, admin, attacker_sme_1, attacker_sme_2, attacker_referrer, bonus_amount, allocation)
}

/// Deploys invoice_nft + referral_rewards + a funded token and returns the clients
/// needed to exercise real on-chain referral reward flows.
fn setup_referral_contracts() -> (
    Env,
    InvoiceNftContractClient<'static>,
    ReferralRewardsClient<'static>,
    Address, // admin
    Address, // marketplace
    Address, // financing pool
    Address, // sme
    Address, // referrer
    Address, // token
    i128,    // bonus
    i128,    // allocation
) {
    let env = Env::default();
    env.mock_all_auths();

    env.ledger().set(LedgerInfo {
        timestamp: 1_700_000_000,
        protocol_version: 21,
        sequence_number: 1,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1000,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 600_000,
    });

    let admin = Address::generate(&env);
    let ac = env.register_contract(None, AccessControlContract);
    let market = Address::generate(&env);
    let pool = Address::generate(&env);
    let sme = Address::generate(&env);
    let referrer = Address::generate(&env);

    let nft_addr = env.register_contract(None, InvoiceNftContract);
    let nft = InvoiceNftContractClient::new(&env, &nft_addr);
    nft.initialize(&admin, &ac);
    nft.set_authorized_callers(&admin, &market, &pool);

    let token_admin = Address::generate(&env);
    let token_addr = env.register_stellar_asset_contract_v2(token_admin).address();
    StellarAssetClient::new(&env, &token_addr).mint(&admin, &10_000_000_000);

    let bonus: i128 = 10;
    let allocation: i128 = 100;
    let rewards_addr = env.register_contract(None, ReferralRewards);
    let rewards = ReferralRewardsClient::new(&env, &rewards_addr);
    rewards.initialize(&admin, &nft_addr, &token_addr, &bonus, &allocation);
    rewards.deposit(&admin, &allocation);

    (
        env, nft, rewards, admin, market, pool, sme, referrer, token_addr, bonus, allocation,
    )
}

/// Mint an SME's first invoice with an optional referrer via `mint_invoice_with_referrer`.
fn mint_invoice(
    env: &Env,
    nft: &InvoiceNftContractClient,
    sme: &Address,
    referrer: Option<Address>,
) -> u64 {
    let hash = Bytes::from_slice(env, &[1; 32]);
    let cid = String::from_str(env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
    nft.mint_invoice_with_referrer(
        sme,
        &hash,
        &1_000_000,
        &Symbol::new(env, "USDC"),
        &(env.ledger().timestamp() + 86_400),
        &cid,
        &30,
        &None,
        &referrer,
    )
    .unwrap()
}

#[test]
fn test_sybil_referral_extraction() {
    let (env, _admin, _sme1, _sme2, _referrer, bonus, allocation) = setup_referral_env();
    
    let mut metrics = AttackMetrics::new();
    
    // Calculate maximum possible referrals from allocation
    let max_referrals = allocation / bonus;
    
    // Cost per fake SME referral cycle:
    // 1. Create invoice (gas + storage)
    // 2. List on marketplace (gas + fee)
    // 3. Fund invoice (gas + fee on funding)
    // 4. Repay invoice (gas + transfer)
    // 5. Claim referral reward (gas)
    
    let invoice_amount = 1_000_000_000i128; // 1K invoice (minimum viable)
    
    // Marketplace fee: 2%
    let marketplace_fee_bps = 200;
    let marketplace_fee = invoice_amount * marketplace_fee_bps / 10_000;
    
    // Financing pool fee on repayment: ~1%
    let pool_fee_bps = 100;
    let pool_fee = invoice_amount * pool_fee_bps / 10_000;
    
    // Gas costs for 5 transactions
    let gas_per_cycle = estimate_gas_cost(50) * 5;
    
    // Total cost per referral claimed
    let cost_per_referral = marketplace_fee + pool_fee + (gas_per_cycle * 1_000);
    
    // Net profit per successful referral
    let profit_per_referral = bonus - cost_per_referral;
    
    metrics.tx_count = (max_referrals * 5) as u32; // 5 tx per referral
    
    if profit_per_referral > 0 {
        // Attack could be profitable if fees are too low
        metrics.capital_deployed = invoice_amount * max_referrals; // capital tied up
        metrics.net_profit = profit_per_referral * max_referrals;
        metrics.outcome = format!(
            "ALERT: Referral self-dealing is PROFITABLE! Attacker can extract {} with {} fake SMEs. FIX REQUIRED: Increase fees or reduce bonus.",
            metrics.net_profit,
            max_referrals
        );
    } else {
        metrics.capital_deployed = invoice_amount;
        metrics.net_profit = profit_per_referral; // negative
        metrics.outcome = format!(
            "Referral self-dealing is UNPROFITABLE. Each cycle loses {}. Attack FAILED - fees exceed bonus.",
            -profit_per_referral
        );
    }
    
    println!("Sybil Referral Extraction Attack Results:");
    println!("  Bonus per referral: {}", bonus);
    println!("  Cost per referral: {}", cost_per_referral);
    println!("  Net per referral: {}", profit_per_referral);
    println!("  Max possible referrals: {}", max_referrals);
    println!("  Total net profit: {}", metrics.net_profit);
    println!("  Outcome: {}", metrics.outcome);
    
    // SECURITY CHECK: Attack must not be exploitable
    assert!(!metrics.is_exploitable(),
        "Referral self-dealing EXPLOIT DETECTED: {:?}", metrics);
    
    assert!(profit_per_referral <= 0,
        "CRITICAL: Referral bonus ({}) exceeds cost per cycle ({}). Reduce bonus or increase fees!",
        bonus, cost_per_referral);
}

#[test]
fn test_referral_rapid_cycling_timing() {
    let (_env, _admin, _sme1, _sme2, _referrer, bonus, _allocation) = setup_referral_env();
    
    let mut metrics = AttackMetrics::new();
    
    // Attacker tries to rapidly cycle through fake invoices to extract rewards quickly
    // Testing if rate limits or timing constraints prevent rapid extraction
    
    let cycles = 100;
    let invoice_amount = 500_000_000i128; // 500 USDC
    
    metrics.capital_deployed = invoice_amount; // recycled capital
    
    // Costs per cycle
    let marketplace_fee = invoice_amount * 200 / 10_000; // 2%
    let pool_fee = invoice_amount * 100 / 10_000; // 1%
    let gas_cost = estimate_gas_cost(50) * 5 * 1_000;
    
    let cost_per_cycle = marketplace_fee + pool_fee + gas_cost;
    let profit_per_cycle = bonus - cost_per_cycle;
    
    metrics.net_profit = profit_per_cycle * cycles;
    metrics.tx_count = cycles * 5;
    
    // Time constraint: each invoice must be created, funded, and repaid
    // Minimum realistic time: 1 ledger per invoice = ~5 seconds
    let min_ledgers_required = cycles * 1;
    metrics.disruption_ledgers = 0; // not disrupting, just extracting
    
    metrics.outcome = format!(
        "Rapid cycling through {} fake invoices over {} ledgers. Net: {}. Per-cycle profit: {}",
        cycles,
        min_ledgers_required,
        metrics.net_profit,
        profit_per_cycle
    );
    
    println!("Rapid Referral Cycling Attack Results:");
    println!("  Cycles: {}", cycles);
    println!("  Time required: {} ledgers (~{} minutes)", min_ledgers_required, min_ledgers_required * 5 / 60);
    println!("  Per-cycle profit: {}", profit_per_cycle);
    println!("  Total net: {}", metrics.net_profit);
    println!("  Outcome: {}", metrics.outcome);
    
    assert!(!metrics.is_exploitable(),
        "Rapid referral cycling should not be exploitable");
    
    assert!(profit_per_cycle <= 0,
        "Per-cycle profit must be non-positive to prevent extraction");
}

#[test]
fn test_cross_referral_network() {
    let (_env, _admin, _sme1, _sme2, _referrer, bonus, allocation) = setup_referral_env();
    
    let mut metrics = AttackMetrics::new();
    
    // Attacker creates a network of N fake SMEs that cross-refer each other
    // in a chain or cycle to maximize extraction
    //
    // Network: SME1 -> SME2 -> SME3 -> SME4 -> ... -> SME1
    //
    // Each SME creates one invoice and refers the next, attempting to claim
    // N referral bonuses with coordinated timing
    
    let network_size = 20;
    let invoice_amount = 1_000_000_000i128; // 1K per invoice
    
    // Capital needed: one invoice amount per SME (can be recycled after repayment)
    metrics.capital_deployed = invoice_amount * network_size;
    
    // Cost per SME in the chain
    let marketplace_fee = invoice_amount * 200 / 10_000;
    let pool_fee = invoice_amount * 100 / 10_000;
    let gas_cost = estimate_gas_cost(50) * 5 * 1_000;
    let cost_per_sme = marketplace_fee + pool_fee + gas_cost;
    
    // Revenue per SME: one bonus claimed (first invoice repaid)
    let revenue_per_sme = bonus;
    
    let total_cost = cost_per_sme * network_size;
    let total_revenue = revenue_per_sme * network_size;
    
    metrics.net_profit = total_revenue - total_cost;
    metrics.tx_count = (network_size * 5) as u32;
    
    // Check if entire allocation can be drained
    let allocation_drained = bonus * network_size;
    let allocation_percentage = (allocation_drained * 100) / allocation;
    
    metrics.outcome = format!(
        "Cross-referral network of {} SMEs. Net profit: {}. Drained {}% of allocation. {}",
        network_size,
        metrics.net_profit,
        allocation_percentage,
        if metrics.net_profit > 0 { "ALERT: PROFITABLE ATTACK" } else { "Attack FAILED - unprofitable" }
    );
    
    println!("Cross-Referral Network Attack Results:");
    println!("  Network size: {} SMEs", network_size);
    println!("  Total cost: {}", total_cost);
    println!("  Total revenue: {}", total_revenue);
    println!("  Net profit: {}", metrics.net_profit);
    println!("  Allocation drained: {}%", allocation_percentage);
    println!("  Outcome: {}", metrics.outcome);
    
    assert!(!metrics.is_exploitable(),
        "Cross-referral network EXPLOIT DETECTED: {:?}", metrics);
    
    assert!(metrics.net_profit <= 0,
        "CRITICAL: Cross-referral attack is profitable! Increase fees or add Sybil resistance.");
}

#[test]
fn test_allocation_exhaustion_griefing() {
    let (_env, _admin, _sme1, _sme2, _referrer, bonus, allocation) = setup_referral_env();
    
    let mut metrics = AttackMetrics::new();
    
    // Attacker doesn't profit but exhausts the referral allocation to grief
    // legitimate users. Tests if griefing is economically prohibitive.
    
    let max_claims = allocation / bonus;
    let invoice_amount = 1_000_000_000i128;
    
    // Minimum cost to exhaust allocation
    let marketplace_fee = invoice_amount * 200 / 10_000;
    let pool_fee = invoice_amount * 100 / 10_000;
    let gas_cost = estimate_gas_cost(50) * 5 * 1_000;
    let cost_per_claim = marketplace_fee + pool_fee + gas_cost;
    
    let total_cost_to_grief = cost_per_claim * max_claims;
    let total_extracted = allocation;
    let attacker_loss = total_cost_to_grief - total_extracted;
    
    metrics.capital_deployed = invoice_amount; // recycled
    metrics.net_profit = -attacker_loss;
    metrics.tx_count = (max_claims * 5) as u32;
    
    // Griefing is successful if attacker can exhaust allocation profitably
    // or with acceptable loss relative to disruption caused
    let disruption_value = allocation; // legitimate users lose this opportunity
    let griefing_ratio = disruption_value / attacker_loss;
    
    metrics.outcome = format!(
        "Exhausting {} allocation costs attacker {}. Griefing ratio: {:.2}x. {}",
        allocation,
        attacker_loss,
        griefing_ratio as f64 / 100.0,
        if griefing_ratio > 200 { "ALERT: Economically viable griefing" } else { "Griefing too expensive" }
    );
    
    println!("Allocation Exhaustion Griefing Attack Results:");
    println!("  Allocation size: {}", allocation);
    println!("  Claims to exhaust: {}", max_claims);
    println!("  Cost to attacker: {}", total_cost_to_grief);
    println!("  Value extracted: {}", total_extracted);
    println!("  Attacker loss: {}", attacker_loss);
    println!("  Griefing ratio: {:.2}x", griefing_ratio as f64 / 100.0);
    println!("  Outcome: {}", metrics.outcome);
    
    // Griefing should not be economically viable (attacker should lose significantly)
    assert!(attacker_loss > allocation / 10,
        "Griefing must cost attacker >10% of allocation to be prohibitive");
    
    assert!(griefing_ratio < 200,
        "Griefing ratio too high ({} > 2x), making attack economically viable for dedicated griefer",
        griefing_ratio);
}

#[test]
fn test_self_referral_never_pays() {
    let (env, nft, rewards, _admin, _market, _pool, sme, _referrer, _token, _bonus, _allocation) =
        setup_referral_contracts();

    let mut metrics = AttackMetrics::new();

    // SME attempts to refer itself -> mint must be rejected on chain
    let self_ref = nft.try_mint_invoice_with_referrer(
        &sme,
        &Bytes::from_slice(&env, &[2; 32]),
        &1_000_000,
        &Symbol::new(&env, "USDC"),
        &(env.ledger().timestamp() + 86_400),
        &String::from_str(&env, "cid"),
        &30,
        &None,
        &Some(sme.clone()),
    );
    assert_eq!(
        self_ref.unwrap_err().unwrap(),
        InvoiceNftError::InvalidReferrer,
        "referrer == sme address must be rejected"
    );

    // No first mint is recorded for a self-referral
    assert_eq!(nft.get_first_mint(&sme), None);

    // Claiming must return false and never pay
    assert!(!rewards.claim_referral_reward(&sme));
    assert_eq!(rewards.paid_total(), 0);

    metrics.capital_deployed = 0;
    metrics.net_profit = 0;
    metrics.tx_count = 1;
    metrics.outcome = "Self-referral rejected; no reward paid.".to_string();

    println!("Self-Referral Rejection Results:");
    println!("  referrer == sme rejected: true");
    println!("  first mint recorded: false");
    println!("  claim_referral_reward returned: false");
    println!("  paid_total: {}", rewards.paid_total());
    println!("  Outcome: {}", metrics.outcome);

    assert!(!metrics.is_exploitable());
    assert_eq!(rewards.paid_total(), 0);
}

#[test]
fn test_circular_referrers_ineligible() {
    let (env, nft, rewards, _admin, market, pool, sme1, sme2, _token, _bonus, _allocation) =
        setup_referral_contracts();

    let mut metrics = AttackMetrics::new();

    // SME1's first mint refers SME2
    let id1 = mint_invoice(&env, &nft, &sme1, Some(sme2.clone()));
    assert_eq!(nft.get_sme_referrer(&sme1), Some(sme2.clone()));

    // SME2's first mint attempts to refer SME1 -> cycle SME1 <-> SME2 must be rejected
    let cycle = nft.try_mint_invoice_with_referrer(
        &sme2,
        &Bytes::from_slice(&env, &[3; 32]),
        &1_000_000,
        &Symbol::new(&env, "USDC"),
        &(env.ledger().timestamp() + 86_400),
        &String::from_str(&env, "cid"),
        &30,
        &None,
        &Some(sme1.clone()),
    );
    assert_eq!(
        cycle.unwrap_err().unwrap(),
        InvoiceNftError::InvalidReferrer,
        "circular referrer graph must be rejected on chain"
    );
    assert_eq!(nft.get_first_mint(&sme2), None);

    // Legitimate half of the pair (SME2 referred SME1) can be repaid and pays exactly once
    nft.set_listed(&market, &id1);
    nft.set_funded(&pool, &id1);
    nft.set_repaid(&pool, &id1);
    assert!(rewards.claim_referral_reward(&sme1));
    assert!(!rewards.claim_referral_reward(&sme1));
    assert_eq!(rewards.paid_total(), 10);

    metrics.capital_deployed = 0;
    metrics.net_profit = 0;
    metrics.tx_count = 4;
    metrics.outcome = "Cycle detection enforced; cycle side ineligible, legit side pays once."
        .to_string();

    println!("Circular Referrer Graph Results:");
    println!("  SME1 refers SME2: ok");
    println!("  SME2 refers SME1 (cycle) rejected: true");
    println!("  SME2 first mint recorded: false");
    println!("  SME1 claim pays once: true");
    println!("  paid_total: {}", rewards.paid_total());
    println!("  Outcome: {}", metrics.outcome);

    assert!(!metrics.is_exploitable());
    assert_eq!(rewards.paid_total(), 10);
}

#[test]
fn test_exhausted_pool_claim_returns_false() {
    let (env, nft, rewards, admin, market, pool, _sme, _referrer, _token, bonus, allocation) =
        setup_referral_contracts();

    let mut metrics = AttackMetrics::new();

    let claims_needed = allocation / bonus; // number of claims to fully exhaust the pool
    let mut paid = 0i128;

    // Drain the pool by paying out every funded bonus
    for i in 0..claims_needed {
        let sme = Address::generate(&env);
        let referrer = Address::generate(&env);
        let id = mint_invoice(&env, &nft, &sme, Some(referrer.clone()));
        nft.set_listed(&market, &id);
        nft.set_funded(&pool, &id);
        nft.set_repaid(&pool, &id);
        assert!(
            rewards.claim_referral_reward(&sme),
            "claim {} should succeed before pool exhaustion",
            i
        );
        paid += bonus;
    }
    assert_eq!(rewards.paid_total(), paid);
    assert_eq!(paid, allocation);

    // Pool exhausted: the next claim must return false, not panic
    let sme = Address::generate(&env);
    let referrer = Address::generate(&env);
    let id = mint_invoice(&env, &nft, &sme, Some(referrer.clone()));
    nft.set_listed(&market, &id);
    nft.set_funded(&pool, &id);
    nft.set_repaid(&pool, &id);
    assert!(!rewards.claim_referral_reward(&sme), "exhausted pool claim must return false");
    assert_eq!(rewards.paid_total(), paid);

    metrics.capital_deployed = 0;
    metrics.net_profit = 0;
    metrics.tx_count = (claims_needed + 1) * 5;
    metrics.outcome = format!(
        "Pool exhausted after {} claims; next claim returns false (no panic).",
        claims_needed
    );

    println!("Exhausted Pool Results:");
    println!("  Claims needed to exhaust: {}", claims_needed);
    println!("  Total paid: {}", rewards.paid_total());
    println!("  Exhausted claim returned false (no panic): true");
    println!("  Outcome: {}", metrics.outcome);

    assert!(!metrics.is_exploitable());
    assert_eq!(rewards.paid_total(), allocation);
}

#[test]
fn test_first_mint_repaid_pays_exactly_once() {
    let (env, nft, rewards, _admin, market, pool, sme, referrer, _token, _bonus, _allocation) =
        setup_referral_contracts();

    let mut metrics = AttackMetrics::new();

    let id = mint_invoice(&env, &nft, &sme, Some(referrer.clone()));
    assert_eq!(nft.get_sme_referrer(&sme), Some(referrer.clone()));
    assert_eq!(nft.get_first_mint(&sme), Some(id));

    // Not yet repaid: claim must not pay
    assert!(!rewards.claim_referral_reward(&sme));
    assert_eq!(rewards.paid_total(), 0);

    // Full repayment -> claim pays exactly once
    nft.set_listed(&market, &id);
    nft.set_funded(&pool, &id);
    nft.set_repaid(&pool, &id);
    assert!(rewards.claim_referral_reward(&sme), "first repaid mint pays");
    assert!(!rewards.claim_referral_reward(&sme), "duplicate claim must not pay again");
    assert_eq!(rewards.paid_total(), 10);

    metrics.capital_deployed = 0;
    metrics.net_profit = 0;
    metrics.tx_count = 4;
    metrics.outcome = "Repaid first mint pays exactly once; duplicates rejected.".to_string();

    println!("First Mint Repays Exactly Once Results:");
    println!("  First mint referrer recorded: true");
    println!("  Pre-repayment claim returned: false");
    println!("  Post-repayment claim returned: true");
    println!("  Duplicate claim returned: false");
    println!("  paid_total: {}", rewards.paid_total());
    println!("  Outcome: {}", metrics.outcome);

    assert!(!metrics.is_exploitable());
    assert_eq!(rewards.paid_total(), 10);
}

#[test]
fn test_default_before_repaid_ineligible() {
    let (env, nft, rewards, admin, market, pool, sme, referrer, _token, _bonus, _allocation) =
        setup_referral_contracts();

    let mut metrics = AttackMetrics::new();

    let id = mint_invoice(&env, &nft, &sme, Some(referrer.clone()));
    nft.set_listed(&market, &id);
    nft.set_funded(&pool, &id);

    // Invoice is Funded but not Repaid: claim must not pay
    assert!(!rewards.claim_referral_reward(&sme));
    assert_eq!(rewards.paid_total(), 0);

    // Advance the clock past the due date and default the invoice.
    // A defaulted invoice is not Repaid and remains ineligible.
    env.ledger().set(LedgerInfo {
        timestamp: env.ledger().timestamp() + 200_000,
        protocol_version: 21,
        sequence_number: env.ledger().sequence_number(),
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1000,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 600_000,
    });
    nft.set_defaulted(&admin, &id);
    assert!(!rewards.claim_referral_reward(&sme));
    assert_eq!(rewards.paid_total(), 0);

    metrics.capital_deployed = 0;
    metrics.net_profit = 0;
    metrics.tx_count = 3;
    metrics.outcome = "Default-before-repaid invoice never pays reward.".to_string();

    println!("Default Before Repaid Results:");
    println!("  Pre-repayment claim returned: false");
    println!("  Invoice defaulted: true");
    println!("  Post-default claim returned: false");
    println!("  paid_total: {}", rewards.paid_total());
    println!("  Outcome: {}", metrics.outcome);

    assert!(!metrics.is_exploitable());
    assert_eq!(rewards.paid_total(), 0);
}

#[test]
fn test_multiple_referrers_same_sme_attempt() {
    let (env, nft, rewards, _admin, market, pool, sme, referrer_a, _token, _bonus, _allocation) =
        setup_referral_contracts();

    let mut metrics = AttackMetrics::new();

    let referrer_b = Address::generate(&env);

    // First mint attributes referrer_a
    let id = mint_invoice(&env, &nft, &sme, Some(referrer_a.clone()));
    assert_eq!(nft.get_sme_referrer(&sme), Some(referrer_a.clone()));

    // A subsequent mint with a different referrer is locked: first-mint attribution is immutable
    let locked = nft.try_mint_invoice_with_referrer(
        &sme,
        &Bytes::from_slice(&env, &[4; 32]),
        &1_000_000,
        &Symbol::new(&env, "USDC"),
        &(env.ledger().timestamp() + 86_400),
        &String::from_str(&env, "cid"),
        &30,
        &None,
        &Some(referrer_b.clone()),
    );
    assert_eq!(
        locked.unwrap_err().unwrap(),
        InvoiceNftError::ReferralAlreadyLocked,
        "multiple referrers for the same SME must be rejected after first mint"
    );

    // Claim pays referrer_a only, exactly once
    nft.set_listed(&market, &id);
    nft.set_funded(&pool, &id);
    nft.set_repaid(&pool, &id);
    assert!(rewards.claim_referral_reward(&sme));
    assert!(!rewards.claim_referral_reward(&sme));
    assert_eq!(rewards.paid_total(), 10);

    metrics.capital_deployed = 0;
    metrics.net_profit = 0;
    metrics.tx_count = 4;
    metrics.outcome = "Multiple referrer attempts rejected; first referrer paid once."
        .to_string();

    println!("Multiple Referrers Same SME Results:");
    println!("  First referrer recorded: {:?}", nft.get_sme_referrer(&sme));
    println!("  Second referrer attempt rejected: true");
    println!("  First referrer paid once: true");
    println!("  paid_total: {}", rewards.paid_total());
    println!("  Outcome: {}", metrics.outcome);

    assert!(!metrics.is_exploitable());
    assert_eq!(rewards.paid_total(), 10);
}
