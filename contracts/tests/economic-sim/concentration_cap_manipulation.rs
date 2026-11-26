//! Concentration cap manipulation attack simulation
//!
//! ## Attack Vector
//!
//! An attacker attempts to bypass per-investor concentration caps by:
//! 1. Using multiple Sybil addresses to exceed intended concentration limits
//! 2. Gaming the fee-tier mechanism through artificial volume splitting
//! 3. Manipulating position-share mechanics to consolidate beyond caps
//! 4. Splitting one contribution into many `fund_invoice` calls, each under
//!    the cap on its own
//! 5. Funding before a cap is enabled or lowered, then topping up
//!
//! ## Expected Defenses
//!
//! - `marketplace` records a running gross contribution per
//!   `(invoice_id, investor)` (`get_gross_contribution`) and rejects any
//!   funding call that would take it above `MaxInvestorShareBps` of the
//!   asking price with `InvestorConcentrationExceeded`. The exact cap is
//!   allowed and the next unit is rejected
//! - The cap is measured in listing-token units after oracle conversion
//! - A rejected call leaves the listing and the running total untouched
//! - Unaccredited addresses cannot fund at all (`InvestorNotAccredited`)
//! - Gas costs make micro-splitting uneconomical
//!
//! ## Sybil note
//!
//! The cap is per address. N accredited addresses controlled by one entity can
//! hold up to `N * cap` of a listing; no on-chain check can link them. The
//! on-chain defense is that every funding address must be individually
//! accredited by the admin (`set_investor_accredited`); entity-level linking
//! is an off-chain KYC responsibility. `test_contract_sybil_requires_accreditation`
//! pins both halves of that statement.
//!
//! Post-funding consolidation through `financing_pool::transfer_position`
//! merges positions without re-checking `MaxPositionBps`; see
//! `test_position_share_consolidation` and the docs note.

#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, Bytes, Env, String, Symbol, Vec,
};

use kora_marketplace::MarketplaceContractClient;
use kora_shared::errors::KoraError;

use crate::economic_sim::{estimate_gas_cost, AttackMetrics};

fn setup_concentration_env() -> (
    Env,
    Address,      // admin
    Vec<Address>, // attacker_sybil_addresses (10)
    i128,         // listing_size
    u32,          // concentration_cap_bps (e.g., 1000 = 10%)
) {
    let env = Env::default();
    env.mock_all_auths();

    env.ledger().set(LedgerInfo {
        timestamp: 1_700_000_000,
        protocol_version: 21,
        sequence_number: 5_000,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1000,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 600_000,
    });

    let admin = Address::generate(&env);

    let mut sybil_addresses = Vec::new(&env);
    for _ in 0..10 {
        sybil_addresses.push_back(Address::generate(&env));
    }

    let listing_size = 5_000_000_000_000i128; // 5M listing
    let concentration_cap_bps = 1_000; // 10% cap per investor

    (
        env,
        admin,
        sybil_addresses,
        listing_size,
        concentration_cap_bps,
    )
}

// ── Model scenarios ───────────────────────────────────────────────────────────

#[test]
fn test_sybil_concentration_bypass() {
    let (_env, _admin, sybil_addresses, listing_size, cap_bps) = setup_concentration_env();
    let mut metrics = AttackMetrics::new();

    // Attacker wants 40% of a listing with a 10% per-address cap.
    let max_per_address = listing_size * (cap_bps as i128) / 10_000;
    let desired_control = listing_size * 40 / 100;
    let addresses_needed = ((desired_control + max_per_address - 1) / max_per_address) as u32;

    metrics.capital_deployed = desired_control;
    let fee_bps = 200;
    let fee_per_address = max_per_address * fee_bps / 10_000;
    let gas_per_address = estimate_gas_cost(50) * 1_000;
    let total_cost = (fee_per_address + gas_per_address) * addresses_needed as i128;
    metrics.net_profit = -total_cost; // splitting yields no extra return
    metrics.tx_count = addresses_needed;
    metrics.outcome = format!(
        "Sybil bypass needs {} separately accredited addresses (cap {}bps each)",
        addresses_needed, cap_bps
    );
    println!("Sybil Concentration Bypass: {}", metrics.outcome);

    assert!(
        addresses_needed > 1,
        "a single address must never be able to exceed the {}bps cap",
        cap_bps
    );
    assert!(
        addresses_needed <= sybil_addresses.len(),
        "scenario must be within the 10-address attacker budget"
    );
    assert!(
        max_per_address * addresses_needed as i128 >= desired_control,
        "per-address cap arithmetic inconsistent"
    );
    // Splitting is never cheaper than funding the same amount from one address.
    assert!(
        metrics.net_profit < 0,
        "sybil splitting must not be profitable"
    );
    assert!(!metrics.is_exploitable(), "sybil exploit: {:?}", metrics);
}

