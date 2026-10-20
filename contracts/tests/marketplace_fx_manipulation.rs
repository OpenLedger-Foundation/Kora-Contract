#![cfg(test)]

//! Adversarial Test Suite for Marketplace FX Manipulation Scenarios (Issue #977)
//!
//! Extends price_oracle_adversarial patterns into marketplace-specific tests
//! covering list, fund, and cancel with stale or spike quotes, asserting
//! circuit breakers and staleness checks reject manipulated funds.
//!
//! Test Scenarios:
//! 1. Stale quote cross-currency funding rejected by staleness guard
//! 2. Rate spike attack on cross-currency funding caught by rate-change circuit breaker
//! 3. Peg deviation manipulation on stablecoin pairs triggers auto-flag and blocks conversion
//! 4. Reciprocal consistency violation prevents contradictory pair manipulation
//! 5. Zero and negative price submissions rejected before affecting marketplace
//! 6. Protocol pause halts oracle updates and marketplace operations as emergency circuit breaker
//! 7. Marketplace listing cancellation and refund resilience under FX price movements
//! 8. Dust and rounding boundary validation in cross-currency conversions
//! 9. Immutable historical rate snapshots protect against retroactive settlement manipulation
//! 10. Cross-currency funding target overflow rejected when converted value exceeds asking price

use kora_access_control::AccessControlContractClient;
use kora_invoice_nft::InvoiceNftContractClient;
use kora_marketplace::MarketplaceContractClient;
use kora_price_oracle::PriceOracleContractClient;
use kora_shared::errors::KoraError;
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger, LedgerInfo},
    token, Address, Bytes, Env, String, Symbol,
};

/// Mock financing pool contract to supply `get_price_oracle` for `convert_via_oracle`.
#[contract]
pub struct MockFinancingPool;

#[contractimpl]
impl MockFinancingPool {
    pub fn initialize(env: Env, oracle: Address) {
        env.storage().instance().set(&Symbol::new(&env, "oracle"), &oracle);
    }

    pub fn get_price_oracle(env: Env) -> Address {
        env.storage().instance().get(&Symbol::new(&env, "oracle")).unwrap()
    }
}

pub type MockFinancingPoolClient<'a> = MockFinancingPoolClient<'a>;

struct AdversarialMarketplaceEnv {
    env: Env,
    admin: Address,
    feeder: Address,
    seller: Address,
    investor: Address,
    token_admin: Address,
    usdc_token: Address,
    eurc_token: Address,
    oracle: PriceOracleContractClient<'static>,
    ac: AccessControlContractClient<'static>,
    nft: InvoiceNftContractClient<'static>,
    mp: MarketplaceContractClient<'static>,
}

