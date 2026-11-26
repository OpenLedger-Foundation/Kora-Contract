#![no_std]

use kora_shared::reentrancy::ReentrancyGuard;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, Env, IntoVal, Symbol,
};

/// A dispute remains actionable for seven days. On timeout anyone can return
/// the payment to its payer; an invoice can only enter escrow once.
pub const DISPUTE_TIMEOUT: u64 = 7 * 86_400;
const DISPUTE_TTL_LEDGERS: u32 = 518_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EscrowError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidAmount = 4,
    AlreadyDisputed = 5,
    NotFound = 6,
    Resolved = 7,
    TimeoutNotReached = 8,
    TimeoutExpired = 9,
    Reentrancy = 10,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowDispute {
    pub invoice_id: u64,
    pub challenger: Address,
    pub payer: Address,
    pub token: Address,
    pub amount: i128,
    pub opened_at: u64,
    pub resolved: bool,
    pub paid_to_pool: bool,
}

#[contracttype]
enum Key {
    Admin,
    Pool,
    Dispute(u64),
}

#[contract]
pub struct DisputeEscrowContract;

#[contractimpl]
impl DisputeEscrowContract {
    /// The admin may be a governance multisig contract address. The pool must
    /// be configured separately with this escrow's address before use.
    pub fn initialize(env: Env, admin: Address, pool: Address) -> Result<(), EscrowError> {
        if env.storage().instance().has(&Key::Admin) {
            return Err(EscrowError::AlreadyInitialized);
        }
        if admin == pool
            || admin == env.current_contract_address()
            || pool == env.current_contract_address()
        {
            return Err(EscrowError::Unauthorized);
        }
        env.storage().instance().set(&Key::Admin, &admin);
        env.storage().instance().set(&Key::Pool, &pool);
        Ok(())
    }

    /// Both the challenger and SME payer sign. Only the SME or an investor with
    /// a recorded pool position may challenge. The amount uses the pool token's
    /// base units, and stays outside pool accounting until adjudication.
    pub fn open_dispute(
        env: Env,
        challenger: Address,
        payer: Address,
        invoice_id: u64,
        amount: i128,
    ) -> Result<(), EscrowError> {
        // AUDIT FIX (#914): protect every custody mutation before pool/token
        // interactions, including callbacks targeting a different invoice.
        let _guard = ReentrancyGuard::new(&env).map_err(|_| EscrowError::Reentrancy)?;
        challenger.require_auth();
        payer.require_auth();
        if amount <= 0 {
            return Err(EscrowError::InvalidAmount);
        }
        let pool: Address = env
            .storage()
            .instance()
            .get(&Key::Pool)
            .ok_or(EscrowError::NotInitialized)?;
        let key = Key::Dispute(invoice_id);
        if env.storage().persistent().has(&key) {
            return Err(EscrowError::AlreadyDisputed);
        }
        let (asset, sme, remaining): (Address, Address, i128) = env.invoke_contract(
            &pool,
            &Symbol::new(&env, "escrow_terms"),
            (invoice_id,).into_val(&env),
        );
        if payer != sme {
            return Err(EscrowError::Unauthorized);
        }
        if amount > remaining {
            return Err(EscrowError::InvalidAmount);
        }
        if challenger != sme {
            let owner: bool = env.invoke_contract(
                &pool,
                &Symbol::new(&env, "has_position"),
                (invoice_id, challenger.clone()).into_val(&env),
            );
            if !owner {
                return Err(EscrowError::Unauthorized);
            }
        }
        let dispute = EscrowDispute {
            invoice_id,
            challenger,
            payer: payer.clone(),
            token: asset.clone(),
            amount,
            opened_at: env.ledger().timestamp(),
            resolved: false,
            paid_to_pool: false,
        };
        env.storage().persistent().set(&key, &dispute);
        env.storage()
            .persistent()
            .extend_ttl(&key, DISPUTE_TTL_LEDGERS, DISPUTE_TTL_LEDGERS);
        // Reserve before the transfer; a failure rolls back the entire invocation.
        let _: () = env.invoke_contract(
            &pool,
            &Symbol::new(&env, "reserve_escrow"),
            (env.current_contract_address(), invoice_id, amount).into_val(&env),
        );
        token::Client::new(&env, &asset).transfer(&payer, &env.current_contract_address(), &amount);
        env.events()
            .publish((Symbol::new(&env, "dispute_open"), invoice_id), amount);
        Ok(())
    }