#[test]
fn test_position_share_consolidation() {
    let (_env, _admin, _sybil_addresses, listing_size, cap_bps) = setup_concentration_env();

    // Five capped addresses fund 10% each, then try to merge into one 50%
    // position. The marketplace leg is capped per address (see the contract
    // tests below); the merge itself happens in financing_pool.
    let num_addresses = 5i128;
    let per_address = listing_size * cap_bps as i128 / 10_000;
    let consolidated_bps = per_address * num_addresses * 10_000 / listing_size;

    println!(
        "Position Share Consolidation: {} addresses x {}bps -> {}bps after merge",
        num_addresses, cap_bps, consolidated_bps
    );

    // Every funding leg is individually within the cap.
    assert!(per_address * 10_000 <= cap_bps as i128 * listing_size);
    // The merged position exceeds the marketplace cap: this is the open item
    // documented in the module header. The financing_pool MaxPositionBps
    // check runs in record_position, not in transfer_position.
    assert!(
        consolidated_bps > cap_bps as i128,
        "model drift: consolidation scenario no longer exceeds the cap"
    );
}

#[test]
fn test_micro_splitting_fee_optimization() {
    let (_env, _admin, _sybil_addresses, _listing_size, _cap_bps) = setup_concentration_env();
    let mut metrics = AttackMetrics::new();

    let total_investment = 1_000_000_000_000i128; // 1M investment
    let micro_tx_size = 10_000_000_000i128; // 10K per transaction
    let num_micro_txs = (total_investment / micro_tx_size) as u32;

    metrics.capital_deployed = total_investment;
    metrics.tx_count = num_micro_txs;

    let gas_per_tx = estimate_gas_cost(50) * 1_000;
    let total_gas = gas_per_tx * (num_micro_txs as i128);
    let single_tx_gas = estimate_gas_cost(50) * 1_000;
    let gas_overhead = total_gas - single_tx_gas;
    let fee_advantage = 0i128; // flat fee, no size tiers

    metrics.net_profit = fee_advantage - gas_overhead;
    metrics.outcome = format!(
        "Micro-splitting {} txs costs {} extra gas for {} fee advantage",
        num_micro_txs, gas_overhead, fee_advantage
    );
    println!("Micro-Splitting Fee Optimization: {}", metrics.outcome);

    assert!(
        metrics.net_profit < 0,
        "Micro-splitting should not be profitable due to gas costs"
    );
    assert!(
        !metrics.is_exploitable(),
        "Micro-splitting attack should not be exploitable"
    );
}