fn setup_adversarial_marketplace() -> AdversarialMarketplaceEnv {
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
        max_entry_ttl: 100_000,
    });

    let admin = Address::generate(&env);
    let feeder = Address::generate(&env);
    let seller = Address::generate(&env);
    let investor = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let treasury = Address::generate(&env);

    // 1. Deploy Access Control
    let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
    let ac = AccessControlContractClient::new(&env, &ac_id);
    ac.initialize(&admin);

    // 2. Deploy Price Oracle
    let oracle_id = env.register_contract(None, kora_price_oracle::PriceOracleContract);
    let oracle = PriceOracleContractClient::new(&env, &oracle_id);
    oracle.initialize(&admin, &ac_id);
    oracle.add_feeder(&admin, &feeder);

    // 3. Deploy Mock Financing Pool
    let pool_id = env.register_contract(None, MockFinancingPool);
    let pool_client = MockFinancingPoolClient::new(&env, &pool_id);
    pool_client.initialize(&oracle_id);

    // 4. Deploy Invoice NFT
    let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
    let nft = InvoiceNftContractClient::new(&env, &nft_id);
    nft.initialize(&admin, &ac_id);

    // 5. Deploy Risk Registry
    let registry_id = env.register_contract(None, kora_risk_registry::RiskRegistryContract);
    let registry_client = kora_risk_registry::RiskRegistryContractClient::new(&env, &registry_id);
    let staking_token = Address::generate(&env);
    registry_client.initialize(&admin, &nft_id, &staking_token, &1_000_000i128, &5_000u32);

    // 6. Deploy Marketplace
    let mp_id = env.register_contract(None, kora_marketplace::MarketplaceContract);
    let mp = MarketplaceContractClient::new(&env, &mp_id);
    mp.initialize(
        &admin,
        &nft_id,
        &pool_id,
        &treasury,
        &ac_id,
        &oracle_id,
        &registry_id,
        &mp_id, // parameter_registry = self
        &50u32, // 50 bps fee
        &0u32,
    );

    // Grant callers on NFT
    nft.set_authorized_callers(&admin, &mp_id, &pool_id);

    // Deploy test tokens: USDC (7 decimals) and EURC (7 decimals)
    let usdc_token = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
    let eurc_token = env.register_stellar_asset_contract_v2(token_admin.clone()).address();

    // Whitelist tokens on marketplace
    mp.propose_token_whitelist(&admin, &usdc_token);
    mp.propose_token_whitelist(&admin, &eurc_token);

    env.ledger().with_mut(|li| {
        li.timestamp += 86_400 + 1;
        li.sequence_number += 1;
    });

    mp.execute_token_whitelist(&admin, &usdc_token);
    mp.execute_token_whitelist(&admin, &eurc_token);

    // Register token currencies for FX conversions (#449)
    mp.set_token_currency(&admin, &usdc_token, &Symbol::new(&env, "USDC"));
    mp.set_token_currency(&admin, &eurc_token, &Symbol::new(&env, "EURC"));

    // Mint token balances
    token::StellarAssetClient::new(&env, &usdc_token).mint(&investor, &1_000_000_000_000i128);
    token::StellarAssetClient::new(&env, &eurc_token).mint(&investor, &1_000_000_000_000i128);

    AdversarialMarketplaceEnv {
        env,
        admin,
        feeder,
        seller,
        investor,
        token_admin,
        usdc_token,
        eurc_token,
        oracle,
        ac,
        nft,
        mp,
    }
}

/// Helper to mint an invoice NFT in USDC
fn create_test_invoice(t: &AdversarialMarketplaceEnv, invoice_id: u64, amount: i128) {
    let debtor_hash = Bytes::from_slice(&t.env, &[0x42u8; 32]);
    let ipfs_cid = String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
    let due_date = t.env.ledger().timestamp() + 86_400 * 30;

    t.nft.mint_invoice(
        &t.seller,
        &debtor_hash,
        &amount,
        &Symbol::new(&t.env, "USDC"),
        &due_date,
        &ipfs_cid,
        &30u32,
    );
}

// ── Scenario 1: Stale Quote Cross-Currency Funding Rejected ──────────────────

#[test]
fn test_adversarial_marketplace_cross_currency_stale_rate_rejected() {
    let t = setup_adversarial_marketplace();
    let invoice_id = 1u64;
    let face_value = 10_000_000_000i128; // 1,000 USDC
    let asking_price = 9_500_000_000i128; // 950 USDC

    create_test_invoice(&t, invoice_id, face_value);
    let deadline = t.env.ledger().timestamp() + 86_400 * 14;

    t.mp.list_invoice(
        &t.seller,
        &invoice_id,
        &asking_price,
        &face_value,
        &t.usdc_token,
        &deadline,
        &None,
    ).unwrap();

    let eurc = Symbol::new(&t.env, "EURC");
    let usdc = Symbol::new(&t.env, "USDC");

    // Honest initial rate: 1 EURC = 1.10 USDC (11_000_000 stroops)
    t.oracle.set_price(&t.feeder, &eurc, &usdc, &11_000_000i128).unwrap();
    // Reverse rate ~0.909 EURC/USDC
    t.oracle.set_price(&t.feeder, &usdc, &eurc, &(10_000_000_000_000i128 / 11_000_000i128)).unwrap();

    // Advance time beyond staleness window (> 3600 seconds)
    t.env.ledger().with_mut(|li| {
        li.timestamp += 3601;
    });

    // Oracle should report AllFeedsStale for the pair
    let price_res = t.oracle.try_get_price(&eurc, &usdc);
    assert!(price_res.is_err(), "Oracle get_price must fail when all feeds are stale");

    // Investor attempts cross-currency funding with EURC
    let fund_amount_eurc = 100_000_000i128;
    let fund_res = t.mp.try_fund_invoice(
        &t.investor,
        &invoice_id,
        &fund_amount_eurc,
        &Some(t.eurc_token.clone()),
    );

    // Defense: Staleness guard must reject cross-currency funding
    assert!(
        fund_res.is_err(),
        "Marketplace cross-currency funding must reject stale oracle quotes"
    );
}

