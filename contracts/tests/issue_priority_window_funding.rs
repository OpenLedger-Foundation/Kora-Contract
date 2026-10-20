//! Priority-window funding fairness regressions for issue #976.

#![cfg(test)]

use kora_access_control::{AccessControlContract, AccessControlContractClient};
use kora_financing_pool::{FinancingPoolContract, FinancingPoolContractClient};
use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient};
use kora_marketplace::{MarketplaceContract, MarketplaceContractClient};
use kora_price_oracle::{PriceOracleContract, PriceOracleContractClient};
use kora_risk_registry::{RiskRegistryContract, RiskRegistryContractClient};
use kora_shared::errors::KoraError;
use kora_treasury::{TreasuryContract, TreasuryContractClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, Bytes, Env, String, Symbol, Vec,
};

const START_TIMESTAMP: u64 = 1_700_000_000;
const ASKING_PRICE: i128 = 100_000_000;
const INVESTOR_CAP: i128 = ASKING_PRICE / 4;
const TEST_BALANCE: i128 = 100_000_000;
const INVOICE_FACE_VALUE: i128 = 110_000_000;
const LEDGER_INTERVAL: u64 = 5;
const FUNDING_DEADLINE_INTERVAL: u64 = 60;

struct TestEnv {
    env: Env,
    admin: Address,
    seller: Address,
    token: Address,
    investors: Vec<Address>,
    marketplace: MarketplaceContractClient<'static>,
    nft: InvoiceNftContractClient<'static>,
}

fn ledger_info(timestamp: u64, sequence_number: u32) -> LedgerInfo {
    LedgerInfo {
        timestamp,
        protocol_version: 21,
        sequence_number,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1_000,
        min_persistent_entry_ttl: 1_000,
        max_entry_ttl: 100_000,
    }
}

fn setup(investor_count: u32) -> TestEnv {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(ledger_info(START_TIMESTAMP, 1));

    let admin = Address::generate(&env);
    let seller = Address::generate(&env);

    let access_id = env.register_contract(None, AccessControlContract);
    let access = AccessControlContractClient::new(&env, &access_id);
    access.initialize(&admin);

    let nft_id = env.register_contract(None, InvoiceNftContract);
    let nft = InvoiceNftContractClient::new(&env, &nft_id);
    nft.initialize(&admin, &access_id);

    let treasury_id = env.register_contract(None, TreasuryContract);
    let treasury = TreasuryContractClient::new(&env, &treasury_id);
    treasury.initialize(&admin, &0u32);

    let oracle_id = env.register_contract(None, PriceOracleContract);
    let oracle = PriceOracleContractClient::new(&env, &oracle_id);
    oracle.initialize(&admin, &access_id);

    let pool_id = env.register_contract(None, FinancingPoolContract);
    let pool = FinancingPoolContractClient::new(&env, &pool_id);
    let dispute_resolution = Address::generate(&env);

    let registry_id = env.register_contract(None, RiskRegistryContract);
    let registry = RiskRegistryContractClient::new(&env, &registry_id);
    let staking_admin = Address::generate(&env);
    let staking_token = env
        .register_stellar_asset_contract_v2(staking_admin)
        .address();
    registry.initialize(&admin, &nft_id, &staking_token, &1i128, &5_000u32);

    pool.initialize(
        &admin,
        &nft_id,
        &registry_id,
        &treasury_id,
        &access_id,
        &200u32,
        &oracle_id,
        &10_000u32,
        &0u64,
        &dispute_resolution,
    );

    let marketplace_id = env.register_contract(None, MarketplaceContract);
    let marketplace = MarketplaceContractClient::new(&env, &marketplace_id);
    marketplace.initialize(
        &admin,
        &nft_id,
        &pool_id,
        &treasury_id,
        &access_id,
        &oracle_id,
        &registry_id,
        &marketplace_id,
        &0u32,
        &0u32,
    );
    nft.set_authorized_callers(&admin, &marketplace_id, &pool_id);

    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    marketplace.propose_token_whitelist(&admin, &token);
    env.ledger().set(ledger_info(START_TIMESTAMP + 86_401, 2));
    marketplace.execute_token_whitelist(&admin, &token);
    marketplace.set_token_currency(&admin, &token, &Symbol::new(&env, "USDC"));
    marketplace.set_min_contribution(&admin, &1_000_000i128);
    marketplace.set_max_investor_share_bps(&admin, &2_500u32);

    let verifier = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1i128);
    registry.add_verifier(&admin, &verifier, &1i128);
    registry.register_sme(&verifier, &seller, &10u32, &true);

    let mut investors = Vec::new(&env);
    for _ in 0..investor_count {
        let investor = Address::generate(&env);
        marketplace.set_investor_accredited(&admin, &investor, &true);
        soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&investor, &TEST_BALANCE);
        investors.push_back(investor);
    }

    TestEnv {
        env,
        admin,
        seller,
        token,
        investors,
        marketplace,
        nft,
    }
}

