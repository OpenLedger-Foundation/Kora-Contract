//! Real resolution -> escrow -> financing-pool scenarios (#914).
//!
//! The two dispute APIs are independent. `resolve_and_settle` deliberately makes
//! two invocations: an authorized governance signer records an outcome, then the
//! escrow admin settles it. This is a harness policy, not production atomicity.
//! `upheld=true` refunds the SME; `upheld=false` releases repayment to investors
//! through the pool. There is no slashing enum or escalation entrypoint today;
//! submitting challenger evidence exercises the available escalation operation.

#[path = "src/lib.rs"]
#[allow(dead_code)]
mod shared;

use kora_access_control::{AccessControlContract, AccessControlContractClient};
use kora_dispute_escrow::{DisputeEscrowContract, DisputeEscrowContractClient, EscrowError};
use kora_dispute_resolution::{
    DisputeResolutionContract, DisputeResolutionContractClient, DisputeResolutionError,
};
use kora_financing_pool::{FinancingPoolContract, FinancingPoolContractClient, FinancingPoolError};
use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient};
use kora_shared::types::InvoiceStatus;
use shared::TestWorld;
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Ledger, LedgerInfo, MockAuth, MockAuthInvoke},
    token,
    xdr::{ScErrorCode, ScErrorType},
    Address, Bytes, Env, Error, IntoVal, String, Symbol,
};

const FACE: i128 = 1_000;
const PRINCIPAL: i128 = 800;
const SME_BALANCE: i128 = 3_000;
const CID: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

struct TestEnv {
    env: Env,
    admin: Address,
    arbiter: Address,
    sme: Address,
    investor: Address,
    marketplace: Address,
    token: Address,
    pool: FinancingPoolContractClient<'static>,
    escrow: DisputeEscrowContractClient<'static>,
    resolution: DisputeResolutionContractClient<'static>,
    nft: InvoiceNftContractClient<'static>,
}

impl TestEnv {
    fn new(malicious_token: bool) -> Self {
        Self::with_late_rate(malicious_token, 0)
    }

    fn with_late_rate(malicious_token: bool, late_rate: u32) -> Self {
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
        let arbiter = world.signers.get(0).unwrap().address;
        let sme = world.smes.get(0).unwrap().address;
        let investor = world.verifiers.get(0).unwrap().address;
        let marketplace = Address::generate(&env);
        let ac_id = env.register_contract(None, AccessControlContract);
        let ac = AccessControlContractClient::new(&env, &ac_id);
        ac.initialize(&admin);
        ac.configure_multisig(&admin, &soroban_sdk::vec![&env, arbiter.clone()], &1);
        let nft_id = env.register_contract(None, InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac_id);
        let resolution_id = env.register_contract(None, DisputeResolutionContract);
        let resolution = DisputeResolutionContractClient::new(&env, &resolution_id);
        resolution.initialize(&admin, &ac_id);
        let pool_id = env.register_contract(None, FinancingPoolContract);
        let pool = FinancingPoolContractClient::new(&env, &pool_id);
        pool.initialize(
            &admin,
            &nft_id,
            &Address::generate(&env),
            &Address::generate(&env),
            &ac_id,
            &late_rate,
            &Address::generate(&env),
            &10_000,
            &0,
            &resolution_id,
        );
        // Issue #909: intentionally NOT calling pool.set_marketplace here. This
        // harness simulates the marketplace as an EOA address; with a stored
        // marketplace, release_funds now binds env.invoker() to it and direct
        // EOA calls (like the ones below) are rejected by design.
        nft.set_authorized_callers(&admin, &marketplace, &pool_id);
        let escrow_id = env.register_contract(None, DisputeEscrowContract);
        let escrow = DisputeEscrowContractClient::new(&env, &escrow_id);
        escrow.initialize(&admin, &pool_id);
        pool.set_dispute_escrow(&admin, &escrow_id);
        let token = if malicious_token {
            let id = env.register_contract(None, ReentrantToken);
            ReentrantTokenClient::new(&env, &id).mint(&sme, &SME_BALANCE);
            id
        } else {
            let id = env
                .register_stellar_asset_contract_v2(admin.clone())
                .address();
            token::StellarAssetClient::new(&env, &id).mint(&sme, &SME_BALANCE);
            id
        };
        Self {
            env,
            admin,
            arbiter,
            sme,
            investor,
            marketplace,
            token,
            pool,
            escrow,
            resolution,
            nft,
        }
    }