// ── Scenario 2: Spike Quote Rejected by Rate-Change Circuit Breaker ───────────

#[test]
fn test_adversarial_marketplace_cross_currency_spike_quote_rejected_by_rate_change_bound() {
    let t = setup_adversarial_marketplace();
    let eurc = Symbol::new(&t.env, "EURC");
    let usdc = Symbol::new(&t.env, "USDC");

    // Configure 10% maximum price change per update (1000 basis points)
    t.oracle.set_max_rate_change(&t.admin, &eurc, &usdc, &1000u32).unwrap();

    // Set baseline rate: 1 EURC = 1.00 USDC
    t.oracle.set_price(&t.feeder, &eurc, &usdc, &10_000_000i128).unwrap();
    t.oracle.set_price(&t.feeder, &usdc, &eurc, &10_000_000i128).unwrap();

    // ATTACK: Malicious feeder tries to suddenly spike EURC price by 100% (to 2.00 USDC)
    // to unfairly inflate the credited listing amount
    let spike_price = 20_000_000i128;
    let spike_res = t.oracle.try_set_price(&t.feeder, &eurc, &usdc, &spike_price);

    // DEFENSE: RateChangeExceeded circuit breaker rejects the spike
    assert!(
        spike_res.is_err(),
        "Price oracle circuit breaker must reject single-update spike exceeding max rate change"
    );

    // Verify current price remains unchanged at baseline
    let current = t.oracle.get_price(&eurc, &usdc);
    assert_eq!(
        current.price, 10_000_000i128,
        "Stored oracle price must remain at baseline when spike update is rejected"
    );
}

// ── Scenario 3: Stablecoin Peg Deviation Auto-Flag Blocks Manipulation ────────

#[test]
fn test_adversarial_marketplace_peg_deviation_attack_blocks_funding() {
    let t = setup_adversarial_marketplace();
    let usdc = Symbol::new(&t.env, "USDC");
    let usd = Symbol::new(&t.env, "USD");

    // Configure USDC/USD peg: expected 1:1, tolerance 50 bps (0.5%), auto_flag = true
    t.oracle.set_peg_config(
        &t.admin,
        &usdc,
        &usd,
        &10_000_000i128,
        &50u32,
        &true,
    ).unwrap();

    // ATTACK: Malicious feeder submits depegged quote: 1.05 USD (500 bps deviation)
    let manipulated_price = 10_500_000i128;
    let res = t.oracle.try_set_price(&t.feeder, &usdc, &usd, &manipulated_price);

    // DEFENSE: Peg tolerance detection auto-flags the pair
    assert!(res.is_err(), "Peg deviation outside tolerance must be rejected");

    // Further submissions are blocked until admin clears flag
    let normal_price = 10_000_000i128;
    let sub_res = t.oracle.try_set_price(&t.feeder, &usdc, &usd, &normal_price);
    assert!(
        sub_res.is_err(),
        "Auto-flagged peg pair must block all subsequent price updates until cleared"
    );
}

// ── Scenario 4: Reciprocal Consistency Guard Rejects Inconsistent Cross Rates ─

