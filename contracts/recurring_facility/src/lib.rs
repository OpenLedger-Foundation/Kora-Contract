#![no_std]

use kora_shared::{
    errors::CommonError,
    reentrancy::ReentrancyGuard,
    validation::{require_non_negative_amount, require_non_zero_amount, safe_add, safe_sub},
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, Env, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RecurringFacilityError {
    AlreadyInitialized = 1,
    NotAdmin = 2,
    NotInitialized = 3,
    CreditLimitExceeded = 4,
    InsufficientCapital = 5,
    DrawNotFound = 6,
    DrawAlreadyRepaid = 7,
    DrawAlreadyDefaulted = 8,
    InvalidAmount = 9,
    ArithmeticOverflow = 10,
    Reentrancy = 11,
    Unauthorized = 12,
}

impl From<CommonError> for RecurringFacilityError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => RecurringFacilityError::InvalidAmount,
            CommonError::ArithmeticOverflow => RecurringFacilityError::ArithmeticOverflow,
            CommonError::Reentrancy => RecurringFacilityError::Reentrancy,
            _ => RecurringFacilityError::InvalidAmount,
        }
    }
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DrawStatus {
    Active = 1,
    Repaid = 2,
    Defaulted = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DrawPosition {
    pub id: u64,
    pub sme: Address,
    pub amount: i128,
    pub drawn_at: u64,
    pub due_date: u64,
    pub repaid_amount: i128,
    pub status: DrawStatus,
}

#[contracttype]
pub enum DataKey {
    Admin,
    RiskRegistry,
    FinancingPool,
    CommittedPoolCapital,
    InvestorCommitment(Address),
    SmeCreditLimit(Address),
    SmeUtilization(Address),
    NextDrawId,
    Draw(u64),
    SmeDraws(Address),
}

#[contract]
pub struct RecurringFacilityContract;

#[contractimpl]
impl RecurringFacilityContract {
    pub fn initialize(
        env: Env,
        admin: Address,
        risk_registry: Address,
        financing_pool: Address,
    ) -> Result<(), RecurringFacilityError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(RecurringFacilityError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::RiskRegistry, &risk_registry);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().instance().set(&DataKey::NextDrawId, &1u64);
        env.storage().instance().set(&DataKey::CommittedPoolCapital, &0i128);
        Ok(())
    }

    pub fn commit_capital(
        env: Env,
        investor: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), RecurringFacilityError> {
        investor.require_auth();
        let _guard = ReentrancyGuard::new(&env)?;
        require_non_zero_amount(amount)?;

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&investor, &env.current_contract_address(), &amount);

        let current_committed: i128 = env
            .storage()
            .instance()
            .get(&DataKey::CommittedPoolCapital)
            .unwrap_or(0);
        let new_committed = safe_add(current_committed, amount)?;
        env.storage().instance().set(&DataKey::CommittedPoolCapital, &new_committed);

        let current_investor: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::InvestorCommitment(investor.clone()))
            .unwrap_or(0);
        let new_investor = safe_add(current_investor, amount)?;
        env.storage().persistent().set(&DataKey::InvestorCommitment(investor), &new_investor);

        Ok(())
    }

    pub fn set_facility_limit(
        env: Env,
        admin: Address,
        sme: Address,
        limit: i128,
    ) -> Result<(), RecurringFacilityError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_non_negative_amount(limit)?;

        env.storage().persistent().set(&DataKey::SmeCreditLimit(sme), &limit);
        Ok(())
    }

    pub fn get_facility_limit(env: Env, sme: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::SmeCreditLimit(sme))
            .unwrap_or(0)
    }

    pub fn get_utilization(env: Env, sme: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::SmeUtilization(sme))
            .unwrap_or(0)
    }

    pub fn get_pool_capital(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::CommittedPoolCapital)
            .unwrap_or(0)
    }

    pub fn draw_down(
        env: Env,
        sme: Address,
        token: Address,
        amount: i128,
        duration_seconds: u64,
    ) -> Result<u64, RecurringFacilityError> {
        sme.require_auth();
        let _guard = ReentrancyGuard::new(&env)?;
        require_non_zero_amount(amount)?;

        let limit = Self::get_facility_limit(env.clone(), sme.clone());
        let current_utilization = Self::get_utilization(env.clone(), sme.clone());
        let new_utilization = safe_add(current_utilization, amount)?;

        if new_utilization > limit {
            return Err(RecurringFacilityError::CreditLimitExceeded);
        }

        let pool_capital = Self::get_pool_capital(env.clone());
        if amount > pool_capital {
            return Err(RecurringFacilityError::InsufficientCapital);
        }

        // Update pool capital and SME utilization
        let new_pool_capital = safe_sub(pool_capital, amount)?;
        env.storage().instance().set(&DataKey::CommittedPoolCapital, &new_pool_capital);
        env.storage().persistent().set(&DataKey::SmeUtilization(sme.clone()), &new_utilization);

        let draw_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextDrawId)
            .unwrap_or(1);
        env.storage().instance().set(&DataKey::NextDrawId, &(draw_id + 1));

        let now = env.ledger().timestamp();
        let due_date = now.checked_add(duration_seconds).ok_or(RecurringFacilityError::ArithmeticOverflow)?;

        let draw = DrawPosition {
            id: draw_id,
            sme: sme.clone(),
            amount,
            drawn_at: now,
            due_date,
            repaid_amount: 0,
            status: DrawStatus::Active,
        };

        env.storage().persistent().set(&DataKey::Draw(draw_id), &draw);

        let mut sme_draws: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::SmeDraws(sme.clone()))
            .unwrap_or_else(|| Vec::new(&env));
        sme_draws.push_back(draw_id);
        env.storage().persistent().set(&DataKey::SmeDraws(sme.clone()), &sme_draws);

        // Transfer funds from facility contract to SME
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&env.current_contract_address(), &sme, &amount);

        Ok(draw_id)
    }

    pub fn repay_draw(
        env: Env,
        sme: Address,
        token: Address,
        draw_id: u64,
        amount: i128,
    ) -> Result<(), RecurringFacilityError> {
        sme.require_auth();
        let _guard = ReentrancyGuard::new(&env)?;
        require_non_zero_amount(amount)?;

        let mut draw: DrawPosition = env
            .storage()
            .persistent()
            .get(&DataKey::Draw(draw_id))
            .ok_or(RecurringFacilityError::DrawNotFound)?;

        if draw.status == DrawStatus::Repaid {
            return Err(RecurringFacilityError::DrawAlreadyRepaid);
        }
        if draw.status == DrawStatus::Defaulted {
            return Err(RecurringFacilityError::DrawAlreadyDefaulted);
        }

        let new_repaid = safe_add(draw.repaid_amount, amount)?;
        if new_repaid >= draw.amount {
            draw.status = DrawStatus::Repaid;
            draw.repaid_amount = draw.amount;
        } else {
            draw.repaid_amount = new_repaid;
        }

        env.storage().persistent().set(&DataKey::Draw(draw_id), &draw);

        // Reduce SME utilization and restore pool capital
        let current_utilization = Self::get_utilization(env.clone(), draw.sme.clone());
        let new_utilization = safe_sub(current_utilization, amount).unwrap_or(0);
        env.storage().persistent().set(&DataKey::SmeUtilization(draw.sme.clone()), &new_utilization);

        let pool_capital = Self::get_pool_capital(env.clone());
        let new_pool_capital = safe_add(pool_capital, amount)?;
        env.storage().instance().set(&DataKey::CommittedPoolCapital, &new_pool_capital);

        // Transfer repayment from SME to facility contract
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&sme, &env.current_contract_address(), &amount);

        Ok(())
    }

    pub fn record_draw_default(
        env: Env,
        admin: Address,
        draw_id: u64,
    ) -> Result<(), RecurringFacilityError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let mut draw: DrawPosition = env
            .storage()
            .persistent()
            .get(&DataKey::Draw(draw_id))
            .ok_or(RecurringFacilityError::DrawNotFound)?;

        if draw.status == DrawStatus::Repaid {
            return Err(RecurringFacilityError::DrawAlreadyRepaid);
        }
        if draw.status == DrawStatus::Defaulted {
            return Err(RecurringFacilityError::DrawAlreadyDefaulted);
        }

        draw.status = DrawStatus::Defaulted;
        env.storage().persistent().set(&DataKey::Draw(draw_id), &draw);
        Ok(())
    }

    pub fn get_draw(env: Env, draw_id: u64) -> Result<DrawPosition, RecurringFacilityError> {
        env.storage()
            .persistent()
            .get(&DataKey::Draw(draw_id))
            .ok_or(RecurringFacilityError::DrawNotFound)
    }

    fn require_admin(env: &Env, admin: &Address) -> Result<(), RecurringFacilityError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(RecurringFacilityError::NotInitialized)?;
        if stored_admin != *admin {
            return Err(RecurringFacilityError::NotAdmin);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        Address, Env,
    };

    #[test]
    fn test_recurring_facility_lifecycle() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let risk_registry = Address::generate(&env);
        let financing_pool = Address::generate(&env);
        let sme = Address::generate(&env);

        let contract_id = env.register_contract(None, RecurringFacilityContract);
        let client = RecurringFacilityContractClient::new(&env, &contract_id);

        client.initialize(&admin, &risk_registry, &financing_pool);

        // Set SME facility credit limit to 1,000,000
        client.set_facility_limit(&admin, &sme, &1_000_000i128);
        assert_eq!(client.get_facility_limit(&sme), 1_000_000i128);
        assert_eq!(client.get_utilization(&sme), 0i128);
    }
}