    fn invoice(&self) -> u64 {
        self.invoice_with(FACE, self.currency())
    }

    fn currency(&self) -> Symbol {
        let symbol = token::Client::new(&self.env, &self.token).symbol();
        let mut bytes = std::vec![0u8; symbol.len() as usize];
        symbol.copy_into_slice(&mut bytes);
        Symbol::new(&self.env, std::str::from_utf8(&bytes).unwrap())
    }

    fn invoice_with(&self, face: i128, currency: Symbol) -> u64 {
        let id = self.nft.mint_invoice(
            &self.sme,
            &Bytes::from_slice(&self.env, &[7; 32]),
            &face,
            &currency,
            &(self.env.ledger().timestamp() + 86_400),
            &String::from_str(&self.env, CID),
            &10,
            &None,
        );
        self.nft.set_listed(&self.marketplace, &id);
        self.pool.release_funds(&self.marketplace, &id, &self.token);
        // Marketplace funding is outside the disputed repayment under test.
        // Record its final allocation using the production admin entrypoint.
        self.pool.record_position(
            &self.admin,
            &id,
            &self.investor,
            &PRINCIPAL,
            &PRINCIPAL,
        );
        assert_eq!(self.nft.get_invoice(&id).status, InvoiceStatus::Funded);
        id
    }

    fn balances(&self, escrow: i128, pool: i128, sme: i128, investor: i128) {
        let token = token::Client::new(&self.env, &self.token);
        assert_eq!(token.balance(&self.escrow.address), escrow);
        assert_eq!(token.balance(&self.pool.address), pool);
        assert_eq!(token.balance(&self.sme), sme);
        assert_eq!(token.balance(&self.investor), investor);
    }

    fn open(&self, id: u64, amount: i128) {
        self.escrow.open_dispute(&self.investor, &self.sme, &id, &amount);
        self.resolution.open_dispute(&self.investor, &id);
    }

    fn resolve_and_settle(&self, id: u64, upheld: bool) {
        self.resolution.resolve_dispute(&self.arbiter, &id, &upheld);
        // Recording the verdict alone must not move any token.
        let held = self.escrow.get_dispute(&id).unwrap();
        assert!(!held.resolved);
        assert_eq!(
            token::Client::new(&self.env, &self.token).balance(&self.escrow.address),
            held.amount
        );
        let outcome = self.resolution.get_dispute(&id).unwrap();
        assert!(outcome.resolved);
        self.escrow.resolve_dispute(&self.admin, &id, &!outcome.upheld);
    }
}

