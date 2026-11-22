// tests/referral_deposit_to_claim.rs  — Issue #1036
//!
//! Workspace integration test for the full referral happy path:
//!
//!   mint_invoice_with_referrer
//!     → fund (via marketplace + financing_pool)
//!     → repay
//!     → invoice_nft.set_repaid
//!     → permissionless claim_referral_reward
//!
//! Assertions:
//!   • The referrer receives exactly the configured bonus token amount.
//!   • A `referral` event is emitted with the correct (referrer, invoice_id, bonus) tuple.
//!   • A second claim for the same SME returns `false` (idempotent, no double-spend).
//!   • When the allocation is fully exhausted a subsequent eligible SME gets `false`.

#[cfg(test)]
mod referral_deposit_to_claim {
    use kora_access_control::AccessControlContract;
    use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient};
    use kora_referral_rewards::{ReferralRewards, ReferralRewardsClient};
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Events, Ledger, LedgerInfo},
        token::{Client as TokenClient, StellarAssetClient},
        Address, Bytes, Env, String, Symbol,
    };

    // ── Ledger constants ──────────────────────────────────────────────────────

    const BASE_TS: u64 = 1_700_000_000;
    const DUE_IN: u64 = 86_400; // 1 day ahead

    // ── Fixture ───────────────────────────────────────────────────────────────

    struct Fixture<'a> {
        env: Env,
        admin: Address,
        market: Address, // simulated marketplace caller
        pool: Address,   // simulated financing_pool caller
        sme: Address,
        referrer: Address,
        token: Address,
        nft: InvoiceNftContractClient<'a>,
        rewards: ReferralRewardsClient<'a>,
    }

    fn setup_fixture<'a>(bonus: i128, allocation: i128) -> Fixture<'a> {
        let env = Env::default();
        env.mock_all_auths();

        env.ledger().set(LedgerInfo {
            timestamp: BASE_TS,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 600_000,
        });

        let admin = Address::generate(&env);
        let market = Address::generate(&env);
        let pool = Address::generate(&env);
        let sme = Address::generate(&env);
        let referrer = Address::generate(&env);

        // Deploy access_control (required by invoice_nft for auth)
        let ac = env.register_contract(None, AccessControlContract);

        // Deploy invoice_nft
        let nft_addr = env.register_contract(None, InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_addr);
        nft.initialize(&admin, &ac);
        nft.set_authorized_callers(&admin, &market, &pool);

        // Mint and fund the token (100 units available to admin)
        let token_addr = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        StellarAssetClient::new(&env, &token_addr).mint(&admin, &allocation);

        // Deploy referral_rewards
        let rewards_addr = env.register_contract(None, ReferralRewards);
        let rewards = ReferralRewardsClient::new(&env, &rewards_addr);
        rewards.initialize(&admin, &nft_addr, &token_addr, &bonus, &allocation);

        Fixture {
            env,
            admin,
            market,
            pool,
            sme,
            referrer,
            token: token_addr,
            nft,
            rewards,
        }
    }

    /// Mint an invoice for `sme` attributed to `referrer`, then walk it through
    /// the full lifecycle up to `set_repaid`.  Returns the invoice id.
    fn mint_fund_repay(f: &Fixture, sme: &Address, referrer: Option<Address>) -> u64 {
        let hash = Bytes::from_slice(&f.env, &[0xAB; 32]);
        let cid = String::from_str(
            &f.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due = f.env.ledger().timestamp() + DUE_IN;

        let id = f.nft.mint_invoice_with_referrer(
            sme,
            &hash,
            &1_000_000,
            &Symbol::new(&f.env, "USDC"),
            &due,
            &cid,
            &30,
            &None,
            &referrer,
        );

        // Simulate marketplace listing and financing_pool funding / repayment
        f.nft.set_listed(&f.market, &id);
        f.nft.set_funded(&f.pool, &id);
        f.nft.set_repaid(&f.pool, &id);
        id
    }

    // ── Happy path ────────────────────────────────────────────────────────────

    #[test]
    fn happy_path_referrer_receives_bonus() {
        let f = setup_fixture(10, 50);

        // Fund the reward pool
        f.rewards.deposit(&f.admin, &10);

        let referrer_balance_before =
            TokenClient::new(&f.env, &f.token).balance(&f.referrer);
        assert_eq!(referrer_balance_before, 0);

        let _invoice_id = mint_fund_repay(&f, &f.sme, Some(f.referrer.clone()));

        let claimed = f.rewards.claim_referral_reward(&f.sme);
        assert!(claimed, "first claim must succeed");

        let referrer_balance_after =
            TokenClient::new(&f.env, &f.token).balance(&f.referrer);
        assert_eq!(
            referrer_balance_after, 10,
            "referrer must receive exactly the bonus amount"
        );
    }

    // ── Event emission ────────────────────────────────────────────────────────

    #[test]
    fn happy_path_emits_referral_event() {
        let f = setup_fixture(10, 50);
        f.rewards.deposit(&f.admin, &10);

        let invoice_id = mint_fund_repay(&f, &f.sme, Some(f.referrer.clone()));
        f.rewards.claim_referral_reward(&f.sme);

        // The event topic is (symbol_short!("referral"), sme) and the data is
        // (referrer, first_invoice_id, bonus).
        let events = f.env.events().all();
        let found = events.iter().any(|(contract, topics, _data)| {
            contract == f.rewards.address
                && topics.len() == 2
                && topics.get(0)
                    == Some(soroban_sdk::Val::from(symbol_short!("referral")))
        });
        assert!(found, "referral event must be emitted after a successful claim");

        // Verify the invoice id appears in the data payload
        let claim_event = events
            .iter()
            .find(|(c, t, _)| {
                *c == f.rewards.address
                    && t.get(0) == Some(soroban_sdk::Val::from(symbol_short!("referral")))
            });
        assert!(claim_event.is_some());
        let (_, _, data) = claim_event.unwrap();
        // data = (referrer: Address, first: u64, bonus: i128)
        let (emitted_referrer, emitted_id, emitted_bonus): (Address, u64, i128) =
            f.env.from_val(data);
        assert_eq!(emitted_referrer, f.referrer);
        assert_eq!(emitted_id, invoice_id);
        assert_eq!(emitted_bonus, 10);
    }

    // ── Idempotency (double-claim) ─────────────────────────────────────────────

    #[test]
    fn second_claim_returns_false_no_double_spend() {
        let f = setup_fixture(10, 50);
        f.rewards.deposit(&f.admin, &20); // fund two bonuses

        mint_fund_repay(&f, &f.sme, Some(f.referrer.clone()));

        assert!(f.rewards.claim_referral_reward(&f.sme), "first claim");
        assert!(
            !f.rewards.claim_referral_reward(&f.sme),
            "second claim must be a no-op"
        );

        // Referrer should still only hold one bonus worth
        let bal = TokenClient::new(&f.env, &f.token).balance(&f.referrer);
        assert_eq!(bal, 10, "no double-spend: referrer must hold exactly one bonus");
    }

    // ── Exhausted allocation ──────────────────────────────────────────────────

    #[test]
    fn exhausted_allocation_returns_false() {
        // allocation = 1 bonus; two eligible SMEs, only the first can claim.
        let f = setup_fixture(10, 10);
        f.rewards.deposit(&f.admin, &10); // fills the full allocation

        let sme2 = Address::generate(&f.env);
        let referrer2 = Address::generate(&f.env);

        mint_fund_repay(&f, &f.sme, Some(f.referrer.clone()));
        mint_fund_repay(&f, &sme2, Some(referrer2.clone()));

        assert!(
            f.rewards.claim_referral_reward(&f.sme),
            "first SME must claim successfully"
        );

        // allocation is now exhausted
        assert!(
            !f.rewards.claim_referral_reward(&sme2),
            "second SME must get false when allocation is exhausted"
        );

        assert_eq!(f.rewards.paid_total(), 10);
    }

    // ── Claim before repaid is rejected ──────────────────────────────────────

    #[test]
    fn claim_before_repaid_returns_false() {
        let f = setup_fixture(10, 50);
        f.rewards.deposit(&f.admin, &10);

        // Only mint + list + fund, do NOT repay
        let hash = Bytes::from_slice(&f.env, &[0xAB; 32]);
        let cid = String::from_str(
            &f.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let id = f.nft.mint_invoice_with_referrer(
            &f.sme,
            &hash,
            &1_000_000,
            &Symbol::new(&f.env, "USDC"),
            &(BASE_TS + DUE_IN),
            &cid,
            &30,
            &None,
            &Some(f.referrer.clone()),
        );
        f.nft.set_listed(&f.market, &id);
        f.nft.set_funded(&f.pool, &id);
        // invoice is now Funded, not Repaid

        assert!(
            !f.rewards.claim_referral_reward(&f.sme),
            "claim must return false when invoice is not yet repaid"
        );
    }

    // ── Unattributed first mint is ineligible ────────────────────────────────

    #[test]
    fn unattributed_first_mint_ineligible() {
        let f = setup_fixture(10, 50);
        f.rewards.deposit(&f.admin, &10);

        // Mint without a referrer
        mint_fund_repay(&f, &f.sme, None);

        assert!(
            !f.rewards.claim_referral_reward(&f.sme),
            "SME with no referrer must not receive a reward"
        );
    }
}
