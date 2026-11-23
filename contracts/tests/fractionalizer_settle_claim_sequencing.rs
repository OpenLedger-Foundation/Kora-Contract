// contracts/tests/fractionalizer_settle_claim_sequencing.rs
//! Fractionalizer settle / claim sequencing and race tests.
//!
//! `docs/fractionalizer.md` defines `PoolNotClosed` as the guard against
//! claiming before the underlying pool closes and the wrap is settled. These
//! tests pin down every ordering of `repay`, `settle`, `transfer_shares` and
//! `redeem` that could otherwise brick a position:
//!
//! - Early claim (pool open, partially repaid, or closed but unsettled) fails
//!   with `PoolNotClosed`, writes no state and emits no `FRAC_REDM`.
//! - Settle before close fails with `PoolNotClosed`.
//! - Double settle fails with `AlreadySettled` and never resets the payout,
//!   including after redemptions have already started.
//! - Shares cannot be sent to a holder that already claimed (`AlreadyClaimed`),
//!   so `outstanding_shares` can always reach zero.
//! - Events: exactly one `FRAC_SETL` per wrap and one `FRAC_REDM` per holder,
//!   with payloads matching the returned values.
//!
//! Runs against the real `financing_pool` and `invoice_nft` contracts with the
//! same deploy order as `fractionalizer_e2e.rs`.