#[test]
fn test_volume_tier_gaming_via_splitting() {
    let (_env, _admin, _sybil_addresses, _listing_size, _cap_bps) = setup_concentration_env();
    let mut metrics = AttackMetrics::new();

    let volume_to_accumulate = 10_000_000_000_000i128; // 10M volume
    let capital_available = 2_000_000_000_000i128; // only 2M capital
    let cycles = (volume_to_accumulate / capital_available) as u32;
    let fee_per_cycle = capital_available * 200 / 10_000;
    let total_fees_lost = fee_per_cycle * (cycles as i128);

    metrics.capital_deployed = capital_available;
    metrics.tx_count = cycles * 2;
    metrics.estimated_gas_cost = estimate_gas_cost(50) * (cycles as i128) * 2;

    let savings_bps = 100;
    let breakeven_investment = (total_fees_lost * 10_000) / (savings_bps as i128);
    metrics.net_profit = -total_fees_lost - (metrics.estimated_gas_cost * 1_000);
    println!(
        "Volume Tier Gaming: {} lost, breakeven at {}",
        -metrics.net_profit, breakeven_investment
    );

    assert!(
        breakeven_investment > volume_to_accumulate,
        "Volume gaming should require >1x investment to break even"
    );
    assert!(
        !metrics.is_exploitable(),
        "Volume tier gaming should not be exploitable"
    );
}

// ── Contract scenarios (real marketplace) ─────────────────────────────────────

const ASKING: i128 = 10_000_000;
const FACE: i128 = 11_000_000;
const CAP_BPS: u32 = 2_500;
const CAP_AMOUNT: i128 = ASKING * CAP_BPS as i128 / 10_000;

struct Mkt {
    env: Env,
    admin: Address,
    seller: Address,
    token: Address,
    mp: MarketplaceContractClient<'static>,
    nft: kora_invoice_nft::InvoiceNftContractClient<'static>,
}

fn set_time(env: &Env, ts: u64, seq: u32) {
    env.ledger().set(LedgerInfo {
        timestamp: ts,
        protocol_version: 21,
        sequence_number: seq,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1000,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 100_000,
    });
}

/// Real marketplace wired to real access_control, treasury, NFT, risk
/// registry, price oracle, parameter registry and financing_pool (same wiring
/// as `marketplace_edge_cases.rs`).
fn setup_marketplace() -> Mkt {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    set_time(&env, 1_700_000_000, 1);

    let admin = Address::generate(&env);
    let seller = Address::generate(&env);

    let ac = env.register_contract(None, kora_access_control::AccessControlContract);
    kora_access_control::AccessControlContractClient::new(&env, &ac).initialize(&admin);
    let treasury_id = env.register_contract(None, kora_treasury::TreasuryContract);
    let treasury = kora_treasury::TreasuryContractClient::new(&env, &treasury_id);
    treasury.initialize(&admin, &50u32);
    let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
    let nft = kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_id);
    nft.initialize(&admin, &ac);

    let registry_id = env.register_contract(None, kora_risk_registry::RiskRegistryContract);
    let registry = kora_risk_registry::RiskRegistryContractClient::new(&env, &registry_id);
    let staking_token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    registry.initialize(&admin, &nft_id, &staking_token, &1_000_000i128, &5_000u32);

    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let oracle_id = env.register_contract(None, kora_price_oracle::PriceOracleContract);
    kora_price_oracle::PriceOracleContractClient::new(&env, &oracle_id).initialize(&admin, &ac);
    let preg_id = env.register_contract(None, kora_parameter_registry::ParameterRegistryContract);
    kora_parameter_registry::ParameterRegistryContractClient::new(&env, &preg_id)
        .initialize(&admin, &None);

    let pool_id = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
    kora_financing_pool::FinancingPoolContractClient::new(&env, &pool_id).initialize(
        &admin,
        &nft_id,
        &registry_id,
        &treasury_id,
        &ac,
        &200u32,
        &oracle_id,
        &10_000u32,
        &0u64,
        &Address::generate(&env),
    );

    let mp_id = env.register_contract(None, kora_marketplace::MarketplaceContract);
    let mp = MarketplaceContractClient::new(&env, &mp_id);
    mp.initialize(
        &admin,
        &nft_id,
        &pool_id,
        &treasury_id,
        &ac,
        &oracle_id,
        &registry_id,
        &preg_id,
        &50u32,
        &0u32,
    );

    mp.propose_token_whitelist(&admin, &token);
    set_time(&env, 1_700_000_000 + 86_400 + 1, 2);
    mp.execute_token_whitelist(&admin, &token);
    treasury.whitelist_token(&admin, &token);
    treasury.set_marketplace(&admin, &mp_id);
    mp.set_token_currency(&admin, &token, &Symbol::new(&env, "USDC"));
    nft.set_authorized_callers(&admin, &mp_id, &pool_id);
    mp.set_min_contribution(&admin, &1);

    let verifier = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &staking_token)
        .mint(&verifier, &1_000_000i128);
    registry.add_verifier(&admin, &verifier, &1_000_000i128);
    registry.register_sme(&verifier, &seller, &50u32, &true);

    Mkt {
        env,
        admin,
        seller,
        token,
        mp,
        nft,
    }
}

