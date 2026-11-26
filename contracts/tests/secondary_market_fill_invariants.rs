//! Secondary-market partial-fill accounting invariants (issue #1101).
//!
//! Part 1 property-tests `kora_secondary_market::quote_fill` — the single
//! source of truth for fill pricing, fee disclosure and remaining size —
//! across random multi-fill sequences.
//!
//! Part 2 checks that the on-chain flow (`quote_purchase`, `buy_position`,
//! `cancel_listing`) settles exactly what the quote discloses, that an exact
//! fill closes the listing, that a closed listing rejects further fills
//! (overfill), and that cancelling leaves the residual with the seller.
//!
//! Invariants are listed in `docs/secondary-market.md`.

use kora_access_control::{AccessControlContract, AccessControlContractClient};
use kora_financing_pool::FinancingPoolContractClient;
use kora_invoice_nft::InvoiceNftContractClient;
use kora_secondary_market::{quote_fill, FillQuote, SecondaryMarketClient, SecondaryMarketError};
use kora_treasury::TreasuryContractClient;
use proptest::prelude::*;
use soroban_sdk::{testutils::Address as _, token, Address, Bytes, Env, String as SorobanString, Symbol};

const MAX_FEE_BPS: u32 = 1_000;

fn bps_of(amount: i128, bps: u32) -> i128 {
    amount * bps as i128 / 10_000
}

/// Apply `fills` (each clamped to the remaining size) and return the quotes.
fn run_fills(price: i128, size: i128, fills: &[i128], fee_bps: u32) -> Vec<FillQuote> {
    let mut filled = 0i128;
    let mut quotes = Vec::new();
    for &f in fills {
        let remaining = size - filled;
        if remaining == 0 {
            break;
        }
        let amount = f.clamp(1, remaining);
        match quote_fill(price, size, filled, amount, fee_bps) {
            Ok(q) => {
                filled = q.filled_after;
                quotes.push(q);
            }
            // A fill too small to carry a non-zero price is rejected; skip it.
            Err(SecondaryMarketError::InvalidAmount) => continue,
            Err(e) => panic!("unexpected error {:?}", e),
        }
    }
    quotes
}

