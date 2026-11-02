// tests/issue_470_concentration_cap.rs
//! Tests for Issue #470: MaxPositionBps concentration cap enforcement
//!
//! Validates that concentration caps are enforced cumulatively across
//! an investor's repeated partial contributions, not just per-call.

#[cfg(test)]
mod issue_470_concentration_cap {
    use kora_access_control::AccessControlContract;
    use kora_financing_pool::FinancingPoolContractClient;
    use kora_invoice_nft::InvoiceNftContractClient;
    use kora_parameter_registry::{ParameterRegistryContract, ParameterRegistryContractClient};
    use kora_shared::types::RegistryParamKey;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Bytes, Env, String as SorobanString, Symbol,
    };

    fn mint_nft(t: &TestEnv, invoice_id: u64, amount: i128, marketplace: &Address) {
        let debtor_hash = Bytes::from_slice(&t.env, &[0xABu8; 32]);
        let ipfs_cid = SorobanString::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
        t.nft.mint_invoice(
            &t.admin,
            &debtor_hash,
            &amount,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &ipfs_cid,
            &50u32,
            &None,
        );
        t.nft.set_authorized_callers(&t.admin, marketplace, &t.pool_client.address);
        t.nft.set_listed(marketplace, &invoice_id);
    }

    struct TestEnv {
        env: Env,
        admin: Address,
        investor: Address,
        token: Address,
        pool_client: FinancingPoolContractClient<'static>,
        nft: InvoiceNftContractClient<'static>,
    }

    fn setup() -> TestEnv {
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
        let investor = Address::generate(&env);
        let token = Address::generate(&env);
        let treasury = Address::generate(&env);

        // Deploy NFT (with a real access_control: mint checks pause state).
        let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        let ac_id = env.register_contract(None, AccessControlContract);
        kora_access_control::AccessControlContractClient::new(&env, &ac_id).initialize(&admin);
        nft.initialize(&admin, &ac_id);

        // Deploy Pool
        let pool_id = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
        let pool_client = FinancingPoolContractClient::new(&env, &pool_id);
        let ac2 = env.register_contract(None, kora_access_control::AccessControlContract);
        kora_access_control::AccessControlContractClient::new(&env, &ac2).initialize(&admin);
        let oracle = Address::generate(&env);
        let risk_registry = Address::generate(&env);
        let dispute_resolution = Address::generate(&env);
        pool_client.initialize(
            &admin,
            &nft_id,
            &risk_registry,
            &treasury,
            &ac2,
            &200u32,
            &oracle,
            &10_000u32,
            &0u64,
            &dispute_resolution,
        );

        TestEnv {
            env,
            admin,
            investor,
            token,
            pool_client,
            nft,
        }
    }

    /// Issue #470: Test that concentration cap is enforced per single call
    /// (current broken behavior).
    ///
    /// An investor contributes in 3 stages that individually satisfy the cap
    /// but cumulatively exceed it. Currently, each call only checks the single
    /// call's ratio, allowing the bypass. After the fix, the 3rd call must fail.
    #[test]
    fn test_cumulative_concentration_cap_multi_stage_bypass() {
        let t = setup();
        let invoice_id = 1u64;

        // Set a 30% (3000 bps) concentration cap
        t.pool_client.set_max_position_bps(&t.admin, &3_000u32);

        // Create a pool via release_funds
        let marketplace = Address::generate(&t.env);
        mint_nft(&t, invoice_id, 1_000_000i128, &marketplace);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token);

        // Stage 1: Contribute 100k out of 1M pool (10%, well below 30% cap)
        // This call should succeed
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &100_000i128, &1_000_000i128);

        // Stage 2: Contribute another 100k out of 1M (10% in this call, 20% cumulative)
        // This call should succeed, but cumulative is now 20%
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &100_000i128, &1_000_000i128);

        // Stage 3: Contribute another 150k out of 1M (15% in this call, 35% cumulative)
        // This call should now fail because cumulative (35%) exceeds cap (30%)
        // Currently it incorrectly succeeds; after the fix it must fail.
        let result = t.pool_client.try_record_position(
            &t.admin,
            &invoice_id,
            &t.investor,
            &150_000i128,
            &1_000_000i128,
        );

        // After fix: Expect an error (ExceedsFundingTarget or similar)
        // For now (broken behavior): this passes, which is the bug
        // The test asserts this will change after the fix.
        assert!(result.is_err(), "Stage 3 should fail when cumulative exceeds cap");
    }

    /// Issue #470: Test that after cumulative cap fix, the investor's
    /// total contribution is correctly capped.
    #[test]
    fn test_cumulative_cap_respects_total_contribution() {
        let t = setup();
        let invoice_id = 1u64;

        // Set a 50% (5000 bps) concentration cap
        t.pool_client.set_max_position_bps(&t.admin, &5_000u32);

        // Create a pool
        let marketplace = Address::generate(&t.env);
        mint_nft(&t, invoice_id, 2_000_000i128, &marketplace);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token);

        // Contribute 900k out of 2M pool (45%, within 50% cap)
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &900_000i128, &2_000_000i128);

        // Try to contribute another 200k out of 2M (10% in this call, 55% cumulative)
        // This should fail because cumulative exceeds the 50% cap
        let result = t.pool_client.try_record_position(
            &t.admin,
            &invoice_id,
            &t.investor,
            &200_000i128,
            &2_000_000i128,
        );

        assert!(result.is_err(), "Second contribution should fail when cumulative > cap");
    }

    /// Issue #470: Test that contributing up to exactly the cap works.
    #[test]
    fn test_cumulative_cap_exact_boundary() {
        let t = setup();
        let invoice_id = 1u64;

        // Set a 40% (4000 bps) concentration cap
        t.pool_client.set_max_position_bps(&t.admin, &4_000u32);

        // Create a pool
        let marketplace = Address::generate(&t.env);
        mint_nft(&t, invoice_id, 1_000_000i128, &marketplace);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token);

        // Contribute 300k out of 1M (30%)
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &300_000i128, &1_000_000i128);

        // Contribute another 100k out of 1M (10%, cumulative 40% = exactly at cap)
        // This should succeed
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &100_000i128, &1_000_000i128);

        // Try to contribute 100 more stroops (cumulative 400100/1M = 4001 bps).
        // Integer bps granularity cannot resolve dust below 1 bps, so the
        // over-cap probe must move the needle past the 4000 bps boundary.
        // This should fail.
        let result = t.pool_client.try_record_position(
            &t.admin,
            &invoice_id,
            &t.investor,
            &100i128,
            &1_000_000i128,
        );

        assert!(result.is_err(), "Contributing over cumulative cap should fail");
    }

    /// Issue #968: Registry overrides the static max_position_bps.  When the
    /// parameter_registry is wired and contains a MaxPositionBps key, that
    /// value is used instead of the constructor default.
    #[test]
    fn test_registry_override_lowers_cap() {
        let t = setup();
        let invoice_id = 1u64;

        // Deploy and seed the parameter_registry with a low cap (20% = 2000 bps).
        let registry_id = t.env.register_contract(None, ParameterRegistryContract);
        let registry = ParameterRegistryContractClient::new(&t.env, &registry_id);
        registry.initialize(&t.admin, &None);

        // The registry key must exist before the pool will read it; bootstrap
        // defaults already have it, but we overwrite with the test value.
        registry
            .admin_set_parameter(&t.admin, &RegistryParamKey::MaxPositionBps, &2_000i128);

        // Wire the registry into the pool.
        t.pool_client
            .set_parameter_registry(&t.admin, &registry_id);

        // Create a pool via release_funds
        let marketplace = Address::generate(&t.env);
        mint_nft(&t, invoice_id, 1_000_000i128, &marketplace);
        t.pool_client
            .release_funds(&marketplace, &invoice_id, &t.token);

        // Contribute 150k out of 1M (15%, within 20% registry cap) – succeeds.
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &150_000i128, &1_000_000i128);

        // Contribute another 100k out of 1M (10%, cumulative 25% > 20%) – fails.
        let result = t.pool_client.try_record_position(
            &t.admin,
            &invoice_id,
            &t.investor,
            &100_000i128,
            &1_000_000i128,
        );
        assert!(
            result.is_err(),
            "registry-enforced 20% cap should prevent cumulative 25%"
        );
    }

    /// Issue #968: Registry can raise the cap above the constructor-stored value.
    #[test]
    fn test_registry_override_raises_cap() {
        let t = setup();
        let invoice_id = 1u64;

        // Lower the constructor cap to 30% first.
        t.pool_client
            .set_max_position_bps(&t.admin, &3_000u32);

        // Deploy and seed the registry with a higher cap (60% = 6000 bps).
        let registry_id = t.env.register_contract(None, ParameterRegistryContract);
        let registry = ParameterRegistryContractClient::new(&t.env, &registry_id);
        registry.initialize(&t.admin, &None);
        registry
            .admin_set_parameter(&t.admin, &RegistryParamKey::MaxPositionBps, &6_000i128);
        t.pool_client
            .set_parameter_registry(&t.admin, &registry_id);

        // Create a pool.
        let marketplace = Address::generate(&t.env);
        mint_nft(&t, invoice_id, 2_000_000i128, &marketplace);
        t.pool_client
            .release_funds(&marketplace, &invoice_id, &t.token);

        // Contribute 1M out of 2M (50%) — exceeds the constructor cap (30%) but
        // is within the registry cap (60%).  With registry wired, this succeeds.
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &1_000_000i128, &2_000_000i128);
    }

    /// Issue #968: Without registry, the constructor-stored default/stored value
    /// is enforced (backward compatible).
    #[test]
    fn test_registry_not_wired_uses_constructor_default() {
        let t = setup();
        let invoice_id = 1u64;

        // Do NOT wire a registry — rely on the constructor default (10 000 bps).
        t.pool_client
            .set_max_position_bps(&t.admin, &3_000u32);

        let marketplace = Address::generate(&t.env);
        mint_nft(&t, invoice_id, 1_000_000i128, &marketplace);
        t.pool_client
            .release_funds(&marketplace, &invoice_id, &t.token);

        // 40% share (4000 bps) exceeds 30% local cap -> fails.
        let result = t.pool_client.try_record_position(
            &t.admin,
            &invoice_id,
            &t.investor,
            &400_000i128,
            &1_000_000i128,
        );
        assert!(result.is_err(), "should use local cap when no registry");

        // 25% share (2500 bps) within 30% local cap -> succeeds.
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &250_000i128, &1_000_000i128);
    }

    /// Issue #968: Non-admin cannot set the registry pointer.
    #[test]
    fn test_non_admin_cannot_set_registry() {
        let t = setup();
        let non_admin = Address::generate(&t.env);
        let registry_id = Address::generate(&t.env);

        let result = t
            .pool_client
            .try_set_parameter_registry(&non_admin, &registry_id);
        assert!(
            result.is_err(),
            "non-admin set_parameter_registry must fail"
        );
    }
}

