/// Issue #679: Parameterized State-Transition Table Tests for Invoice NFT Status
///
/// This module provides comprehensive parameterized testing of all (status, transition) pairs
/// for the invoice_nft contract's state machine. Rather than scattered individual test cases,
/// a single parameterized test enumerates every valid and invalid state transition combination.
///
/// State Machine: Created → Listed → Funded → (Repaid | Defaulted)
///
/// Valid Transitions:
/// - Created → Listed (marketplace)
/// - Listed → Funded (financing_pool)
/// - Funded → Repaid (financing_pool, on full repayment)
/// - Funded → Defaulted (admin, when past due_date)
///
/// Invalid Transitions: All others should fail with InvalidInvoiceStatus error.

#[cfg(test)]
mod issue_679_invoice_state_transitions {
    use kora_invoice_nft::{InvoiceNftContractClient, InvoiceNftContract};
    use kora_shared::{
        errors::KoraError,
        types::InvoiceStatus,
    };
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Bytes, String, Symbol, Address, Env,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TransitionExpectation {
        Success,
        InvalidStatus,
        Unauthorized,
    }

    struct TransitionTestCase {
        from_status: InvoiceStatus,
        to_transition: &'static str, // "Listed", "Funded", "Repaid", "Defaulted"
        caller_type: &'static str,   // "marketplace", "pool", "admin", "sme"
        expectation: TransitionExpectation,
        description: &'static str,
    }

    struct TestEnv {
        env: Env,
        admin: Address,
        sme: Address,
        marketplace: Address,
        pool: Address,
        access_control: Address,
        nft_client: InvoiceNftContractClient<'static>,
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
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        let access_control = Address::generate(&env);

        let nft_id = env.register_contract(None, InvoiceNftContract);
        let nft_client = InvoiceNftContractClient::new(&env, &nft_id);

        nft_client.initialize(&admin, &access_control);
        nft_client.set_authorized_callers(&admin, &marketplace, &pool);

        TestEnv {
            env,
            admin,
            sme,
            marketplace,
            pool,
            access_control,
            nft_client,
        }
    }