fn assert_quote_identities(q: &FillQuote, size: i128) {
    assert!(q.gross > 0);
    assert!(q.fee >= 0 && q.seller_proceeds >= 0);
    assert_eq!(q.fee + q.seller_proceeds, q.gross, "fee bps identity");
    assert!(q.fee <= q.gross);
    assert_eq!(q.filled_after + q.remaining, size, "remaining accounting");
    assert!(q.remaining >= 0);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    /// Multi-fill: per-fill identities hold, remaining is strictly decreasing,
    /// and totals never exceed the listing's price or fee.
    #[test]
    fn multi_fill_invariants(
        price in 1i128..1_000_000_000_000,
        size in 1i128..1_000_000,
        fee_bps in 0u32..=MAX_FEE_BPS,
        fills in prop::collection::vec(1i128..200_000, 1..40),
    ) {
        let quotes = run_fills(price, size, &fills, fee_bps);
        let mut prev_remaining = size;
        let (mut gross, mut fee, mut proceeds) = (0i128, 0i128, 0i128);
        for q in &quotes {
            assert_quote_identities(q, size);
            prop_assert!(q.remaining < prev_remaining);
            prev_remaining = q.remaining;
            gross += q.gross;
            fee += q.fee;
            proceeds += q.seller_proceeds;
            // Cumulative totals track the closed-form cumulative price/fee.
            let cum_price = price * q.filled_after / size;
            prop_assert_eq!(gross, cum_price);
            prop_assert_eq!(fee, bps_of(cum_price, fee_bps));
        }
        prop_assert!(gross <= price);
        prop_assert!(fee <= bps_of(price, fee_bps));
        prop_assert_eq!(fee + proceeds, gross);
    }

    /// Filling the listing to completion (any split) charges exactly the
    /// listed price and exactly the single-shot fee — no rounding drift.
    #[test]
    fn complete_fill_matches_single_shot(
        price in 1i128..1_000_000_000_000,
        size in 1i128..10_000,
        fee_bps in 0u32..=MAX_FEE_BPS,
        fills in prop::collection::vec(1i128..5_000, 1..50),
    ) {
        // Price >= size guarantees every unit carries a non-zero price.
        let price = price.max(size);
        let mut fills = fills;
        fills.push(size); // final sweep closes whatever remains
        let quotes = run_fills(price, size, &fills, fee_bps);
        let last = quotes.last().unwrap();
        prop_assert_eq!(last.remaining, 0, "exact fill closes listing");

        let gross: i128 = quotes.iter().map(|q| q.gross).sum();
        let fee: i128 = quotes.iter().map(|q| q.fee).sum();
        let proceeds: i128 = quotes.iter().map(|q| q.seller_proceeds).sum();
        prop_assert_eq!(gross, price);
        prop_assert_eq!(fee, bps_of(price, fee_bps));
        prop_assert_eq!(proceeds, price - bps_of(price, fee_bps));

        let single = quote_fill(price, size, 0, size, fee_bps).unwrap();
        prop_assert_eq!(single.gross, gross);
        prop_assert_eq!(single.fee, fee);
        prop_assert_eq!(single.seller_proceeds, proceeds);
    }

    /// Any fill larger than the remaining size is rejected with `Overfill`.
    #[test]
    fn overfill_rejected(
        price in 1i128..1_000_000_000,
        size in 1i128..1_000_000,
        filled_frac in 0u32..=100,
        excess in 1i128..1_000_000,
        fee_bps in 0u32..=MAX_FEE_BPS,
    ) {
        let filled = size * filled_frac as i128 / 100;
        let remaining = size - filled;
        prop_assert_eq!(
            quote_fill(price, size, filled, remaining + excess, fee_bps),
            Err(SecondaryMarketError::Overfill)
        );
    }

    /// Cancelling after partial fills leaves exactly the unfilled residual
    /// with the seller and the buyers paid exactly the cumulative price.
    #[test]
    fn cancel_with_residual(
        price in 1i128..1_000_000_000_000,
        size in 2i128..1_000_000,
        fee_bps in 0u32..=MAX_FEE_BPS,
        fills in prop::collection::vec(1i128..100_000, 1..10),
    ) {
        let quotes = run_fills(price, size, &fills, fee_bps);
        let filled = quotes.last().map(|q| q.filled_after).unwrap_or(0);
        let residual = size - filled;
        let gross: i128 = quotes.iter().map(|q| q.gross).sum();
        prop_assert_eq!(residual, quotes.last().map(|q| q.remaining).unwrap_or(size));
        prop_assert_eq!(gross, price * filled / size);
        if residual > 0 {
            // A cancelled (or still-open) listing can still be filled up to the residual only.
            prop_assert_eq!(
                quote_fill(price, size, filled, residual + 1, fee_bps),
                Err(SecondaryMarketError::Overfill)
            );
        }
    }
}

#[test]
fn exact_fill_closes_listing_and_further_fills_overfill() {
    let q = quote_fill(1_000, 10, 4, 6, 250).unwrap();
    assert_eq!(q.remaining, 0);
    assert_eq!(q.filled_after, 10);
    assert_eq!(quote_fill(1_000, 10, 10, 1, 250), Err(SecondaryMarketError::Overfill));
}