impl Mkt {
    /// Mint and list a fresh invoice; returns its id.
    fn listing(&self, salt: u8) -> u64 {
        let id = self.nft.mint_invoice(
            &self.seller,
            &Bytes::from_slice(&self.env, &[salt; 32]),
            &FACE,
            &Symbol::new(&self.env, "USDC"),
            &(self.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&self.env, "ipfs://cap"),
            &50u32,
            &None,
        );
        self.mp.list_invoice(
            &self.seller,
            &id,
            &ASKING,
            &FACE,
            &self.token,
            &(self.env.ledger().timestamp() + 86_400),
            &None,
        );
        id
    }

    fn investor(&self) -> Address {
        let a = Address::generate(&self.env);
        self.mp.set_investor_accredited(&self.admin, &a, &true);
        soroban_sdk::token::StellarAssetClient::new(&self.env, &self.token)
            .mint(&a, &(ASKING * 10));
        a
    }

    fn fund(&self, who: &Address, id: u64, amount: i128) -> Result<(), KoraError> {
        match self.mp.try_fund_invoice(who, &id, &amount, &None) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(e),
            other => panic!("unexpected host failure funding {}: {:?}", amount, other),
        }
    }

    fn assert_rejected_cleanly(&self, who: &Address, id: u64, amount: i128, why: &str) {
        let gross = self.mp.get_gross_contribution(&id, who);
        let funded = self.mp.get_listing(&id).funded_amount;
        assert_eq!(
            self.fund(who, id, amount),
            Err(KoraError::InvestorConcentrationExceeded),
            "{}: funding {} on top of {} must hit the {}bps cap",
            why,
            amount,
            gross,
            self.mp.get_max_investor_share_bps()
        );
        assert_eq!(
            self.mp.get_gross_contribution(&id, who),
            gross,
            "{}: rejected call changed the investor's running total",
            why
        );
        assert_eq!(
            self.mp.get_listing(&id).funded_amount,
            funded,
            "{}: rejected call changed funded_amount",
            why
        );
    }
}

#[test]
fn test_contract_cap_exact_boundary() {
    let m = setup_marketplace();
    m.mp.set_max_investor_share_bps(&m.admin, &CAP_BPS);
    let id = m.listing(1);
    let inv = m.investor();

    m.fund(&inv, id, CAP_AMOUNT)
        .expect("exactly the cap must be allowed");
    assert_eq!(m.mp.get_gross_contribution(&id, &inv), CAP_AMOUNT);
    m.assert_rejected_cleanly(&inv, id, 1, "one unit over the cap");
}

/// Before the running total was recorded, every call was checked on its own,
/// so this sequence funded 100% of the listing from one address.
#[test]
fn test_contract_micro_splitting_cannot_bypass_cap() {
    let m = setup_marketplace();
    m.mp.set_max_investor_share_bps(&m.admin, &CAP_BPS);
    let id = m.listing(2);
    let inv = m.investor();

    let slice = CAP_AMOUNT / 25;
    for i in 0..25 {
        m.fund(&inv, id, slice)
            .unwrap_or_else(|e| panic!("slice {} within cap rejected: {:?}", i, e));
    }
    assert_eq!(m.mp.get_gross_contribution(&id, &inv), CAP_AMOUNT);
    m.assert_rejected_cleanly(&inv, id, slice, "26th micro-slice");
    m.assert_rejected_cleanly(&inv, id, 1, "dust after micro-slices");
}

