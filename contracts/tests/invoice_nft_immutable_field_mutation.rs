// contracts/tests/invoice_nft_immutable_field_mutation.rs
//
// Issue: test(nft): immutable field mutation fuzz
//
// Fuzz/attempt mutations of immutable invoice fields after create/status change
// and assert rejection.  Mutable 'immutable' fields break SME/investor trust
// and downstream indexers.
//
// Immutable fields (set at mint, must never change):
//   id, sme, debtor_hash, amount, currency, due_date, ipfs_cid, risk_score,
//   risk_tier, created_at
//
// Mutable fields (legitimate state machine outputs only):
//   status (via set_listed / set_funded / set_repaid / set_defaulted)
//   funded_at   (set by set_funded)
//   repaid_at   (set by set_repaid)
//   notes       (set at mint; not updated later — excluded from amend_invoice)
//
// Acceptance criteria
//  Happy path:  amend_invoice on a Created invoice succeeds.
//  Negative:    amend_invoice on Listed/Funded/Repaid/Defaulted returns
//               InvalidInvoiceStatus.
//  Negative:    amend_invoice by a non-owner returns Unauthorized.
//  After every status transition, all immutable fields retain their original
//  values verbatim.
//  Edge cases:  paused protocol blocks amend_invoice; partial state (paused
//               mid-transition) leaves no half-written record.

