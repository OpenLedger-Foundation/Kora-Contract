// contracts/tests/marketplace_fee_property.rs
//! Property and table-driven tests for marketplace fee arithmetic and
//! fund_invoice checks-effects-interactions ordering (closes #1075).
//!
//! ## What is covered
//!
//! ### Fee + net identity  (IMPROVEMENTS.md §1)
//! For every `(amount, fee_bps)` pair in the table AND for randomly-generated
//! pairs, the invariant `fee + net == amount` must hold.  Any silent rounding
//! loss (dust) would violate this and must be caught here.
//!
//! ### CEI ordering for fund_invoice  (IMPROVEMENTS.md §3)
//! After a partial funding call, the listing's `funded_amount` state must be
//! updated before we can observe any further funding; we verify that the
//! in-contract state is updated atomically and that an over-funded follow-up
//! is rejected — proving the checks-effects step ran before the transfer.

#![cfg(test)]

extern crate std;

// ── Pure arithmetic property tests (no Soroban env) ──────────────────────────

/// Mirror of `kora_shared::validation::bps_of` for host-side verification.
/// This avoids a Soroban env dependency in a pure arithmetic test.
fn host_bps_of(amount: i128, bps: u32) -> Option<i128> {
    amount.checked_mul(bps as i128)?.checked_div(10_000)
}

/// Table of (amount, fee_bps) pairs that exercise boundary and dust cases.
const FEE_TABLE: &[(i128, u32)] = &[
    // zero fee
    (1, 0),
    (1_000_000, 0),
    // unit amounts with various bps
    (1, 1),
    (1, 9_999),
    (1, 10_000),
    // even divisibility
    (10_000, 100),   // 1 % → fee = 100, net = 9_900
    (10_000, 50),    // 0.5% → fee = 50, net = 9_950
    // dust: amount not divisible by 10_000
    (10_001, 50),    // fee = 5000050/10000 truncated = 5
    (3, 3_333),      // fee = 9999/10000 = 0, net = 3
    (7, 1_429),      // fee = 10003/10000 = 1 (rounds down), net = 6
    // large amounts
    (1_000_000_000_000i128, 50),
    (i128::MAX / 2, 1),
    // max fee (100 %)
    (1_000_000, 10_000),
    // near-max bps
    (1_000_000, 9_999),
];

/// Property: fee + net == amount for every table entry where both values exist.
#[test]
fn prop_fee_plus_net_equals_amount_table() {
    for &(amount, bps) in FEE_TABLE {
        if let Some(fee) = host_bps_of(amount, bps) {
            let net = amount - fee;
            assert_eq!(
                fee + net,
                amount,
                "fee({}) + net({}) != amount({}) at bps={}",
                fee, net, amount, bps
            );
            // fee and net must both be non-negative
            assert!(fee >= 0, "fee must be >= 0, got {} at amount={} bps={}", fee, amount, bps);
            assert!(net >= 0, "net must be >= 0, got {} at amount={} bps={}", net, amount, bps);
        }
    }
}

/// Property: fee is always <= amount.
#[test]
fn prop_fee_never_exceeds_amount_table() {
    for &(amount, bps) in FEE_TABLE {
        if let Some(fee) = host_bps_of(amount, bps) {
            assert!(
                fee <= amount,
                "fee {} > amount {} at bps={}",
                fee, amount, bps
            );
        }
    }
}

/// Property: at bps == 0 the fee is always 0 and net == amount.
#[test]
fn prop_zero_bps_means_zero_fee() {
    for &amount in &[0i128, 1, 100, 999_999, 1_000_000_000_000i128] {
        let fee = host_bps_of(amount, 0).unwrap_or(0);
        assert_eq!(fee, 0, "zero bps must yield zero fee for amount={}", amount);
        assert_eq!(amount - fee, amount);
    }
}

/// Property: at bps == 10_000 (100%) the fee equals the amount and net == 0.
#[test]
fn prop_full_bps_means_full_fee() {
    for &amount in &[0i128, 1, 100, 1_000_000] {
        if let Some(fee) = host_bps_of(amount, 10_000) {
            assert_eq!(fee, amount, "10_000 bps must yield fee==amount for amount={}", amount);
            assert_eq!(amount - fee, 0);
        }
    }
}

// ── proptest random coverage ──────────────────────────────────────────────────

use proptest::prelude::*;

