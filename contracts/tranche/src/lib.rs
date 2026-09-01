#![no_std]

//! # Tranche Contract
//!
//! Pools multiple individually-listed invoices into a single diversified
//! investment product with blended risk/yield.

use kora_shared::{
    errors::CommonError,
    events,
    types::{Invoice, Pool, Position},
    validation::require_non_zero_amount,
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env, Vec};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TrancheError {
    AlreadyInitialized = 1,
    ArithmeticOverflow = 2,
    InvalidAddress = 3,
    InvalidAmount = 4,
    NotAdmin = 5,
    NotInitialized = 6,
    PoolNotFound = 7,
    PoolAlreadyClosed = 8,
    TrancheNotFound = 9,
}

impl From<CommonError> for TrancheError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => TrancheError::InvalidAmount,
            CommonError::InvalidAddress => TrancheError::InvalidAddress,
            CommonError::ArithmeticOverflow => TrancheError::ArithmeticOverflow,
            _ => TrancheError::InvalidAmount,
        }
    }
}

#[contracttype]
pub enum DataKey {
    Admin,
    InvoiceNft,
    FinancingPool,
    Treasury,
    AccessControl,
    Tranche(u64),
    TranchePositions(u64, Address),
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Tranche {
    pub id: u64,
    pub creator: Address,
    pub invoice_ids: Vec<u64>,
    pub total_face_value: i128,
    pub total_funded: i128,
    pub is_closed: bool,
    pub created_at: u64,
}

#[contract]
pub struct TrancheContract;

#[contractimpl]
impl TrancheContract {
    pub fn initialize(
        env: Env,
        admin: Address,
        invoice_nft: Address,
        financing_pool: Address,
        treasury: Address,
        access_control: Address,
    ) -> Result<(), TrancheError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(TrancheError::AlreadyInitialized);
        }
        kora_shared::validation::require_not_self(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &invoice_nft)?;
        kora_shared::validation::require_not_self(&env, &financing_pool)?;
        kora_shared::validation::require_not_self(&env, &treasury)?;
        kora_shared::validation::require_not_self(&env, &access_control)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::InvoiceNft, &invoice_nft);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        Ok(())
    }

    pub fn create_tranche(
        env: Env,
        creator: Address,
        invoice_ids: Vec<u64>,
    ) -> Result<u64, TrancheError> {
        creator.require_auth();

        if invoice_ids.is_empty() {
            return Err(TrancheError::InvalidAmount);
        }

        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(TrancheError::NotInitialized)?;
        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);

        let mut total_face_value: i128 = 0;
        for i in 0..invoice_ids.len() {
            let invoice_id = invoice_ids.get(i).unwrap();
            let invoice = nft_client.get_invoice(&invoice_id)?;
            total_face_value = total_face_value
                .checked_add(invoice.amount)
                .ok_or(TrancheError::ArithmeticOverflow)?;
        }

        let id: u64 = env.storage().instance().get(&DataKey::Tranche(0)).unwrap_or(1);
        let tranche = Tranche {
            id,
            creator: creator.clone(),
            invoice_ids,
            total_face_value,
            total_funded: 0,
            is_closed: false,
            created_at: env.ledger().timestamp(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::Tranche(id), &tranche);
        env.storage().instance().set(&DataKey::Tranche(0), &(id + 1));

        events::tranche_created(&env, id, &creator, total_face_value);
        Ok(id)
    }

    pub fn fund_tranche(
        env: Env,
        investor: Address,
        tranche_id: u64,
        amount: i128,
    ) -> Result<(), TrancheError> {
        investor.require_auth();
        require_non_zero_amount(amount)?;

        let mut tranche: Tranche = env
            .storage()
            .persistent()
            .get(&DataKey::Tranche(tranche_id))
            .ok_or(TrancheError::TrancheNotFound)?;

        if tranche.is_closed {
            return Err(TrancheError::PoolAlreadyClosed);
        }

        tranche.total_funded = tranche
            .total_funded
            .checked_add(amount)
            .ok_or(TrancheError::ArithmeticOverflow)?;

        env.storage()
            .persistent()
            .set(&DataKey::Tranche(tranche_id), &tranche);

        let position_key = DataKey::TranchePositions(tranche_id, investor.clone());
        let position = Position {
            investor: investor.clone(),
            invoice_id: tranche_id,
            contributed: amount,
            share_bps: 0,
            yield_claimed: 0,
        };
        env.storage().persistent().set(&position_key, &position);

        events::tranche_funded(&env, tranche_id, &investor, amount);
        Ok(())
    }

    pub fn get_tranche(env: Env, tranche_id: u64) -> Result<Tranche, TrancheError> {
        env.storage()
            .persistent()
            .get(&DataKey::Tranche(tranche_id))
            .ok_or(TrancheError::TrancheNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env};

    #[test]
    fn test_create_tranche_success() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TrancheContract);
        let client = TrancheContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let pool = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        client.initialize(&admin, &nft, &pool, &treasury, &ac);
        let creator = Address::generate(&env);
        let invoice_ids = Vec::new(&env);
        let result = client.try_create_tranche(&creator, &invoice_ids);
        assert!(result.is_err());
    }
}
