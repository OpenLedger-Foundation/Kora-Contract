#![no_std]

use kora_shared::{
    errors::CommonError,
    reentrancy::ReentrancyGuard,
    types::RiskTier,
    validation::{require_non_negative_amount, require_non_zero_amount, safe_add, safe_mul, safe_sub},
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum InsurancePoolError {
    AlreadyInitialized = 1,
    NotAdmin = 2,
    NotInitialized = 3,
    PolicyNotFound = 4,
    AlreadyClaimed = 5,
    InvalidAmount = 6,
    ArithmeticOverflow = 7,
    Reentrancy = 8,
    InsufficientPoolSolvency = 9,
    Unauthorized = 10,
}

impl From<CommonError> for InsurancePoolError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => InsurancePoolError::InvalidAmount,
            CommonError::ArithmeticOverflow => InsurancePoolError::ArithmeticOverflow,
            CommonError::Reentrancy => InsurancePoolError::Reentrancy,
            _ => InsurancePoolError::InvalidAmount,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsurancePolicy {
    pub invoice_id: u64,
    pub investor: Address,
    pub coverage_amount: i128,
    pub premium_paid: i128,
    pub purchased_at: u64,
    pub claimed: bool,
}

#[contracttype]
pub enum DataKey {
    Admin,
    FinancingPool,
    RiskRegistry,
    PoolBalance,
    Policy(u64, Address),
    ClaimPaid(u64, Address),
}

#[contract]
pub struct InsurancePoolContract;

#[contractimpl]
impl InsurancePoolContract {
    pub fn initialize(
        env: Env,
        admin: Address,
        financing_pool: Address,
        risk_registry: Address,
    ) -> Result<(), InsurancePoolError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(InsurancePoolError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().instance().set(&DataKey::RiskRegistry, &risk_registry);
        env.storage().instance().set(&DataKey::PoolBalance, &0i128);
        Ok(())
    }

    pub fn deposit_premium(
        env: Env,
        investor: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), InsurancePoolError> {
        investor.require_auth();
        let _guard = ReentrancyGuard::new(&env)?;
        require_non_zero_amount(amount)?;

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&investor, &env.current_contract_address(), &amount);

        let balance: i128 = env.storage().instance().get(&DataKey::PoolBalance).unwrap_or(0);
        let new_balance = safe_add(balance, amount)?;
        env.storage().instance().set(&DataKey::PoolBalance, &new_balance);

        Ok(())
    }

    pub fn purchase_coverage(
        env: Env,
        investor: Address,
        token: Address,
        invoice_id: u64,
        coverage_amount: i128,
        risk_score: u32,
    ) -> Result<(), InsurancePoolError> {
        investor.require_auth();
        let _guard = ReentrancyGuard::new(&env)?;
        require_non_zero_amount(coverage_amount)?;

        // Premium rate scales with risk score: base 1% (100 bps) + risk_score * 5 bps
        let tier = RiskTier::from_score(risk_score);
        let premium_bps: i128 = match tier {
            RiskTier::AAA => 100,
            RiskTier::AA => 200,
            RiskTier::A => 300,
            RiskTier::B => 500,
            RiskTier::C => 800,
        };

        let premium_amount = safe_mul(coverage_amount, premium_bps)? / 10_000i128;
        let actual_premium = premium_amount.max(1i128);

        // Transfer premium from investor to insurance pool contract
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&investor, &env.current_contract_address(), &actual_premium);

        let balance: i128 = env.storage().instance().get(&DataKey::PoolBalance).unwrap_or(0);
        let new_balance = safe_add(balance, actual_premium)?;
        env.storage().instance().set(&DataKey::PoolBalance, &new_balance);

        let policy = InsurancePolicy {
            invoice_id,
            investor: investor.clone(),
            coverage_amount,
            premium_paid: actual_premium,
            purchased_at: env.ledger().timestamp(),
            claimed: false,
        };

        let policy_key = DataKey::Policy(invoice_id, investor);
        env.storage().persistent().set(&policy_key, &policy);
        Ok(())
    }

    pub fn file_claim(
        env: Env,
        investor: Address,
        token: Address,
        invoice_id: u64,
    ) -> Result<i128, InsurancePoolError> {
        investor.require_auth();
        let _guard = ReentrancyGuard::new(&env)?;

        let policy_key = DataKey::Policy(invoice_id, investor.clone());
        let mut policy: InsurancePolicy = env
            .storage()
            .persistent()
            .get(&policy_key)
            .ok_or(InsurancePoolError::PolicyNotFound)?;

        if policy.claimed {
            return Err(InsurancePoolError::AlreadyClaimed);
        }

        let claim_key = DataKey::ClaimPaid(invoice_id, investor.clone());
        if env.storage().persistent().get::<_, bool>(&claim_key).unwrap_or(false) {
            return Err(InsurancePoolError::AlreadyClaimed);
        }

        // CEI: Mark claimed in state before external transfer
        policy.claimed = true;
        env.storage().persistent().set(&policy_key, &policy);
        env.storage().persistent().set(&claim_key, &true);

        let balance: i128 = env.storage().instance().get(&DataKey::PoolBalance).unwrap_or(0);

        // Payout bounded by available pool solvency (pro-rata / max available)
        let payout = if balance >= policy.coverage_amount {
            policy.coverage_amount
        } else {
            balance
        };

        if payout > 0 {
            let new_balance = safe_sub(balance, payout)?;
            env.storage().instance().set(&DataKey::PoolBalance, &new_balance);

            let token_client = token::Client::new(&env, &token);
            token_client.transfer(&env.current_contract_address(), &investor, &payout);
        }

        Ok(payout)
    }

    pub fn get_pool_balance(env: Env) -> i128 {
        env.storage().instance().get(&DataKey::PoolBalance).unwrap_or(0)
    }

    pub fn get_policy(
        env: Env,
        invoice_id: u64,
        investor: Address,
    ) -> Result<InsurancePolicy, InsurancePoolError> {
        env.storage()
            .persistent()
            .get(&DataKey::Policy(invoice_id, investor))
            .ok_or(InsurancePoolError::PolicyNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::Address as _,
        Address, Env,
    };

    #[test]
    fn test_insurance_pool_initialization() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let financing_pool = Address::generate(&env);
        let risk_registry = Address::generate(&env);

        let contract_id = env.register_contract(None, InsurancePoolContract);
        let client = InsurancePoolContractClient::new(&env, &contract_id);

        client.initialize(&admin, &financing_pool, &risk_registry);
        assert_eq!(client.get_pool_balance(), 0i128);
    }
}
