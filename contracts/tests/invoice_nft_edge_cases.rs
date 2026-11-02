// tests/invoice_nft_edge_cases.rs
//! Edge case tests for Invoice NFT Contract
//!
//! This test module covers:
//! - Status transition validation
//! - Immutability enforcement
//! - ID overflow prevention
//! - Migration logic
//! - Authorization checks

#[cfg(test)]
mod invoice_nft_edge_cases {
    use kora_invoice_nft::{InvoiceNftContractClient, InvoiceNftContract};
    use kora_shared::{
        errors::KoraError,
        types::InvoiceStatus,
    };
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Bytes, String, Symbol, Address, Env,
    };

    struct TestEnv {
        env: Env,
        admin: Address,
        sme: Address,
        marketplace: Address,
        pool: Address,
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

        let nft_id = env.register_contract(None, InvoiceNftContract);
        let nft_client = InvoiceNftContractClient::new(&env, &nft_id);

        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        kora_access_control::AccessControlContractClient::new(&env, &ac_id).initialize(&admin);
        nft_client.initialize(&admin, &ac_id);
        nft_client.set_authorized_callers(&admin, &marketplace, &pool);

        TestEnv {
            env,
            admin,
            sme,
            marketplace,
            pool,
            nft_client,
        }
    }

    // ── Status Transition Edge Cases ──────────────────────────────────────────

    #[test]
    fn test_created_to_funded_fails() {
        let t = setup();

        // Mint invoice (status = Created)
        let invoice_id = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        // Try to go directly from Created to Funded (should fail)
        let result = t.nft_client.try_set_funded(&t.pool, &invoice_id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_listed_to_repaid_fails() {
        let t = setup();

        // Mint and transition to Listed
        let invoice_id = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        t.nft_client.set_listed(&t.marketplace, &invoice_id);

        // Try to go directly from Listed to Repaid (should fail)
        let result = t.nft_client.try_set_repaid(&t.pool, &invoice_id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_cannot_transition_backward() {
        let t = setup();

        // Create complete lifecycle and verify no backward transitions
        let invoice_id = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        t.nft_client.set_listed(&t.marketplace, &invoice_id);
        t.nft_client.set_funded(&t.pool, &invoice_id);
        t.nft_client.set_repaid(&t.pool, &invoice_id);

        // Try to go back to Created
        let result = t.nft_client.try_set_listed(&t.marketplace, &invoice_id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);

        // Try to go back to Listed
        let result = t.nft_client.try_set_funded(&t.pool, &invoice_id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_status_transition_valid_path() {
        let t = setup();

        let invoice_id = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        let invoice = t.nft_client.get_invoice(&invoice_id);
        assert_eq!(invoice.status, InvoiceStatus::Created);

        t.nft_client.set_listed(&t.marketplace, &invoice_id);
        let invoice = t.nft_client.get_invoice(&invoice_id);
        assert_eq!(invoice.status, InvoiceStatus::Listed);

        t.nft_client.set_funded(&t.pool, &invoice_id);
        let invoice = t.nft_client.get_invoice(&invoice_id);
        assert_eq!(invoice.status, InvoiceStatus::Funded);

        t.nft_client.set_repaid(&t.pool, &invoice_id);
        let invoice = t.nft_client.get_invoice(&invoice_id);
        assert_eq!(invoice.status, InvoiceStatus::Repaid);
    }

    // ── Immutability Edge Cases ───────────────────────────────────────────────

    #[test]
    fn test_invoice_fields_immutable_after_creation() {
        let t = setup();

        let original_hash = Bytes::from_slice(&t.env, &[1u8; 32]);
        let original_amount = 1_000_000i128;
        let original_currency = Symbol::new(&t.env, "USDC");
        let original_due_date = t.env.ledger().timestamp() + 86_400 * 30;

        let invoice_id = t.nft_client.mint_invoice(
            &t.sme,
            &original_hash,
            &original_amount,
            &original_currency,
            &original_due_date,
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        t.nft_client.set_listed(&t.marketplace, &invoice_id);
        t.nft_client.set_funded(&t.pool, &invoice_id);

        // Retrieve invoice and verify fields haven't changed
        let invoice = t.nft_client.get_invoice(&invoice_id);
        assert_eq!(invoice.debtor_hash, original_hash);
        assert_eq!(invoice.amount, original_amount);
        assert_eq!(invoice.currency, original_currency);
        assert_eq!(invoice.due_date, original_due_date);
    }

    #[test]
    fn test_timestamps_recorded_correctly() {
        let t = setup();

        let created_at = t.env.ledger().timestamp();

        let invoice_id = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(created_at + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        let invoice = t.nft_client.get_invoice(&invoice_id);
        assert_eq!(invoice.created_at, created_at);
        assert!(invoice.funded_at.is_none());
        assert!(invoice.repaid_at.is_none());

        // Advance time and fund
        let future_time = created_at + 1000;
        t.env.ledger().set(LedgerInfo {
            timestamp: future_time,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        t.nft_client.set_listed(&t.marketplace, &invoice_id);
        t.nft_client.set_funded(&t.pool, &invoice_id);

        let invoice = t.nft_client.get_invoice(&invoice_id);
        assert_eq!(invoice.created_at, created_at);
        assert_eq!(invoice.funded_at, Some(future_time));
    }

    // ── ID Overflow Prevention ────────────────────────────────────────────────

    #[test]
    fn test_invoice_id_increments() {
        let t = setup();

        let id1 = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        let id2 = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[2u8; 32]),
            &2_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &50u32,
        &None,
        );

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(t.nft_client.next_id(), 3);
    }

    #[test]
    fn test_invoice_count_accurate() {
        let t = setup();

        assert_eq!(t.nft_client.invoice_count(), 0);

        t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        assert_eq!(t.nft_client.invoice_count(), 1);

        t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[2u8; 32]),
            &2_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &50u32,
        &None,
        );

        assert_eq!(t.nft_client.invoice_count(), 2);
    }

    // ── Validation Edge Cases ─────────────────────────────────────────────────

    #[test]
    fn test_invoice_not_found_returns_error() {
        let t = setup();

        let result = t.nft_client.try_get_invoice(&999u64);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvoiceNotFound);
    }

    #[test]
    fn test_set_defaulted_requires_past_due_date() {
        let t = setup();

        let due_date = t.env.ledger().timestamp() + 86_400;

        let invoice_id = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &due_date,
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        t.nft_client.set_listed(&t.marketplace, &invoice_id);
        t.nft_client.set_funded(&t.pool, &invoice_id);

        // Try to mark defaulted before due date
        let result = t.nft_client.try_set_defaulted(&t.admin, &invoice_id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);

        // Advance past due date
        t.env.ledger().set(LedgerInfo {
            timestamp: due_date + 1,
            protocol_version: 21,
            sequence_number: 3,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        // Now should succeed
        let result = t.nft_client.try_set_defaulted(&t.admin, &invoice_id);
        assert!(result.is_ok());

        let invoice = t.nft_client.get_invoice(&invoice_id);
        assert_eq!(invoice.status, InvoiceStatus::Defaulted);
    }

    #[test]
    fn test_set_defaulted_admin_only() {
        let t = setup();

        let due_date = t.env.ledger().timestamp() + 86_400;

        let invoice_id = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &due_date,
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        t.nft_client.set_listed(&t.marketplace, &invoice_id);
        t.nft_client.set_funded(&t.pool, &invoice_id);

        // Advance past due date
        t.env.ledger().set(LedgerInfo {
            timestamp: due_date + 1,
            protocol_version: 21,
            sequence_number: 3,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        // Non-admin cannot mark defaulted
        let non_admin = Address::generate(&t.env);
        let result = t.nft_client.try_set_defaulted(&non_admin, &invoice_id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    // ── Migration Edge Cases ──────────────────────────────────────────────────

    #[test]
    fn test_migrate_idempotent() {
        let t = setup();

        // Migrate once
        t.nft_client.migrate(&t.admin);

        // Migrate again should not fail
        let result = t.nft_client.try_migrate(&t.admin);
        assert!(result.is_ok());

        // And again
        let result = t.nft_client.try_migrate(&t.admin);
        assert!(result.is_ok());
    }

    #[test]
    fn test_migrate_non_admin_fails() {
        let t = setup();
        let non_admin = Address::generate(&t.env);

        let result = t.nft_client.try_migrate(&non_admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_migrate_preserves_existing_invoices() {
        let t = setup();

        // Mint invoice before migration
        let id1 = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &1_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        &None,
        );

        let invoice_before = t.nft_client.get_invoice(&id1);

        // Migrate
        t.nft_client.migrate(&t.admin);

        // Verify invoice still exists and is unchanged
        let invoice_after = t.nft_client.get_invoice(&id1);
        assert_eq!(invoice_before.id, invoice_after.id);
        assert_eq!(invoice_before.amount, invoice_after.amount);
        assert_eq!(invoice_before.status, invoice_after.status);

        // Verify can still mint invoices after migration
        let id2 = t.nft_client.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[2u8; 32]),
            &2_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &(t.env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(&t.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &50u32,
        &None,
        );
        assert_eq!(id2, 2);
    }
}