fn create_listing(t: &TestEnv) -> (u64, u64) {
    let debtor_hash = Bytes::from_slice(&t.env, &[0xAB; 32]);
    let due_date = t.env.ledger().timestamp() + 86_400 * 30;
    let invoice_id = t.nft.mint_invoice(
        &t.seller,
        &debtor_hash,
        &INVOICE_FACE_VALUE,
        &Symbol::new(&t.env, "USDC"),
        &due_date,
        &String::from_str(
            &t.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        ),
        &30u32,
    );
    let funding_deadline = t.env.ledger().timestamp() + FUNDING_DEADLINE_INTERVAL;
    t.marketplace.list_invoice(
        &t.seller,
        &invoice_id,
        &ASKING_PRICE,
        &INVOICE_FACE_VALUE,
        &t.token,
        &funding_deadline,
        &None,
    );
    (invoice_id, funding_deadline)
}

fn advance_ledger(t: &TestEnv, timestamp: u64, sequence_number: u32) {
    t.env.ledger().set(ledger_info(timestamp, sequence_number));
}

fn funding_order(t: &TestEnv, invoice_id: u64) -> Vec<Address> {
    t.marketplace
        .get_listing_investors(&invoice_id, &0u32, &50u32)
}

fn token_balance(t: &TestEnv, investor: &Address) -> i128 {
    soroban_sdk::token::Client::new(&t.env, &t.token).balance(investor)
}

#[test]
fn priority_window_preserves_first_funded_order_and_limits_last_ledger_sniping() {
    let t = setup(3);
    let early_one = t.investors.get(0).unwrap();
    let early_two = t.investors.get(1).unwrap();
    let late_sniper = t.investors.get(2).unwrap();
    let (invoice_id, _) = create_listing(&t);

    let start = t.env.ledger().timestamp();
    let window_end = start + 3 * LEDGER_INTERVAL;
    let mut whitelist = Vec::new(&t.env);
    whitelist.push_back(early_one.clone());
    whitelist.push_back(early_two.clone());
    whitelist.push_back(late_sniper.clone());
    t.marketplace
        .set_priority_window(&t.admin, &invoice_id, &whitelist, &window_end);

    advance_ledger(&t, start + LEDGER_INTERVAL, 3);
    t.marketplace
        .fund_invoice(&early_one, &invoice_id, &INVESTOR_CAP, &None);
    advance_ledger(&t, start + 2 * LEDGER_INTERVAL, 4);
    t.marketplace
        .fund_invoice(&early_two, &invoice_id, &INVESTOR_CAP, &None);

    // The last-ledger investor tries to take all remaining allocation at once.
    advance_ledger(&t, window_end, 5);
    let result = t.marketplace.try_fund_invoice(
        &late_sniper,
        &invoice_id,
        &(ASKING_PRICE - 2 * INVESTOR_CAP),
        &None,
    );
    assert_eq!(
        result.unwrap_err().unwrap(),
        KoraError::InvestorConcentrationExceeded,
        "last-ledger snipe should be capped; current investor ordering: {:?}",
        funding_order(&t, invoice_id)
    );

    let expected_order = Vec::from_array(&t.env, [early_one.clone(), early_two.clone()]);
    let actual_order = funding_order(&t, invoice_id);
    assert_eq!(
        actual_order, expected_order,
        "funding order must match successful execution order; late sniper: {:?}",
        late_sniper
    );
    assert_eq!(
        t.marketplace.get_listing(&invoice_id).funded_amount,
        2 * INVESTOR_CAP
    );
    assert_eq!(
        t.marketplace.get_outstanding_contribution(&invoice_id),
        2 * INVESTOR_CAP
    );

    for investor in [&early_one, &early_two] {
        assert_eq!(
            token_balance(&t, investor),
            TEST_BALANCE - INVESTOR_CAP,
            "each equal-cap investor should receive the same allocation: {:?}",
            actual_order
        );
    }
    assert_eq!(
        token_balance(&t, &late_sniper),
        TEST_BALANCE,
        "a rejected snipe must not debit funds; order: {:?}",
        actual_order
    );
}

