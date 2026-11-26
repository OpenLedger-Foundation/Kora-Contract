// tests/fee_clawback_policy.rs
//! Fee clawback policy enforcement tests (Issue #1111).
//!
//! Each rule in `docs/FEE_CLAWBACK_POLICY.md` is mapped to at least one
//! positive and/or negative test below. Test names are cited from the policy
//! document, so renaming a test here requires updating the doc as well.
//!
//! Section 1 drives `treasury::refund_fee` directly (the clawback primitive).
//! Section 2 drives the full `marketplace::fund_invoice` → `claim_refund`
//! flow against real backing contracts so token balances actually settle.

#[cfg(test)]
mod fee_clawback_policy {
    use kora_financing_pool::FinancingPoolContractClient;
    use kora_invoice_nft::InvoiceNftContractClient;
    use kora_marketplace::MarketplaceContractClient;
    use kora_risk_registry::RiskRegistryContractClient;
    use kora_shared::errors::KoraError;
    use kora_treasury::{TreasuryContractClient, TreasuryError};
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events, Ledger, LedgerInfo},
        token::{Client as TokenClient, StellarAssetClient},
        Address, Bytes, Env, String as SorobanString, Symbol, TryFromVal,
    };

    const T0: u64 = 1_700_000_000;

    fn set_ledger(env: &Env, timestamp: u64, sequence_number: u32) {
        env.ledger().set(LedgerInfo {
            timestamp,
            protocol_version: 21,
            sequence_number,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
    }

    /// True when `contract` emitted a `FEE_REV` event in the last invocation.
    fn emitted_fee_rev(env: &Env, contract: &Address) -> bool {
        let events = env.events().all();
        for i in 0..events.len() {
            let (emitter, topics, _data) = events.get(i).unwrap();
            if &emitter != contract || topics.len() < 2 {
                continue;
            }
            if let Ok(sym) = Symbol::try_from_val(env, &topics.get(1).unwrap()) {
                if sym == symbol_short!("FEE_REV") {
                    return true;
                }
            }
        }
        false
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Section 1 — treasury::refund_fee (clawback primitive)
    // ═════════════════════════════════════════════════════════════════════════

    const SEEDED_FEES: i128 = 10_000;

    struct TreasuryEnv {
        env: Env,
        admin: Address,
        marketplace: Address,
        token: Address,
        treasury: Address,
        client: TreasuryContractClient<'static>,
    }

    /// Treasury with a whitelisted SAC token, an authorized marketplace and
    /// `SEEDED_FEES` already collected (balance and ledger agree).
    fn treasury_setup(with_marketplace: bool) -> TreasuryEnv {
        let env = Env::default();
        env.mock_all_auths();
        set_ledger(&env, T0, 1);

        let admin = Address::generate(&env);
        let marketplace = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_admin).address();

        let treasury = env.register_contract(None, kora_treasury::TreasuryContract);
        let client = TreasuryContractClient::new(&env, &treasury);
        client.initialize(&admin, &50u32);
        client.whitelist_token(&admin, &token);
        if with_marketplace {
            client.set_marketplace(&admin, &marketplace);
        }

        StellarAssetClient::new(&env, &token).mint(&treasury, &SEEDED_FEES);
        client.collect_fee(&token, &SEEDED_FEES);

        TreasuryEnv { env, admin, marketplace, token, treasury, client }
    }

    /// Policy: "caller must be the authorized marketplace".
    #[test]
    fn clawback_treasury_rejects_non_marketplace_caller() {
        let t = treasury_setup(true);
        let attacker = Address::generate(&t.env);
        let investor = Address::generate(&t.env);

        let result = t.client.try_refund_fee(&attacker, &t.token, &1_000i128, &investor);
        assert_eq!(result.unwrap_err().unwrap(), TreasuryError::NotAdmin);

        // The admin is not the marketplace either.
        let result = t.client.try_refund_fee(&t.admin, &t.token, &1_000i128, &investor);
        assert_eq!(result.unwrap_err().unwrap(), TreasuryError::NotAdmin);

        assert_eq!(t.client.get_collected(&t.token), SEEDED_FEES);
        assert_eq!(TokenClient::new(&t.env, &t.token).balance(&investor), 0);
    }

    /// Policy: "caller must be the authorized marketplace" — with no
    /// marketplace configured, no caller can claw back.
    #[test]
    fn clawback_treasury_rejects_when_marketplace_unset() {
        let t = treasury_setup(false);
        let investor = Address::generate(&t.env);

        let result = t.client.try_refund_fee(&t.marketplace, &t.token, &1_000i128, &investor);
        assert_eq!(result.unwrap_err().unwrap(), TreasuryError::NotInitialized);
        assert_eq!(t.client.get_collected(&t.token), SEEDED_FEES);
    }

    /// Clawback amounts must be strictly positive.
    #[test]
    fn clawback_treasury_rejects_non_positive_amount() {
        let t = treasury_setup(true);
        let investor = Address::generate(&t.env);

        for amount in [0i128, -1i128] {
            let result = t.client.try_refund_fee(&t.marketplace, &t.token, &amount, &investor);
            assert_eq!(result.unwrap_err().unwrap(), TreasuryError::InvalidAmount);
        }
        assert_eq!(t.client.get_collected(&t.token), SEEDED_FEES);
    }

    /// Clawback only moves whitelisted fee tokens.
    #[test]
    fn clawback_treasury_rejects_non_whitelisted_token() {
        let t = treasury_setup(true);
        let investor = Address::generate(&t.env);
        let other_admin = Address::generate(&t.env);
        let other = t.env.register_stellar_asset_contract_v2(other_admin).address();

        let result = t.client.try_refund_fee(&t.marketplace, &other, &1_000i128, &investor);
        assert_eq!(result.unwrap_err().unwrap(), TreasuryError::TokenNotWhitelisted);
    }

    /// Policy: "fails with InsufficientPoolBalance when the treasury balance
    /// cannot cover it, e.g. fees already spent via grants". State is untouched.
    #[test]
    fn clawback_treasury_insufficient_balance_rejected() {
        let t = treasury_setup(true);
        let investor = Address::generate(&t.env);

        // Fees already spent: drain the treasury's live balance.
        let sink = Address::generate(&t.env);
        t.env.as_contract(&t.treasury, || {
            TokenClient::new(&t.env, &t.token).transfer(&t.treasury, &sink, &(SEEDED_FEES - 1));
        });

        let result = t.client.try_refund_fee(&t.marketplace, &t.token, &SEEDED_FEES, &investor);
        assert_eq!(result.unwrap_err().unwrap(), TreasuryError::InsufficientPoolBalance);
        assert_eq!(t.client.get_collected(&t.token), SEEDED_FEES);
        assert_eq!(TokenClient::new(&t.env, &t.token).balance(&investor), 0);
    }

    /// Edge case — partial clawback: each clawback decrements the `Collected`
    /// ledger and the live balance by exactly the clawed-back amount, and the
    /// treasury can never refund more than it holds in aggregate.
    #[test]
    fn clawback_treasury_partial_clawback_decrements_ledger_exactly() {
        let t = treasury_setup(true);
        let investor = Address::generate(&t.env);
        let token = TokenClient::new(&t.env, &t.token);

        t.client.refund_fee(&t.marketplace, &t.token, &4_000i128, &investor);
        assert_eq!(t.client.get_collected(&t.token), SEEDED_FEES - 4_000);
        assert_eq!(token.balance(&t.treasury), SEEDED_FEES - 4_000);
        assert_eq!(token.balance(&investor), 4_000);

        t.client.refund_fee(&t.marketplace, &t.token, &(SEEDED_FEES - 4_000), &investor);
        assert_eq!(t.client.get_collected(&t.token), 0);
        assert_eq!(token.balance(&t.treasury), 0);
        assert_eq!(token.balance(&investor), SEEDED_FEES);

        let result = t.client.try_refund_fee(&t.marketplace, &t.token, &1i128, &investor);
        assert_eq!(result.unwrap_err().unwrap(), TreasuryError::InsufficientPoolBalance);
    }

    /// Policy: "treasury emits FEE_REV".
    #[test]
    fn clawback_treasury_emits_fee_rev_event() {
        let t = treasury_setup(true);
        let investor = Address::generate(&t.env);

        t.client.refund_fee(&t.marketplace, &t.token, &1_000i128, &investor);
        assert!(
            emitted_fee_rev(&t.env, &t.treasury),
            "treasury must emit FEE_REV on clawback"
        );
    }

    // ═════════════════════════════════════════════════════════════════════════
    // Section 2 — marketplace::claim_refund end-to-end clawback
    // ═════════════════════════════════════════════════════════════════════════

    const ASKING: i128 = 10_000_000_000;
    const FACE: i128 = 11_000_000_000;
    const START_BALANCE: i128 = 1_000_000_000_000;

    struct MarketEnv {
        env: Env,
        admin: Address,
        seller: Address,
        investor: Address,
        token: Address,
        treasury: Address,
        mp: MarketplaceContractClient<'static>,
        nft: InvoiceNftContractClient<'static>,
        treasury_client: TreasuryContractClient<'static>,
    }

    /// Mirrors the real-contract setup in `marketplace_edge_cases.rs` with a
    /// configurable referrer split.
    fn market_setup(referrer_split_bps: u32) -> MarketEnv {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        set_ledger(&env, T0, 1);

        let admin = Address::generate(&env);
        let seller = Address::generate(&env);
        let investor = Address::generate(&env);

        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        kora_access_control::AccessControlContractClient::new(&env, &ac_id).initialize(&admin);

        let treasury = env.register_contract(None, kora_treasury::TreasuryContract);
        let treasury_client = TreasuryContractClient::new(&env, &treasury);
        treasury_client.initialize(&admin, &50u32);

        let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac_id);

        let registry_id = env.register_contract(None, kora_risk_registry::RiskRegistryContract);
        let registry_client = RiskRegistryContractClient::new(&env, &registry_id);
        let staking_token_admin = Address::generate(&env);
        let staking_token = env.register_stellar_asset_contract_v2(staking_token_admin).address();
        registry_client.initialize(&admin, &nft_id, &staking_token, &1_000_000i128, &5_000u32);

        let token_admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_admin).address();

        let oracle_id = env.register_contract(None, kora_price_oracle::PriceOracleContract);
        kora_price_oracle::PriceOracleContractClient::new(&env, &oracle_id).initialize(&admin, &ac_id);
        let preg_id = env.register_contract(None, kora_parameter_registry::ParameterRegistryContract);
        kora_parameter_registry::ParameterRegistryContractClient::new(&env, &preg_id).initialize(&admin, &None);

        let pool_id = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
        let pool_client = FinancingPoolContractClient::new(&env, &pool_id);
        let dispute_resolution = Address::generate(&env);
        pool_client.initialize(
            &admin, &nft_id, &registry_id, &treasury, &ac_id, &200u32, &oracle_id, &10_000u32, &0u64, &dispute_resolution,
        );

        let mp_id = env.register_contract(None, kora_marketplace::MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        mp.initialize(
            &admin, &nft_id, &pool_id, &treasury, &ac_id, &oracle_id, &registry_id, &preg_id, &50u32, &referrer_split_bps,
        );

        mp.propose_token_whitelist(&admin, &token);
        set_ledger(&env, T0 + 86_400 + 1, 2);
        mp.execute_token_whitelist(&admin, &token);
        treasury_client.whitelist_token(&admin, &token);
        treasury_client.set_marketplace(&admin, &mp_id);
        mp.set_token_currency(&admin, &token, &Symbol::new(&env, "USDC"));

        mp.set_investor_accredited(&admin, &investor, &true);
        StellarAssetClient::new(&env, &token).mint(&investor, &START_BALANCE);

        nft.set_authorized_callers(&admin, &mp_id, &pool_id);
        mp.set_min_contribution(&admin, &1);

        let setup_verifier = Address::generate(&env);
        StellarAssetClient::new(&env, &staking_token).mint(&setup_verifier, &1_000_000i128);
        registry_client.add_verifier(&admin, &setup_verifier, &1_000_000i128);
        registry_client.register_sme(&setup_verifier, &seller, &50u32, &true);

        MarketEnv { env, admin, seller, investor, token, treasury, mp, nft, treasury_client }
    }

    impl MarketEnv {
        fn balance(&self, who: &Address) -> i128 {
            TokenClient::new(&self.env, &self.token).balance(who)
        }

        fn new_investor(&self) -> Address {
            let investor = Address::generate(&self.env);
            self.mp.set_investor_accredited(&self.admin, &investor, &true);
            StellarAssetClient::new(&self.env, &self.token).mint(&investor, &START_BALANCE);
            investor
        }

        /// Mint + list invoice 1; returns the funding deadline.
        fn list(&self, referrer: Option<Address>) -> u64 {
            let debtor_hash = Bytes::from_slice(&self.env, &[0xABu8; 32]);
            let ipfs_cid = SorobanString::from_str(
                &self.env,
                "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            );
            self.nft.mint_invoice(
                &self.seller,
                &debtor_hash,
                &FACE,
                &Symbol::new(&self.env, "USDC"),
                &(self.env.ledger().timestamp() + 86_400 * 30),
                &ipfs_cid,
                &50u32,
                &None,
            );
            let deadline = self.env.ledger().timestamp() + 86_400;
            self.mp.list_invoice(&self.seller, &1u64, &ASKING, &FACE, &self.token, &deadline, &referrer);
            deadline
        }

        /// Fund invoice 1 and return the fee the treasury received.
        fn fund(&self, investor: &Address, amount: i128) -> i128 {
            let before = self.balance(&self.treasury);
            self.mp.fund_invoice(investor, &1u64, &amount, &None);
            self.balance(&self.treasury) - before
        }

        fn expire(&self, deadline: u64) {
            self.env.ledger().with_mut(|li| {
                li.timestamp = deadline + 1;
                li.sequence_number += 1;
            });
        }
    }

    /// Policy: "`claim_refund` claws back `fee_contributed` via
    /// `treasury::refund_fee`" — expired listing, investor made whole.
    #[test]
    fn clawback_refund_returns_full_fee_on_expired_listing() {
        let t = market_setup(0);
        let deadline = t.list(None);
        let collected_before = t.treasury_client.get_collected(&t.token);

        let fee = t.fund(&t.investor, 1_000_000);
        assert!(fee > 0, "precondition: a non-zero fee was collected");
        assert_eq!(t.balance(&t.investor), START_BALANCE - 1_000_000);

        t.expire(deadline);
        t.mp.claim_refund(&t.investor, &1u64);

        assert_eq!(t.balance(&t.investor), START_BALANCE, "net + fee returned");
        assert_eq!(t.balance(&t.treasury), 0, "fee left the treasury");
        assert_eq!(t.treasury_client.get_collected(&t.token), collected_before);
    }

    /// Policy: clawback also applies to the two-phase cancellation path.
    #[test]
    fn clawback_refund_returns_full_fee_after_confirmed_cancellation() {
        let t = market_setup(0);
        t.list(None);
        let fee = t.fund(&t.investor, 1_000_000);
        assert!(fee > 0);

        t.mp.request_cancellation(&t.seller, &1u64);
        t.mp.admin_confirm_cancellation(&t.admin, &1u64);
        // Deadline has NOT passed — confirmation alone unlocks the refund.
        t.mp.claim_refund(&t.investor, &1u64);

        assert_eq!(t.balance(&t.investor), START_BALANCE);
        assert_eq!(t.balance(&t.treasury), 0);
    }

    /// Negative: no clawback while the listing is live and not cancelled.
    #[test]
    fn clawback_refund_rejected_before_deadline_without_cancellation() {
        let t = market_setup(0);
        t.list(None);
        let fee = t.fund(&t.investor, 1_000_000);

        let result = t.mp.try_claim_refund(&t.investor, &1u64);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::FundingNotExpired);
        assert_eq!(t.balance(&t.treasury), fee, "fee stays in treasury");
    }

    /// Policy: "The `FeeContribution` record is cleared after clawback and
    /// `RefundClaimed` is set first (CEI), so repeated refunds cannot
    /// double-clawback."
    #[test]
    fn clawback_refund_cannot_double_clawback() {
        let t = market_setup(0);
        let deadline = t.list(None);
        // Leave extra fees in the treasury so a double clawback would be
        // payable if the guard were missing.
        let other = t.new_investor();
        t.fund(&other, 2_000_000);
        t.fund(&t.investor, 1_000_000);

        t.expire(deadline);
        t.mp.claim_refund(&t.investor, &1u64);
        let treasury_after_first = t.balance(&t.treasury);
        let investor_after_first = t.balance(&t.investor);

        let result = t.mp.try_claim_refund(&t.investor, &1u64);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::AlreadyInitialized);
        assert_eq!(t.balance(&t.treasury), treasury_after_first);
        assert_eq!(t.balance(&t.investor), investor_after_first);
    }

    /// Edge case — partial clawback: with several investors, each refund
    /// claws back only that investor's own fee share.
    #[test]
    fn clawback_partial_refund_only_reverses_own_fee_share() {
        let t = market_setup(0);
        let deadline = t.list(None);
        let second = t.new_investor();

        let fee_a = t.fund(&t.investor, 1_000_000);
        let fee_b = t.fund(&second, 3_000_000);
        assert!(fee_a > 0 && fee_b > fee_a);
        let collected_full = t.treasury_client.get_collected(&t.token);

        t.expire(deadline);
        t.mp.claim_refund(&t.investor, &1u64);
        assert_eq!(t.balance(&t.investor), START_BALANCE);
        assert_eq!(t.balance(&t.treasury), fee_b, "only investor A's fee left");
        assert_eq!(t.treasury_client.get_collected(&t.token), collected_full - fee_a);

        t.mp.claim_refund(&second, &1u64);
        assert_eq!(t.balance(&second), START_BALANCE);
        assert_eq!(t.balance(&t.treasury), 0);
        assert_eq!(t.treasury_client.get_collected(&t.token), collected_full - fee_a - fee_b);
    }

    /// Policy: "fails with `InsufficientPoolBalance` when the treasury balance
    /// cannot cover it". The whole refund reverts atomically — the investor
    /// is not marked refunded and can claim once the treasury is solvent.
    #[test]
    fn clawback_refund_reverts_when_treasury_cannot_cover_fee() {
        let t = market_setup(0);
        let deadline = t.list(None);
        let fee = t.fund(&t.investor, 1_000_000);
        let investor_funded = t.balance(&t.investor);

        // Fees already spent (e.g. via grants).
        let sink = Address::generate(&t.env);
        t.env.as_contract(&t.treasury, || {
            TokenClient::new(&t.env, &t.token).transfer(&t.treasury, &sink, &fee);
        });

        t.expire(deadline);
        assert!(t.mp.try_claim_refund(&t.investor, &1u64).is_err());
        assert_eq!(t.balance(&t.investor), investor_funded, "no partial refund");

        // Treasury replenished → the same investor can now claim in full.
        StellarAssetClient::new(&t.env, &t.token).mint(&t.treasury, &fee);
        t.mp.claim_refund(&t.investor, &1u64);
        assert_eq!(t.balance(&t.investor), START_BALANCE);
    }

    /// Policy: "The full fee is reversed, including any referrer-split
    /// portion". The referrer keeps its payout; the treasury bears the full
    /// clawback (it must hold enough balance — see the next test).
    #[test]
    fn clawback_refund_reverses_full_fee_including_referrer_split() {
        let t = market_setup(5_000);
        let referrer = Address::generate(&t.env);
        let deadline = t.list(Some(referrer.clone()));
        // Other fees sitting in the treasury cover the referrer portion.
        let other = t.new_investor();
        t.fund(&other, 4_000_000);

        let treasury_before = t.balance(&t.treasury);
        let referrer_before = t.balance(&referrer);
        t.fund(&t.investor, 1_000_000);
        let referrer_share = t.balance(&referrer) - referrer_before;
        assert!(referrer_share > 0, "precondition: referrer was paid a split");
        let referrer_total = t.balance(&referrer);

        t.expire(deadline);
        t.mp.claim_refund(&t.investor, &1u64);

        assert_eq!(t.balance(&t.investor), START_BALANCE, "full fee reversed");
        assert_eq!(t.balance(&referrer), referrer_total, "referrer payout is not clawed back");
        assert_eq!(
            t.balance(&t.treasury),
            treasury_before - referrer_share,
            "treasury absorbs the referrer portion"
        );
    }

    /// Negative / known limitation: with a referrer split and no other fees
    /// in the treasury, clawing back the full fee exceeds the treasury
    /// balance, so the refund reverts with `InsufficientPoolBalance`.
    #[test]
    fn clawback_refund_with_referrer_split_requires_treasury_buffer() {
        let t = market_setup(5_000);
        let referrer = Address::generate(&t.env);
        let deadline = t.list(Some(referrer.clone()));
        t.fund(&t.investor, 1_000_000);
        assert!(t.balance(&referrer) > 0);

        t.expire(deadline);
        assert!(t.mp.try_claim_refund(&t.investor, &1u64).is_err());
    }

    /// Policy (#974): token exposure is released on each `claim_refund` by
    /// the refunded gross (`net + fee`).
    #[test]
    fn clawback_refund_releases_token_exposure_by_gross() {
        let t = market_setup(0);
        let deadline = t.list(None);
        let second = t.new_investor();
        t.fund(&t.investor, 1_000_000);
        t.fund(&second, 3_000_000);

        t.expire(deadline);
        let exposure_before = t.mp.get_token_exposure_current(&t.token);
        t.mp.claim_refund(&t.investor, &1u64);
        let exposure_mid = t.mp.get_token_exposure_current(&t.token);
        assert_eq!(exposure_before - exposure_mid, 1_000_000, "released by A's gross");

        t.mp.claim_refund(&second, &1u64);
        let exposure_after = t.mp.get_token_exposure_current(&t.token);
        assert_eq!(exposure_mid - exposure_after, 3_000_000, "released by B's gross");
    }

    /// Policy: "treasury emits FEE_REV …; marketplace emits FEE_REV".
    #[test]
    fn clawback_refund_emits_fee_rev_from_treasury_and_marketplace() {
        let t = market_setup(0);
        let deadline = t.list(None);
        t.fund(&t.investor, 1_000_000);

        t.expire(deadline);
        t.mp.claim_refund(&t.investor, &1u64);

        assert!(emitted_fee_rev(&t.env, &t.treasury), "treasury FEE_REV missing");
        assert!(emitted_fee_rev(&t.env, &t.mp.address), "marketplace FEE_REV missing");
    }
}