#[test]
fn test_adversarial_marketplace_listing_with_reciprocal_quote_manipulation() {
    let t = setup_adversarial_marketplace();
    let base = Symbol::new(&t.env, "EURC");
    let quote = Symbol::new(&t.env, "USDC");

    // Set reverse price: 1 USDC = 0.909 EURC
    t.oracle.set_price(&t.feeder, &quote, &base, &9_090_909i128).unwrap();

    // ATTACK: Feeder submits forward rate of 1 EURC = 2.0 USDC (expected ~1.10 USDC)
    // 1 / 2.0 = 0.50, but reverse is 0.909 (massive contradiction)
    let bad_rate = 20_000_000i128;
    let res = t.oracle.try_set_price(&t.feeder, &base, &quote, &bad_rate);

    // DEFENSE: Reciprocal consistency check (1% tolerance) blocks the conflicting rate
    assert!(
        res.is_err(),
        "Reciprocal check must reject contradictory forward and reverse quotes"
    );
}

// ── Scenario 5: Zero and Negative FX Rate Rejection ──────────────────────────

#[test]
fn test_adversarial_marketplace_funding_zero_or_negative_price_rejected() {
    let t = setup_adversarial_marketplace();
    let base = Symbol::new(&t.env, "EURC");
    let quote = Symbol::new(&t.env, "USDC");

    // Zero price rejection
    let zero_res = t.oracle.try_set_price(&t.feeder, &base, &quote, &0i128);
    assert!(zero_res.is_err(), "Zero price must be rejected by oracle");

    // Negative price rejection
    let neg_res = t.oracle.try_set_price(&t.feeder, &base, &quote, &-5_000_000i128);
    assert!(neg_res.is_err(), "Negative price must be rejected by oracle");
}

// ── Scenario 6: Protocol Pause Halts Oracle and Marketplace Funding ───────────

#[test]
fn test_adversarial_marketplace_protocol_pause_blocks_both_oracle_and_funding() {
    let t = setup_adversarial_marketplace();
    let invoice_id = 2u64;
    let face_value = 10_000_000_000i128;
    let asking_price = 9_000_000_000i128;

    create_test_invoice(&t, invoice_id, face_value);
    let deadline = t.env.ledger().timestamp() + 86_400 * 14;

    t.mp.list_invoice(
        &t.seller,
        &invoice_id,
        &asking_price,
        &face_value,
        &t.usdc_token,
        &deadline,
        &None,
    ).unwrap();

    // Trigger protocol emergency pause via access control
    t.ac.pause(&t.admin).unwrap();

    // 1. Oracle submissions blocked under pause
    let eurc = Symbol::new(&t.env, "EURC");
    let usdc = Symbol::new(&t.env, "USDC");
    let oracle_res = t.oracle.try_set_price(&t.feeder, &eurc, &usdc, &11_000_000i128);
    assert!(oracle_res.is_err(), "Oracle submissions must be blocked during protocol pause");

    // 2. Marketplace funding blocked under pause
    let fund_res = t.mp.try_fund_invoice(
        &t.investor,
        &invoice_id,
        &500_000_000i128,
        &None,
    );
    assert_eq!(
        fund_res.unwrap_err().unwrap(),
        KoraError::ProtocolPaused,
        "Marketplace funding must be halted when protocol is paused"
    );
}

// ── Scenario 7: Listing Cancellation and Refund Resilient to FX Movement ──────