#[test]
fn both_verdicts_route_real_tokens_and_preserve_pool_accounting() {
    for upheld in [false, true] {
        let t = TestEnv::new(false);
        let id = t.invoice();
        t.balances(0, 0, SME_BALANCE, 0);
        t.open(id, FACE);
        t.balances(FACE, 0, SME_BALANCE - FACE, 0);
        assert_eq!(
            t.escrow.try_open_dispute(&t.investor, &t.sme, &id, &FACE),
            Err(Ok(EscrowError::AlreadyDisputed))
        );
        assert!(t.resolution.has_open_dispute(&id));
        assert_eq!(t.pool.get_pool(&id).repaid_amount, 0);
        assert_eq!(
            t.pool.try_repay(&t.sme, &id, &t.token, &1),
            Err(Ok(FinancingPoolError::EscrowPending))
        );
        t.resolution
            .submit_evidence(&t.investor, &id, &String::from_str(&t.env, CID));
        assert_eq!(
            t.resolution.get_dispute(&id).unwrap().evidence_cid,
            Some(String::from_str(&t.env, CID))
        );
        t.balances(FACE, 0, SME_BALANCE - FACE, 0);
        t.resolve_and_settle(id, upheld);
        let pool = t.pool.get_pool(&id);
        let held = t.escrow.get_dispute(&id).unwrap();
        assert!(held.resolved);
        assert_eq!(held.paid_to_pool, !upheld);
        assert!(!t.resolution.has_open_dispute(&id));
        assert_eq!(pool.is_closed, !upheld);
        assert_eq!(pool.repaid_amount, if upheld { 0 } else { FACE });
        assert_eq!(
            t.nft.get_invoice(&id).status,
            if upheld {
                InvoiceStatus::Funded
            } else {
                InvoiceStatus::Repaid
            }
        );
        if upheld {
            t.balances(0, 0, SME_BALANCE, 0);
            // A refund releases the pool reservation: ordinary repayment works.
            t.pool.repay(&t.sme, &id, &t.token, &FACE);
        }
        t.balances(0, 0, SME_BALANCE - FACE, FACE);
        assert_eq!(
            token::Client::new(&t.env, &t.token).balance(&t.investor) - PRINCIPAL,
            200
        );
        assert_eq!(t.pool.get_position(&id, &t.investor).contributed, PRINCIPAL);
        assert_eq!(t.pool.get_protocol_stats().total_repaid, FACE);
        assert_eq!(t.pool.get_protocol_stats().active_pools, 0);
        assert_eq!(
            t.resolution.try_resolve_dispute(&t.arbiter, &id, &upheld),
            Err(Ok(DisputeResolutionError::DisputeAlreadyResolved))
        );
        assert_eq!(
            t.escrow.try_resolve_dispute(&t.admin, &id, &!upheld),
            Err(Ok(EscrowError::Resolved))
        );
        t.balances(0, 0, SME_BALANCE - FACE, FACE);
    }
}

#[test]
fn partial_escrow_keeps_tokens_until_remaining_repayment() {
    let t = TestEnv::new(false);
    let id = t.invoice();
    t.open(id, 400);
    t.balances(400, 0, SME_BALANCE - 400, 0);
    t.resolve_and_settle(id, false);
    t.balances(0, 400, SME_BALANCE - 400, 0);
    assert_eq!(t.pool.get_pool(&id).repaid_amount, 400);
    assert!(!t.pool.get_pool(&id).is_closed);
    assert_eq!(
        t.pool.try_repay_partial(&t.sme, &id, &t.token, &100),
        Err(Ok(FinancingPoolError::PartialRepayInvalid))
    );
    let other = t.invoice();
    assert_eq!(
        t.pool.try_net_settle(
            &t.sme,
            &soroban_sdk::vec![&t.env, id, other],
            &t.token,
            &600,
        ),
        Err(Ok(FinancingPoolError::PartialRepayInvalid))
    );
    t.balances(0, 400, SME_BALANCE - 400, 0);
    t.pool.repay(&t.sme, &id, &t.token, &600);
    t.balances(0, 0, SME_BALANCE - FACE, FACE);
    assert!(t.pool.get_pool(&id).is_closed);
    assert_eq!(t.nft.get_invoice(&id).status, InvoiceStatus::Repaid);
    assert_eq!(t.pool.get_protocol_stats().total_repaid, FACE);
}

#[test]
fn partial_escrow_default_distributes_custody_once() {
    let t = TestEnv::new(false);
    let id = t.invoice();
    t.open(id, 400);
    t.resolve_and_settle(id, false);
    t.balances(0, 400, SME_BALANCE - 400, 0);
    let due_date = t.nft.get_invoice(&id).due_date;
    t.env.ledger().with_mut(|ledger| {
        ledger.timestamp = due_date + 1;
        ledger.sequence_number += 1;
    });
    t.pool.mark_default(&t.admin, &id, &t.token);
    let pool = t.pool.get_pool(&id);
    assert!(pool.is_closed);
    assert_eq!(pool.repaid_amount, 400);
    assert_eq!(t.nft.get_invoice(&id).status, InvoiceStatus::Defaulted);
    t.balances(0, 0, SME_BALANCE - 400, 400);
    let stats = t.pool.get_protocol_stats();
    assert_eq!(stats.total_repaid, 400);
    assert_eq!(stats.active_pools, 0);
    assert_eq!(stats.pools_defaulted, 1);
    assert_eq!(
        t.pool.try_mark_default(&t.admin, &id, &t.token),
        Err(Ok(FinancingPoolError::PoolAlreadyClosed))
    );
    t.balances(0, 0, SME_BALANCE - 400, 400);
    assert_eq!(t.pool.get_protocol_stats().total_repaid, 400);
    assert_eq!(t.pool.get_protocol_stats().pools_defaulted, 1);
}

