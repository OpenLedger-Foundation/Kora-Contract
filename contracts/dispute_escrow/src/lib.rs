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
    InvalidSplit = 11,
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
    /// Portion of `amount` released to the pool on a split judgment.
    pub released_to_pool: i128,
    /// Portion of `amount` refunded to the SME payer on a split judgment.
    pub refunded_to_payer: i128,
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
            released_to_pool: 0,
            refunded_to_payer: 0,
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
        let (to_pool, to_payer) = if pay_pool {
            (dispute.amount, 0)
        } else {
            (0, dispute.amount)
        };
        Self::settle(&env, dispute, to_pool, to_payer)
    }

    /// Governance adjudication for split judgments: release `to_pool` to the
    /// pool and refund `to_payer` to the SME payer. The two legs must sum to the
    /// escrowed amount so no funds are stranded or double-counted. Only the
    /// configured admin signs.
    pub fn resolve_dispute_split(
        env: Env,
        resolver: Address,
        invoice_id: u64,
        to_pool: i128,
        to_payer: i128,
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
        if to_pool < 0 || to_payer < 0 {
            return Err(EscrowError::InvalidSplit);
        }
        let dispute = Self::load_open(&env, invoice_id)?;
        if env.ledger().timestamp() >= dispute.opened_at.saturating_add(DISPUTE_TIMEOUT) {
            return Err(EscrowError::TimeoutExpired);
        }
        if to_pool.checked_add(to_payer) != Some(dispute.amount) {
            return Err(EscrowError::InvalidSplit);
        }
        Self::settle(&env, dispute, to_pool, to_payer)
    }

    /// Permissionless default after timeout: return funds to the SME payer.
    /// The one-dispute-per-invoice rule prevents reopening to extend a deadline.
    pub fn resolve_timeout(env: Env, invoice_id: u64) -> Result<(), EscrowError> {
        let _guard = ReentrancyGuard::new(&env).map_err(|_| EscrowError::Reentrancy)?;
        let dispute = Self::load_open(&env, invoice_id)?;
        if env.ledger().timestamp() < dispute.opened_at.saturating_add(DISPUTE_TIMEOUT) {
            return Err(EscrowError::TimeoutNotReached);
        }
        Self::settle(&env, dispute, 0, dispute.amount)
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

    fn settle(
        env: &Env,
        mut dispute: EscrowDispute,
        to_pool: i128,
        to_payer: i128,
    ) -> Result<(), EscrowError> {
        let pool: Address = env
            .storage()
            .instance()
            .get(&Key::Pool)
            .ok_or(EscrowError::NotInitialized)?;
        dispute.resolved = true;
        dispute.paid_to_pool = to_pool > 0;
        dispute.released_to_pool = to_pool;
        dispute.refunded_to_payer = to_payer;
        env.storage()
            .persistent()
            .set(&Key::Dispute(dispute.invoice_id), &dispute);
        env.storage().persistent().extend_ttl(
            &Key::Dispute(dispute.invoice_id),
            DISPUTE_TTL_LEDGERS,
            DISPUTE_TTL_LEDGERS,
        );
        let token_client = token::Client::new(env, &dispute.token);
        if to_pool > 0 {
            token_client.transfer(&env.current_contract_address(), &pool, &to_pool);
        }
        if to_payer > 0 {
            token_client.transfer(&env.current_contract_address(), &dispute.payer, &to_payer);
        }
        let _: () = env.invoke_contract(
            &pool,
            &Symbol::new(env, "settle_escrow"),
            (
                env.current_contract_address(),
                dispute.invoice_id,
                to_pool,
                to_payer,
            )
                .into_val(env),
        );
        env.events().publish(
            (Symbol::new(env, "dispute_resolve"), dispute.invoice_id),
            (to_pool, to_payer),
        );
        Ok(())
    }
}
