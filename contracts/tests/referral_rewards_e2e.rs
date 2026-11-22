//! End-to-end referral reward integration test (#1089).
//!
//! Exercises the real, multi-contract sequence that unit tests cannot cover:
//!
//!   1. treasury admin deposits a reward allocation into `referral_rewards`
//!   2. an SME mints its **first** invoice with a referrer (`invoice_nft`)
//!   3. the invoice is listed + funded, opening a `financing_pool` pool
//!   4. the SME repays in full, which closes the pool and writes the canonical
//!      `Repaid` status back to `invoice_nft`
//!   5. anyone (permissionless) calls `claim_referral_reward(sme)` and the fixed
//!      bonus is paid to the referrer exactly once
//!
//! Every contract here is driven through its real entrypoints — no status
//! shortcuts (`set_repaid` is only ever reached by the pool's `repay`) — so
//! sequencing bugs between the rewards, NFT and pool contracts surface here.

#[path = "src/lib.rs"]
#[allow(dead_code)]
mod shared;

use kora_access_control::{AccessControlContract, AccessControlContractClient};
use kora_financing_pool::{FinancingPoolContract, FinancingPoolContractClient};
use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient, InvoiceNftError};
use kora_referral_rewards::{ReferralRewards, ReferralRewardsClient};
use kora_shared::types::InvoiceStatus;
use shared::TestWorld;
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger, LedgerInfo},
    token, Address, Bytes, Env, IntoVal, String, Symbol, Val,
};

const FACE: i128 = 1_000;
const PRINCIPAL: i128 = 800;
const SME_BALANCE: i128 = 3_000;
const BONUS: i128 = 100;
const ALLOCATION: i128 = 1_000;
const CID: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

/// Deployed contracts + actors for a single referral scenario.
struct Harness {
    env: Env,
    admin: Address,
    sme: Address,
    referrer: Address,
    investor: Address,
    marketplace: Address,
    token: Address,
    ac: AccessControlContractClient<'static>,
    nft: InvoiceNftContractClient<'static>,
    pool: FinancingPoolContractClient<'static>,
    rewards: ReferralRewardsClient<'static>,
}