#[test]
fn rejects_invalid_inputs() {
    assert_eq!(quote_fill(0, 10, 0, 1, 250), Err(SecondaryMarketError::InvalidAmount));
    assert_eq!(quote_fill(100, 0, 0, 1, 250), Err(SecondaryMarketError::InvalidAmount));
    assert_eq!(quote_fill(100, 10, 0, 0, 250), Err(SecondaryMarketError::InvalidAmount));
    assert_eq!(quote_fill(100, 10, -1, 1, 250), Err(SecondaryMarketError::InvalidAmount));
    assert_eq!(quote_fill(100, 10, 11, 1, 250), Err(SecondaryMarketError::InvalidAmount));
    assert_eq!(quote_fill(100, 10, 0, 1, 1_001), Err(SecondaryMarketError::InvalidFeeRate));
    // 1 unit of a 1_000-unit listing priced at 10 has zero price: rejected, not free.
    assert_eq!(quote_fill(10, 1_000, 0, 1, 250), Err(SecondaryMarketError::InvalidAmount));
}

#[test]
fn rounding_dust_is_not_lost_across_fills() {
    // 100 split into thirds: 33 + 33 + 34 == 100; fees 0 + 1 + 1 == bps_of(100, 250) == 2.
    let a = quote_fill(100, 3, 0, 1, 250).unwrap();
    let b = quote_fill(100, 3, 1, 1, 250).unwrap();
    let c = quote_fill(100, 3, 2, 1, 250).unwrap();
    assert_eq!((a.gross, b.gross, c.gross), (33, 33, 34));
    assert_eq!(a.fee + b.fee + c.fee, 2);
    assert_eq!(c.remaining, 0);
}

// ── Part 2: on-chain settlement matches disclosure ───────────────────────────

const INVOICE_ID: u64 = 47_100;
const POOL_AMOUNT: i128 = 1_000_000_000;
const TRADE_PRICE: i128 = 12_345_679;

struct TestEnv {
    admin: Address,
    seller: Address,
    buyer: Address,
    treasury_address: Address,
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
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token = token::Client::new(&env, &token_address);
    token::StellarAssetClient::new(&env, &token_address).mint(&buyer, &(TRADE_PRICE * 2));

    let access_control_address = env.register_contract(None, AccessControlContract);
    AccessControlContractClient::new(&env, &access_control_address).initialize(&admin);

    let treasury_address = env.register_contract(None, kora_treasury::TreasuryContract);
    TreasuryContractClient::new(&env, &treasury_address).initialize(&admin, &50u32);

    let nft_address = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
    let nft = InvoiceNftContractClient::new(&env, &nft_address);
    nft.initialize(&admin, &access_control_address);

    let pool_address = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
    let pool = FinancingPoolContractClient::new(&env, &pool_address);
    pool.initialize(
        &admin,
        &nft_address,
        &Address::generate(&env),
        &treasury_address,
        &access_control_address,
        &200u32,
        &Address::generate(&env),
        &10_000u32,
        &0u64,
        &Address::generate(&env),
    );