#[test]
fn test_contract_cap_enabled_or_lowered_counts_prior_funding() {
    let m = setup_marketplace();
    let id = m.listing(3);
    let inv = m.investor();

    // Uncapped: fund 20% before any cap exists.
    m.fund(&inv, id, ASKING / 5).expect("uncapped funding");
    assert_eq!(m.mp.get_gross_contribution(&id, &inv), ASKING / 5);

    // Enabling a 25% cap leaves exactly 5% of headroom, not a fresh 25%.
    m.mp.set_max_investor_share_bps(&m.admin, &CAP_BPS);
    m.assert_rejected_cleanly(&inv, id, ASKING / 20 + 1, "headroom after enabling cap");
    m.fund(&inv, id, ASKING / 20).expect("remaining headroom");

    // Lowering the cap below the current holding blocks any top-up but does
    // not claw back the existing position.
    m.mp.set_max_investor_share_bps(&m.admin, &1_000u32);
    m.assert_rejected_cleanly(&inv, id, 1, "top-up after cap lowered");
    assert_eq!(m.mp.get_gross_contribution(&id, &inv), CAP_AMOUNT);

    // Disabling the cap (0) restores uncapped funding.
    m.mp.set_max_investor_share_bps(&m.admin, &0u32);
    m.fund(&inv, id, 1).expect("cap disabled");
}

#[test]
fn test_contract_sybil_requires_accreditation() {
    let m = setup_marketplace();
    m.mp.set_max_investor_share_bps(&m.admin, &CAP_BPS);
    let id = m.listing(4);

    // An unaccredited sybil cannot fund at all.
    let sybil = Address::generate(&m.env);
    soroban_sdk::token::StellarAssetClient::new(&m.env, &m.token).mint(&sybil, &ASKING);
    assert_eq!(
        m.fund(&sybil, id, 1),
        Err(KoraError::InvestorNotAccredited),
        "unaccredited sybil must be rejected before the cap is even consulted"
    );

    // Accredited sybils are each capped independently (see Sybil note).
    let sybils: std::vec::Vec<Address> = (0..3).map(|_| m.investor()).collect();
    for s in &sybils {
        m.fund(s, id, CAP_AMOUNT)
            .expect("each accredited address up to cap");
        m.assert_rejected_cleanly(s, id, 1, "accredited sybil over cap");
        assert!(m.mp.get_gross_contribution(&id, s) * 10_000 <= CAP_BPS as i128 * ASKING);
    }
    let total: i128 = sybils
        .iter()
        .map(|s| m.mp.get_gross_contribution(&id, s))
        .sum();
    assert_eq!(
        total,
        CAP_AMOUNT * 3,
        "sybil aggregate is n * cap, never more"
    );
}

#[test]
fn test_contract_cap_is_scoped_per_listing() {
    let m = setup_marketplace();
    m.mp.set_max_investor_share_bps(&m.admin, &CAP_BPS);
    let a = m.listing(5);
    let b = m.listing(6);
    let inv = m.investor();

    m.fund(&inv, a, CAP_AMOUNT).expect("cap on listing a");
    m.assert_rejected_cleanly(&inv, a, 1, "listing a full");
    m.fund(&inv, b, CAP_AMOUNT)
        .expect("listing b has its own cap budget");
    assert_eq!(m.mp.get_gross_contribution(&b, &inv), CAP_AMOUNT);
    m.assert_rejected_cleanly(&inv, b, 1, "listing b full");
}

#[test]
fn test_contract_oversized_single_call_rejected() {
    let m = setup_marketplace();
    m.mp.set_max_investor_share_bps(&m.admin, &CAP_BPS);
    let id = m.listing(7);
    let inv = m.investor();
    m.assert_rejected_cleanly(&inv, id, CAP_AMOUNT + 1, "single call over cap");
    assert_eq!(m.mp.get_gross_contribution(&id, &inv), 0);
    // Other investors are unaffected by the rejected attempt.
    let other = m.investor();
    m.fund(&other, id, CAP_AMOUNT)
        .expect("independent investor");
}