#[test]
fn role_checks_and_missing_signatures_cannot_release_the_hold() {
    let t = TestEnv::new(false);
    let id = t.invoice();
    t.open(id, FACE);
    let outsider = Address::generate(&t.env);
    t.env.mock_auths(&[MockAuth {
        address: &outsider,
        invoke: &MockAuthInvoke {
            contract: &t.resolution.address,
            fn_name: "resolve_dispute",
            args: (&outsider, id, false).into_val(&t.env),
            sub_invokes: &[],
        },
    }]);
    assert_eq!(
        t.resolution.try_resolve_dispute(&outsider, &id, &false),
        Err(Ok(DisputeResolutionError::NotGovernance))
    );
    t.env.mock_auths(&[MockAuth {
        address: &outsider,
        invoke: &MockAuthInvoke {
            contract: &t.escrow.address,
            fn_name: "resolve_dispute",
            args: (&outsider, id, true).into_val(&t.env),
            sub_invokes: &[],
        },
    }]);
    assert_eq!(
        t.escrow.try_resolve_dispute(&outsider, &id, &true),
        Err(Ok(EscrowError::Unauthorized))
    );
    t.env.mock_auths(&[]);
    assert!(t.resolution.try_resolve_dispute(&t.arbiter, &id, &false).is_err());
    assert!(t.escrow.try_resolve_dispute(&t.admin, &id, &true).is_err());
    // Naming the configured escrow as caller cannot forge contract auth.
    assert!(t
        .pool
        .try_settle_escrow(&t.escrow.address, &id, &FACE, &true)
        .is_err());
    assert!(t.pool.try_reserve_escrow(&t.escrow.address, &id, &FACE).is_err());
    assert!(!t.resolution.get_dispute(&id).unwrap().resolved);
    assert!(!t.escrow.get_dispute(&id).unwrap().resolved);
    assert_eq!(t.pool.get_pool(&id).repaid_amount, 0);
    t.balances(FACE, 0, SME_BALANCE - FACE, 0);
    // The same signature checks accept the configured signer and escrow admin
    // when those precise invocations are authorized, without mock_all_auths.
    t.env.mock_auths(&[MockAuth {
        address: &t.arbiter,
        invoke: &MockAuthInvoke {
            contract: &t.resolution.address,
            fn_name: "resolve_dispute",
            args: (&t.arbiter, id, false).into_val(&t.env),
            sub_invokes: &[],
        },
    }]);
    t.resolution.resolve_dispute(&t.arbiter, &id, &false);
    t.balances(FACE, 0, SME_BALANCE - FACE, 0);
    t.env.mock_auths(&[MockAuth {
        address: &t.admin,
        invoke: &MockAuthInvoke {
            contract: &t.escrow.address,
            fn_name: "resolve_dispute",
            args: (&t.admin, id, true).into_val(&t.env),
            sub_invokes: &[],
        },
    }]);
    t.escrow.resolve_dispute(&t.admin, &id, &true);
    t.balances(0, 0, SME_BALANCE - FACE, FACE);
}

#[test]
fn failed_transfer_rolls_back_hold_and_pool_reservation() {
    let t = TestEnv::new(false);
    let id = t.invoice();
    let sink = Address::generate(&t.env);
    token::Client::new(&t.env, &t.token).transfer(&t.sme, &sink, &(SME_BALANCE - 100));
    assert!(t.escrow.try_open_dispute(&t.investor, &t.sme, &id, &FACE).is_err());
    assert!(t.escrow.get_dispute(&id).is_none());
    assert_eq!(t.pool.get_pool(&id).repaid_amount, 0);
    t.balances(0, 0, 100, 0);
    token::StellarAssetClient::new(&t.env, &t.token).mint(&t.sme, &900);
    // Retrying the same invoice proves both local hold and callback reservation
    // rolled back when the actual Stellar asset rejected the token transfer.
    t.open(id, FACE);
    t.balances(FACE, 0, 0, 0);
    t.resolve_and_settle(id, false);
    t.balances(0, 0, 0, FACE);
}

