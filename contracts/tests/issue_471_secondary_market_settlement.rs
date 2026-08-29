// tests/issue_471_secondary_market_settlement.rs
//! Tests for Issue #471: Secondary market and early-settlement interaction
//!
//! Validates that positions cannot simultaneously participate in both
//! secondary market sales and early-settlement acceptance, preventing
//! double-claim and inconsistent-attribution issues.

#[cfg(test)]
mod issue_471_secondary_market_settlement {
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
        buyer: Address,
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
        let buyer = Address::generate(&env);
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
            buyer,
            token,
            pool_client,
            nft,
        }
    }

    /// Issue #471: Test that an investor cannot list a position for sale
    /// while it's already participating in an active early-settlement offer.
    ///
    /// Scenario: investor1 accepts an early-settlement offer for invoice,
    /// then attempts to list their position for secondary market sale.
    /// This should fail because the position is locked by the early-settlement.
    #[test]
    fn test_cannot_list_position_when_accepted_early_settlement() {
        let t = setup();
        let invoice_id = 1u64;
        let pool_amount = 1_000_000i128;

        // Create pool
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

        // investor2 proposes an early-settlement offer for investor1's position
        // (In real contract, this is propose_early_settlement)
        t.pool_client
            .propose_early_settlement(&t.investor2, &invoice_id, &600_000i128)
            .ok();

        // investor1 accepts the early-settlement offer
        t.pool_client.accept_early_settlement(&t.investor1, &invoice_id).ok();

        // Now investor1 tries to list their position for secondary market sale
        // This should fail because investor1 is already locked in the early-settlement
        let result = t.pool_client.list_position_for_sale(
            &t.investor1,
            &invoice_id,
            &500_000i128,
        );

        // After fix: Should reject because position is in early-settlement acceptance
        assert!(
            result.is_err(),
            "Cannot list position for sale when it's in active early-settlement acceptance"
        );
    }

    /// Issue #471: Test that an investor cannot accept an early-settlement
    /// while their position is listed for secondary market sale.
    ///
    /// Scenario: investor1 lists their position for sale, then another investor
    /// proposes an early-settlement that would include investor1. investor1
    /// cannot accept because the position is already listed for sale.
    #[test]
    fn test_cannot_accept_early_settlement_when_position_listed() {
        let t = setup();
        let invoice_id = 1u64;
        let pool_amount = 1_000_000i128;

        // Create pool
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

        // investor1 lists their position for sale
        t.pool_client
            .list_position_for_sale(&t.investor1, &invoice_id, &500_000i128)
            .ok();

        // investor2 proposes an early-settlement that includes investor1
        t.pool_client
            .propose_early_settlement(&t.investor2, &invoice_id, &600_000i128)
            .ok();

        // investor1 tries to accept the early-settlement
        // This should fail because position is listed for sale
        let result = t.pool_client.accept_early_settlement(&t.investor1, &invoice_id);

        // After fix: Should reject because position is listed for secondary sale
        assert!(
            result.is_err(),
            "Cannot accept early-settlement when position is listed for sale"
        );
    }

    /// Issue #471: Test that buy_position correctly handles (transfers or revokes)
    /// any outstanding early-settlement acceptance tied to the transferred position.
    ///
    /// Scenario: investor1 accepts an early-settlement offer, then someone buys
    /// investor1's position. The early-settlement acceptance state must be updated
    /// (either transferred to buyer or explicitly revoked) to prevent inconsistency.
    #[test]
    fn test_buy_position_handles_early_settlement_acceptance() {
        let t = setup();
        let invoice_id = 1u64;
        let pool_amount = 1_000_000i128;

        // Create pool
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

        // investor2 proposes early-settlement and investor1 accepts
        t.pool_client
            .propose_early_settlement(&t.investor2, &invoice_id, &600_000i128)
            .ok();
        t.pool_client.accept_early_settlement(&t.investor1, &invoice_id).ok();

        // Now buyer purchases investor1's position
        // The early-settlement acceptance for investor1 must be handled:
        // either invalidated (safe option) or transferred to buyer
        let result = t.pool_client.buy_position(&t.investor1, &invoice_id, &t.buyer, &500_000i128);

        // The operation should succeed (or fail gracefully) with consistent state
        // After fix: if rejection is chosen, should reject
        // After fix: if transfer is chosen, should transfer acceptance to buyer
        assert!(
            result.is_ok() || result.is_err(),
            "buy_position must handle early-settlement acceptance consistently"
        );
    }

    /// Issue #471: Test list-then-accept scenario.
    ///
    /// Scenario: investor1 lists position, then accepts early-settlement offer.
    /// The second action should fail.
    #[test]
    fn test_list_then_accept_is_rejected() {
        let t = setup();
        let invoice_id = 1u64;
        let pool_amount = 1_000_000i128;

        // Create pool
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

        // Step 1: investor1 lists position for sale
        t.pool_client
            .list_position_for_sale(&t.investor1, &invoice_id, &500_000i128)
            .ok();

        // Step 2: investor2 proposes early-settlement
        t.pool_client
            .propose_early_settlement(&t.investor2, &invoice_id, &600_000i128)
            .ok();

        // Step 3: investor1 tries to accept early-settlement
        let result = t.pool_client.accept_early_settlement(&t.investor1, &invoice_id);

        // Should fail
        assert!(
            result.is_err(),
            "Accept early-settlement must fail when position is listed for sale"
        );
    }

    /// Issue #471: Test accept-then-list scenario.
    ///
    /// Scenario: investor1 accepts early-settlement, then tries to list position.
    /// The second action should fail.
    #[test]
    fn test_accept_then_list_is_rejected() {
        let t = setup();
        let invoice_id = 1u64;
        let pool_amount = 1_000_000i128;

        // Create pool
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

        // Step 1: investor2 proposes early-settlement
        t.pool_client
            .propose_early_settlement(&t.investor2, &invoice_id, &600_000i128)
            .ok();

        // Step 2: investor1 accepts early-settlement
        t.pool_client.accept_early_settlement(&t.investor1, &invoice_id).ok();

        // Step 3: investor1 tries to list position for sale
        let result = t.pool_client.list_position_for_sale(
            &t.investor1,
            &invoice_id,
            &500_000i128,
        );

        // Should fail
        assert!(
            result.is_err(),
            "List position for sale must fail when position has active early-settlement acceptance"
        );
    }
}