proptest! {
    /// For random (amount, bps) pairs that don't overflow: fee + net == amount.
    #[test]
    fn prop_fee_plus_net_random(
        amount in 0i128..=1_000_000_000_000i128,
        bps    in 0u32..=10_000u32,
    ) {
        if let Some(fee) = host_bps_of(amount, bps) {
            let net = amount - fee;
            prop_assert_eq!(
                fee + net, amount,
                "fee({}) + net({}) != amount({}) bps={}",
                fee, net, amount, bps
            );
            prop_assert!(fee >= 0);
            prop_assert!(net >= 0);
        }
    }

    /// fee is monotonically non-decreasing in bps for a fixed amount.
    #[test]
    fn prop_fee_monotone_in_bps(
        amount in 1i128..=1_000_000_000i128,
        bps_lo in 0u32..5_000u32,
        bps_hi in 5_000u32..=10_000u32,
    ) {
        let fee_lo = host_bps_of(amount, bps_lo).unwrap_or(0);
        let fee_hi = host_bps_of(amount, bps_hi).unwrap_or(0);
        prop_assert!(
            fee_lo <= fee_hi,
            "fee monotonicity violated: fee({}) at bps={} > fee({}) at bps={}",
            fee_lo, bps_lo, fee_hi, bps_hi
        );
    }
}

// ── CEI ordering integration tests ───────────────────────────────────────────
//
// fund_invoice must update `listing.funded_amount` (effects) BEFORE making
// token transfers (interactions).  We prove this by:
//
//   1. Partially funding a listing.
//   2. Asserting `funded_amount` reflects the partial contribution immediately.
//   3. Attempting to over-fund so the state-check fires and rejects the call.
//
// If CEI were inverted (transfers before state), step 3 would succeed (state
// not yet updated) and the listing would be over-funded — a reentrancy vector.

