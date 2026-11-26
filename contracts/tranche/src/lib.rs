#![no_std]

//! # Tranche Contract
//!
//! Pools multiple individually-listed invoices into a single diversified
//! investment product with blended risk/yield.
//!
//! ## Cross-default propagation
//!
//! By default a default in one tranche is isolated: it does not affect other
//! tranches. The admin may opt into cross-default propagation via
//! [`TrancheContract::set_cross_default_propagation`]. When enabled, marking a
//! tranche as defaulted (`mark_tranche_default`) closes every other open
//! tranche as well, so junior/senior expectations are explicit and consistent.
//! The flag is stored under [`DataKey::CrossDefaultPropagation`] and fails
//! closed: if it is unset, propagation is treated as disabled.

use kora_shared::{
    errors::CommonError,
    events,
    types::Position,
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
    TrancheAlreadyDefaulted = 10,
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
    CrossDefaultPropagation,
    TrancheDefaulted(u64),
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
            let invoice = nft_client.get_invoice(&invoice_id);
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

    /// Admin-only toggle for cross-default propagation. When enabled, a default
    /// in one tranche closes every other open tranche. Defaults to disabled
    /// (fail closed) when never set.
    pub fn set_cross_default_propagation(
        env: Env,
        admin: Address,
        enabled: bool,
    ) -> Result<(), TrancheError> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(TrancheError::NotInitialized)?;
        if admin != stored_admin {
            return Err(TrancheError::NotAdmin);
        }
        env.storage()
            .instance()
            .set(&DataKey::CrossDefaultPropagation, &enabled);
        Ok(())
    }

    /// Returns whether cross-default propagation is enabled. Unset config is
    /// treated as disabled (fail closed).
    pub fn cross_default_propagation(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::CrossDefaultPropagation)
            .unwrap_or(false)
    }

    /// Marks a tranche as defaulted. If cross-default propagation is enabled,
    /// every other open tranche is closed as well. Re-entry on an already
    /// defaulted tranche is idempotent and returns `TrancheAlreadyDefaulted`.
    pub fn mark_tranche_default(
        env: Env,
        admin: Address,
        tranche_id: u64,
    ) -> Result<(), TrancheError> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(TrancheError::NotInitialized)?;
        if admin != stored_admin {
            return Err(TrancheError::NotAdmin);
        }

        let mut tranche: Tranche = env
            .storage()
            .persistent()
            .get(&DataKey::Tranche(tranche_id))
            .ok_or(TrancheError::TrancheNotFound)?;

        if env
            .storage()
            .persistent()
            .has(&DataKey::TrancheDefaulted(tranche_id))
        {
            return Err(TrancheError::TrancheAlreadyDefaulted);
        }

        env.storage()
            .persistent()
            .set(&DataKey::TrancheDefaulted(tranche_id), &true);
        tranche.is_closed = true;
        env.storage()
            .persistent()
            .set(&DataKey::Tranche(tranche_id), &tranche);

        let propagate: bool = env
            .storage()
            .instance()
            .get(&DataKey::CrossDefaultPropagation)
            .unwrap_or(false);

        if propagate {
            let next_id: u64 = env
                .storage()
                .instance()
                .get(&DataKey::Tranche(0))
                .unwrap_or(1);
            let mut id: u64 = 1;
            while id < next_id {
                if id != tranche_id {
                    if let Some(mut other) = env
                        .storage()
                        .persistent()
                        .get::<DataKey, Tranche>(&DataKey::Tranche(id))
                    {
                        if !other.is_closed {
                            other.is_closed = true;
                            env.storage()
                                .persistent()
                                .set(&DataKey::Tranche(id), &other);
                        }
                    }
                }
                id += 1;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env};

    fn setup(env: &Env) -> (TrancheContractClient, Address) {
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TrancheContract);
        let client = TrancheContractClient::new(env, &contract_id);
        let admin = Address::generate(env);
        let nft = Address::generate(env);
        let pool = Address::generate(env);
        let treasury = Address::generate(env);
        let ac = Address::generate(env);
        client.initialize(&admin, &nft, &pool, &treasury, &ac);
        (client, admin)
    }

    fn seed_tranche(env: &Env, client: &TrancheContractClient, id: u64) {
        let creator = Address::generate(env);
        let tranche = Tranche {
            id,
            creator,
            invoice_ids: Vec::new(env),
            total_face_value: 0,
            total_funded: 0,
            is_closed: false,
            created_at: 0,
        };
        env.as_contract(&client.address, || {
            env.storage()
                .persistent()
                .set(&DataKey::Tranche(id), &tranche);
        });
    }

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

    #[test]
    fn test_cross_default_flag_defaults_disabled() {
        let env = Env::default();
        let (client, _admin) = setup(&env);
        assert!(!client.cross_default_propagation());
    }

    #[test]
    fn test_cross_default_flag_toggle() {
        let env = Env::default();
        let (client, admin) = setup(&env);
        client.set_cross_default_propagation(&admin, &true);
        assert!(client.cross_default_propagation());
        client.set_cross_default_propagation(&admin, &false);
        assert!(!client.cross_default_propagation());
    }

    #[test]
    fn test_cross_default_flag_requires_admin() {
        let env = Env::default();
        let (client, _admin) = setup(&env);
        let stranger = Address::generate(&env);
        let result = client.try_set_cross_default_propagation(&stranger, &true);
        assert_eq!(result, Err(Ok(TrancheError::NotAdmin)));
    }

    #[test]
    fn test_default_isolated_when_flag_disabled() {
        let env = Env::default();
        let (client, admin) = setup(&env);
        seed_tranche(&env, &client, 1);
        seed_tranche(&env, &client, 2);
        client.mark_tranche_default(&admin, &1);
        assert!(client.get_tranche(&1).is_closed);
        assert!(!client.get_tranche(&2).is_closed);
    }

    #[test]
    fn test_default_propagates_when_flag_enabled() {
        let env = Env::default();
        let (client, admin) = setup(&env);
        seed_tranche(&env, &client, 1);
        seed_tranche(&env, &client, 2);
        seed_tranche(&env, &client, 3);
        client.set_cross_default_propagation(&admin, &true);
        client.mark_tranche_default(&admin, &2);
        assert!(client.get_tranche(&1).is_closed);
        assert!(client.get_tranche(&2).is_closed);
        assert!(client.get_tranche(&3).is_closed);
    }

    #[test]
    fn test_mark_default_requires_admin() {
        let env = Env::default();
        let (client, _admin) = setup(&env);
        seed_tranche(&env, &client, 1);
        let stranger = Address::generate(&env);
        let result = client.try_mark_tranche_default(&stranger, &1);
        assert_eq!(result, Err(Ok(TrancheError::NotAdmin)));
    }

    #[test]
    fn test_mark_default_missing_tranche_fails_closed() {
        let env = Env::default();
        let (client, admin) = setup(&env);
        let result = client.try_mark_tranche_default(&admin, &99);
        assert_eq!(result, Err(Ok(TrancheError::TrancheNotFound)));
    }

    #[test]
    fn test_mark_default_idempotent_reentry() {
        let env = Env::default();
        let (client, admin) = setup(&env);
        seed_tranche(&env, &client, 1);
        client.mark_tranche_default(&admin, &1);
        let result = client.try_mark_tranche_default(&admin, &1);
        assert_eq!(result, Err(Ok(TrancheError::TrancheAlreadyDefaulted)));
    }

    #[test]
    fn test_fund_closed_tranche_rejected() {
        let env = Env::default();
        let (client, admin) = setup(&env);
        seed_tranche(&env, &client, 1);
        client.mark_tranche_default(&admin, &1);
        let investor = Address::generate(&env);
        let result = client.try_fund_tranche(&investor, &1, &100);
        assert_eq!(result, Err(Ok(TrancheError::PoolAlreadyClosed)));
    }
}