    /// Governance adjudication: `pay_pool=true` applies the repayment to the
    /// invoice; false refunds its SME payer. Only the configured admin signs.
    pub fn resolve_dispute(
        env: Env,
        resolver: Address,
        invoice_id: u64,
        pay_pool: bool,
    ) -> Result<(), EscrowError> {
        let _guard = ReentrancyGuard::new(&env).map_err(|_| EscrowError::Reentrancy)?;
        resolver.require_auth();
        let admin: Address = env
            .storage()
            .instance()
            .get(&Key::Admin)
            .ok_or(EscrowError::NotInitialized)?;
        if resolver != admin {
            return Err(EscrowError::Unauthorized);
        }
        let dispute = Self::load_open(&env, invoice_id)?;
        if env.ledger().timestamp() >= dispute.opened_at.saturating_add(DISPUTE_TIMEOUT) {
            return Err(EscrowError::TimeoutExpired);
        }
        Self::settle(&env, dispute, pay_pool)
    }

    /// Permissionless default after timeout: return funds to the SME payer.
    /// The one-dispute-per-invoice rule prevents reopening to extend a deadline.
    pub fn resolve_timeout(env: Env, invoice_id: u64) -> Result<(), EscrowError> {
        let _guard = ReentrancyGuard::new(&env).map_err(|_| EscrowError::Reentrancy)?;
        let dispute = Self::load_open(&env, invoice_id)?;
        if env.ledger().timestamp() < dispute.opened_at.saturating_add(DISPUTE_TIMEOUT) {
            return Err(EscrowError::TimeoutNotReached);
        }
        Self::settle(&env, dispute, false)
    }

    pub fn get_dispute(env: Env, invoice_id: u64) -> Option<EscrowDispute> {
        env.storage().persistent().get(&Key::Dispute(invoice_id))
    }

    fn load_open(env: &Env, invoice_id: u64) -> Result<EscrowDispute, EscrowError> {
        let dispute: EscrowDispute = env
            .storage()
            .persistent()
            .get(&Key::Dispute(invoice_id))
            .ok_or(EscrowError::NotFound)?;
        if dispute.resolved {
            return Err(EscrowError::Resolved);
        }
        Ok(dispute)
    }

    fn settle(env: &Env, mut dispute: EscrowDispute, pay_pool: bool) -> Result<(), EscrowError> {
        let pool: Address = env
            .storage()
            .instance()
            .get(&Key::Pool)
            .ok_or(EscrowError::NotInitialized)?;
        dispute.resolved = true;
        dispute.paid_to_pool = pay_pool;
        env.storage()
            .persistent()
            .set(&Key::Dispute(dispute.invoice_id), &dispute);
        env.storage().persistent().extend_ttl(
            &Key::Dispute(dispute.invoice_id),
            DISPUTE_TTL_LEDGERS,
            DISPUTE_TTL_LEDGERS,
        );
        if pay_pool {
            token::Client::new(env, &dispute.token).transfer(
                &env.current_contract_address(),
                &pool,
                &dispute.amount,
            );
        } else {
            token::Client::new(env, &dispute.token).transfer(
                &env.current_contract_address(),
                &dispute.payer,
                &dispute.amount,
            );
        }
        let _: () = env.invoke_contract(
            &pool,
            &Symbol::new(env, "settle_escrow"),
            (
                env.current_contract_address(),
                dispute.invoice_id,
                dispute.amount,
                pay_pool,
            )
                .into_val(env),
        );
        env.events()
            .publish((Symbol::new(env, "dispute_end"), dispute.invoice_id), pay_pool);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        token::StellarAssetClient,
    };

    #[contract]
    struct MockPool;