#[test]
fn repaid_invoice_cannot_lock_new_funds() {
    let t = TestEnv::new(false);
    let id = t.invoice();
    t.pool.repay(&t.sme, &id, &t.token, &FACE);
    assert!(t.pool.get_pool(&id).is_closed);
    assert!(t.escrow.try_open_dispute(&t.investor, &t.sme, &id, &FACE).is_err());
    assert!(t.escrow.get_dispute(&id).is_none());
    t.balances(0, 0, SME_BALANCE - FACE, FACE);
    // Resolution currently accepts arbitrary invoice ids; admission against the
    // funded pool is enforced by escrow, not by the independent verdict record.
}

#[test]
fn frozen_invoice_rejects_admission_and_settlement_but_allows_refund() {
    let t = TestEnv::new(false);
    let id = t.invoice();
    t.nft.freeze_invoice(&t.admin, &id);
    assert_eq!(
        t.pool.try_escrow_terms(&id),
        Err(Ok(FinancingPoolError::InvoiceFrozen))
    );
    assert!(t.escrow.try_open_dispute(&t.investor, &t.sme, &id, &FACE).is_err());
    assert!(t.escrow.get_dispute(&id).is_none());
    t.balances(0, 0, SME_BALANCE, 0);
    t.nft.unfreeze_invoice(&t.admin, &id);
    t.open(id, FACE);
    t.nft.freeze_invoice(&t.admin, &id);
    t.resolution.resolve_dispute(&t.arbiter, &id, &false);
    assert!(t.escrow.try_resolve_dispute(&t.admin, &id, &true).is_err());
    // Verdict and settlement are separate transactions: the verdict persists,
    // while the failed callback rolls back the escrow transfer and pool writes.
    assert!(t.resolution.get_dispute(&id).unwrap().resolved);
    assert!(!t.escrow.get_dispute(&id).unwrap().resolved);
    assert_eq!(t.pool.get_pool(&id).repaid_amount, 0);
    assert_eq!(t.pool.get_protocol_stats().total_repaid, 0);
    t.balances(FACE, 0, SME_BALANCE - FACE, 0);
    t.escrow.resolve_dispute(&t.admin, &id, &false);
    t.balances(0, 0, SME_BALANCE, 0);
    assert!(t.escrow.get_dispute(&id).unwrap().resolved);
}

#[test]
fn unsupported_currency_precision_penalty_and_high_value_debt_cannot_enter_escrow() {
    let mismatch = TestEnv::new(false);
    let id = mismatch.invoice_with(FACE, Symbol::new(&mismatch.env, "OTHER"));
    assert_eq!(
        mismatch.pool.try_escrow_terms(&id),
        Err(Ok(FinancingPoolError::InvalidAmount))
    );
    assert!(mismatch
        .escrow
        .try_open_dispute(&mismatch.investor, &mismatch.sme, &id, &FACE)
        .is_err());
    mismatch.balances(0, 0, SME_BALANCE, 0);

    let precision = TestEnv::new(true);
    let id = precision.invoice();
    ReentrantTokenClient::new(&precision.env, &precision.token).set_decimals(&6);
    assert_eq!(
        precision.pool.try_escrow_terms(&id),
        Err(Ok(FinancingPoolError::InvalidAmount))
    );
    assert!(precision
        .escrow
        .try_open_dispute(&precision.investor, &precision.sme, &id, &FACE)
        .is_err());
    precision.balances(0, 0, SME_BALANCE, 0);

    let penalty = TestEnv::with_late_rate(false, 100);
    let id = penalty.invoice();
    assert_eq!(
        penalty.pool.try_escrow_terms(&id),
        Err(Ok(FinancingPoolError::InvalidAmount))
    );
    assert!(penalty
        .escrow
        .try_open_dispute(&penalty.investor, &penalty.sme, &id, &FACE)
        .is_err());
    penalty.balances(0, 0, SME_BALANCE, 0);

    let high_value = TestEnv::new(false);
    let id = high_value.invoice_with(
        kora_financing_pool::REPAYMENT_APPROVAL_THRESHOLD,
        high_value.currency(),
    );
    assert_eq!(
        high_value.pool.try_escrow_terms(&id),
        Err(Ok(FinancingPoolError::ApprovalThresholdNotMet))
    );
    // A small requested hold must not bypass the invoice's approval threshold.
    assert!(high_value
        .escrow
        .try_open_dispute(&high_value.investor, &high_value.sme, &id, &100)
        .is_err());
    high_value.balances(0, 0, SME_BALANCE, 0);
}