#[cfg(test)]
mod fractionalizer_settle_claim_sequencing {
    use kora_financing_pool::FinancingPoolContractClient;
    use kora_fractionalizer::{FractionalizerContractClient, FractionalizerError};
    use kora_invoice_nft::InvoiceNftContractClient;
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events, Ledger, LedgerInfo},
        token, Address, Bytes, Env, String, Symbol, TryFromVal, Val,
    };

    const FACE: i128 = 10_000_000;
    const TOTAL_SHARES: u64 = 1_000;

    struct T {
        env: Env,
        admin: Address,
        sme: Address,
        investor: Address,
        holder_a: Address,
        holder_b: Address,
        token: Address,
        marketplace: Address,
        nft: InvoiceNftContractClient<'static>,
        pool: FinancingPoolContractClient<'static>,
        frac: FractionalizerContractClient<'static>,
    }

    fn setup() -> T {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10_000,
            min_persistent_entry_ttl: 10_000,
            max_entry_ttl: 1_000_000,
        });

        let admin = Address::generate(&env);
        let sme = Address::generate(&env);
        let investor = Address::generate(&env);
        let holder_a = Address::generate(&env);
        let holder_b = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();
        let marketplace = Address::generate(&env);

        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        kora_access_control::AccessControlContractClient::new(&env, &ac_id).initialize(&admin);

        let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac_id);

        let pool_id = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
        let pool = FinancingPoolContractClient::new(&env, &pool_id);
        pool.initialize(
            &admin,
            &nft_id,
            &Address::generate(&env),
            &Address::generate(&env),
            &ac_id,
            &200u32,
            &Address::generate(&env),
            &10_000u32,
            &0u64,
            &Address::generate(&env),
        );
        token::StellarAssetClient::new(&env, &token).mint(&sme, &100_000_000_000i128);

        let frac_id = env.register_contract(None, kora_fractionalizer::FractionalizerContract);
        let frac = FractionalizerContractClient::new(&env, &frac_id);
        frac.initialize(&admin, &pool_id, &ac_id);

        nft.set_authorized_callers(&admin, &marketplace, &pool_id);

        T {
            env,
            admin,
            sme,
            investor,
            holder_a,
            holder_b,
            token,
            marketplace,
            nft,
            pool,
            frac,
        }
    }

    /// Mint, list, open the pool, fund it fully and wrap into TOTAL_SHARES.
    fn wrapped_pool(t: &T) -> u64 {
        let due = t.env.ledger().timestamp() + 86_400 * 30;
        let id = t.nft.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[7u8; 32]),
            &FACE,
            &Symbol::new(&t.env, "USDC"),
            &due,
            &String::from_str(&t.env, "ipfs://seq"),
            &50u32,
            &None,
        );
        t.nft.set_listed(&t.marketplace, &id);
        t.pool.release_funds(&t.marketplace, &id, &t.token);
        t.pool
            .record_position(&t.admin, &id, &t.investor, &FACE, &FACE);
        t.frac.wrap(&t.investor, &id, &TOTAL_SHARES);
        id
    }

    /// All fractionalizer events with `topic`, oldest first. The test host
    /// accumulates events across invocations, so callers compare lengths
    /// before and after a call.
    fn frac_events(t: &T, topic: Symbol) -> std::vec::Vec<Val> {
        let mut out = std::vec::Vec::new();
        for (contract, topics, data) in t.env.events().all().iter() {
            if contract != t.frac.address || topics.is_empty() {
                continue;
            }
            let first = Symbol::try_from_val(&t.env, &topics.get(0).unwrap());
            if first.map(|s| s == topic).unwrap_or(false) {
                out.push(data);
            }
        }
        out
    }

    /// Number of fractionalizer events emitted so far.
    fn frac_event_count(t: &T) -> usize {
        t.env
            .events()
            .all()
            .iter()
            .filter(|(c, _, _)| *c == t.frac.address)
            .count()
    }

    fn assert_no_new_frac_events(t: &T, before: usize) {
        assert_eq!(
            frac_event_count(t),
            before,
            "rejected call must not emit fractionalizer events"
        );
    }

    // ── Early claim ───────────────────────────────────────────────────────────

    #[test]
    fn early_claim_on_open_pool_fails_pool_not_closed() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.frac
            .transfer_shares(&t.investor, &t.investor, &id, &t.holder_a, &400);

        for holder in [&t.investor, &t.holder_a] {
            let before = frac_event_count(&t);
            let r = t.frac.try_redeem(holder, &id, &t.investor);
            assert_eq!(r, Err(Ok(FractionalizerError::PoolNotClosed)));
            assert_no_new_frac_events(&t, before);
        }
        // No state written: balances intact, wrap still unsettled.
        assert_eq!(t.frac.get_share_balance(&id, &t.investor, &t.holder_a), 400);
        let w = t.frac.get_wrap(&id, &t.investor);
        assert!(!w.settled);
        assert_eq!(w.outstanding_shares, TOTAL_SHARES);
    }

    #[test]
    fn early_claim_after_partial_repay_fails_pool_not_closed() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.pool.repay_partial(&t.sme, &id, &t.token, &(FACE / 2));
        assert!(!t.pool.get_pool(&id).is_closed);

        assert_eq!(
            t.frac.try_settle(&id, &t.investor),
            Err(Ok(FractionalizerError::PoolNotClosed))
        );
        assert_eq!(
            t.frac.try_redeem(&t.investor, &id, &t.investor),
            Err(Ok(FractionalizerError::PoolNotClosed))
        );
        assert!(!t.frac.get_wrap(&id, &t.investor).settled);
    }

    /// Edge case from the issue: close, then claim immediately in the same
    /// ledger without settling. Must fail, and must not poison a later claim.
    #[test]
    fn close_then_immediate_claim_fails_then_succeeds_after_settle() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.frac
            .transfer_shares(&t.investor, &t.investor, &id, &t.holder_a, &250);
        t.pool.repay(&t.sme, &id, &t.token, &FACE);
        assert!(t.pool.get_pool(&id).is_closed);

        let before = frac_event_count(&t);
        let r = t.frac.try_redeem(&t.holder_a, &id, &t.investor);
        assert_eq!(r, Err(Ok(FractionalizerError::PoolNotClosed)));
        assert_no_new_frac_events(&t, before);

        t.frac.settle(&id, &t.investor);

        let before = token::Client::new(&t.env, &t.token).balance(&t.holder_a);
        let paid = t.frac.redeem(&t.holder_a, &id, &t.investor);
        assert_eq!(paid, FACE / 4);
        assert_eq!(
            token::Client::new(&t.env, &t.token).balance(&t.holder_a) - before,
            paid
        );
    }

    #[test]
    fn settle_unknown_wrap_fails_wrap_not_found() {
        let t = setup();
        let id = wrapped_pool(&t);
        assert_eq!(
            t.frac.try_settle(&id, &t.holder_a),
            Err(Ok(FractionalizerError::WrapNotFound))
        );
        assert_eq!(
            t.frac.try_settle(&(id + 99), &t.investor),
            Err(Ok(FractionalizerError::WrapNotFound))
        );
    }

    // ── Double settle ─────────────────────────────────────────────────────────

    #[test]
    fn double_settle_rejected_and_payout_unchanged() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.pool.repay(&t.sme, &id, &t.token, &FACE);

        t.frac.settle(&id, &t.investor);
        let settled = t.frac.get_wrap(&id, &t.investor);
        assert!(settled.settled);
        assert_eq!(settled.total_payout, FACE);

        let before = frac_event_count(&t);
        assert_eq!(
            t.frac.try_settle(&id, &t.investor),
            Err(Ok(FractionalizerError::AlreadySettled))
        );
        assert_no_new_frac_events(&t, before);
        let again = t.frac.get_wrap(&id, &t.investor);
        assert_eq!(again.total_payout, settled.total_payout);
        assert_eq!(again.outstanding_shares, settled.outstanding_shares);
    }

    /// The dangerous race: a re-settle between two redemptions would reset
    /// `total_payout` against the already-reduced share supply and let the
    /// remaining holders drain more than their pro-rata share.
    #[test]
    fn resettle_mid_redemption_cannot_inflate_payout() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.frac
            .transfer_shares(&t.investor, &t.investor, &id, &t.holder_a, &500);
        t.pool.repay(&t.sme, &id, &t.token, &FACE);
        t.frac.settle(&id, &t.investor);

        let a = t.frac.redeem(&t.holder_a, &id, &t.investor);
        assert_eq!(a, FACE / 2);
        let mid = t.frac.get_wrap(&id, &t.investor);
        assert_eq!(mid.total_payout, FACE / 2);
        assert_eq!(mid.outstanding_shares, 500);

        assert_eq!(
            t.frac.try_settle(&id, &t.investor),
            Err(Ok(FractionalizerError::AlreadySettled))
        );
        let after = t.frac.get_wrap(&id, &t.investor);
        assert_eq!(after.total_payout, mid.total_payout);

        let i = t.frac.redeem(&t.investor, &id, &t.investor);
        assert_eq!(i, FACE / 2);
        assert_eq!(a + i, FACE, "total paid must equal settled payout exactly");
        let done = t.frac.get_wrap(&id, &t.investor);
        assert_eq!(done.total_payout, 0);
        assert_eq!(done.outstanding_shares, 0);
    }

    // ── Claim ordering ────────────────────────────────────────────────────────

    #[test]
    fn double_claim_rejected_already_claimed() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.pool.repay(&t.sme, &id, &t.token, &FACE);
        t.frac.settle(&id, &t.investor);
        t.frac.redeem(&t.investor, &id, &t.investor);
        let before = frac_event_count(&t);
        assert_eq!(
            t.frac.try_redeem(&t.investor, &id, &t.investor),
            Err(Ok(FractionalizerError::AlreadyClaimed))
        );
        assert_no_new_frac_events(&t, before);
    }

    /// Sending shares to a holder that already redeemed would strand them and
    /// keep `outstanding_shares` above zero forever.
    #[test]
    fn transfer_to_claimed_holder_rejected() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.frac
            .transfer_shares(&t.investor, &t.investor, &id, &t.holder_a, &300);
        t.pool.repay(&t.sme, &id, &t.token, &FACE);
        t.frac.settle(&id, &t.investor);
        t.frac.redeem(&t.holder_a, &id, &t.investor);

        assert_eq!(
            t.frac
                .try_transfer_shares(&t.investor, &t.investor, &id, &t.holder_a, &100),
            Err(Ok(FractionalizerError::AlreadyClaimed))
        );
        assert_eq!(t.frac.get_share_balance(&id, &t.investor, &t.investor), 700);

        // Transfer to a fresh holder after settle is still fine, and the
        // remaining holders drain the wrap to exactly zero.
        t.frac
            .transfer_shares(&t.investor, &t.investor, &id, &t.holder_b, &200);
        let b = t.frac.redeem(&t.holder_b, &id, &t.investor);
        let i = t.frac.redeem(&t.investor, &id, &t.investor);
        assert_eq!(b, FACE * 2 / 10);
        assert_eq!(i, FACE * 5 / 10);
        assert_eq!(t.frac.get_wrap(&id, &t.investor).outstanding_shares, 0);
    }

    #[test]
    fn non_holder_claim_after_settle_fails_not_share_holder() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.pool.repay(&t.sme, &id, &t.token, &FACE);
        t.frac.settle(&id, &t.investor);
        assert_eq!(
            t.frac.try_redeem(&t.holder_b, &id, &t.investor),
            Err(Ok(FractionalizerError::NotShareHolder))
        );
    }

    // ── Event assertions ──────────────────────────────────────────────────────

    #[test]
    fn settle_and_redeem_emit_exactly_one_event_each() {
        let t = setup();
        let id = wrapped_pool(&t);
        t.frac
            .transfer_shares(&t.investor, &t.investor, &id, &t.holder_a, &333);
        t.pool.repay(&t.sme, &id, &t.token, &FACE);

        let setl_before = frac_events(&t, symbol_short!("FRAC_SETL")).len();
        t.frac.settle(&id, &t.investor);
        let setl = frac_events(&t, symbol_short!("FRAC_SETL"));
        assert_eq!(setl.len(), setl_before + 1, "exactly one FRAC_SETL");
        let (who, payout) = <(Address, i128)>::try_from_val(&t.env, setl.last().unwrap()).unwrap();
        assert_eq!(who, t.investor);
        assert_eq!(payout, FACE);

        // A rejected re-settle must not add a second FRAC_SETL.
        let _ = t.frac.try_settle(&id, &t.investor);
        assert_eq!(
            frac_events(&t, symbol_short!("FRAC_SETL")).len(),
            setl.len()
        );

        let redm_before = frac_events(&t, symbol_short!("FRAC_REDM")).len();
        let paid_a = t.frac.redeem(&t.holder_a, &id, &t.investor);
        let redm = frac_events(&t, symbol_short!("FRAC_REDM"));
        assert_eq!(redm.len(), redm_before + 1, "exactly one FRAC_REDM");
        let ev = <(Address, u64, i128)>::try_from_val(&t.env, redm.last().unwrap()).unwrap();
        assert_eq!(ev, (t.holder_a.clone(), 333, paid_a));

        // The last redeemer takes the exact remainder (dust) of the payout.
        let paid_i = t.frac.redeem(&t.investor, &id, &t.investor);
        let redm = frac_events(&t, symbol_short!("FRAC_REDM"));
        assert_eq!(redm.len(), redm_before + 2);
        let ev = <(Address, u64, i128)>::try_from_val(&t.env, redm.last().unwrap()).unwrap();
        assert_eq!(ev, (t.investor.clone(), 667, paid_i));
        assert_eq!(
            paid_a + paid_i,
            FACE,
            "payouts must sum to the settled amount"
        );
    }
}