    #[contractimpl]
    impl MockPool {
        pub fn initialize(env: Env, token: Address, sme: Address) {
            env.storage().instance().set(&0u32, &token);
            env.storage().instance().set(&1u32, &sme);
        }
        pub fn escrow_terms(env: Env, _invoice_id: u64) -> (Address, Address, i128) {
            (
                env.storage().instance().get(&0u32).unwrap(),
                env.storage().instance().get(&1u32).unwrap(),
                100,
            )
        }
        pub fn has_position(_env: Env, _invoice_id: u64, _investor: Address) -> bool {
            true
        }
        pub fn reserve_escrow(env: Env, escrow: Address, _invoice_id: u64, amount: i128) {
            escrow.require_auth();
            env.storage().instance().set(&2u32, &amount);
        }
        pub fn settle_escrow(env: Env, escrow: Address, _invoice_id: u64, amount: i128, to_pool: bool) {
            escrow.require_auth();
            assert_eq!(env.storage().instance().get::<_, i128>(&2u32), Some(amount));
            env.storage().instance().set(&3u32, &to_pool);
        }
        pub fn paid_to_pool(env: Env) -> Option<bool> {
            env.storage().instance().get(&3u32)
        }
    }

    fn setup() -> (
        Env,
        DisputeEscrowContractClient<'static>,
        MockPoolClient<'static>,
        Address,
        Address,
        Address,
    ) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 100,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });
        let admin = Address::generate(&env);
        let sme = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract_v2(admin.clone());
        let asset = token_id.address();
        StellarAssetClient::new(&env, &asset).mint(&sme, &300);
        let pool_id = env.register_contract(None, MockPool);
        let pool = MockPoolClient::new(&env, &pool_id);
        pool.initialize(&asset, &sme);
        let escrow_id = env.register_contract(None, DisputeEscrowContract);
        let escrow = DisputeEscrowContractClient::new(&env, &escrow_id);
        escrow.initialize(&admin, &pool_id);
        (env, escrow, pool, admin, sme, asset)
    }

    #[test]
    fn locks_funds_without_crediting_pool_and_rejects_repeat() {
        let (env, escrow, pool, _admin, sme, asset) = setup();
        escrow.open_dispute(&sme, &sme, &1, &60);
        let token = token::Client::new(&env, &asset);
        assert_eq!(token.balance(&escrow.address), 60);
        assert_eq!(token.balance(&pool.address), 0);
        assert_eq!(escrow.get_dispute(&1).unwrap().amount, 60);
        assert_eq!(
            escrow
                .try_open_dispute(&sme, &sme, &1, &10)
                .unwrap_err()
                .unwrap(),
            EscrowError::AlreadyDisputed
        );
    }

    #[test]
    fn adjudication_routes_to_pool_or_returns_to_payer() {
        for to_pool in [false, true] {
            let (env, escrow, pool, admin, sme, asset) = setup();
            escrow.open_dispute(&sme, &sme, &2, &70);
            escrow.resolve_dispute(&admin, &2, &to_pool);
            let token = token::Client::new(&env, &asset);
            assert_eq!(token.balance(&escrow.address), 0);
            assert_eq!(token.balance(&pool.address), if to_pool { 70 } else { 0 });
            assert_eq!(token.balance(&sme), if to_pool { 230 } else { 300 });
            assert_eq!(pool.paid_to_pool(), Some(to_pool));
            assert_eq!(
                escrow
                    .try_resolve_dispute(&admin, &2, &to_pool)
                    .unwrap_err()
                    .unwrap(),
                EscrowError::Resolved
            );
        }
    }

    #[test]
    fn timeout_refunds_and_cannot_restart_dispute() {
        let (env, escrow, _pool, admin, sme, asset) = setup();
        escrow.open_dispute(&sme, &sme, &3, &50);
        assert_eq!(
            escrow.try_resolve_timeout(&3).unwrap_err().unwrap(),
            EscrowError::TimeoutNotReached
        );
        env.ledger().set(LedgerInfo {
            timestamp: 100 + DISPUTE_TIMEOUT,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });
        assert_eq!(
            escrow
                .try_resolve_dispute(&admin, &3, &true)
                .unwrap_err()
                .unwrap(),
            EscrowError::TimeoutExpired
        );
        escrow.resolve_timeout(&3);
        assert_eq!(token::Client::new(&env, &asset).balance(&sme), 300);
        assert_eq!(
            escrow
                .try_open_dispute(&sme, &sme, &3, &10)
                .unwrap_err()
                .unwrap(),
            EscrowError::AlreadyDisputed
        );
    }

    // ── Escrow release coupling tests ─────────────────────────────────────────
    //
    // These tests verify that:
    // 1. Resolution routes funds exactly as instructed (pool vs. payer).
    // 2. After resolution the dispute is permanently finalized — a second
    //    resolve call returns `Resolved`, not a fund-movement side effect.
    // 3. The escrow contract holds exactly the disputed amount in custody
    //    while the dispute is open — the pool balance is not credited early.
    // 4. Timeout refund is not available before the deadline.
    // 5. Governance resolve is not available after the timeout.

    /// Funds are locked in escrow (not in pool) while the dispute is open.
    #[test]
    fn escrow_holds_funds_while_dispute_open() {
        let (env, escrow, pool, _admin, sme, asset) = setup();
        let token = token::Client::new(&env, &asset);

        let pre_escrow_balance = token.balance(&escrow.address);
        let pre_pool_balance = token.balance(&pool.address);
        escrow.open_dispute(&sme, &sme, &100, &80);

        // Funds must be locked in escrow, not sent to pool
        assert_eq!(
            token.balance(&escrow.address),
            pre_escrow_balance + 80,
            "escrow must hold the disputed amount"
        );
        assert_eq!(
            token.balance(&pool.address),
            pre_pool_balance,
            "pool must not receive funds before resolution"
        );

        let dispute = escrow.get_dispute(&100).unwrap();
        assert!(!dispute.resolved, "dispute must remain open");
        assert!(!dispute.paid_to_pool, "paid_to_pool must be false before resolution");
    }

    /// Governance resolve with pay_pool=true transfers funds to pool and
    /// marks the dispute as paid_to_pool=true.
    #[test]
    fn escrow_resolution_pay_pool_routes_to_pool() {
        let (env, escrow, pool, admin, sme, asset) = setup();
        let token = token::Client::new(&env, &asset);
        escrow.open_dispute(&sme, &sme, &101, &70);

        escrow.resolve_dispute(&admin, &101, &true);

        assert_eq!(token.balance(&escrow.address), 0, "escrow must be empty after resolution");
        assert_eq!(token.balance(&pool.address), 70, "pool must receive funds when pay_pool=true");
        assert_eq!(token.balance(&sme), 230, "sme must keep the balance minus escrowed amount");

        let dispute = escrow.get_dispute(&101).unwrap();
        assert!(dispute.resolved);
        assert!(dispute.paid_to_pool);
    }

    /// Governance resolve with pay_pool=false refunds to payer.
    #[test]
    fn escrow_resolution_refund_routes_to_payer() {
        let (env, escrow, pool, admin, sme, asset) = setup();
        let token = token::Client::new(&env, &asset);
        escrow.open_dispute(&sme, &sme, &102, &65);

        escrow.resolve_dispute(&admin, &102, &false);

        assert_eq!(token.balance(&escrow.address), 0, "escrow must be empty after refund");
        assert_eq!(token.balance(&pool.address), 0, "pool must not receive funds when refunding");
        assert_eq!(token.balance(&sme), 300, "sme must receive full refund");

        let dispute = escrow.get_dispute(&102).unwrap();
        assert!(dispute.resolved);
        assert!(!dispute.paid_to_pool);
    }

    /// After resolution, every subsequent resolve call returns `Resolved` —
    /// no second fund movement can occur.
    #[test]
    fn escrow_resolved_dispute_is_immutable_no_double_spend() {
        let (env, escrow, _pool, admin, sme, _asset) = setup();
        escrow.open_dispute(&sme, &sme, &103, &50);
        escrow.resolve_dispute(&admin, &103, &true);

        // Both resolve paths must be blocked after finality
        let result_gov = escrow.try_resolve_dispute(&admin, &103, &false);
        assert_eq!(
            result_gov.unwrap_err().unwrap(),
            EscrowError::Resolved,
            "governance resolve on finalized dispute must return Resolved"
        );

        let result_timeout = escrow.try_resolve_timeout(&103);
        assert_eq!(
            result_timeout.unwrap_err().unwrap(),
            EscrowError::Resolved,
            "timeout resolve on finalized dispute must return Resolved"
        );
    }

    /// Timeout resolve before the deadline returns `TimeoutNotReached` —
    /// it must not release funds early.
    #[test]
    fn escrow_timeout_before_deadline_blocked_typed_error() {
        let (env, escrow, _pool, _admin, sme, asset) = setup();
        let token = token::Client::new(&env, &asset);
        escrow.open_dispute(&sme, &sme, &104, &40);

        // Advance to 1 second before timeout
        env.ledger().set(LedgerInfo {
            timestamp: 100 + DISPUTE_TIMEOUT - 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        let result = escrow.try_resolve_timeout(&104);
        assert_eq!(
            result.unwrap_err().unwrap(),
            EscrowError::TimeoutNotReached,
            "timeout before deadline must surface typed TimeoutNotReached"
        );
        // Funds must still be locked in escrow
        assert_eq!(token.balance(&escrow.address), 40, "funds must remain in escrow");
    }

    /// Governance resolve after the timeout deadline returns `TimeoutExpired`.
    #[test]
    fn escrow_governance_resolve_after_timeout_blocked_typed_error() {
        let (env, escrow, _pool, admin, sme, asset) = setup();
        let token = token::Client::new(&env, &asset);
        escrow.open_dispute(&sme, &sme, &105, &35);

        // Advance to exactly the timeout boundary (opened_at=100, timeout=100+DISPUTE_TIMEOUT)
        env.ledger().set(LedgerInfo {
            timestamp: 100 + DISPUTE_TIMEOUT,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        let result = escrow.try_resolve_dispute(&admin, &105, &true);
        assert_eq!(
            result.unwrap_err().unwrap(),
            EscrowError::TimeoutExpired,
            "governance resolve at/after timeout must surface typed TimeoutExpired"
        );
        // Escrow still holds the amount — no unintended release
        assert_eq!(token.balance(&escrow.address), 35, "funds must stay in escrow when governance resolve is blocked");
    }

    /// Timeout resolve at exactly the deadline (opened_at + DISPUTE_TIMEOUT) succeeds.
    #[test]
    fn escrow_timeout_at_exact_boundary_succeeds() {
        let (env, escrow, _pool, _admin, sme, asset) = setup();
        let token = token::Client::new(&env, &asset);
        escrow.open_dispute(&sme, &sme, &106, &45);

        // Advance to exactly the timeout boundary
        env.ledger().set(LedgerInfo {
            timestamp: 100 + DISPUTE_TIMEOUT,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        // resolve_timeout uses `< DISPUTE_TIMEOUT`, so at exactly the boundary it should succeed
        let result = escrow.try_resolve_timeout(&106);
        assert!(
            result.is_ok(),
            "timeout at exactly the boundary must succeed"
        );
        assert_eq!(token.balance(&sme), 300, "payer must be fully refunded after timeout");
    }

    /// Non-admin cannot perform governance adjudication.
    #[test]
    fn escrow_non_admin_resolve_rejected() {
        let (env, escrow, _pool, _admin, sme, _asset) = setup();
        escrow.open_dispute(&sme, &sme, &107, &30);

        let stranger = Address::generate(&env);
        let result = escrow.try_resolve_dispute(&stranger, &107, &true);
        assert_eq!(
            result.unwrap_err().unwrap(),
            EscrowError::Unauthorized,
            "non-admin adjudication must be rejected"
        );
    }

    /// Opening a dispute for an invoice that already has one is rejected.
    #[test]
    fn escrow_duplicate_dispute_rejected() {
        let (env, escrow, _pool, _admin, sme, _asset) = setup();
        escrow.open_dispute(&sme, &sme, &108, &20);
        let result = escrow.try_open_dispute(&sme, &sme, &108, &10);
        assert_eq!(
            result.unwrap_err().unwrap(),
            EscrowError::AlreadyDisputed,
            "duplicate dispute on same invoice must be rejected"
        );
    }
}