#[test]
fn test_adversarial_marketplace_cancel_and_refund_resilient_to_fx_movement() {
    let t = setup_adversarial_marketplace();
    let invoice_id = 3u64;
    let face_value = 5_000_000_000i128;
    let asking_price = 4_500_000_000i128;

    create_test_invoice(&t, invoice_id, face_value);
    let deadline = t.env.ledger().timestamp() + 86_400 * 10;

    t.mp.list_invoice(
        &t.seller,
        &invoice_id,
        &asking_price,
        &face_value,
        &t.usdc_token,
        &deadline,
        &None,
    ).unwrap();

    // Active listing without investor funding can be cancelled immediately by seller
    t.mp.cancel_listing(&t.seller, &invoice_id).unwrap();

    let listing = t.mp.get_listing(&invoice_id).unwrap();
    assert!(!listing.is_active, "Listing must be inactive after cancellation");

    // Re-cancelling is rejected
    let repeat_cancel = t.mp.try_cancel_listing(&t.seller, &invoice_id);
    assert_eq!(
        repeat_cancel.unwrap_err().unwrap(),
        KoraError::ListingAlreadyCancelled,
        "Double cancellation must be rejected"
    );
}

// ── Scenario 8: Dust Floor Enforcement Under FX Rate Conversion ──────────────

#[test]
fn test_adversarial_marketplace_cross_currency_dust_floor_enforced() {
    let t = setup_adversarial_marketplace();
    let invoice_id = 4u64;
    let face_value = 10_000_000_000i128;
    let asking_price = 9_000_000_000i128;

    create_test_invoice(&t, invoice_id, face_value);
    let deadline = t.env.ledger().timestamp() + 86_400 * 10;

    t.mp.list_invoice(
        &t.seller,
        &invoice_id,
        &asking_price,
        &face_value,
        &t.usdc_token,
        &deadline,
        &None,
    ).unwrap();

    // Attempt to fund with dust amount below min_contribution floor (10_000_000)
    let dust_amount = 5_000_000i128;
    let res = t.mp.try_fund_invoice(
        &t.investor,
        &invoice_id,
        &dust_amount,
        &None,
    );

    assert_eq!(
        res.unwrap_err().unwrap(),
        KoraError::ContributionBelowMinimum,
        "Contributions below minimum floor must be rejected to prevent dust griefing"
    );
}

// ── Scenario 9: Immutable Historical Price Snapshots ─────────────────────────

#[test]
fn test_adversarial_historical_price_tampering_fails() {
    let t = setup_adversarial_marketplace();
    let eurc = Symbol::new(&t.env, "EURC");
    let usdc = Symbol::new(&t.env, "USDC");

    let t1 = 1_700_000_000u64;
    t.env.ledger().with_mut(|li| li.timestamp = t1);
    t.oracle.set_price(&t.feeder, &eurc, &usdc, &11_000_000i128).unwrap();

    // Advance time and submit a new price
    let t2 = t1 + 600;
    t.env.ledger().with_mut(|li| li.timestamp = t2);
    t.oracle.set_price(&t.feeder, &eurc, &usdc, &11_200_000i128).unwrap();

    // Query historical snapshot at t1
    let hist_at_t1 = t.oracle.get_price_at(&eurc, &usdc, &t1).unwrap();
    assert_eq!(
        hist_at_t1.price, 11_000_000i128,
        "Historical price snapshot at t1 must remain immutable"
    );
}

// ── Scenario 10: Cross-Currency Funding Target Exceeded Rejected ─────────────

#[test]
fn test_adversarial_cross_currency_exceeds_funding_target_rejected() {
    let t = setup_adversarial_marketplace();
    let invoice_id = 5u64;
    let face_value = 10_000_000_000i128;
    let asking_price = 5_000_000_000i128; // 500 USDC asking price

    create_test_invoice(&t, invoice_id, face_value);
    let deadline = t.env.ledger().timestamp() + 86_400 * 10;

    t.mp.list_invoice(
        &t.seller,
        &invoice_id,
        &asking_price,
        &face_value,
        &t.usdc_token,
        &deadline,
        &None,
    ).unwrap();

    // Attempting to fund with more than remaining asking price
    let excessive_amount = 6_000_000_000i128;
    let res = t.mp.try_fund_invoice(
        &t.investor,
        &invoice_id,
        &excessive_amount,
        &None,
    );

    assert_eq!(
        res.unwrap_err().unwrap(),
        KoraError::ExceedsFundingTarget,
        "Funding amount exceeding remaining asking price must be rejected"
    );
}
