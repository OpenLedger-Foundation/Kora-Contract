// contracts/tests/fractionalizer_e2e.rs
//! E2E test: fractionalizer mint and redeem against financing_pool
//!
//! Closes #913
//!
//! This suite tests the complete fractionalizer lifecycle against the real
//! financing_pool contract using `mock_all_auths` and in-process native
//! contract registration (no WASM required), following the env/deploy-order
//! pattern from `full_lifecycle.rs` and `scripts/deploy.sh`.
//!
//! ## Flow
//!
//! 1. Deploy: invoice_nft + financing_pool + fractionalizer
//! 2. Mint an invoice via `invoice_nft::mint_invoice`
//! 3. Open a pool via `financing_pool::release_funds`
//! 4. Fund via `financing_pool::record_position` (up to full face value)
//! 5. Wrap the position via `fractionalizer::wrap` (e.g. 1 000 shares)
//! 6. Transfer shares to several holders
//! 7. Repay via `financing_pool::repay` to close the pool
//! 8. Settle the wrap via `fractionalizer::settle`
//! 9. Redeem for each holder — assert USDC payouts, share balances, and
//!    pool aggregates; verify pro-rata waterfall including dust
//!
//! ## Failure / edge cases (acceptance criteria from issue #913)
//!
//! - Redeem before settle → PoolNotClosed
//! - Double redeem        → AlreadyClaimed
//! - Zero-share redeem    → NotShareHolder (no panic)
//! - mark_default         → haircut redeem payout = 0
//! - repay_partial then complete repay before redeem
//! - Multiple investors fractionalizing the same pool
//! - Settle is idempotent
//! - Pool aggregates / share balances tracked correctly after every step