impl Harness {
    fn new() -> Self {
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
            max_entry_ttl: 1_000_000,
        });

        let world = TestWorld::builder(&env).signers(1).smes(1).build();
        let admin = world.admin;
        let sme = world.smes.get(0).unwrap().address;
        let referrer = Address::generate(&env);
        let investor = Address::generate(&env);
        let marketplace = Address::generate(&env);

        // ── Access control (shared pause switch) ──────────────────────────────
        let ac_id = env.register_contract(None, AccessControlContract);
        let ac = AccessControlContractClient::new(&env, &ac_id);
        ac.initialize(&admin);

        // ── Invoice NFT ───────────────────────────────────────────────────────
        let nft_id = env.register_contract(None, InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac_id);

        // ── Financing pool ────────────────────────────────────────────────────
        let pool_id = env.register_contract(None, FinancingPoolContract);
        let pool = FinancingPoolContractClient::new(&env, &pool_id);
        pool.initialize(
            &admin,
            &nft_id,
            &Address::generate(&env), // risk_registry (unused on this path)
            &Address::generate(&env), // treasury (no penalty accrues: rate = 0)
            &ac_id,
            &0u32,                    // late_penalty_bps
            &Address::generate(&env), // price_oracle
            &10_000u32,               // max_position_bps
            &0u64,                    // grace_period
            &Address::generate(&env), // dispute_resolution
        );

        // Wire the pool + marketplace as the only authorized invoice callers.
        nft.set_authorized_callers(&admin, &marketplace, &pool_id);

        // ── Reward token ──────────────────────────────────────────────────────
        let token_admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_admin).address();
        let asset = token::StellarAssetClient::new(&env, &token);
        asset.mint(&admin, &ALLOCATION); // funds the referral deposit
        asset.mint(&sme, &SME_BALANCE); // funds the SME's repayment

        // ── Referral rewards ──────────────────────────────────────────────────
        let rewards_id = env.register_contract(None, ReferralRewards);
        let rewards = ReferralRewardsClient::new(&env, &rewards_id);
        rewards.initialize(&admin, &nft_id, &token, &BONUS, &ALLOCATION);
        rewards.deposit(&admin, &ALLOCATION);

        Self {
            env,
            admin,
            sme,
            referrer,
            investor,
            marketplace,
            token,
            ac,
            nft,
            pool,
            rewards,
        }
    }

    /// Invoice currency symbol, sourced from the reward token so the pool's
    /// currency conversion is a no-op (`convert_if_needed`).
    fn currency(&self) -> Symbol {
        let symbol = token::Client::new(&self.env, &self.token).symbol();
        let mut bytes = std::vec![0u8; symbol.len() as usize];
        symbol.copy_into_slice(&mut bytes);
        Symbol::new(&self.env, std::str::from_utf8(&bytes).unwrap())
    }

    fn balance(&self, who: &Address) -> i128 {
        token::Client::new(&self.env, &self.token).balance(who)
    }

    /// Mint the SME's first invoice, attributing an optional referrer.
    fn mint_first_invoice(&self, referrer: Option<Address>) -> u64 {
        self.nft.mint_invoice_with_referrer(
            &self.sme,
            &Bytes::from_slice(&self.env, &[7; 32]),
            &FACE,
            &self.currency(),
            &(self.env.ledger().timestamp() + 86_400),
            &String::from_str(&self.env, CID),
            &10u32,
            &None,
            &referrer,
        )
    }

    /// List and fund the invoice, opening a pool and moving it to `Funded`.
    fn list_and_fund(&self, invoice_id: u64) {
        self.nft.set_listed(&self.marketplace, &invoice_id);
        self.pool
            .release_funds(&self.marketplace, &invoice_id, &self.token);
        self.pool.record_position(
            &self.admin,
            &invoice_id,
            &self.investor,
            &PRINCIPAL,
            &PRINCIPAL,
        );
        assert_eq!(
            self.nft.get_invoice(&invoice_id).status,
            InvoiceStatus::Funded
        );
    }

    /// Full repayment through the pool; the pool closes and marks the NFT `Repaid`.
    fn repay_in_full(&self, invoice_id: u64) {
        self.pool.repay(&self.sme, &invoice_id, &self.token, &FACE);
        assert_eq!(
            self.nft.get_invoice(&invoice_id).status,
            InvoiceStatus::Repaid
        );
    }
}

/// Assert the rewards contract emitted `referral(sme) -> (referrer, id, bonus)`.
fn assert_referral_event(
    env: &Env,
    rewards: &Address,
    sme: &Address,
    referrer: &Address,
    invoice_id: u64,
    bonus: i128,
) {
    let topic0: Val = symbol_short!("referral").into_val(env);
    let topic1: Val = sme.clone().into_val(env);
    let expected_data: Val = (referrer.clone(), invoice_id, bonus).into_val(env);

    let events = env.events().all();
    let mut matched = false;
    for i in 0..events.len() {
        let (contract, topics, data) = events.get(i).unwrap();
        if contract == *rewards
            && topics.len() == 2
            && topics.get(0).unwrap() == topic0
            && topics.get(1).unwrap() == topic1
            && data == expected_data
        {
            matched = true;
        }
    }
    assert!(matched, "expected a referral reward event from the rewards contract");
}

// ── Acceptance criteria ───────────────────────────────────────────────────────

/// Happy path: deposit -> repay -> claim pays the referrer exactly once.
#[test]
fn deposit_repaid_then_claim_pays_referrer_once() {
    let h = Harness::new();

    // The reward contract is pre-funded up to its immutable allocation.
    assert_eq!(h.balance(&h.rewards.address), ALLOCATION);

    let id = h.mint_first_invoice(Some(h.referrer.clone()));
    assert_eq!(h.nft.get_first_mint(&h.sme), Some(id));
    assert_eq!(h.nft.get_sme_referrer(&h.sme), Some(h.referrer.clone()));

    h.list_and_fund(id);

    // Eligibility only exists once the first invoice is canonically Repaid.
    assert!(!h.rewards.claim_referral_reward(&h.sme));
    assert_eq!(h.rewards.paid_total(), 0);

    h.repay_in_full(id);

    let referrer_before = h.balance(&h.referrer);
    assert!(h.rewards.claim_referral_reward(&h.sme), "claim must succeed once repaid");
    assert!(h.rewards.has_claimed(&h.sme));
    assert_eq!(h.balance(&h.referrer), referrer_before + BONUS);
    assert_eq!(h.rewards.paid_total(), BONUS);
    assert_eq!(h.balance(&h.rewards.address), ALLOCATION - BONUS);

    assert_referral_event(&h.env, &h.rewards.address, &h.sme, &h.referrer, id, BONUS);
}

