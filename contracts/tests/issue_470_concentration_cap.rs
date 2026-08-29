// tests/issue_470_concentration_cap.rs
//! Tests for Issue #470: MaxPositionBps concentration cap enforcement
//!
//! Validates that concentration caps are enforced cumulatively across
//! an investor's repeated partial contributions, not just per-call.

#[cfg(test)]
mod issue_470_concentration_cap {
    use kora_financing_pool::FinancingPoolContractClient;
    use kora_invoice_nft::InvoiceNftContractClient;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env,
    };

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

        // Deploy NFT
        let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        let ac = Address::generate(&env);
        nft.initialize(&admin, &ac);

        // Deploy Pool
        let pool_id = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
        let pool_client = FinancingPoolContractClient::new(&env, &pool_id);
        let ac2 = Address::generate(&env);
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
        t.pool_client.set_max_position_bps(&t.admin, &3_000u32).unwrap();

        // Create a pool via release_funds
        t.nft.mint(&t.admin, &invoice_id, &1_000_000i128, &Default::default());
        let marketplace = Address::generate(&t.env);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token).unwrap();

        // Stage 1: Contribute 100k out of 1M pool (10%, well below 30% cap)
        // This call should succeed
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &100_000i128, &1_000_000i128)
            .unwrap();

        // Stage 2: Contribute another 100k out of 1M (10% in this call, 20% cumulative)
        // This call should succeed, but cumulative is now 20%
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &100_000i128, &1_000_000i128)
            .unwrap();

        // Stage 3: Contribute another 150k out of 1M (15% in this call, 35% cumulative)
        // This call should now fail because cumulative (35%) exceeds cap (30%)
        // Currently it incorrectly succeeds; after the fix it must fail.
        let result = t.pool_client.record_position(
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
        t.pool_client.set_max_position_bps(&t.admin, &5_000u32).unwrap();

        // Create a pool
        t.nft.mint(&t.admin, &invoice_id, &2_000_000i128, &Default::default());
        let marketplace = Address::generate(&t.env);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token).unwrap();

        // Contribute 900k out of 2M pool (45%, within 50% cap)
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &900_000i128, &2_000_000i128)
            .unwrap();

        // Try to contribute another 200k out of 2M (10% in this call, 55% cumulative)
        // This should fail because cumulative exceeds the 50% cap
        let result = t.pool_client.record_position(
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
        t.pool_client.set_max_position_bps(&t.admin, &4_000u32).unwrap();

        // Create a pool
        t.nft.mint(&t.admin, &invoice_id, &1_000_000i128, &Default::default());
        let marketplace = Address::generate(&t.env);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token).unwrap();

        // Contribute 300k out of 1M (30%)
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &300_000i128, &1_000_000i128)
            .unwrap();

        // Contribute another 100k out of 1M (10%, cumulative 40% = exactly at cap)
        // This should succeed
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor, &100_000i128, &1_000_000i128)
            .unwrap();

        // Try to contribute 1 more token (cumulative would be 40.0001%)
        // This should fail
        let result = t.pool_client.record_position(
            &t.admin,
            &invoice_id,
            &t.investor,
            &1i128,
            &1_000_000i128,
        );

        assert!(result.is_err(), "Contributing over cumulative cap should fail");
    }
}