#[cfg(test)]
mod fractionalizer_e2e {
    use kora_financing_pool::FinancingPoolContractClient;
    use kora_fractionalizer::FractionalizerContractClient;
    use kora_invoice_nft::InvoiceNftContractClient;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Bytes, Env, String, Symbol,
    };

    // ── Constants ─────────────────────────────────────────────────────────────
    const INVOICE_AMOUNT: i128 = 10_000_000; // 10 USDC (1e7 scale)
    const INVESTOR1_CONTRIBUTION: i128 = 6_000_000; // 60%
    const INVESTOR2_CONTRIBUTION: i128 = 4_000_000; // 40%
    const REPAY_AMOUNT: i128 = 10_000_000; // full repayment
    const TOTAL_SHARES: u64 = 1_000;

    // ── Test environment ──────────────────────────────────────────────────────

    struct E2EEnv {
        env: Env,
        admin: Address,
        sme: Address,
        investor1: Address,
        investor2: Address,
        holder_a: Address,
        holder_b: Address,
        token: Address,
        marketplace: Address,
        nft: InvoiceNftContractClient<'static>,
        pool: FinancingPoolContractClient<'static>,
        frac: FractionalizerContractClient<'static>,
    }

    fn setup() -> E2EEnv {
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
        let investor1 = Address::generate(&env);
        let investor2 = Address::generate(&env);
        let holder_a = Address::generate(&env);
        let holder_b = Address::generate(&env);
        let token = Address::generate(&env);
        let marketplace = Address::generate(&env);
        let treasury = Address::generate(&env);

        // 1. Deploy invoice_nft
        let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        let ac = Address::generate(&env);
        nft.initialize(&admin, &ac);

        // 2. Deploy financing_pool
        let pool_id = env.register_contract(None, kora_financing_pool::FinancingPoolContract);
        let pool = FinancingPoolContractClient::new(&env, &pool_id);
        let oracle = Address::generate(&env);
        let risk_registry = Address::generate(&env);
        let dispute_resolution = Address::generate(&env);
        pool.initialize(
            &admin,
            &nft_id,
            &risk_registry,
            &treasury,
            &ac,
            &200u32,    // late_penalty_bps
            &oracle,
            &10_000u32, // max_position_bps = 100% (allow full single-investor funding)
            &dispute_resolution,
        );

        // 3. Deploy fractionalizer
        let frac_id = env.register_contract(None, kora_fractionalizer::FractionalizerContract);
        let frac = FractionalizerContractClient::new(&env, &frac_id);
        frac.initialize(&admin, &pool_id, &ac);

        // Wire NFT authorized callers so the pool may update invoice state
        nft.set_authorized_callers(&admin, &marketplace, &pool_id);

        E2EEnv {
            env,
            admin,
            sme,
            investor1,
            investor2,
            holder_a,
            holder_b,
            token,
            marketplace,
            nft,
            pool,
            frac,
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Mint an invoice and open a pool via release_funds.
    /// Returns the invoice_id assigned by mint_invoice.
    fn create_pool(t: &E2EEnv, amount: i128) -> u64 {
        let due_date = t.env.ledger().timestamp().saturating_add(86_400 * 30);
        let invoice_id = t.nft.mint_invoice(
            &t.sme,
            &Bytes::from_slice(&t.env, &[1u8; 32]),
            &amount,
            &Symbol::new(&t.env, "USDC"),
            &due_date,
            &String::from_str(&t.env, "ipfs://test"),
            &50u32,
            &None,
        );
        t.pool.release_funds(&t.marketplace, &invoice_id, &t.token);
        invoice_id
    }

    /// Record a single investor position.
    fn fund_position(
        t: &E2EEnv,
        invoice_id: u64,
        investor: &Address,
        contributed: i128,
        total: i128,
    ) {
        t.pool
            .record_position(&t.admin, &invoice_id, investor, &contributed, &total);
    }

    // ── Happy path: single investor wraps, distributes, repays, redeems ───────

    /// Full lifecycle with ONE investor, three share-holders, full repayment.
    #[test]
    fn test_e2e_single_investor_wrap_repay_redeem() {
        let t = setup();
        let invoice_id = create_pool(&t, INVOICE_AMOUNT);

        // Fund the pool with a single investor
        fund_position(&t, invoice_id, &t.investor1, INVOICE_AMOUNT, INVOICE_AMOUNT);

        // Wrap the position into TOTAL_SHARES shares
        t.frac.wrap(&t.investor1, &invoice_id, &TOTAL_SHARES);

        // All shares start with investor1
        let bal_before = t.frac.get_share_balance(&invoice_id, &t.investor1, &t.investor1);
        assert_eq!(bal_before, TOTAL_SHARES, "Investor1 must hold all minted shares");

        // Transfer 400 shares to holder_a, 300 to holder_b, keep 300
        t.frac.transfer_shares(&t.investor1, &t.investor1, &invoice_id, &t.holder_a, &400);
        t.frac.transfer_shares(&t.investor1, &t.investor1, &invoice_id, &t.holder_b, &300);

        assert_eq!(t.frac.get_share_balance(&invoice_id, &t.investor1, &t.investor1), 300);
        assert_eq!(t.frac.get_share_balance(&invoice_id, &t.investor1, &t.holder_a), 400);
        assert_eq!(t.frac.get_share_balance(&invoice_id, &t.investor1, &t.holder_b), 300);

        // Repay invoice fully to close pool
        t.pool.repay(&t.sme, &invoice_id, &t.token, &REPAY_AMOUNT);

        let pool_data = t.pool.get_pool(&invoice_id).expect("pool must exist");
        assert!(pool_data.is_closed, "Pool must be closed after full repayment");
        assert_eq!(pool_data.repaid_amount, REPAY_AMOUNT);

        // Settle the wrap
        t.frac.settle(&invoice_id, &t.investor1);

        // Redeem — verify pro-rata payouts
        // holder_a: 400/1000 × 10_000_000 = 4_000_000
        let payout_a = t.frac.redeem(&t.holder_a, &invoice_id, &t.investor1);
        assert_eq!(payout_a, 4_000_000i128, "holder_a 40% payout");

        // holder_b: 300/1000 = 3_000_000
        let payout_b = t.frac.redeem(&t.holder_b, &invoice_id, &t.investor1);
        assert_eq!(payout_b, 3_000_000i128, "holder_b 30% payout");

        // investor1: 300/1000 = 3_000_000 (last redeemer absorbs dust)
        let payout_i1 = t.frac.redeem(&t.investor1, &invoice_id, &t.investor1);
        assert_eq!(payout_i1, 3_000_000i128, "investor1 30% payout");

        // Total payouts must equal repaid amount
        let total_paid = payout_a
            .checked_add(payout_b)
            .and_then(|v| v.checked_add(payout_i1))
            .expect("payout sum overflow");
        assert_eq!(total_paid, REPAY_AMOUNT, "Sum of all redeems must equal repaid_amount");

        // Post-redeem: all share balances are 0
        assert_eq!(t.frac.get_share_balance(&invoice_id, &t.investor1, &t.holder_a), 0);
        assert_eq!(t.frac.get_share_balance(&invoice_id, &t.investor1, &t.holder_b), 0);
        assert_eq!(t.frac.get_share_balance(&invoice_id, &t.investor1, &t.investor1), 0);
    }

    /// Dust (remainder from integer division) goes to the last redeemer.
    #[test]
    fn test_e2e_dust_goes_to_last_redeemer() {
        let t = setup();

        // odd_amount is not divisible by 3
        let odd_amount = 1_000_001i128;
        let invoice_id = create_pool(&t, odd_amount);
        fund_position(&t, invoice_id, &t.investor1, odd_amount, odd_amount);
        t.frac.wrap(&t.investor1, &invoice_id, &3u64);

        // Give 1 share to holder_a; keep 2 with investor1
        t.frac.transfer_shares(&t.investor1, &t.investor1, &invoice_id, &t.holder_a, &1);

        t.pool.repay(&t.sme, &invoice_id, &t.token, &odd_amount);
        t.frac.settle(&invoice_id, &t.investor1);

        // holder_a (1 share): floor(1_000_001 * 1 / 3) = 333_333
        let payout_a = t.frac.redeem(&t.holder_a, &invoice_id, &t.investor1);
        assert_eq!(payout_a, 333_333i128, "holder_a 1-share payout");

        // investor1 (2 shares, last redeemer): 1_000_001 − 333_333 = 666_668
        let payout_i1 = t.frac.redeem(&t.investor1, &invoice_id, &t.investor1);
        assert_eq!(payout_i1, 666_668i128, "last redeemer absorbs dust");

        let total = payout_a.checked_add(payout_i1).expect("payout sum overflow");
        assert_eq!(total, odd_amount, "Total must equal odd repayment amount");
    }

    // ── Edge case: redeem before settle ──────────────────────────────────────

    /// Redeem before settle must fail with PoolNotClosed.
    #[test]
    fn test_redeem_before_settle_fails_pool_not_closed() {
        let t = setup();
        let invoice_id = create_pool(&t, INVOICE_AMOUNT);
        fund_position(&t, invoice_id, &t.investor1, INVOICE_AMOUNT, INVOICE_AMOUNT);
        t.frac.wrap(&t.investor1, &invoice_id, &TOTAL_SHARES);

        // Pool still open — no repay, no settle
        let result = t.frac.try_redeem(&t.investor1, &invoice_id, &t.investor1);
        assert!(result.is_err(), "Redeem before settle must fail (PoolNotClosed)");
    }

    // ── Edge case: double redeem ───────────────────────────────────────────────

    /// Second redeem by the same holder must fail with AlreadyClaimed.
    #[test]
    fn test_double_redeem_fails_already_claimed() {
        let t = setup();
        let invoice_id = create_pool(&t, INVOICE_AMOUNT);
        fund_position(&t, invoice_id, &t.investor1, INVOICE_AMOUNT, INVOICE_AMOUNT);
        t.frac.wrap(&t.investor1, &invoice_id, &TOTAL_SHARES);
        t.pool.repay(&t.sme, &invoice_id, &t.token, &REPAY_AMOUNT);
        t.frac.settle(&invoice_id, &t.investor1);

        let result1 = t.frac.try_redeem(&t.investor1, &invoice_id, &t.investor1);
        assert!(result1.is_ok(), "First redeem must succeed");

        let result2 = t.frac.try_redeem(&t.investor1, &invoice_id, &t.investor1);
        assert!(result2.is_err(), "Double redeem must fail with AlreadyClaimed");
    }

    // ── Edge case: zero-share holder ──────────────────────────────────────────

    /// Holder with zero shares gets NotShareHolder — no panic.
    #[test]
    fn test_zero_share_redeem_fails_not_share_holder_no_panic() {
        let t = setup();
        let invoice_id = create_pool(&t, INVOICE_AMOUNT);
        fund_position(&t, invoice_id, &t.investor1, INVOICE_AMOUNT, INVOICE_AMOUNT);
        t.frac.wrap(&t.investor1, &invoice_id, &TOTAL_SHARES);
        t.pool.repay(&t.sme, &invoice_id, &t.token, &REPAY_AMOUNT);
        t.frac.settle(&invoice_id, &t.investor1);

        // investor2 never received shares from this wrap
        let result = t.frac.try_redeem(&t.investor2, &invoice_id, &t.investor1);
        assert!(result.is_err(), "Zero-share redeem must return NotShareHolder — no panic");
    }

    // ── Edge case: mark_default haircutting ───────────────────────────────────

    /// After mark_default the pool is closed with repaid_amount == 0.
    /// Settle records 0 payout; redeem distributes 0 tokens (100% haircut).
    #[test]
    fn test_mark_default_haircuts_redeem_to_zero() {
        let t = setup();
        let invoice_id = create_pool(&t, INVOICE_AMOUNT);
        fund_position(&t, invoice_id, &t.investor1, INVOICE_AMOUNT, INVOICE_AMOUNT);
        t.frac.wrap(&t.investor1, &invoice_id, &TOTAL_SHARES);

        // Advance time past due_date and mark default
        t.env.ledger().with_mut(|li| {
            li.timestamp = li.timestamp.saturating_add(86_400 * 365);
        });
        t.pool.mark_default(&t.admin, &invoice_id, &t.token);

        let pool_data = t.pool.get_pool(&invoice_id).expect("pool must exist");
        assert!(pool_data.is_closed, "Pool must be closed after mark_default");
        // repaid_amount stays 0 after default (no repayment was made)
        assert_eq!(pool_data.repaid_amount, 0, "repaid_amount must be 0 after default");

        // Settle records 0 payout
        t.frac.settle(&invoice_id, &t.investor1);

        // Redeem returns 0 (full haircut)
        let payout = t.frac.redeem(&t.investor1, &invoice_id, &t.investor1);
        assert_eq!(payout, 0i128, "After mark_default payout must be 0 (haircut)");
    }

    // ── Edge case: repay_partial then complete repay ───────────────────────────

    /// partial repay followed by final repay — settle/redeem use full repaid_amount.
    #[test]
    fn test_repay_partial_then_complete_repay_correct_payout() {
        let t = setup();
        let face = 10_000_000i128;
        let partial = 6_000_000i128;
        let remainder = face.checked_sub(partial).expect("arithmetic overflow");

        let invoice_id = create_pool(&t, face);
        fund_position(&t, invoice_id, &t.investor1, face, face);
        t.frac.wrap(&t.investor1, &invoice_id, &1_000u64);

        // Transfer 500 shares to holder_a
        t.frac.transfer_shares(&t.investor1, &t.investor1, &invoice_id, &t.holder_a, &500);

        // First: partial repayment (pool stays open)
        t.pool.repay_partial(&t.sme, &invoice_id, &t.token, &partial);

        // Complete repayment closes the pool
        t.pool.repay(&t.sme, &invoice_id, &t.token, &remainder);

        let pool_data = t.pool.get_pool(&invoice_id).expect("pool must exist");
        assert!(pool_data.is_closed, "Pool must be closed after complete repayment");
        assert_eq!(pool_data.repaid_amount, face, "Full repaid_amount must be recorded");

        // Settle and redeem
        t.frac.settle(&invoice_id, &t.investor1);

        let payout_a = t.frac.redeem(&t.holder_a, &invoice_id, &t.investor1);
        assert_eq!(payout_a, 5_000_000i128, "holder_a 50% = 5_000_000");

        let payout_i1 = t.frac.redeem(&t.investor1, &invoice_id, &t.investor1);
        assert_eq!(payout_i1, 5_000_000i128, "investor1 50% = 5_000_000");

        let total = payout_a.checked_add(payout_i1).expect("payout sum overflow");
        assert_eq!(total, face, "Total must equal full face value");
    }

    // ── Multiple investors fractionalize the same pool ─────────────────────────

    /// Two investors each wrap their own position from the same pool.
    /// Each wrap is independent; payouts are proportional to contributed amounts.
    #[test]
    fn test_multiple_investors_fractionalize_same_pool() {
        let t = setup();
        let total = INVESTOR1_CONTRIBUTION
            .checked_add(INVESTOR2_CONTRIBUTION)
            .expect("total overflow");

        let invoice_id = create_pool(&t, total);
        fund_position(&t, invoice_id, &t.investor1, INVESTOR1_CONTRIBUTION, total);
        fund_position(&t, invoice_id, &t.investor2, INVESTOR2_CONTRIBUTION, total);

        // Each investor wraps their own position independently
        t.frac.wrap(&t.investor1, &invoice_id, &600u64); // 60% pool share
        t.frac.wrap(&t.investor2, &invoice_id, &400u64); // 40% pool share

        // Distribute shares
        t.frac.transfer_shares(&t.investor1, &t.investor1, &invoice_id, &t.holder_a, &200);
        t.frac.transfer_shares(&t.investor2, &t.investor2, &invoice_id, &t.holder_b, &200);

        // Repay fully
        t.pool.repay(&t.sme, &invoice_id, &t.token, &total);

        let pool_data = t.pool.get_pool(&invoice_id).expect("pool must exist");
        assert!(pool_data.is_closed, "Pool must be closed");

        // Settle both wraps
        t.frac.settle(&invoice_id, &t.investor1);
        t.frac.settle(&invoice_id, &t.investor2);

        // Investor1 wrap total_payout:
        //   share_bps = INVESTOR1_CONTRIBUTION * 10_000 / total = 6_000_000 * 10000 / 10_000_000 = 6000
        //   payout = repaid_amount * share_bps / 10_000 = 10_000_000 * 6000 / 10_000 = 6_000_000
        // holder_a: 200/600 × 6_000_000 = 2_000_000
        let payout_a = t.frac.redeem(&t.holder_a, &invoice_id, &t.investor1);
        assert_eq!(payout_a, 2_000_000i128, "holder_a from wrap1");

        // investor1: 400/600 × 6_000_000 = 4_000_000 (last redeemer)
        let payout_i1 = t.frac.redeem(&t.investor1, &invoice_id, &t.investor1);
        assert_eq!(payout_i1, 4_000_000i128, "investor1 from wrap1");

        // Investor2 wrap total_payout:
        //   share_bps = 4_000_000 * 10_000 / 10_000_000 = 4000
        //   payout = 10_000_000 * 4000 / 10_000 = 4_000_000
        // holder_b: 200/400 × 4_000_000 = 2_000_000
        let payout_b = t.frac.redeem(&t.holder_b, &invoice_id, &t.investor2);
        assert_eq!(payout_b, 2_000_000i128, "holder_b from wrap2");

        // investor2: 200/400 × 4_000_000 = 2_000_000 (last redeemer)
        let payout_i2 = t.frac.redeem(&t.investor2, &invoice_id, &t.investor2);
        assert_eq!(payout_i2, 2_000_000i128, "investor2 from wrap2");

        // Grand total must equal total repaid
        let grand_total = payout_a
            .checked_add(payout_i1)
            .and_then(|v| v.checked_add(payout_b))
            .and_then(|v| v.checked_add(payout_i2))
            .expect("grand total overflow");
        assert_eq!(grand_total, total, "Grand total must equal full repaid amount");
    }

    // ── Settle is idempotent ───────────────────────────────────────────────────

    /// Calling settle twice must be a no-op and must not affect redeem payouts.
    #[test]
    fn test_settle_is_idempotent() {
        let t = setup();
        let invoice_id = create_pool(&t, INVOICE_AMOUNT);
        fund_position(&t, invoice_id, &t.investor1, INVOICE_AMOUNT, INVOICE_AMOUNT);
        t.frac.wrap(&t.investor1, &invoice_id, &TOTAL_SHARES);
        t.pool.repay(&t.sme, &invoice_id, &t.token, &REPAY_AMOUNT);

        t.frac.settle(&invoice_id, &t.investor1);
        // Second settle — must not error
        let result = t.frac.try_settle(&invoice_id, &t.investor1);
        assert!(result.is_ok(), "Second settle must be a no-op (idempotent)");

        // Full payout still available
        let payout = t.frac.redeem(&t.investor1, &invoice_id, &t.investor1);
        assert_eq!(payout, REPAY_AMOUNT, "Full payout intact after double-settle");
    }

    // ── Pool aggregate and share balance tracking ──────────────────────────────

    /// Verify share balances and pool state are correctly tracked step-by-step.
    #[test]
    fn test_pool_aggregates_and_share_balances_tracked_correctly() {
        let t = setup();
        let invoice_id = create_pool(&t, INVOICE_AMOUNT);
        fund_position(&t, invoice_id, &t.investor1, INVOICE_AMOUNT, INVOICE_AMOUNT);

        // Pre-wrap: investor1 holds a pool position
        let pos_before = t.pool.get_position(&invoice_id, &t.investor1);
        assert_eq!(pos_before.contributed, INVOICE_AMOUNT, "Pre-wrap position contributed");

        // Wrap
        t.frac.wrap(&t.investor1, &invoice_id, &TOTAL_SHARES);

        // Post-wrap: fractionalizer owns the position
        let frac_pos = t.pool.get_position(&invoice_id, &t.frac.address);
        assert_eq!(frac_pos.contributed, INVOICE_AMOUNT, "Fractionalizer must own position after wrap");

        // Investor1 holds all shares
        assert_eq!(
            t.frac.get_share_balance(&invoice_id, &t.investor1, &t.investor1),
            TOTAL_SHARES,
            "Investor1 must hold TOTAL_SHARES"
        );

        // Repay and settle
        t.pool.repay(&t.sme, &invoice_id, &t.token, &REPAY_AMOUNT);
        t.frac.settle(&invoice_id, &t.investor1);

        // Redeem
        t.frac.redeem(&t.investor1, &invoice_id, &t.investor1);

        // Post-redeem: share balance is 0
        assert_eq!(
            t.frac.get_share_balance(&invoice_id, &t.investor1, &t.investor1),
            0,
            "Share balance must be 0 after redeem"
        );

        // Cannot redeem again (AlreadyClaimed)
        let result = t.frac.try_redeem(&t.investor1, &invoice_id, &t.investor1);
        assert!(result.is_err(), "Post-redeem double-claim must fail");
    }
}