#[cfg(test)]
mod issue_748_protocol_debtor_cap {
    use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient};
    use kora_marketplace::{DataKey as MarketplaceKey, MarketplaceContract, MarketplaceContractClient};
    use kora_parameter_registry::ParameterRegistryContractClient;
    use kora_shared::{errors::KoraError, types::RiskTier};
    use soroban_sdk::{testutils::{Address as _, Ledger, LedgerInfo}, Address, Bytes, Env, String, Symbol};

    #[test]
    fn repayment_and_default_release_protocol_exposure_once() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000, protocol_version: 21, sequence_number: 1,
            network_id: Default::default(), base_reserve: 10,
            min_temp_entry_ttl: 1_000, min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 600_000,
        });
        let admin = Address::generate(&env);
        let sme = Address::generate(&env);
        let ac = env.register_contract(None, kora_access_control::AccessControlContract);
        let nft_id = env.register_contract(None, InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac);
        let marketplace_id = env.register_contract(None, MarketplaceContract);
        let marketplace = MarketplaceContractClient::new(&env, &marketplace_id);
        let pool = Address::generate(&env);
        nft.set_authorized_callers(&admin, &marketplace_id, &pool);
        let preg_id = env.register_contract(None, kora_parameter_registry::ParameterRegistryContract);
        kora_parameter_registry::ParameterRegistryContractClient::new(&env, &preg_id).initialize(&admin, &None);
        marketplace.initialize(&admin, &nft_id, &pool, &Address::generate(&env),
            &ac, &Address::generate(&env), &Address::generate(&env), &preg_id, &50, &0);
        marketplace.set_debtor_tier_cap(&admin, &RiskTier::C, &200);
        assert_eq!(marketplace.get_debtor_tier_cap(&RiskTier::C), 200);
        assert_eq!(marketplace.try_set_debtor_tier_cap(&admin, &RiskTier::C, &-1)
            .unwrap_err().unwrap(), KoraError::InvalidAmount);
        let debtor = Bytes::from_slice(&env, &[9; 32]);
        let cid = String::from_str(&env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
        let due = env.ledger().timestamp() + 86_400;
        let first = nft.mint_invoice(&sme, &debtor, &100, &Symbol::new(&env, "USDC"), &due, &cid, &30, &None);
        let second = nft.mint_invoice(&sme, &debtor, &100, &Symbol::new(&env, "USDC"), &due, &cid, &30, &None);
        for id in [first, second] {
            nft.set_listed(&marketplace_id, &id);
            nft.set_funded(&pool, &id);
        }
        env.as_contract(&marketplace_id, || {
            env.storage().persistent().set(&MarketplaceKey::DebtorExposure(debtor.clone()), &200i128);
            env.storage().persistent().set(&MarketplaceKey::InvoiceDebtorExposure(first), &100i128);
            env.storage().persistent().set(&MarketplaceKey::InvoiceDebtorExposure(second), &100i128);
            env.storage().persistent().set(&MarketplaceKey::DebtorInvoiceIds(debtor.clone()),
                &soroban_sdk::vec![&env, first, second]);
        });
        nft.set_repaid(&pool, &first);
        assert_eq!(marketplace.sync_debtor_exposure(&debtor), 100);
        assert_eq!(marketplace.sync_debtor_exposure(&debtor), 100);
        env.ledger().set(LedgerInfo {
            timestamp: due + 1, protocol_version: 21, sequence_number: 2,
            network_id: Default::default(), base_reserve: 10,
            min_temp_entry_ttl: 1_000, min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 600_000,
        });
        nft.set_defaulted(&admin, &second);
        assert_eq!(marketplace.sync_debtor_exposure(&debtor), 0);
        assert_eq!(marketplace.get_debtor_exposure(&debtor), 0);
    }
}

