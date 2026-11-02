//! Mock financing-pool fixture for dispute fuzzing. Split from
//! dispute.rs: Soroban generates one module-level __configure
//! per #[contractimpl] block, so two mock contracts cannot share a file.
use soroban_sdk::{contract, contractimpl, contracttype, Address, Env};

#[contracttype]
#[derive(Clone)]
enum PoolKey {
    Token,
    Payer,
    Investor,
    Escrow,
    Reserved(u64),
    Released(u64),
}

#[contract]
pub struct FuzzDisputePool;

#[contractimpl]
impl FuzzDisputePool {
    pub fn configure(env: Env, token: Address, payer: Address, investor: Address, escrow: Address) {
        env.storage().instance().set(&PoolKey::Token, &token);
        env.storage().instance().set(&PoolKey::Payer, &payer);
        env.storage().instance().set(&PoolKey::Investor, &investor);
        env.storage().instance().set(&PoolKey::Escrow, &escrow);
    }

    pub fn escrow_terms(env: Env, _invoice_id: u64) -> (Address, Address, i128) {
        (
            env.storage().instance().get(&PoolKey::Token).unwrap(),
            env.storage().instance().get(&PoolKey::Payer).unwrap(),
            100,
        )
    }

    pub fn has_position(env: Env, _invoice_id: u64, investor: Address) -> bool {
        investor
            == env
                .storage()
                .instance()
                .get::<_, Address>(&PoolKey::Investor)
                .unwrap()
    }

    pub fn reserve_escrow(env: Env, escrow: Address, invoice_id: u64, amount: i128) {
        escrow.require_auth();
        assert_eq!(Some(escrow), env.storage().instance().get(&PoolKey::Escrow));
        assert!(!env.storage().instance().has(&PoolKey::Reserved(invoice_id)));
        env.storage()
            .instance()
            .set(&PoolKey::Reserved(invoice_id), &amount);
    }

    pub fn settle_escrow(env: Env, escrow: Address, invoice_id: u64, amount: i128, _to_pool: bool) {
        escrow.require_auth();
        assert_eq!(Some(escrow), env.storage().instance().get(&PoolKey::Escrow));
        assert_eq!(
            Some(amount),
            env.storage().instance().get(&PoolKey::Reserved(invoice_id))
        );
        // A second release is a finding even if token balances still look valid.
        assert!(!env.storage().instance().has(&PoolKey::Released(invoice_id)));
        env.storage()
            .instance()
            .set(&PoolKey::Released(invoice_id), &1u32);
    }

    pub fn releases(env: Env, invoice_id: u64) -> u32 {
        env.storage()
            .instance()
            .get(&PoolKey::Released(invoice_id))
            .unwrap_or(0)
    }
}