    let market_address = env.register_contract(None, kora_secondary_market::SecondaryMarket);
    let market = SecondaryMarketClient::new(&env, &market_address);
    nft.set_authorized_callers(&admin, &market_address, &pool_address);
    let debtor_hash = Bytes::from_slice(&env, &[0xABu8; 32]);
    let ipfs_cid = SorobanString::from_str(&env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
    nft.mint_invoice(&seller, &debtor_hash, &POOL_AMOUNT, &Symbol::new(&env, "USDC"), &(env.ledger().timestamp() + 86_400 * 30), &ipfs_cid, &50u32, &None);
    pool.release_funds(&market_address, &INVOICE_ID, &token_address);
    pool.record_position(&admin, &INVOICE_ID, &seller, &POOL_AMOUNT, &POOL_AMOUNT);

    market.initialize(&admin, &pool_address, &treasury_address, &protocol_fee_bps);

    TestEnv {
        admin,
        seller,
        buyer,
        treasury_address,
        token_address,
        token,
        pool,
        market,
    }
}

#[test]
fn on_chain_exact_fill_settles_disclosed_quote_and_closes_listing() {
    for fee_bps in [0u32, 250, 1_000] {
        let t = setup(fee_bps);
        t.market.list_position(&t.seller, &INVOICE_ID, &t.token_address, &TRADE_PRICE);

        let disclosed = t.market.quote_purchase(&INVOICE_ID, &t.seller);
        assert_eq!(disclosed, quote_fill(TRADE_PRICE, 1, 0, 1, fee_bps).unwrap());
        assert_eq!(disclosed.gross, TRADE_PRICE);
        assert_eq!(disclosed.fee, bps_of(TRADE_PRICE, fee_bps));
        assert_eq!(disclosed.remaining, 0);

        let buyer_before = t.token.balance(&t.buyer);
        let seller_before = t.token.balance(&t.seller);
        let treasury_before = t.token.balance(&t.treasury_address);
        t.market.buy_position(&t.buyer, &INVOICE_ID, &t.seller);

        assert_eq!(buyer_before - t.token.balance(&t.buyer), disclosed.gross);
        assert_eq!(t.token.balance(&t.seller) - seller_before, disclosed.seller_proceeds);
        assert_eq!(t.token.balance(&t.treasury_address) - treasury_before, disclosed.fee);

        // Exact fill closes the listing: no quote, and a second fill is rejected.
        assert_eq!(
            t.market.try_quote_purchase(&INVOICE_ID, &t.seller),
            Err(Ok(SecondaryMarketError::ListingNotFound))
        );
        assert_eq!(
            t.market.try_buy_position(&t.buyer, &INVOICE_ID, &t.seller),
            Err(Ok(SecondaryMarketError::ListingNotFound))
        );
        assert_eq!(t.pool.get_position(&INVOICE_ID, &t.buyer).contributed, POOL_AMOUNT);
    }
}

#[test]
fn disclosure_tracks_fee_updates() {
    let t = setup(250);
    t.market.list_position(&t.seller, &INVOICE_ID, &t.token_address, &TRADE_PRICE);
    t.market.update_protocol_fee_bps(&t.admin, &900u32);
    let disclosed = t.market.quote_purchase(&INVOICE_ID, &t.seller);
    assert_eq!(disclosed.fee, bps_of(TRADE_PRICE, 900));

    let treasury_before = t.token.balance(&t.treasury_address);
    t.market.buy_position(&t.buyer, &INVOICE_ID, &t.seller);
    assert_eq!(t.token.balance(&t.treasury_address) - treasury_before, disclosed.fee);
}

#[test]
fn cancel_leaves_residual_with_seller_and_moves_no_funds() {
    let t = setup(250);
    t.market.list_position(&t.seller, &INVOICE_ID, &t.token_address, &TRADE_PRICE);

    let buyer_before = t.token.balance(&t.buyer);
    let seller_before = t.token.balance(&t.seller);
    let treasury_before = t.token.balance(&t.treasury_address);
    t.market.cancel_listing(&t.seller, &INVOICE_ID);

    assert_eq!(t.token.balance(&t.buyer), buyer_before);
    assert_eq!(t.token.balance(&t.seller), seller_before);
    assert_eq!(t.token.balance(&t.treasury_address), treasury_before);
    // The full (unfilled) position stays with the seller.
    assert_eq!(t.pool.get_position(&INVOICE_ID, &t.seller).contributed, POOL_AMOUNT);
    assert_eq!(t.pool.get_position(&INVOICE_ID, &t.buyer).contributed, 0);

    assert_eq!(
        t.market.try_buy_position(&t.buyer, &INVOICE_ID, &t.seller),
        Err(Ok(SecondaryMarketError::ListingNotFound))
    );
    assert_eq!(
        t.market.try_cancel_listing(&t.seller, &INVOICE_ID),
        Err(Ok(SecondaryMarketError::ListingNotFound))
    );

    // The residual can be relisted and sold normally.
    t.market.list_position(&t.seller, &INVOICE_ID, &t.token_address, &TRADE_PRICE);
    t.market.buy_position(&t.buyer, &INVOICE_ID, &t.seller);
    assert_eq!(t.pool.get_position(&INVOICE_ID, &t.buyer).contributed, POOL_AMOUNT);
}