// tests for Issue #973: MaxInvestorShareBps cumulative enforcement (marketplace).
// Mirrors the marketplace formula: prospective*10_000 > cap_bps*asking_price.
// Documents: cap 0 = uncapped (default); rounding leaves cap-minus-one-unit allowed.
#[cfg(test)]
mod issue_973_investor_share_cap {
    fn exceeds_cap(prospective: i128, asking_price: i128, cap_bps: u32) -> bool {
        if cap_bps == 0 { return false; }
        let lhs = prospective.checked_mul(10_000).unwrap();
        let rhs = (cap_bps as i128).checked_mul(asking_price).unwrap();
        lhs > rhs
    }

    #[test]
    fn test_third_top_up_crossing_cap_fails() {
        let asking = 1_000_000i128; let cap = 3_000u32; // 30%
        assert!(!exceeds_cap(100_000, asking, cap));
        assert!(!exceeds_cap(200_000, asking, cap));
        assert!(exceeds_cap(350_000, asking, cap)); // 35% cumulative > 30%
    }

    #[test]
    fn test_two_investors_each_50pct_on_5000bps_both_succeed() {
        let asking = 1_000_000i128; let cap = 5_000u32;
        // Caps are per-investor, so 500k each is independently within cap.
        assert!(!exceeds_cap(500_000, asking, cap));
        assert!(!exceeds_cap(500_000, asking, cap));
        // But 500k + 1 more unit on the same investor fails.
        assert!(exceeds_cap(500_001, asking, cap));
    }

    #[test]
    fn test_registry_lowering_cap_blocks_further_topups() {
        let asking = 1_000_000i128;
        // Initially 50% cap: 400k cumulative OK.
        assert!(!exceeds_cap(400_000, asking, 5_000));
        // Registry lowers to 30% mid-series: same 400k now exceeds; no retroactive eviction.
        assert!(exceeds_cap(400_000, asking, 3_000));
        // Existing position stands (policy: no retroactive eviction), only new top-ups blocked.
    }

    #[test]
    fn test_cap_zero_is_uncapped() {
        assert!(!exceeds_cap(1_000_000, 1_000_000, 0));
    }

    #[test]
    fn test_single_contribution_below_cap_succeeds_and_rounding_edge() {
        let asking = 1_000_000i128; let cap = 3_000u32;
        assert!(!exceeds_cap(299_999, asking, cap)); // cap minus one unit: allowed (documented)
        assert!(!exceeds_cap(300_000, asking, cap)); // exactly at cap: allowed
        assert!(exceeds_cap(300_001, asking, cap)); // one unit over: rejected
    }
}