#[test]
fn escrow_custody_shortfall_rolls_back_payout_and_can_be_retried() {
    let t = TestEnv::new(true);
    let id = t.invoice();
    t.open(id, FACE);
    let token = ReentrantTokenClient::new(&t.env, &t.token);
    // A deliberately malicious token changes custody after deposit. This is
    // different from the payer being underfunded when opening a hold.
    token.burn(&t.escrow.address, &900);
    t.resolution.resolve_dispute(&t.arbiter, &id, &false);
    assert!(t.escrow.try_resolve_dispute(&t.admin, &id, &true).is_err());
    assert!(t.resolution.get_dispute(&id).unwrap().resolved);
    assert!(!t.escrow.get_dispute(&id).unwrap().resolved);
    assert_eq!(t.pool.get_pool(&id).repaid_amount, 0);
    assert_eq!(t.pool.get_protocol_stats().total_repaid, 0);
    assert_eq!(
        t.pool.try_repay(&t.sme, &id, &t.token, &1),
        Err(Ok(FinancingPoolError::EscrowPending))
    );
    t.balances(100, 0, SME_BALANCE - FACE, 0);
    token.mint(&t.escrow.address, &900);
    t.escrow.resolve_dispute(&t.admin, &id, &true);
    t.balances(0, 0, SME_BALANCE - FACE, FACE);
    assert!(t.pool.get_pool(&id).is_closed);
    assert_eq!(t.nft.get_invoice(&id).status, InvoiceStatus::Repaid);
}

#[contracttype]
#[derive(Clone)]
enum TokenKey {
    Balance(Address),
    Attack(Address, u64),
    Rejection,
    Decimals,
}

/// Adversarial token only; all dispute, pool, NFT and role contracts remain real.
#[contract]
struct ReentrantToken;

#[contractimpl]
impl ReentrantToken {
    pub fn mint(env: Env, to: Address, amount: i128) {
        let balance = Self::balance(env.clone(), to.clone());
        env.storage().persistent().set(&TokenKey::Balance(to), &(balance + amount));
    }

    pub fn decimals(env: Env) -> u32 {
        env.storage().instance().get(&TokenKey::Decimals).unwrap_or(7)
    }

    pub fn set_decimals(env: Env, decimals: u32) {
        env.storage().instance().set(&TokenKey::Decimals, &decimals);
    }

    pub fn symbol(env: Env) -> String {
        String::from_str(&env, "USDC")
    }

    pub fn burn(env: Env, from: Address, amount: i128) {
        let balance = Self::balance(env.clone(), from.clone());
        assert!(amount >= 0 && balance >= amount);
        env.storage().persistent().set(&TokenKey::Balance(from), &(balance - amount));
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage().persistent().get(&TokenKey::Balance(id)).unwrap_or(0)
    }

    pub fn arm(env: Env, escrow: Address, invoice_id: u64) {
        env.storage().instance().set(&TokenKey::Attack(escrow, invoice_id), &true);
    }