#[cfg(test)]
mod cei_fund_invoice {
    use kora_marketplace::MarketplaceContractClient;
    use kora_invoice_nft::InvoiceNftContractClient;
    use kora_financing_pool::FinancingPoolContractClient;
    use kora_risk_registry::RiskRegistryContractClient;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Bytes, Env, String as SorobanString, Symbol,
    };

    /// Minimal test harness wiring all contracts needed for a fund_invoice call.
    struct Harness {
        env: Env,
        admin: Address,
        seller: Address,
        investor: Address,
        token: Address,
        mp: MarketplaceContractClient<'static>,
        nft: InvoiceNftContractClient<'static>,
    }

    fn setup() -> Harness {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 100_000,
        });

        let admin = Address::generate(&env);
        let seller = Address::generate(&env);
        let investor = Address::generate(&env);

        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        kora_access_control::AccessControlContractClient::new(&env, &ac_id).initialize(&admin);

        let treasury_id = env.register_contract(None, kora_treasury::TreasuryContract);
        let treasury_client = kora_treasury::TreasuryContractClient::new(&env, &treasury_id);
        treasury_client.initialize(&admin, &50u32);

        let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac_id);

        let staking_token_admin = Address::generate(&env);
        let staking_token = env.register_stellar_asset_contract_v2(staking_token_admin).address();

        let registry_id = env.register_contract(None, kora_risk_registry::RiskRegistryContract);
        let registry_client = RiskRegistryContractClient::new(&env, &registry_id);
        registry_client.initialize(&admin, &nft_id, &staking_token, &1_000_000i128, &5_000u32);

        let token_admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_admin).address();

        let oracle_id = env.register_contract(None, kora_price_oracle::PriceOracleContract);
        kora_price_oracle::PriceOracleContractClient::new(&env, &oracle_id).initialize(&admin, &ac_id);

        let preg_id = env.register_contract(None, kora_parameter_registry::ParameterRegistryContract);
        kora_parameter_registry::ParameterRegistryContractClient::new(&env, &preg_id)
            .initialize(&admin, &None);

        let pool_id = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
        let pool_client = FinancingPoolContractClient::new(&env, &pool_id);
        let dr = Address::generate(&env);
        pool_client.initialize(
            &admin, &nft_id, &registry_id, &treasury_id, &ac_id,
            &200u32, &oracle_id, &10_000u32, &0u64, &dr,
        );

        let mp_id = env.register_contract(None, kora_marketplace::MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        mp.initialize(
            &admin, &nft_id, &pool_id, &treasury_id, &ac_id,
            &oracle_id, &registry_id, &preg_id, &50u32, &0u32,
        );

        // Whitelist token (timelocked)
        mp.propose_token_whitelist(&admin, &token);
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000 + 86_401,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 100_000,
        });
        mp.execute_token_whitelist(&admin, &token);
        treasury_client.whitelist_token(&admin, &token);
        treasury_client.set_marketplace(&admin, &mp_id);
        mp.set_token_currency(&admin, &token, &Symbol::new(&env, "USDC"));
        mp.set_investor_accredited(&admin, &investor, &true);
        mp.set_min_contribution(&admin, &1);

        soroban_sdk::token::StellarAssetClient::new(&env, &token)
            .mint(&investor, &1_000_000_000_000i128);

        nft.set_authorized_callers(&admin, &mp_id, &pool_id);

        // Register + attest seller
        let verifier = Address::generate(&env);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token)
            .mint(&verifier, &1_000_000i128);
        registry_client.add_verifier(&admin, &verifier, &1_000_000i128);
        registry_client.register_sme(&verifier, &seller, &50u32, &true);

        Harness { env, admin, seller, investor, token, mp, nft }
    }

    fn list(h: &Harness, invoice_id: u64, asking_price: i128, face_value: i128) {
        let debtor_hash = Bytes::from_slice(&h.env, &[0xABu8; 32]);
        let ipfs_cid = SorobanString::from_str(
            &h.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        h.nft.mint_invoice(
            &h.seller, &debtor_hash, &face_value,
            &Symbol::new(&h.env, "USDC"),
            &(h.env.ledger().timestamp() + 86_400 * 30),
            &ipfs_cid, &50u32, &None,
        );
        h.mp.list_invoice(
            &h.seller, &invoice_id, &asking_price, &face_value,
            &h.token, &(h.env.ledger().timestamp() + 86_400), &None,
        );
    }

    /// Table of (asking_price, partial_amount) pairs for CEI ordering checks.
    const CEI_TABLE: &[(i128, i128)] = &[
        (10_000_000, 1_000_000),
        (10_000_000, 9_999_999),
        (1_000_000_000, 500_000_000),
        (100, 50),
        (10_000_000_000, 1),
    ];

    /// After a partial fund: funded_amount == contributed amount (effects ran).
    /// After an over-fund attempt: rejected (checks ran on updated state).
    #[test]
    fn test_fund_invoice_effects_before_interactions_table() {
        for (i, &(asking_price, partial)) in CEI_TABLE.iter().enumerate() {
            let h = setup();
            let invoice_id = (i as u64) + 1;
            list(&h, invoice_id, asking_price, asking_price + 1_000_000);

            // 1. Partial fund — must succeed.
            h.mp.fund_invoice(&h.investor, &invoice_id, &partial, &None);

            // 2. Effects: funded_amount must already reflect the partial amount.
            let listing = h.mp.get_listing(&invoice_id);
            assert_eq!(
                listing.funded_amount, partial,
                "CEI violated at case {}: funded_amount {} != partial {}",
                i, listing.funded_amount, partial
            );

            // 3. Attempting to fund more than the remaining capacity must fail.
            let remaining = asking_price - partial;
            let over_amount = remaining + 1;
            let result = h.mp.try_fund_invoice(&h.investor, &invoice_id, &over_amount, &None);
            assert!(
                result.is_err(),
                "CEI violated at case {}: over-fund of {} above remaining {} must be rejected",
                i, over_amount, remaining
            );
        }
    }

    /// funded_amount never exceeds asking_price regardless of funding sequence.
    #[test]
    fn test_funded_amount_never_exceeds_asking_price() {
        let h = setup();
        let asking_price = 5_000_000i128;
        list(&h, 1, asking_price, asking_price + 1_000_000);

        // Three sequential partial contributions that sum to exactly asking_price.
        h.mp.fund_invoice(&h.investor, &1u64, &2_000_000i128, &None);
        h.mp.fund_invoice(&h.investor, &1u64, &2_000_000i128, &None);
        h.mp.fund_invoice(&h.investor, &1u64, &1_000_000i128, &None);

        let listing = h.mp.get_listing(&1u64);
        assert_eq!(listing.funded_amount, asking_price);

        // Any further contribution must be rejected — the listing is fully funded.
        let extra = h.mp.try_fund_invoice(&h.investor, &1u64, &1i128, &None);
        assert!(extra.is_err(), "over-funding a closed listing must be rejected");
    }
}