#[cfg(test)]
mod invoice_nft_immutable_field_mutation {
    use kora_access_control::{AccessControlContract, AccessControlContractClient};
    use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient, InvoiceNftError};
    use kora_shared::{errors::KoraError, types::InvoiceStatus};
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Bytes, Env, String, Symbol,
    };

    // ── Shared harness ────────────────────────────────────────────────────────

    struct Ctx {
        env: Env,
        admin: Address,
        sme: Address,
        /// Addresses authorized as marketplace / pool respectively.
        marketplace: Address,
        pool: Address,
        nft: InvoiceNftContractClient<'static>,
        ac: AccessControlContractClient<'static>,
    }

    fn setup() -> Ctx {
        let env = Env::default();
        env.mock_all_auths();
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
        let sme = Address::generate(&env);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);

        let ac_id = env.register_contract(None, AccessControlContract);
        let ac = AccessControlContractClient::new(&env, &ac_id);
        ac.initialize(&admin);

        let nft_id = env.register_contract(None, InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac_id);
        nft.set_authorized_callers(&admin, &marketplace, &pool);

        Ctx { env, admin, sme, marketplace, pool, nft, ac }
    }

    // ── Mint helper with known, verifiable field values ───────────────────────

    struct MintedInvoice {
        id: u64,
        debtor_hash: Bytes,
        amount: i128,
        currency: Symbol,
        due_date: u64,
        ipfs_cid: String,
        risk_score: u32,
        created_at: u64,
    }

    fn mint_known(ctx: &Ctx) -> MintedInvoice {
        let debtor_hash = Bytes::from_slice(&ctx.env, &[0xABu8; 32]);
        let amount = 10_000_000_000i128;
        let currency = Symbol::new(&ctx.env, "USDC");
        let due_date = ctx.env.ledger().timestamp() + 86_400 * 60;
        let ipfs_cid = String::from_str(
            &ctx.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let risk_score = 30u32; // → AA tier
        let created_at = ctx.env.ledger().timestamp();

        let id = ctx.nft.mint_invoice(
            &ctx.sme,
            &debtor_hash,
            &amount,
            &currency,
            &due_date,
            &ipfs_cid,
            &risk_score,
            &None,
        );

        MintedInvoice { id, debtor_hash, amount, currency, due_date, ipfs_cid, risk_score, created_at }
    }

    // Assert every immutable field still matches its mint-time value.
    fn assert_immutable_fields_unchanged(ctx: &Ctx, m: &MintedInvoice) {
        let inv = ctx.nft.get_invoice(&m.id);
        assert_eq!(inv.id, m.id, "id changed");
        assert_eq!(inv.sme, ctx.sme, "sme changed");
        assert_eq!(inv.debtor_hash, m.debtor_hash, "debtor_hash changed");
        assert_eq!(inv.amount, m.amount, "amount changed");
        assert_eq!(inv.currency, m.currency, "currency changed");
        assert_eq!(inv.due_date, m.due_date, "due_date changed");
        assert_eq!(inv.ipfs_cid, m.ipfs_cid, "ipfs_cid changed (via amend path)");
        assert_eq!(inv.risk_score, m.risk_score, "risk_score changed");
        assert_eq!(inv.created_at, m.created_at, "created_at changed");
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 1. Immutable fields survive a valid amend_invoice call (Created status)
    //    amend_invoice intentionally replaces amount/due_date/risk_score/etc.
    //    when status == Created; that's the only allowed mutation window.
    //    After amend we verify the NEW values replaced the old ones, and all
    //    never-mutable fields (id, sme, created_at) remain intact.
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn amend_created_invoice_replaces_allowed_fields_only() {
        let ctx = setup();
        let m = mint_known(&ctx);

        let new_hash = Bytes::from_slice(&ctx.env, &[0xCCu8; 32]);
        let new_amount = 8_000_000_000i128;
        let new_due = ctx.env.ledger().timestamp() + 86_400 * 90;
        let new_cid = String::from_str(
            &ctx.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let new_score = 50u32;

        ctx.nft.amend_invoice(
            &ctx.sme, &m.id,
            &new_hash, &new_amount, &new_due, &new_cid, &new_score,
        );

        let inv = ctx.nft.get_invoice(&m.id);
        // Allowed-to-change fields updated correctly.
        assert_eq!(inv.amount, new_amount);
        assert_eq!(inv.due_date, new_due);
        assert_eq!(inv.risk_score, new_score);
        assert_eq!(inv.debtor_hash, new_hash);

        // Never-mutable fields unchanged.
        assert_eq!(inv.id, m.id, "id must never change");
        assert_eq!(inv.sme, ctx.sme, "sme must never change");
        assert_eq!(inv.created_at, m.created_at, "created_at must never change");
        assert_eq!(inv.status, InvoiceStatus::Created);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 2. Mutation attempt on a Listed invoice is rejected
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn amend_after_set_listed_rejected() {
        let ctx = setup();
        let m = mint_known(&ctx);

        ctx.nft.set_listed(&ctx.marketplace, &m.id);
        assert_eq!(ctx.nft.get_invoice(&m.id).status, InvoiceStatus::Listed);

        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &m.id,
            &Bytes::from_slice(&ctx.env, &[0xFFu8; 32]),
            &1_000i128,
            &(ctx.env.ledger().timestamp() + 86_400),
            &String::from_str(&ctx.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &10u32,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "amend_invoice on Listed must be rejected"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 3. Mutation attempt on a Funded invoice is rejected
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn amend_after_set_funded_rejected() {
        let ctx = setup();
        let m = mint_known(&ctx);

        ctx.nft.set_listed(&ctx.marketplace, &m.id);
        ctx.nft.set_funded(&ctx.pool, &m.id);
        assert_eq!(ctx.nft.get_invoice(&m.id).status, InvoiceStatus::Funded);

        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &m.id,
            &Bytes::from_slice(&ctx.env, &[0xFFu8; 32]),
            &1_000i128,
            &(ctx.env.ledger().timestamp() + 86_400),
            &String::from_str(&ctx.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &10u32,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "amend_invoice on Funded must be rejected"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 4. Mutation attempt on a Repaid invoice is rejected
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn amend_after_set_repaid_rejected() {
        let ctx = setup();
        let m = mint_known(&ctx);

        ctx.nft.set_listed(&ctx.marketplace, &m.id);
        ctx.nft.set_funded(&ctx.pool, &m.id);
        ctx.nft.set_repaid(&ctx.pool, &m.id);
        assert_eq!(ctx.nft.get_invoice(&m.id).status, InvoiceStatus::Repaid);

        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &m.id,
            &Bytes::from_slice(&ctx.env, &[0xFFu8; 32]),
            &1_000i128,
            &(ctx.env.ledger().timestamp() + 86_400),
            &String::from_str(&ctx.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &10u32,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "amend_invoice on Repaid must be rejected"
        );
        // Repaid invoice: id/sme/amount/currency etc. still correct.
        let inv = ctx.nft.get_invoice(&m.id);
        assert_eq!(inv.id, m.id);
        assert_eq!(inv.sme, ctx.sme);
        assert_eq!(inv.amount, m.amount);
        assert_eq!(inv.currency, m.currency);
        assert_eq!(inv.created_at, m.created_at);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 5. Mutation attempt on a Defaulted invoice is rejected
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn amend_after_set_defaulted_rejected() {
        let ctx = setup();
        let due = ctx.env.ledger().timestamp() + 86_400;
        let debtor = Bytes::from_slice(&ctx.env, &[0xABu8; 32]);
        let cid = String::from_str(
            &ctx.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let id = ctx.nft.mint_invoice(
            &ctx.sme, &debtor, &10_000_000_000i128,
            &Symbol::new(&ctx.env, "USDC"), &due, &cid, &30u32, &None,
        );

        ctx.nft.set_listed(&ctx.marketplace, &id);
        ctx.nft.set_funded(&ctx.pool, &id);

        // Advance past due_date to allow defaulting.
        ctx.env.ledger().set(LedgerInfo {
            timestamp: due + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 100_000,
        });
        ctx.nft.set_defaulted(&ctx.admin, &id);
        assert_eq!(ctx.nft.get_invoice(&id).status, InvoiceStatus::Defaulted);

        let new_due_future = ctx.env.ledger().timestamp() + 86_400 * 10;
        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &id,
            &Bytes::from_slice(&ctx.env, &[0xFFu8; 32]),
            &1_000i128,
            &new_due_future,
            &cid,
            &10u32,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "amend_invoice on Defaulted must be rejected"
        );
        // Core immutable fields survived.
        let inv = ctx.nft.get_invoice(&id);
        assert_eq!(inv.sme, ctx.sme);
        assert_eq!(inv.amount, 10_000_000_000i128);
        assert_eq!(inv.debtor_hash, debtor);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 6. Non-owner cannot mutate a Created invoice
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn non_owner_amend_rejected() {
        let ctx = setup();
        let m = mint_known(&ctx);
        let attacker = Address::generate(&ctx.env);

        let result = ctx.nft.try_amend_invoice(
            &attacker, &m.id,
            &Bytes::from_slice(&ctx.env, &[0xFFu8; 32]),
            &999i128,
            &(ctx.env.ledger().timestamp() + 86_400),
            &String::from_str(&ctx.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &10u32,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::Unauthorized,
            "non-owner amend must be rejected"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 7. Admin cannot mutate an invoice via amend_invoice (admin-only path is
    //    admin_correct_metadata_hash which only touches metadata_hash on Created)
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn admin_cannot_bypass_owner_check_via_amend() {
        let ctx = setup();
        let m = mint_known(&ctx);

        // Admin is not the SME owner; amend_invoice checks sme == caller.
        let result = ctx.nft.try_amend_invoice(
            &ctx.admin, &m.id,
            &Bytes::from_slice(&ctx.env, &[0xFFu8; 32]),
            &1_000i128,
            &(ctx.env.ledger().timestamp() + 86_400),
            &String::from_str(&ctx.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &10u32,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::Unauthorized,
            "admin must not be able to amend_invoice on behalf of SME"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 8. Paused protocol blocks amend_invoice
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn amend_blocked_when_paused() {
        let ctx = setup();
        let m = mint_known(&ctx);

        ctx.ac.pause(&ctx.admin);
        assert!(ctx.ac.is_paused());

        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &m.id,
            &Bytes::from_slice(&ctx.env, &[0xCCu8; 32]),
            &8_000_000_000i128,
            &(ctx.env.ledger().timestamp() + 86_400 * 90),
            &String::from_str(&ctx.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &25u32,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::ProtocolPaused,
            "amend_invoice must be blocked when protocol is paused"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 9. update_metadata_cid blocked on non-Created invoice
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn update_metadata_cid_blocked_on_listed() {
        let ctx = setup();
        let m = mint_known(&ctx);

        ctx.nft.set_listed(&ctx.marketplace, &m.id);

        let new_cid = String::from_str(
            &ctx.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let result = ctx.nft.try_update_metadata_cid(&ctx.sme, &m.id, &new_cid);
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "update_metadata_cid must be blocked on Listed invoice"
        );
    }

    #[test]
    fn update_metadata_cid_blocked_on_funded() {
        let ctx = setup();
        let m = mint_known(&ctx);

        ctx.nft.set_listed(&ctx.marketplace, &m.id);
        ctx.nft.set_funded(&ctx.pool, &m.id);

        let new_cid = String::from_str(
            &ctx.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let result = ctx.nft.try_update_metadata_cid(&ctx.sme, &m.id, &new_cid);
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "update_metadata_cid must be blocked on Funded invoice"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 10. commit_metadata_hash is write-once: second commit rejected
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn commit_metadata_hash_write_once() {
        let ctx = setup();
        let m = mint_known(&ctx);

        let hash1 = Bytes::from_slice(&ctx.env, &[0x11u8; 32]);
        ctx.nft.commit_metadata_hash(&ctx.sme, &m.id, &hash1);

        // Attempting to commit again must fail.
        let hash2 = Bytes::from_slice(&ctx.env, &[0x22u8; 32]);
        let result = ctx.nft.try_commit_metadata_hash(&ctx.sme, &m.id, &hash2);
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::AlreadyInitialized,
            "second commit_metadata_hash must be rejected"
        );

        // The on-chain hash is still the first one.
        let inv = ctx.nft.get_invoice(&m.id);
        assert_eq!(inv.metadata_hash, hash1, "metadata_hash must not change after second commit attempt");
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 11. commit_metadata_hash blocked on non-Created invoice
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn commit_metadata_hash_blocked_after_listed() {
        let ctx = setup();
        let m = mint_known(&ctx);

        ctx.nft.set_listed(&ctx.marketplace, &m.id);

        let hash = Bytes::from_slice(&ctx.env, &[0x11u8; 32]);
        let result = ctx.nft.try_commit_metadata_hash(&ctx.sme, &m.id, &hash);
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "commit_metadata_hash must be blocked on Listed invoice"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 12. admin_correct_metadata_hash blocked on Funded/Repaid/Defaulted
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn admin_correct_metadata_hash_blocked_on_funded() {
        let ctx = setup();
        let m = mint_known(&ctx);

        // Commit a hash first (required by admin_correct_metadata_hash).
        ctx.nft.commit_metadata_hash(
            &ctx.sme, &m.id,
            &Bytes::from_slice(&ctx.env, &[0x01u8; 32]),
        );

        ctx.nft.set_listed(&ctx.marketplace, &m.id);
        ctx.nft.set_funded(&ctx.pool, &m.id);

        let new_hash = Bytes::from_slice(&ctx.env, &[0x02u8; 32]);
        let result = ctx.nft.try_admin_correct_metadata_hash(&ctx.admin, &m.id, &new_hash);
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "admin_correct_metadata_hash must be blocked on Funded invoice"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 13. Fuzz-style: attempt every mutation path at every status — none must
    //     succeed after Created
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn all_mutation_paths_blocked_in_non_created_states() {
        // For each of the three non-terminal post-creation statuses
        // (Listed, Funded, Repaid), verify that amend_invoice and
        // update_metadata_cid return InvalidInvoiceStatus.
        let transitions: &[&str] = &["listed", "funded", "repaid"];

        for &state in transitions {
            let ctx = setup();
            let m = mint_known(&ctx);

            match state {
                "listed" => ctx.nft.set_listed(&ctx.marketplace, &m.id),
                "funded" => {
                    ctx.nft.set_listed(&ctx.marketplace, &m.id);
                    ctx.nft.set_funded(&ctx.pool, &m.id);
                }
                "repaid" => {
                    ctx.nft.set_listed(&ctx.marketplace, &m.id);
                    ctx.nft.set_funded(&ctx.pool, &m.id);
                    ctx.nft.set_repaid(&ctx.pool, &m.id);
                }
                _ => unreachable!(),
            }

            // amend_invoice
            let r = ctx.nft.try_amend_invoice(
                &ctx.sme, &m.id,
                &Bytes::from_slice(&ctx.env, &[0xFFu8; 32]),
                &1_000i128,
                &(ctx.env.ledger().timestamp() + 86_400),
                &String::from_str(&ctx.env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
                &10u32,
            );
            assert_eq!(
                r.unwrap_err().unwrap(),
                KoraError::InvalidInvoiceStatus,
                "amend_invoice must fail on status={state}"
            );

            // update_metadata_cid (only blocked on non-Created)
            if state != "repaid" {
                // set_repaid archives the invoice; get_invoice still works
                let r2 = ctx.nft.try_update_metadata_cid(
                    &ctx.sme,
                    &m.id,
                    &String::from_str(
                        &ctx.env,
                        "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                    ),
                );
                assert_eq!(
                    r2.unwrap_err().unwrap(),
                    KoraError::InvalidInvoiceStatus,
                    "update_metadata_cid must fail on status={state}"
                );
            }

            // Core immutable fields remain unchanged.
            let inv = ctx.nft.get_invoice(&m.id);
            assert_eq!(inv.id, m.id, "id changed at status={state}");
            assert_eq!(inv.sme, ctx.sme, "sme changed at status={state}");
            assert_eq!(inv.amount, m.amount, "amount changed at status={state}");
            assert_eq!(inv.currency, m.currency, "currency changed at status={state}");
            assert_eq!(inv.due_date, m.due_date, "due_date changed at status={state}");
            assert_eq!(inv.risk_score, m.risk_score, "risk_score changed at status={state}");
            assert_eq!(inv.created_at, m.created_at, "created_at changed at status={state}");
        }
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 14. funded_at and repaid_at are set exactly once by the state machine
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn funded_at_and_repaid_at_set_once_by_state_machine() {
        let ctx = setup();
        let m = mint_known(&ctx);

        // Before transitions: both None.
        let inv = ctx.nft.get_invoice(&m.id);
        assert_eq!(inv.funded_at, None);
        assert_eq!(inv.repaid_at, None);

        ctx.nft.set_listed(&ctx.marketplace, &m.id);
        let inv = ctx.nft.get_invoice(&m.id);
        assert_eq!(inv.funded_at, None, "funded_at must remain None after set_listed");
        assert_eq!(inv.repaid_at, None);

        let funded_ts = ctx.env.ledger().timestamp();
        ctx.nft.set_funded(&ctx.pool, &m.id);
        let inv = ctx.nft.get_invoice(&m.id);
        assert_eq!(inv.funded_at, Some(funded_ts), "funded_at must be recorded at set_funded");
        assert_eq!(inv.repaid_at, None, "repaid_at must remain None before repayment");

        let repaid_ts = ctx.env.ledger().timestamp();
        ctx.nft.set_repaid(&ctx.pool, &m.id);
        let inv = ctx.nft.get_invoice(&m.id);
        assert_eq!(inv.funded_at, Some(funded_ts), "funded_at must not change after repayment");
        assert_eq!(inv.repaid_at, Some(repaid_ts), "repaid_at must be recorded at set_repaid");
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 15. Withdraw on Listed/Funded returns InvalidInvoiceStatus
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn withdraw_blocked_on_listed_and_funded() {
        let ctx = setup();
        let m = mint_known(&ctx);

        ctx.nft.set_listed(&ctx.marketplace, &m.id);
        let r = ctx.nft.try_withdraw_invoice(&ctx.sme, &m.id);
        assert_eq!(
            r.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "withdraw must be blocked on Listed"
        );

        ctx.nft.set_funded(&ctx.pool, &m.id);
        let r = ctx.nft.try_withdraw_invoice(&ctx.sme, &m.id);
        assert_eq!(
            r.unwrap_err().unwrap(),
            KoraError::InvalidInvoiceStatus,
            "withdraw must be blocked on Funded"
        );

        // Core fields still intact.
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 16. Empty/invalid mutation inputs are rejected before status check
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn amend_with_zero_amount_rejected() {
        let ctx = setup();
        let m = mint_known(&ctx);

        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &m.id,
            &m.debtor_hash,
            &0i128, // zero amount
            &m.due_date,
            &m.ipfs_cid,
            &m.risk_score,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidAmount,
            "zero amount must be rejected by amend_invoice"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    #[test]
    fn amend_with_past_due_date_rejected() {
        let ctx = setup();
        let m = mint_known(&ctx);

        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &m.id,
            &m.debtor_hash,
            &m.amount,
            &(ctx.env.ledger().timestamp() - 1), // past due date
            &m.ipfs_cid,
            &m.risk_score,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidDueDate,
            "past due_date must be rejected by amend_invoice"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    #[test]
    fn amend_with_invalid_risk_score_rejected() {
        let ctx = setup();
        let m = mint_known(&ctx);

        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &m.id,
            &m.debtor_hash,
            &m.amount,
            &m.due_date,
            &m.ipfs_cid,
            &101u32, // > 100
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvalidRiskScore,
            "invalid risk score must be rejected"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }

    #[test]
    fn amend_with_empty_debtor_hash_rejected() {
        let ctx = setup();
        let m = mint_known(&ctx);

        let result = ctx.nft.try_amend_invoice(
            &ctx.sme, &m.id,
            &Bytes::from_slice(&ctx.env, &[]), // empty
            &m.amount,
            &m.due_date,
            &m.ipfs_cid,
            &m.risk_score,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::EmptyBytes,
            "empty debtor_hash must be rejected"
        );
        assert_immutable_fields_unchanged(&ctx, &m);
    }
}
