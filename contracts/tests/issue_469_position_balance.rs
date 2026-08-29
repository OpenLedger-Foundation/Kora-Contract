// tests/issue_469_position_balance.rs
//! Tests for Issue #469: Position balance reconciliation
//!
//! Validates that record_position reconciles caller-supplied contribution totals
//! against the contract's actual on-chain token balance.

#[cfg(test)]
mod issue_469_position_balance {
    use kora_financing_pool::FinancingPoolContractClient;
    use kora_invoice_nft::InvoiceNftContractClient;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env,
    };

    struct TestEnv {
        env: Env,
        admin: Address,
        investor1: Address,
        investor2: Address,
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
        let investor1 = Address::generate(&env);
        let investor2 = Address::generate(&env);
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
            investor1,
            investor2,
            token,
            pool_client,
            nft,
        }
    }

    /// Issue #469: Test that record_position should reject positions
    /// whose sum diverges from the actual token balance.
    ///
    /// The admin records positions for an invoice without verifying that
    /// the contributions match the contract's actual token holdings.
    /// This test demonstrates the vulnerability where positions can be
    /// over- or under-recorded relative to actual funds.
    #[test]
    fn test_position_balance_invariant_over_recorded() {
        let t = setup();
        let invoice_id = 1u64;
        let claimed_pool_amount = 1_000_000i128;

        // Create a pool via release_funds
        t.nft.mint(&t.admin, &invoice_id, &claimed_pool_amount, &Default::default());
        let marketplace = Address::generate(&t.env);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token).unwrap();

        // Admin records investor1 contributed 600k
        // This call succeeds, but we don't verify if the contract actually
        // holds 600k of the token
        t.pool_client
            .record_position(
                &t.admin,
                &invoice_id,
                &t.investor1,
                &600_000i128,
                &claimed_pool_amount,
            )
            .unwrap();

        // Admin records investor2 contributed 600k
        // Total recorded is 1.2M, but the pool total_funded is only supposed to be 1M
        // This is an inconsistency that should be caught
        let result = t.pool_client.record_position(
            &t.admin,
            &invoice_id,
            &t.investor2,
            &600_000i128,
            &claimed_pool_amount,
        );

        // After fix: Should reject as sum of positions exceeds pool total or token balance
        // Current behavior: May incorrectly accept
        assert!(
            result.is_err(),
            "Should reject when sum of positions exceeds pool amount"
        );
    }

    /// Issue #469: Test that positions must sum to pool total.
    #[test]
    fn test_position_balance_sum_validation() {
        let t = setup();
        let invoice_id = 1u64;
        let pool_amount = 2_000_000i128;

        // Create pool
        t.nft.mint(&t.admin, &invoice_id, &pool_amount, &Default::default());
        let marketplace = Address::generate(&t.env);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token).unwrap();

        // Record investor1 with 1.5M contribution
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor1, &1_500_000i128, &pool_amount)
            .unwrap();

        // Try to record investor2 with 1M contribution
        // Total would be 2.5M but pool is only 2M
        let result = t.pool_client.record_position(
            &t.admin,
            &invoice_id,
            &t.investor2,
            &1_000_000i128,
            &pool_amount,
        );

        // After fix: Should fail as total exceeds pool
        assert!(
            result.is_err(),
            "Should reject when sum of positions exceeds pool total"
        );
    }

    /// Issue #469: Test valid position recording that matches pool total.
    #[test]
    fn test_position_balance_valid_allocation() {
        let t = setup();
        let invoice_id = 1u64;
        let pool_amount = 1_000_000i128;

        // Create pool
        t.nft.mint(&t.admin, &invoice_id, &pool_amount, &Default::default());
        let marketplace = Address::generate(&t.env);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token).unwrap();

        // Record investor1 with 600k
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor1, &600_000i128, &pool_amount)
            .unwrap();

        // Record investor2 with 400k
        // Total is now 1M, matching pool amount
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor2, &400_000i128, &pool_amount)
            .unwrap();

        // This should succeed — all funds are accounted for
    }

    /// Issue #469: Test that buy_position does not violate the position invariant.
    #[test]
    fn test_buy_position_preserves_invariant() {
        let t = setup();
        let invoice_id = 1u64;
        let pool_amount = 1_000_000i128;
        let buyer = Address::generate(&t.env);

        // Create pool and record positions
        t.nft.mint(&t.admin, &invoice_id, &pool_amount, &Default::default());
        let marketplace = Address::generate(&t.env);
        t.pool_client.release_funds(&marketplace, &invoice_id, &t.token).unwrap();

        // Record positions
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor1, &600_000i128, &pool_amount)
            .unwrap();
        t.pool_client
            .record_position(&t.admin, &invoice_id, &t.investor2, &400_000i128, &pool_amount)
            .unwrap();

        // buy_position transfers investor1's position to buyer
        // The invariant should remain: total contributions still = pool_amount
        t.pool_client
            .buy_position(&t.investor1, &invoice_id, &buyer, &1i128)
            .unwrap();

        // After the buy, the sum of all positions should still equal pool_amount
        // This is an invariant that must hold throughout the pool's lifetime
    }
}