    pub fn rejection(env: Env) -> Option<Error> {
        env.storage().instance().get(&TokenKey::Rejection)
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        assert!(amount >= 0);
        let source = Self::balance(env.clone(), from.clone());
        assert!(source >= amount);
        // Probe the second held invoice whenever the active escrow sends funds.
        let key = TokenKey::Attack(from.clone(), 2);
        if env.storage().instance().has(&key) {
            let attempt = env.try_invoke_contract::<(), Error>(
                &from,
                &Symbol::new(&env, "resolve_timeout"),
                (2u64,).into_val(&env),
            );
            let rejection = match attempt {
                Err(Ok(error)) => error,
                _ => panic!("reentrant escrow call must return a specific rejection"),
            };
            let host_reentry =
                Error::from_type_and_code(ScErrorType::Context, ScErrorCode::InvalidAction);
            let guard_reentry = Error::from_contract_error(EscrowError::Reentrancy as u32);
            assert!(rejection == host_reentry || rejection == guard_reentry);
            env.storage().instance().set(&TokenKey::Rejection, &rejection);
        }
        let destination = Self::balance(env.clone(), to.clone());
        env.storage().persistent().set(&TokenKey::Balance(from), &(source - amount));
        env.storage().persistent().set(&TokenKey::Balance(to), &(destination + amount));
    }
}

#[test]
fn token_callback_cannot_reenter_another_held_invoice() {
    let t = TestEnv::new(true);
    let first = t.invoice();
    let second = t.invoice();
    assert_eq!(second, 2);
    t.open(first, FACE);
    t.open(second, FACE);
    t.balances(2 * FACE, 0, SME_BALANCE - 2 * FACE, 0);
    let evil = ReentrantTokenClient::new(&t.env, &t.token);
    evil.arm(&t.escrow.address, &second);
    // Refund only the first invoice. The malicious token tries a second invoice
    // so the first invoice's resolved bit alone cannot satisfy this regression.
    t.escrow.resolve_dispute(&t.admin, &first, &false);
    assert!(evil.rejection().is_some());
    assert!(t.escrow.get_dispute(&first).unwrap().resolved);
    assert!(!t.escrow.get_dispute(&second).unwrap().resolved);
    assert_eq!(t.pool.get_pool(&second).repaid_amount, 0);
    t.balances(FACE, 0, SME_BALANCE - FACE, 0);
    assert_eq!(
        t.pool.try_repay(&t.sme, &second, &t.token, &1),
        Err(Ok(FinancingPoolError::EscrowPending))
    );
}

#[test]
fn stored_guard_blocks_each_mutator_until_released() {
    let t = TestEnv::new(false);
    let id = t.invoice();
    // Install the shared guard in this escrow's own storage without an active
    // invocation. This exercises the contract guard independently of the host's
    // prohibition on calling back into an already executing contract.
    t.env.as_contract(&t.escrow.address, || {
        kora_shared::reentrancy::acquire_guard(&t.env).unwrap();
    });
    assert_eq!(
        t.escrow.try_open_dispute(&t.investor, &t.sme, &id, &FACE),
        Err(Ok(EscrowError::Reentrancy))
    );
    t.balances(0, 0, SME_BALANCE, 0);
    t.env.as_contract(&t.escrow.address, || {
        kora_shared::reentrancy::release_guard(&t.env);
    });
    t.open(id, FACE);
    t.env.as_contract(&t.escrow.address, || {
        kora_shared::reentrancy::acquire_guard(&t.env).unwrap();
    });
    assert_eq!(
        t.escrow.try_resolve_dispute(&t.admin, &id, &true),
        Err(Ok(EscrowError::Reentrancy))
    );
    assert_eq!(
        t.escrow.try_resolve_timeout(&id),
        Err(Ok(EscrowError::Reentrancy))
    );
    t.balances(FACE, 0, SME_BALANCE - FACE, 0);
    assert!(!t.escrow.get_dispute(&id).unwrap().resolved);
    assert_eq!(t.pool.get_pool(&id).repaid_amount, 0);
    t.env.as_contract(&t.escrow.address, || {
        kora_shared::reentrancy::release_guard(&t.env);
    });
    t.resolve_and_settle(id, false);
    t.balances(0, 0, SME_BALANCE - FACE, FACE);
}