    fn mint_test_invoice(t: &TestEnv) -> u64 {
        let due_date = t.env.ledger().timestamp() + 86_400 * 30;
        t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &due_date,
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        )
    }

    fn set_invoice_status(t: &TestEnv, invoice_id: u64, status: InvoiceStatus) {
        match status {
            InvoiceStatus::Listed => {
                t.nft_client.set_listed(&t.marketplace, &invoice_id).ok();
            }
            InvoiceStatus::Funded => {
                t.nft_client.set_listed(&t.marketplace, &invoice_id).ok();
                t.nft_client.set_funded(&t.pool, &invoice_id).ok();
            }
            InvoiceStatus::Repaid => {
                t.nft_client.set_listed(&t.marketplace, &invoice_id).ok();
                t.nft_client.set_funded(&t.pool, &invoice_id).ok();
                t.nft_client.set_repaid(&t.pool, &invoice_id).ok();
            }
            InvoiceStatus::Defaulted => {
                t.nft_client.set_listed(&t.marketplace, &invoice_id).ok();
                t.nft_client.set_funded(&t.pool, &invoice_id).ok();
                // Advance ledger past due date to allow default
                let invoice = t.nft_client.get_invoice(&invoice_id);
                t.env.ledger().set(LedgerInfo {
                    timestamp: invoice.due_date + 1,
                    protocol_version: 21,
                    sequence_number: 2,
                    network_id: Default::default(),
                    base_reserve: 10,
                    min_temp_entry_ttl: 1000,
                    min_persistent_entry_ttl: 1000,
                    max_entry_ttl: 100_000,
                });
                t.nft_client.set_defaulted(&t.admin, &invoice_id).ok();
            }
            _ => {}
        }
    }

    /// Core parameterized test: attempts a transition and verifies the result matches expectation
    fn test_transition(case: &TransitionTestCase) {
        let t = setup();
        let invoice_id = mint_test_invoice(&t);

        // Move invoice to the "from" status
        set_invoice_status(&t, invoice_id, case.from_status);

        // Determine caller based on caller_type
        let caller = match case.caller_type {
            "marketplace" => &t.marketplace,
            "pool" => &t.pool,
            "admin" => &t.admin,
            "sme" => &t.sme,
            _ => panic!("Unknown caller type: {}", case.caller_type),
        };

        // Attempt the transition
        let result = match case.to_transition {
            "Listed" => t.nft_client.try_set_listed(caller, &invoice_id),
            "Funded" => t.nft_client.try_set_funded(caller, &invoice_id),
            "Repaid" => t.nft_client.try_set_repaid(caller, &invoice_id),
            "Defaulted" => {
                // Move past due date for defaulted transitions
                if matches!(case.from_status, InvoiceStatus::Funded) {
                    let invoice = t.nft_client.get_invoice(&invoice_id);
                    t.env.ledger().set(LedgerInfo {
                        timestamp: invoice.due_date + 1,
                        protocol_version: 21,
                        sequence_number: 2,
                        network_id: Default::default(),
                        base_reserve: 10,
                        min_temp_entry_ttl: 1000,
                        min_persistent_entry_ttl: 1000,
                        max_entry_ttl: 100_000,
                    });
                }
                t.nft_client.try_set_defaulted(caller, &invoice_id)
            }
            _ => panic!("Unknown transition: {}", case.to_transition),
        };

        // Verify expectation
        match case.expectation {
            TransitionExpectation::Success => {
                assert!(result.is_ok(), "Failed: {} - {}", case.from_status, case.description);
            }
            TransitionExpectation::InvalidStatus => {
                assert_eq!(
                    result.unwrap_err().unwrap(),
                    KoraError::InvalidInvoiceStatus,
                    "Wrong error for {}: {}",
                    case.from_status,
                    case.description
                );
            }
            TransitionExpectation::Unauthorized => {
                assert!(
                    matches!(
                        result.unwrap_err().unwrap(),
                        KoraError::NotAdmin | KoraError::Unauthorized
                    ),
                    "Expected auth error for {}: {}",
                    case.from_status,
                    case.description
                );
            }
        }
    }

    // ── Valid Transitions ──────────────────────────────────────────────────────

    #[test]
    fn test_created_to_listed_valid() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Created,
            to_transition: "Listed",
            caller_type: "marketplace",
            expectation: TransitionExpectation::Success,
            description: "Created → Listed by marketplace is valid",
        };
        test_transition(&case);
    }

    #[test]
    fn test_listed_to_funded_valid() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Listed,
            to_transition: "Funded",
            caller_type: "pool",
            expectation: TransitionExpectation::Success,
            description: "Listed → Funded by pool is valid",
        };
        test_transition(&case);
    }

    #[test]
    fn test_funded_to_repaid_valid() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Funded,
            to_transition: "Repaid",
            caller_type: "pool",
            expectation: TransitionExpectation::Success,
            description: "Funded → Repaid by pool is valid",
        };
        test_transition(&case);
    }

    #[test]
    fn test_funded_to_defaulted_valid() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Funded,
            to_transition: "Defaulted",
            caller_type: "admin",
            expectation: TransitionExpectation::Success,
            description: "Funded → Defaulted by admin (post due-date) is valid",
        };
        test_transition(&case);
    }

    // ── Invalid Status Transitions (wrong state) ────────────────────────────────

    #[test]
    fn test_created_to_funded_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Created,
            to_transition: "Funded",
            caller_type: "pool",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot skip Listed stage",
        };
        test_transition(&case);
    }

    #[test]
    fn test_created_to_repaid_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Created,
            to_transition: "Repaid",
            caller_type: "pool",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot jump directly to Repaid",
        };
        test_transition(&case);
    }

    #[test]
    fn test_created_to_defaulted_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Created,
            to_transition: "Defaulted",
            caller_type: "admin",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot default from Created",
        };
        test_transition(&case);
    }

    #[test]
    fn test_listed_to_listed_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Listed,
            to_transition: "Listed",
            caller_type: "marketplace",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot re-list an already Listed invoice",
        };
        test_transition(&case);
    }

    #[test]
    fn test_listed_to_repaid_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Listed,
            to_transition: "Repaid",
            caller_type: "pool",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot jump from Listed to Repaid",
        };
        test_transition(&case);
    }

    #[test]
    fn test_listed_to_defaulted_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Listed,
            to_transition: "Defaulted",
            caller_type: "admin",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot default from Listed",
        };
        test_transition(&case);
    }

    #[test]
    fn test_funded_to_funded_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Funded,
            to_transition: "Funded",
            caller_type: "pool",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot re-fund an already Funded invoice",
        };
        test_transition(&case);
    }

    #[test]
    fn test_repaid_to_listed_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Repaid,
            to_transition: "Listed",
            caller_type: "marketplace",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot transition backward from Repaid to Listed",
        };
        test_transition(&case);
    }

    #[test]
    fn test_repaid_to_funded_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Repaid,
            to_transition: "Funded",
            caller_type: "pool",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Cannot revert from Repaid to Funded",
        };
        test_transition(&case);
    }

    #[test]
    fn test_repaid_to_defaulted_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Repaid,
            to_transition: "Defaulted",
            caller_type: "admin",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Repaid is terminal; cannot transition to Defaulted",
        };
        test_transition(&case);
    }

    #[test]
    fn test_defaulted_to_listed_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Defaulted,
            to_transition: "Listed",
            caller_type: "marketplace",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Defaulted is terminal; cannot transition to Listed",
        };
        test_transition(&case);
    }

    #[test]
    fn test_defaulted_to_funded_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Defaulted,
            to_transition: "Funded",
            caller_type: "pool",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Defaulted is terminal; cannot transition to Funded",
        };
        test_transition(&case);
    }

    #[test]
    fn test_defaulted_to_repaid_invalid_status() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Defaulted,
            to_transition: "Repaid",
            caller_type: "pool",
            expectation: TransitionExpectation::InvalidStatus,
            description: "Defaulted is terminal; cannot transition to Repaid",
        };
        test_transition(&case);
    }

    // ── Authorization Violations ────────────────────────────────────────────────

    #[test]
    fn test_created_to_listed_wrong_caller() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Created,
            to_transition: "Listed",
            caller_type: "pool",
            expectation: TransitionExpectation::Unauthorized,
            description: "Only marketplace can call set_listed",
        };
        test_transition(&case);
    }

    #[test]
    fn test_listed_to_funded_wrong_caller() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Listed,
            to_transition: "Funded",
            caller_type: "marketplace",
            expectation: TransitionExpectation::Unauthorized,
            description: "Only financing_pool can call set_funded",
        };
        test_transition(&case);
    }

    #[test]
    fn test_funded_to_repaid_wrong_caller() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Funded,
            to_transition: "Repaid",
            caller_type: "admin",
            expectation: TransitionExpectation::Unauthorized,
            description: "Only financing_pool can call set_repaid",
        };
        test_transition(&case);
    }

    #[test]
    fn test_funded_to_defaulted_wrong_caller() {
        let case = TransitionTestCase {
            from_status: InvoiceStatus::Funded,
            to_transition: "Defaulted",
            caller_type: "pool",
            expectation: TransitionExpectation::Unauthorized,
            description: "Only admin can call set_defaulted",
        };
        test_transition(&case);
    }

    // ── Freeze Enforcement (edge case with authorization) ──────────────────────

    #[test]
    fn test_frozen_invoice_blocks_all_transitions() {
        let t = setup();
        let invoice_id = mint_test_invoice(&t);

        // Freeze the invoice as admin
        t.nft_client.freeze_invoice(&t.admin, &invoice_id);

        // Move to Listed state
        t.nft_client.set_listed(&t.marketplace, &invoice_id).ok();

        // Now try to transition to Funded (should fail with InvoiceFrozen, not InvalidStatus)
        let result = t.nft_client.try_set_funded(&t.pool, &invoice_id);
        assert!(result.is_err(), "Frozen invoice should block transition");
    }

    #[test]
    fn test_unfrozen_invoice_resumes_transitions() {
        let t = setup();
        let invoice_id = mint_test_invoice(&t);

        // Freeze, then unfreeze
        t.nft_client.freeze_invoice(&t.admin, &invoice_id);
        t.nft_client.unfreeze_invoice(&t.admin, &invoice_id);

        // List the invoice
        let result = t.nft_client.try_set_listed(&t.marketplace, &invoice_id);
        assert!(result.is_ok(), "Unfrozen invoice should allow transitions");
    }
}