#[test]
fn priority_window_boundaries_and_listing_funding_deadline_are_distinct() {
    let t = setup(2);
    let whitelisted = t.investors.get(0).unwrap();
    let outsider = t.investors.get(1).unwrap();
    let (invoice_id, funding_deadline) = create_listing(&t);

    let start = t.env.ledger().timestamp();
    let window_end = start + 2 * LEDGER_INTERVAL;
    let mut whitelist = Vec::new(&t.env);
    whitelist.push_back(whitelisted.clone());
    t.marketplace
        .set_priority_window(&t.admin, &invoice_id, &whitelist, &window_end);

    // The start timestamp is included in the priority phase.
    t.marketplace
        .fund_invoice(&whitelisted, &invoice_id, &10_000_000i128, &None);

    // window_end is inclusive: a whitelisted contribution succeeds while an
    // equally eligible but non-whitelisted investor remains blocked.
    advance_ledger(&t, window_end, 3);
    t.marketplace
        .fund_invoice(&whitelisted, &invoice_id, &10_000_000i128, &None);
    let at_window_end =
        t.marketplace
            .try_fund_invoice(&outsider, &invoice_id, &10_000_000i128, &None);
    assert_eq!(
        at_window_end.unwrap_err().unwrap(),
        KoraError::Unauthorized,
        "outsider must remain blocked through window_end; funded order: {:?}",
        funding_order(&t, invoice_id)
    );

    // After the priority phase, access opens to any accredited investor, but
    // the listing itself still observes its separate funding deadline.
    advance_ledger(&t, window_end + 1, 4);
    t.marketplace
        .fund_invoice(&outsider, &invoice_id, &10_000_000i128, &None);

    advance_ledger(&t, funding_deadline + 1, 5);
    let after_funding_deadline =
        t.marketplace
            .try_fund_invoice(&whitelisted, &invoice_id, &10_000_000i128, &None);
    assert_eq!(
        after_funding_deadline.unwrap_err().unwrap(),
        KoraError::FundingDeadlinePassed,
        "funding after listing deadline must be rejected; funded order: {:?}",
        funding_order(&t, invoice_id)
    );

    let expected_order = Vec::from_array(&t.env, [whitelisted.clone(), outsider.clone()]);
    let actual_order = funding_order(&t, invoice_id);
    assert_eq!(
        actual_order, expected_order,
        "successful contributions should retain their execution order"
    );
    assert_eq!(
        t.marketplace.get_listing(&invoice_id).funded_amount,
        30_000_000i128
    );
    assert_eq!(token_balance(&t, &outsider), TEST_BALANCE - 10_000_000i128);
}

#[test]
fn expired_zero_end_window_does_not_restrict_accredited_investors() {
    let t = setup(1);
    let investor = t.investors.get(0).unwrap();
    let (invoice_id, _) = create_listing(&t);
    let empty_whitelist = Vec::new(&t.env);

    // A zero end timestamp is already expired at the current ledger time.
    t.marketplace
        .set_priority_window(&t.admin, &invoice_id, &empty_whitelist, &0u64);
    t.marketplace
        .fund_invoice(&investor, &invoice_id, &10_000_000i128, &None);

    assert_eq!(
        t.marketplace.get_listing(&invoice_id).funded_amount,
        10_000_000i128
    );
    assert_eq!(
        funding_order(&t, invoice_id),
        Vec::from_array(&t.env, [investor])
    );
}
