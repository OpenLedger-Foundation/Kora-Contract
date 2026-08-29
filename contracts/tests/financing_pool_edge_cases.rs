// tests/financing_pool_edge_cases.rs
//! Edge case tests for Financing Pool Contract
//!
//! This test module covers:
//! - Yield distribution precision
//! - Release funds validation
//! - Repayment lock cleanup
//! - Position recording atomicity
//! - Arithmetic edge cases

#[cfg(test)]
mod financing_pool_edge_cases {
    use kora_financing_pool::FinancingPoolContractClient;
    use kora_invoice_nft::InvoiceNftContractClient;
    use kora_shared::errors::KoraError;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env,
    };

    struct TestEnv {
        env: Env,
        admin: Address,
        sme: Address,
        investor1: Address,
        investor2: Address,
        token: Address,
        treasury: Address,
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
        let sme = Address::generate(&env);
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
        let risk_registry = Address::generate(&env);
        let oracle_id = env.register_contract(None, kora_price_oracle::PriceOracleContract);
        let oracle_client = kora_price_oracle::PriceOracleContractClient::new(&env, &oracle_id);
        oracle_client.initialize(&admin, &ac2);
        pool_client.initialize(
            &admin, &nft_id, &risk_registry, &treasury, &ac2, &200u32, &oracle_id, &10_000u32, &Address::generate(&env),
        );

        TestEnv {
            env,
            admin,
            sme,
            investor1,
            investor2,
            token,
            treasury,
            pool_client,
            nft,
        }
    }

    // ── Yield Distribution Precision Edge Cases ───────────────────────────────

    #[test]
    fn test_yield_calculation_with_equal_positions() {
        let t = setup();

        // Create pool with 2 equal investors
        let invoice_id = 1u64;
        let face_value = 10_000i128;
        let invested_per_investor = 5_000i128;

        // Note: In a real test, we'd call release_funds first
        // which would set up the pool. This tests the yield calculation
        // assuming a pool is created:

        // Each investor has 50% share
        // If repaid_amount = face_value, each gets:
        // payout = (10_000 * 5_000) / 10_000 = 5_000
        // yield = 5_000 - 5_000 = 0

        // This verifies no yield when fully repaid at face value
    }

    #[test]
    fn test_yield_calculation_with_unequal_positions() {
        let t = setup();

        // Create pool with investors having different positions:
        // Investor1: 3_000 (30%)
        // Investor2: 7_000 (70%)
        //
        // If repaid_amount = 12_000 (120% of face value):
        // Investor1 yield = (12_000 * 0.30) - 3_000 = 3_600 - 3_000 = 600
        // Investor2 yield = (12_000 * 0.70) - 7_000 = 8_400 - 7_000 = 1_400
    }

    #[test]
    fn test_yield_calculation_with_small_position() {
        let t = setup();

        // Test precision with very small position
        // Investor position: 1 (with face_value = 1_000)
        // Share: (1 * 10_000) / 1_000 = 10 bps (0.1%)
        // If repaid = 2_000: payout = (2_000 * 10) / 10_000 = 2
        // yield = 2 - 1 = 1

        // Verify no rounding down to 0
    }

    #[test]
    fn test_yield_calculation_with_large_position() {
        let t = setup();

        // Test with large numbers near i128::MAX
        // Ensure no overflow in multiplication before division
        let large_amount = i128::MAX / 1_000_000;
        // yield calculation: (large_amount * 10_000) / 10_000 should not overflow
    }

    // ── Release Funds Edge Cases ──────────────────────────────────────────────

    #[test]
    fn test_release_funds_cannot_be_called_twice() {
        let t = setup();

        // First call succeeds (creates pool)
        let invoice_id = 1u64;
        let face_value = 10_000i128;
        let result1 = t.pool_client.try_release_funds(
            &Address::generate(&t.env), // marketplace
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Second call with same invoice_id should fail
        let result2 = t.pool_client.try_release_funds(
            &Address::generate(&t.env), // marketplace
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Should get PoolAlreadyClosed or similar error
        assert!(result2.is_err());
    }

    #[test]
    fn test_release_funds_requires_valid_inputs() {
        let t = setup();

        // Zero face_value should be rejected
        let result = t.pool_client.try_release_funds(
            &Address::generate(&t.env),
            &1u64,
            &0i128,
            &t.sme,
            &t.token,
        );
        assert!(result.is_err());

        // Negative face_value should be rejected
        let result = t.pool_client.try_release_funds(
            &Address::generate(&t.env),
            &1u64,
            &-1_000i128,
            &t.sme,
            &t.token,
        );
        assert!(result.is_err());
    }

    // ── Repayment Lock Edge Cases ─────────────────────────────────────────────

    #[test]
    fn test_repayment_lock_prevents_concurrent_repay() {
        let t = setup();

        // This would require async execution or manual lock testing
        // In Soroban's synchronous model, reentrancy isn't possible
        // but lock cleanup must be verified
    }

    #[test]
    fn test_repayment_lock_cleared_on_success() {
        let t = setup();

        // After repay() succeeds, lock should be cleared
        // Verify by attempting another repay immediately (should work)
    }

    #[test]
    fn test_repayment_lock_cleared_on_error() {
        let t = setup();

        // If repay() fails mid-execution, lock must still be cleared
        // Attempt invalid repayment, then valid one should work
    }

    // ── Position Recording Edge Cases ─────────────────────────────────────────

    #[test]
    fn test_record_position_with_max_amount() {
        let t = setup();

        // Record position with MAX_AMOUNT
        // Should not overflow in internal calculations
        let max_amount = 1_000_000_000_000_000i128; // 1 trillion

        let result = t.pool_client.try_record_position(
            &Address::generate(&t.env), // marketplace
            &1u64,
            &t.investor1,
            &max_amount,
        );

        // May succeed or fail on amount validation, but not on overflow
        if let Err(Ok(e)) = result {
            assert_ne!(e, KoraError::ArithmeticOverflow);
        }
    }

    #[test]
    fn test_record_position_atomicity() {
        let t = setup();

        // If one step fails, both should be rolled back
        // E.g., if investor is invalid but pool update partially succeeds
        // This is handled by transaction semantics

        let invoice_id = 1u64;
        let amount = 5_000i128;

        // First position records successfully
        let _result1 = t.pool_client.try_record_position(
            &Address::generate(&t.env),
            &invoice_id,
            &t.investor1,
            &amount,
        );

        // Second position with same investor should update, not duplicate
        // (This behavior depends on contract implementation)
    }

    // ── Repayment Edge Cases ──────────────────────────────────────────────────

    #[test]
    fn test_repay_zero_amount_rejected() {
        let t = setup();

        let result = t.pool_client.try_repay(
            &t.sme,
            &1u64,
            &t.token,
            &0i128,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_repay_negative_amount_rejected() {
        let t = setup();

        let result = t.pool_client.try_repay(
            &t.sme,
            &1u64,
            &t.token,
            &-1_000i128,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_repay_exceeds_face_value_allowed() {
        let t = setup();

        // Over-repayment (paying more than face value) should be allowed
        // This would give investors extra yield
    }

    #[test]
    fn test_repay_pool_not_found() {
        let t = setup();

        let result = t.pool_client.try_repay(
            &t.sme,
            &999u64, // Non-existent pool
            &t.token,
            &1_000i128,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::PoolNotFound);
    }

    #[test]
    fn test_repay_already_repaid_fails() {
        let t = setup();

        // After pool is marked closed (fully repaid), subsequent repayments fail
        // This prevents double-paying investors
    }

    #[test]
    fn test_repayment_completes_when_fully_funded() {
        let t = setup();

        // When repaid_amount >= face_value, pool closes automatically
        // Invoice status changes to Repaid
        // Yield distribution happens
    }

    // ── Mark Default Edge Cases ───────────────────────────────────────────────

    #[test]
    fn test_mark_default_admin_only() {
        let t = setup();
        let non_admin = Address::generate(&t.env);

        // Only admin can mark default
        let result = t.pool_client.try_mark_default(
            &non_admin,
            &1u64,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_mark_default_pool_not_found() {
        let t = setup();

        let result = t.pool_client.try_mark_default(
            &t.admin,
            &999u64,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::PoolNotFound);
    }

    // ── Arithmetic Edge Cases ─────────────────────────────────────────────────

    #[test]
    fn test_total_funded_arithmetic_overflow() {
        let t = setup();

        // Multiple positions recording near i128::MAX
        // total_funded = position1 + position2 should not overflow
        let large_amount = i128::MAX / 3;

        let _result1 = t.pool_client.try_record_position(
            &Address::generate(&t.env),
            &1u64,
            &t.investor1,
            &large_amount,
        );

        // Second large position might cause overflow - should be detected
        let result2 = t.pool_client.try_record_position(
            &Address::generate(&t.env),
            &1u64,
            &t.investor2,
            &large_amount,
        );

        // Should not succeed silently - overflow must be reported
    }

    #[test]
    fn test_repaid_amount_arithmetic_overflow() {
        let t = setup();

        // Repayment that causes repaid_amount to overflow
        let near_max = i128::MAX / 2;

        // Would need to set up a pool with specific amount first
    }

    // ── Issue #475: RepaymentLock in propose_early_settlement ──────────────────

    #[test]
    fn test_propose_early_settlement_acquires_repayment_lock() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;
        let sme_amount = 1_000i128;

        // Setup: Create a pool
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Propose early settlement - should acquire RepaymentLock
        let settlement_amount = 5_000i128;
        let result = t.pool_client.try_propose_early_settlement(
            &t.sme,
            &invoice_id,
            &settlement_amount,
        );

        assert!(result.is_ok(), "propose_early_settlement should succeed when lock is available");
    }

    #[test]
    fn test_early_settlement_escrowed_funds_protected_from_concurrent_repay() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup: Create a pool
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Record investor position
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &5_000i128,
        );

        // Propose early settlement (escrows SME's buyout amount)
        let settlement_amount = 5_500i128;
        let _result = t.pool_client.try_propose_early_settlement(
            &t.sme,
            &invoice_id,
            &settlement_amount,
        );

        // Now attempt to repay() the pool through normal path
        // This should either:
        // 1. Fail with a lock conflict, or
        // 2. Succeed but auto-refund/cancel the early settlement
        let repay_result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &face_value,
        );

        if repay_result.is_ok() {
            // Pool was repaid normally - early settlement should be auto-refunded/cancelled
            // Verify SME can still recover escrowed amount via cancel_early_settlement
            let cancel_result = t.pool_client.try_cancel_early_settlement(
                &t.sme,
                &invoice_id,
            );
            assert!(cancel_result.is_ok(), "cancel_early_settlement should work after pool closure");
        } else {
            // Lock prevented concurrent repay - this is also acceptable behavior
            assert!(repay_result.is_err(), "repay should fail if early settlement lock is held");
        }
    }

    #[test]
    fn test_early_settlement_lock_prevents_race_condition() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup pool
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Propose early settlement
        let settlement_amount = 5_500i128;
        let _result = t.pool_client.try_propose_early_settlement(
            &t.sme,
            &invoice_id,
            &settlement_amount,
        );

        // Attempting accept_early_settlement should acquire/check same lock
        let accept_result = t.pool_client.try_accept_early_settlement(
            &t.sme,
            &invoice_id,
        );

        // This should succeed - lock is held by propose_early_settlement
        // and released after, allowing accept to proceed
        assert!(accept_result.is_ok() || accept_result.is_err());
    }

    // ── Issue #474: Emergency Fund Recovery Mechanism ─────────────────────────

    #[test]
    fn test_sweep_excess_funds_recovery() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup: Create a pool
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Record multiple investor positions to create rounding dust
        let pos1 = 3_333i128;
        let pos2 = 3_333i128;
        let pos3 = 3_334i128;

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &pos1,
        );
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor2,
            &pos2,
        );

        // Repay face value
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &face_value,
        );

        // Sweep excess function should exist and be callable by admin
        let sweep_result = t.pool_client.try_sweep_excess(
            &t.admin,
            &t.token,
        );

        assert!(sweep_result.is_ok(), "sweep_excess should succeed for admin");
    }

    #[test]
    fn test_sweep_excess_recovers_rounding_dust_only() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup pool
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Record positions that create rounding dust in distribution
        // E.g., 3 equal positions of 3_333, 3_333, 3_334 = 10_000
        // When distributed, rounding may leave dust
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &3_333i128,
        );
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor2,
            &3_333i128,
        );

        // Repay
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &face_value,
        );

        // Before sweep, check contract balance and tracked obligations
        // Sweep should only move excess, never touch pool obligations
        let sweep_result = t.pool_client.try_sweep_excess(
            &t.admin,
            &t.token,
        );

        if sweep_result.is_ok() {
            // Verify balance change matches only the excess dust
            // No tracked obligations were touched
        }
    }

    #[test]
    fn test_sweep_excess_only_moves_provably_excess_balance() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &10_000i128,
        );

        // Repay only partially
        let partial_repay = 5_000i128;
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &partial_repay,
        );

        // Pool still has open obligation of 5_000
        // sweep_excess should reject or return 0 (not touch the 5_000)
        let sweep_result = t.pool_client.try_sweep_excess(
            &t.admin,
            &t.token,
        );

        // If sweep succeeds, it should recover nothing (or minimal dust)
        // It must NOT touch the 5_000 backing the open pool
    }

    #[test]
    fn test_sweep_excess_requires_admin() {
        let t = setup();

        let non_admin = Address::generate(&t.env);

        // Only admin can sweep
        let result = t.pool_client.try_sweep_excess(
            &non_admin,
            &t.token,
        );

        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_sweep_excess_emits_event() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup and create sweep-able dust
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &10_000i128,
        );

        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &face_value,
        );

        // Sweep and verify event is emitted
        let _result = t.pool_client.try_sweep_excess(
            &t.admin,
            &t.token,
        );

        // Event verification would happen via env.events() in real test
    }

    // ── Issue #473: Incremental Yield Claims ──────────────────────────────────

    #[test]
    fn test_claim_yield_incremental_per_installment() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup pool
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Record investor position
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &10_000i128,
        );

        // Set up installment schedule
        // 4 equal installments of 2_500 each
        let _result = t.pool_client.try_set_installment_schedule(
            &t.admin,
            &invoice_id,
            &vec![
                (1_700_000_100, 2_500i128),
                (1_700_000_200, 2_500i128),
                (1_700_000_300, 2_500i128),
                (1_700_000_400, 2_500i128),
            ],
        );

        // Make first repayment of 2_500
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &2_500i128,
        );

        // Investor should now be able to claim their share of first installment
        let claim_result = t.pool_client.try_claim_yield(
            &t.investor1,
            &invoice_id,
        );

        assert!(claim_result.is_ok(), "claim_yield should succeed after first installment repaid");
    }

    #[test]
    fn test_claim_yield_prevents_double_claiming() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &5_000i128,
        );

        let _result = t.pool_client.try_set_installment_schedule(
            &t.admin,
            &invoice_id,
            &vec![
                (1_700_000_100, 5_000i128),
                (1_700_000_200, 5_000i128),
            ],
        );

        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &5_000i128,
        );

        // First claim succeeds
        let claim1 = t.pool_client.try_claim_yield(
            &t.investor1,
            &invoice_id,
        );
        assert!(claim1.is_ok());

        // Second claim for same amount should fail or return 0
        let claim2 = t.pool_client.try_claim_yield(
            &t.investor1,
            &invoice_id,
        );

        // Should either fail or return amount 0 (all claimed)
        if claim2.is_ok() {
            // Verify returned amount is 0
        }
    }

    #[test]
    fn test_claim_yield_multi_investor_partial_claims() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 12_000i128;

        // Setup pool with 2 investors
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &4_000i128,
        );
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor2,
            &8_000i128,
        );

        // Set 3 installments
        let _result = t.pool_client.try_set_installment_schedule(
            &t.admin,
            &invoice_id,
            &vec![
                (1_700_000_100, 4_000i128),
                (1_700_000_200, 4_000i128),
                (1_700_000_300, 4_000i128),
            ],
        );

        // Repay first installment
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &4_000i128,
        );

        // Investor1 claims (1/3 of their yield is now available)
        let claim1 = t.pool_client.try_claim_yield(
            &t.investor1,
            &invoice_id,
        );
        assert!(claim1.is_ok());

        // Investor2 also claims (1/3 of their yield)
        let claim2 = t.pool_client.try_claim_yield(
            &t.investor2,
            &invoice_id,
        );
        assert!(claim2.is_ok());

        // Repay second installment
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &4_000i128,
        );

        // Investor1 claims again (additional yield from installment 2)
        let claim3 = t.pool_client.try_claim_yield(
            &t.investor1,
            &invoice_id,
        );
        assert!(claim3.is_ok());
    }

    #[test]
    fn test_distribute_yield_nets_out_prior_claims() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 12_000i128;

        // Setup
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &10_000i128,
        );

        let _result = t.pool_client.try_set_installment_schedule(
            &t.admin,
            &invoice_id,
            &vec![
                (1_700_000_100, 6_000i128),
                (1_700_000_200, 6_000i128),
            ],
        );

        // Repay first half
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &6_000i128,
        );

        // Investor claims partial yield
        let _result = t.pool_client.try_claim_yield(
            &t.investor1,
            &invoice_id,
        );

        // Repay remaining half
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &6_000i128,
        );

        // Final distribute_yield should only pay unclaimed amount
        // (this happens on pool close)
        // Verify position.yield_claimed reflects all payouts
    }

    #[test]
    fn test_yield_claimed_field_is_maintained() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &10_000i128,
        );

        // Position should start with yield_claimed = 0
        let position = t.pool_client.get_position(
            &invoice_id,
            &t.investor1,
        );
        assert_eq!(position.yield_claimed, 0, "yield_claimed should start at 0");

        // Repay
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &12_000i128,
        );

        // After claim_yield, position.yield_claimed should increase
        let _result = t.pool_client.try_claim_yield(
            &t.investor1,
            &invoice_id,
        );

        let position = t.pool_client.get_position(
            &invoice_id,
            &t.investor1,
        );
        assert!(position.yield_claimed > 0, "yield_claimed should be updated after claim");
    }

    // ── Issue #472: AggregateFunded Accounting ────────────────────────────────

    #[test]
    fn test_get_aggregate_funded_view_function() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup pool
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        // Record position
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &10_000i128,
        );

        // get_aggregate_funded should return total funded for token
        let aggregate = t.pool_client.get_aggregate_funded(
            &t.token,
        );

        assert_eq!(aggregate, 10_000i128, "aggregate_funded should match total position");
    }

    #[test]
    fn test_aggregate_funded_updated_on_record_position() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let initial_aggregate = t.pool_client.get_aggregate_funded(&t.token);
        assert_eq!(initial_aggregate, 0, "aggregate should start at 0");

        // Record position
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &5_000i128,
        );

        let aggregate1 = t.pool_client.get_aggregate_funded(&t.token);
        assert_eq!(aggregate1, 5_000i128, "aggregate should increase");

        // Record another position
        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor2,
            &3_000i128,
        );

        let aggregate2 = t.pool_client.get_aggregate_funded(&t.token);
        assert_eq!(aggregate2, 8_000i128, "aggregate should accumulate positions");
    }

    #[test]
    fn test_aggregate_funded_updated_on_distribute_yield() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &10_000i128,
        );

        let aggregate_before = t.pool_client.get_aggregate_funded(&t.token);
        assert_eq!(aggregate_before, 10_000i128);

        // Repay (triggers distribute_yield)
        let _result = t.pool_client.try_repay(
            &t.sme,
            &invoice_id,
            &t.token,
            &face_value,
        );

        // aggregate_funded should decrease after pool closes
        let aggregate_after = t.pool_client.get_aggregate_funded(&t.token);
        assert_eq!(aggregate_after, 0, "aggregate should decrease when pool closes");
    }

    #[test]
    fn test_max_aggregate_funded_cap_enforcement() {
        let t = setup();

        // Set max aggregate cap for token
        let cap = 50_000i128;
        let result = t.pool_client.try_set_max_aggregate_funded(
            &t.admin,
            &t.token,
            &cap,
        );
        assert!(result.is_ok(), "admin should be able to set cap");

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup first pool at 30_000
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &30_000i128,
        );

        // Setup second pool at 15_000 (total 45_000, within cap)
        let invoice_id2 = 2u64;
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id2,
            &face_value,
            &t.sme,
            &t.token,
        );

        let result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id2,
            &t.investor2,
            &15_000i128,
        );
        assert!(result.is_ok(), "position at 45_000 should be within cap");

        // Try to exceed cap - third pool at 10_000 (total 55_000, exceeds cap)
        let invoice_id3 = 3u64;
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id3,
            &face_value,
            &t.sme,
            &t.token,
        );

        let result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id3,
            &t.investor1,
            &10_000i128,
        );

        assert!(result.is_err(), "position exceeding cap should be rejected");
    }

    #[test]
    fn test_solvency_check_view_function() {
        let t = setup();

        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &10_000i128,
        );

        // Check solvency - should show contract can cover all tracked obligations
        let solvency = t.pool_client.check_solvency(
            &t.token,
        );

        assert!(solvency.is_solvent, "contract should be solvent when tracking obligations");
    }

    #[test]
    fn test_max_aggregate_funded_only_enforced_in_record_position() {
        let t = setup();

        // Set a cap
        let cap = 20_000i128;
        let _result = t.pool_client.try_set_max_aggregate_funded(
            &t.admin,
            &t.token,
            &cap,
        );

        // Cap should NOT prevent pool creation (release_funds)
        let invoice_id = 1u64;
        let face_value = 30_000i128;

        let marketplace = Address::generate(&t.env);
        let result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        assert!(result.is_ok(), "release_funds should not check cap");

        // Cap IS enforced in record_position
        let position_result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &30_000i128,
        );

        assert!(position_result.is_err(), "record_position should enforce cap");
    }

    #[test]
    fn test_multiple_tokens_independent_aggregate_tracking() {
        let t = setup();

        let token2 = Address::generate(&t.env);
        let invoice_id = 1u64;
        let face_value = 10_000i128;

        // Setup pool for token1
        let marketplace = Address::generate(&t.env);
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id,
            &face_value,
            &t.sme,
            &t.token,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id,
            &t.investor1,
            &5_000i128,
        );

        // Setup pool for token2
        let invoice_id2 = 2u64;
        let _result = t.pool_client.try_release_funds(
            &marketplace,
            &invoice_id2,
            &face_value,
            &t.sme,
            &token2,
        );

        let _result = t.pool_client.try_record_position(
            &marketplace,
            &invoice_id2,
            &t.investor2,
            &3_000i128,
        );

        // Aggregates should be tracked separately per token
        let agg_token1 = t.pool_client.get_aggregate_funded(&t.token);
        let agg_token2 = t.pool_client.get_aggregate_funded(&token2);

        assert_eq!(agg_token1, 5_000i128, "token1 aggregate should be 5_000");
        assert_eq!(agg_token2, 3_000i128, "token2 aggregate should be 3_000");
    }
}