/// A premature claim (invoice only Funded) must not pay.
#[test]
fn premature_claim_before_repayment_is_rejected() {
    let h = Harness::new();
    let id = h.mint_first_invoice(Some(h.referrer.clone()));
    h.list_and_fund(id);

    assert!(!h.rewards.claim_referral_reward(&h.sme));
    assert!(!h.rewards.has_claimed(&h.sme));
    assert_eq!(h.rewards.paid_total(), 0);
    assert_eq!(h.balance(&h.referrer), 0);

    // The only missing condition was repayment: after the pool repays, it pays.
    h.repay_in_full(id);
    assert!(h.rewards.claim_referral_reward(&h.sme));
    assert_eq!(h.rewards.paid_total(), BONUS);
}

/// A second claim for the same SME must be rejected with no extra payout.
#[test]
fn double_claim_is_rejected() {
    let h = Harness::new();
    let id = h.mint_first_invoice(Some(h.referrer.clone()));
    h.list_and_fund(id);
    h.repay_in_full(id);

    assert!(h.rewards.claim_referral_reward(&h.sme));

    let referrer_after_first = h.balance(&h.referrer);
    let rewards_after_first = h.balance(&h.rewards.address);

    assert!(!h.rewards.claim_referral_reward(&h.sme), "double claim must not pay");
    assert_eq!(h.balance(&h.referrer), referrer_after_first);
    assert_eq!(h.balance(&h.rewards.address), rewards_after_first);
    assert_eq!(h.rewards.paid_total(), BONUS);
}

/// Self-referral is forbidden at mint time, so no reward can ever accrue.
#[test]
fn self_referral_mint_is_rejected() {
    let h = Harness::new();

    let result = h.nft.try_mint_invoice_with_referrer(
        &h.sme,
        &Bytes::from_slice(&h.env, &[8; 32]),
        &FACE,
        &h.currency(),
        &(h.env.ledger().timestamp() + 86_400),
        &String::from_str(&h.env, CID),
        &10u32,
        &None,
        &Some(h.sme.clone()),
    );
    assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidReferrer);

    // No first mint was recorded and no reward is claimable.
    assert_eq!(h.nft.get_first_mint(&h.sme), None);
    assert!(!h.rewards.claim_referral_reward(&h.sme));
    assert_eq!(h.rewards.paid_total(), 0);
}

/// While the shared protocol pause is active the invoice cannot reach `Repaid`,
/// so the reward stays ineligible; unpausing restores the flow.
#[test]
fn paused_protocol_blocks_repaid_transition_and_claim() {
    let h = Harness::new();
    let id = h.mint_first_invoice(Some(h.referrer.clone()));
    h.list_and_fund(id);

    h.ac.pause(&h.admin);

    // The canonical Funded -> Repaid transition is gated by the pause switch.
    assert_eq!(
        h.nft.try_set_repaid(&h.pool.address, &id),
        Err(Ok(InvoiceNftError::ProtocolPaused))
    );

    // No reward can be paid while the invoice is ineligible.
    assert!(!h.rewards.claim_referral_reward(&h.sme));
    assert!(!h.rewards.has_claimed(&h.sme));
    assert_eq!(h.rewards.paid_total(), 0);

    // Unpause: repayment drives the real status transition and the claim pays.
    h.ac.unpause(&h.admin);
    h.repay_in_full(id);
    assert!(h.rewards.claim_referral_reward(&h.sme));
    assert_eq!(h.rewards.paid_total(), BONUS);
}
