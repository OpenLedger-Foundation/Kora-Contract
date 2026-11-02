//! Integration coverage for secondary-market protocol fee settlement.

use kora_access_control::{AccessControlContract, AccessControlContractClient};
use kora_financing_pool::FinancingPoolContractClient;
use kora_invoice_nft::InvoiceNftContractClient;
use kora_secondary_market::SecondaryMarketClient;
use soroban_sdk::{Bytes, String as SorobanString, Symbol};
use kora_treasury::TreasuryContractClient;
use soroban_sdk::{
    testutils::Address as _, token, Address, Env,
};

const INVOICE_ID: u64 = 47_100;
const POOL_AMOUNT: i128 = 1_000_000_000;
const CONTRIBUTION: i128 = 1_000_000_000;
const TRADE_PRICE: i128 = 12_345_679;

struct TestEnv {
    admin: Address,
    seller: Address,
    buyer: Address,
    observer: Address,
    treasury_address: Address,
    secondary_market_address: Address,
    token_address: Address,
    token: token::Client<'static>,
    pool: FinancingPoolContractClient<'static>,
    market: SecondaryMarketClient<'static>,
}

fn setup(protocol_fee_bps: u32) -> TestEnv {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);
    let observer = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token = token::Client::new(&env, &token_address);
    let token_admin_client = token::StellarAssetClient::new(&env, &token_address);
    token_admin_client.mint(&buyer, &TRADE_PRICE);

    let access_control_address = env.register_contract(None, AccessControlContract);
    let access_control = AccessControlContractClient::new(&env, &access_control_address);
    access_control.initialize(&admin);

    let treasury_address = env.register_contract(None, kora_treasury::TreasuryContract);
    let treasury = TreasuryContractClient::new(&env, &treasury_address);
    treasury.initialize(&admin, &50u32);

    let nft_address = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
    let nft = InvoiceNftContractClient::new(&env, &nft_address);
    nft.initialize(&admin, &access_control_address);

    let pool_address = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
    let pool = FinancingPoolContractClient::new(&env, &pool_address);
    let risk_registry = Address::generate(&env);
    let price_oracle = Address::generate(&env);
    let dispute_resolution = Address::generate(&env);
    pool.initialize(
        &admin,
        &nft_address,
        &risk_registry,
        &treasury_address,
        &access_control_address,
        &200u32,
        &price_oracle,
        &10_000u32,
        &0u64,
        &dispute_resolution,
    );

    let market_address = env.register_contract(None, kora_secondary_market::SecondaryMarket);
    let market = SecondaryMarketClient::new(&env, &market_address);
    nft.set_authorized_callers(&admin, &market_address, &pool_address);
    let debtor_hash = Bytes::from_slice(&env, &[0xABu8; 32]);
    let ipfs_cid = SorobanString::from_str(&env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
    nft.mint_invoice(&seller, &debtor_hash, &POOL_AMOUNT, &Symbol::new(&env, "USDC"), &(env.ledger().timestamp() + 86_400 * 30), &ipfs_cid, &50u32, &None);
    pool.release_funds(&market_address, &INVOICE_ID, &token_address);
    pool.record_position(
        &admin,
        &INVOICE_ID,
        &seller,
        &CONTRIBUTION,
        &POOL_AMOUNT,
    );

    market.initialize(
        &admin,
        &pool_address,
        &treasury_address,
        &protocol_fee_bps,
    );

    TestEnv {
        admin,
        seller,
        buyer,
        observer,
        treasury_address,
        secondary_market_address: market_address,
        token_address,
        token,
        pool,
        market,
    }
}

fn assert_fee_settlement(t: &TestEnv, fee_bps: u32) {
    let buyer_before = t.token.balance(&t.buyer);
    let seller_before = t.token.balance(&t.seller);
    let treasury_before = t.token.balance(&t.treasury_address);
    let observer_before = t.token.balance(&t.observer);
    let market_before = t.token.balance(&t.secondary_market_address);

    t.market
        .list_position(&t.seller, &INVOICE_ID, &t.token_address, &TRADE_PRICE);
    t.market.buy_position(&t.buyer, &INVOICE_ID, &t.seller);

    let expected_fee = TRADE_PRICE * fee_bps as i128 / 10_000;
    let expected_seller_proceeds = TRADE_PRICE - expected_fee;
    assert_eq!(buyer_before - t.token.balance(&t.buyer), TRADE_PRICE);
    assert_eq!(
        t.token.balance(&t.seller) - seller_before,
        expected_seller_proceeds,
    );
    assert_eq!(
        t.token.balance(&t.treasury_address) - treasury_before,
        expected_fee,
    );
    assert_eq!(t.token.balance(&t.observer), observer_before);
    assert_eq!(t.token.balance(&t.secondary_market_address), market_before);

    let buyer_position = t.pool.get_position(&INVOICE_ID, &t.buyer);
    let seller_position = t.pool.get_position(&INVOICE_ID, &t.seller);
    assert_eq!(buyer_position.investor, t.buyer);
    assert_eq!(buyer_position.contributed, CONTRIBUTION);
    assert_eq!(seller_position.contributed, 0);

    let buyer_after_settlement = t.token.balance(&t.buyer);
    let seller_after_settlement = t.token.balance(&t.seller);
    let treasury_after_settlement = t.token.balance(&t.treasury_address);
    assert!(t
        .market
        .try_buy_position(&t.buyer, &INVOICE_ID, &t.seller)
        .is_err());
    assert_eq!(t.token.balance(&t.buyer), buyer_after_settlement);
    assert_eq!(t.token.balance(&t.seller), seller_after_settlement);
    assert_eq!(t.token.balance(&t.treasury_address), treasury_after_settlement);
}

#[test]
fn buy_routes_configured_fee_to_treasury_without_leakage() {
    for fee_bps in [0, 1, 250, 999, 1_000] {
        let t = setup(fee_bps);
        assert_fee_settlement(&t, fee_bps);
    }
}

#[test]
fn buy_uses_admin_updated_fee_rate_for_settlement() {
    let t = setup(125);
    let updated_fee_bps = 775;
    t.market
        .update_protocol_fee_bps(&t.admin, &updated_fee_bps);
    assert_fee_settlement(&t, updated_fee_bps);
}