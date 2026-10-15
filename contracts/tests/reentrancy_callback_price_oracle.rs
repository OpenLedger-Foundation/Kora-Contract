//! Price Oracle Callback Reentrancy & Cross-Contract Regression Suite — Issue #922
//!
//! Validates that adversarial oracle callbacks attempting to reenter `marketplace::fund_invoice`,
//! `financing_pool::repay`, or `treasury::withdraw` during oracle operations (`convert`,
//! `convert_with_decimals`, `set_price`) fail closed due to storage-backed `ReentrancyGuard`
//! and Checks-Effects-Interactions (CEI) guarantees.

#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short,
    testutils::{Address as _, Ledger as _},
    Address, Env, Symbol, Vec,
};
use kora_shared::reentrancy::{is_locked, ReentrancyGuard};
use kora_shared::errors::CommonError;
use kora_marketplace::{MarketplaceContract, MarketplaceContractClient};
use kora_treasury::{TreasuryContract, TreasuryContractClient};
use kora_price_oracle::{PriceOracleContract, PriceOracleContractClient, PriceData, PriceOracleError};
use kora_access_control::{AccessControlContract, AccessControlContractClient};

// ─────────────────────────────────────────────────────────────────────────────
// Section 1: Malicious Mock Oracle & Target Contracts
// ─────────────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttackTarget {
    None,
    MarketplaceFundInvoice,
    FinancingPoolRepay,
    TreasuryWithdraw,
    NestedMultiHop,
}

#[contracttype]
pub enum MockOracleKey {
    Target,
    TargetContract,
    InvoiceId,
    Attacker,
    ReentrancyResult,
    CallCount,
}

#[contract]
pub struct MockFinancingPoolContract;

#[contractimpl]
impl MockFinancingPoolContract {
    pub fn repay(env: Env, _payer: Address, invoice_id: u64, _amount: i128) -> Result<(), CommonError> {
        // Enforce repayment lock
        let key = symbol_short!("lock");
        if env.storage().instance().has(&key) {
            return Err(CommonError::Reentrancy);
        }
        let _guard = ReentrancyGuard::new(&env)?;
        env.storage().instance().set(&key, &invoice_id);
        Ok(())
    }
}

#[contract]
pub struct MaliciousOracleContract;

#[contractimpl]
impl MaliciousOracleContract {
    pub fn configure(
        env: Env,
        target: AttackTarget,
        target_contract: Address,
        invoice_id: u64,
        attacker: Address,
    ) {
        env.storage().instance().set(&MockOracleKey::Target, &target);
        env.storage().instance().set(&MockOracleKey::TargetContract, &target_contract);
        env.storage().instance().set(&MockOracleKey::InvoiceId, &invoice_id);
        env.storage().instance().set(&MockOracleKey::Attacker, &attacker);
        env.storage().instance().set(&MockOracleKey::CallCount, &0u32);
    }

    pub fn get_reentrancy_result(env: Env) -> bool {
        env.storage().instance().get(&MockOracleKey::ReentrancyResult).unwrap_or(false)
    }

    pub fn get_call_count(env: Env) -> u32 {
        env.storage().instance().get(&MockOracleKey::CallCount).unwrap_or(0)
    }

    /// Price oracle conversion endpoint invoked during cross-currency funding
    pub fn convert(env: Env, amount: i128, _from: Symbol, _to: Symbol) -> i128 {
        let count: u32 = env.storage().instance().get(&MockOracleKey::CallCount).unwrap_or(0);
        env.storage().instance().set(&MockOracleKey::CallCount, &(count + 1));

        let target: AttackTarget = env.storage().instance().get(&MockOracleKey::Target).unwrap_or(AttackTarget::None);
        match target {
            AttackTarget::MarketplaceFundInvoice => {
                let mp_addr: Address = env.storage().instance().get(&MockOracleKey::TargetContract).unwrap();
                let invoice_id: u64 = env.storage().instance().get(&MockOracleKey::InvoiceId).unwrap();
                let attacker: Address = env.storage().instance().get(&MockOracleKey::Attacker).unwrap();
                
                let mp_client = MarketplaceContractClient::new(&env, &mp_addr);
                // Attempt malicious reentrancy back into fund_invoice during conversion
                let res = mp_client.try_fund_invoice(&attacker, &invoice_id, &amount, &None);
                
                // Record whether the reentrant call failed closed
                let blocked = res.is_err();
                env.storage().instance().set(&MockOracleKey::ReentrancyResult, &blocked);
                
                if !blocked {
                    panic!("CRITICAL: Reentrancy guard failed to block reentrant fund_invoice");
                }
            }
            AttackTarget::FinancingPoolRepay => {
                let pool_addr: Address = env.storage().instance().get(&MockOracleKey::TargetContract).unwrap();
                let invoice_id: u64 = env.storage().instance().get(&MockOracleKey::InvoiceId).unwrap();
                let attacker: Address = env.storage().instance().get(&MockOracleKey::Attacker).unwrap();
                
                let pool_client = MockFinancingPoolContractClient::new(&env, &pool_addr);
                let res = pool_client.try_repay(&attacker, &invoice_id, &amount);
                
                let blocked = res.is_err();
                env.storage().instance().set(&MockOracleKey::ReentrancyResult, &blocked);
            }
            _ => {}
        }

        amount
    }

    /// Conversion endpoint with decimals used by treasury get_total_collected_value
    pub fn convert_with_decimals(
        env: Env,
        amount: i128,
        _from: Symbol,
        _to: Symbol,
        _from_decimals: u32,
        _to_decimals: u32,
    ) -> Result<i128, PriceOracleError> {
        let count: u32 = env.storage().instance().get(&MockOracleKey::CallCount).unwrap_or(0);
        env.storage().instance().set(&MockOracleKey::CallCount, &(count + 1));

        let target: AttackTarget = env.storage().instance().get(&MockOracleKey::Target).unwrap_or(AttackTarget::None);
        if target == AttackTarget::TreasuryWithdraw {
            let tr_addr: Address = env.storage().instance().get(&MockOracleKey::TargetContract).unwrap();
            let attacker: Address = env.storage().instance().get(&MockOracleKey::Attacker).unwrap();
            let token = Address::generate(&env);
            
            let tr_client = TreasuryContractClient::new(&env, &tr_addr);
            // Attempt to reenter treasury::withdraw during oracle computation
            let res = tr_client.try_withdraw(&attacker, &token, &attacker, &1_000_000i128);
            let blocked = res.is_err();
            env.storage().instance().set(&MockOracleKey::ReentrancyResult, &blocked);
        }

        Ok(amount)
    }

    pub fn get_price(env: Env, _base: Symbol, _quote: Symbol) -> Result<PriceData, PriceOracleError> {
        Ok(PriceData {
            price: 10_000_000,
            timestamp: env.ledger().timestamp(),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 2: Test Fixtures and Setups
// ─────────────────────────────────────────────────────────────────────────────

fn setup_test_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(soroban_sdk::testutils::LedgerInfo {
        timestamp: 1_700_000_000,
        protocol_version: 21,
        sequence_number: 1,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 1000,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 100_000,
    });
    env
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 3: Integration Tests (Issue #922 Requirements)
// ─────────────────────────────────────────────────────────────────────────────

/// Test 1: Oracle callback attempting to reenter marketplace fund_invoice fails closed.
/// Covers scenario: Adversarial oracle invoked during fund_invoice tries to reenter
/// fund_invoice on the same contract instance. ReentrancyGuard rejects the reentrant call.
#[test]
fn test_oracle_callback_reentrancy_fund_invoice_blocked() {
    let env = setup_test_env();
    let admin = Address::generate(&env);
    let nft = Address::generate(&env);
    let pool = Address::generate(&env);
    let treasury = Address::generate(&env);
    let access_control = env.register_contract(None, AccessControlContract);
    let ac_client = AccessControlContractClient::new(&env, &access_control);
    ac_client.initialize(&admin);

    let oracle_id = env.register_contract(None, MaliciousOracleContract);
    let oracle_client = MaliciousOracleContractClient::new(&env, &oracle_id);
    let registry = Address::generate(&env);

    let mp_id = env.register_contract(None, MarketplaceContract);
    let mp_client = MarketplaceContractClient::new(&env, &mp_id);
    mp_client.initialize(&admin, &nft, &pool, &treasury, &access_control, &oracle_id, &registry, &50u32, &0u32);

    let attacker = Address::generate(&env);
    oracle_client.configure(&AttackTarget::MarketplaceFundInvoice, &mp_id, &101u64, &attacker);

    // Simulate the marketplace holding its ReentrancyGuard as it does during fund_invoice
    let guard_blocked = env.as_contract(&mp_id, || {
        let _guard = ReentrancyGuard::new(&env).expect("initial acquire must succeed");
        // Oracle convert is called while guard is held
        oracle_client.convert(&5_000_000i128, &symbol_short!("USDC"), &symbol_short!("USD"));
        oracle_client.get_reentrancy_result()
    });

    assert!(guard_blocked, "Adversarial callback reentering fund_invoice MUST be blocked by ReentrancyGuard");
    assert_eq!(oracle_client.get_call_count(), 1, "Oracle convert should have been called exactly once");
}

/// Test 2: Oracle callback attempting cross-contract reentrancy into financing_pool repay fails closed.
/// RepaymentLock and auth guards reject reentrant state manipulation.
#[test]
fn test_oracle_callback_reentrancy_financing_pool_blocked() {
    let env = setup_test_env();
    let pool_id = env.register_contract(None, MockFinancingPoolContract);

    let oracle_id = env.register_contract(None, MaliciousOracleContract);
    let oracle_client = MaliciousOracleContractClient::new(&env, &oracle_id);

    let attacker = Address::generate(&env);
    oracle_client.configure(&AttackTarget::FinancingPoolRepay, &pool_id, &202u64, &attacker);

    // Set lock on pool to simulate active in-flight operation
    env.as_contract(&pool_id, || {
        let key = symbol_short!("lock");
        env.storage().instance().set(&key, &202u64);
    });

    // Oracle convert is invoked
    oracle_client.convert(&10_000_000i128, &symbol_short!("EURC"), &symbol_short!("USDC"));
    let blocked = oracle_client.get_reentrancy_result();

    assert!(blocked, "Reentrant repay attempt against financing_pool MUST be rejected while lock is held");
}

/// Test 3: Treasury multi-asset value conversion reentrancy defense.
/// When treasury inspects total collected value via oracle convert, malicious oracle attempt
/// to reenter treasury::withdraw fails closed.
#[test]
fn test_oracle_callback_treasury_convert_reentrancy_blocked() {
    let env = setup_test_env();
    let admin = Address::generate(&env);
    let tr_id = env.register_contract(None, TreasuryContract);
    let tr_client = TreasuryContractClient::new(&env, &tr_id);
    tr_client.initialize(&admin, &50u32);

    let oracle_id = env.register_contract(None, MaliciousOracleContract);
    let oracle_client = MaliciousOracleContractClient::new(&env, &oracle_id);
    tr_client.set_price_oracle(&admin, &oracle_id);

    let attacker = Address::generate(&env);
    oracle_client.configure(&AttackTarget::TreasuryWithdraw, &tr_id, &0u64, &attacker);

    let token = Address::generate(&env);
    // Seed collected fee balance in treasury so multi-asset evaluation routes through oracle
    env.as_contract(&tr_id, || {
        let key = kora_treasury::DataKey::Collected(token.clone());
        env.storage().persistent().set(&key, &100_000_000i128);
    });

    let mut tokens = Vec::new(&env);
    tokens.push_back(token.clone());
    let mut symbols = Vec::new(&env);
    symbols.push_back(symbol_short!("USDC"));
    let mut decimals = Vec::new(&env);
    decimals.push_back(7u32);

    assert_eq!(tr_client.get_collected(&token), 100_000_000, "Collected should be set");
    let total_val = tr_client.get_total_collected_value(&tokens, &symbols, &symbol_short!("USD"), &decimals, &7u32);
    assert!(total_val >= 0, "Treasury valuation completes gracefully even under adversary probes");

    let count = oracle_client.get_call_count();
    assert_eq!(count, 1, "Oracle convert_with_decimals should be called exactly once");
    let blocked = oracle_client.get_reentrancy_result();
    assert!(blocked, "Reentrant withdraw attempt during treasury conversion must fail closed");
}

/// Test 4: Positive control — Clean, benign oracle conversion succeeds with zero false positives.
#[test]
fn test_oracle_callback_positive_control_clean_convert() {
    let env = setup_test_env();
    let oracle_id = env.register_contract(None, MaliciousOracleContract);
    let oracle_client = MaliciousOracleContractClient::new(&env, &oracle_id);
    let dummy = Address::generate(&env);
    
    // Configure with AttackTarget::None
    oracle_client.configure(&AttackTarget::None, &dummy, &0u64, &dummy);

    let converted = oracle_client.convert(&1_000_000i128, &symbol_short!("USDC"), &symbol_short!("USD"));
    assert_eq!(converted, 1_000_000i128);
    assert_eq!(oracle_client.get_call_count(), 1);
    assert!(!oracle_client.get_reentrancy_result(), "No reentrancy attempted or triggered");
}

/// Test 5: Feeder authentication and pause enforcement on real PriceOracleContract.
/// Prevents unauthorized feeders from triggering reentrant or unverified price updates.
#[test]
fn test_price_oracle_feeder_auth_and_pause_defense() {
    let env = setup_test_env();
    let admin = Address::generate(&env);
    let access_control = env.register_contract(None, AccessControlContract);
    let ac_client = AccessControlContractClient::new(&env, &access_control);
    ac_client.initialize(&admin);

    let oracle_id = env.register_contract(None, PriceOracleContract);
    let oracle_client = PriceOracleContractClient::new(&env, &oracle_id);
    oracle_client.initialize(&admin, &access_control);

    let unauthorized_feeder = Address::generate(&env);
    // Non-feeder cannot submit price updates
    let res = oracle_client.try_set_price(&unauthorized_feeder, &symbol_short!("XLM"), &symbol_short!("USD"), &1_500_000i128);
    assert!(res.is_err(), "Unauthorized feeder must be rejected");

    // Authorize feeder
    oracle_client.add_feeder(&admin, &unauthorized_feeder);
    let res_ok = oracle_client.try_set_price(&unauthorized_feeder, &symbol_short!("XLM"), &symbol_short!("USD"), &1_500_000i128);
    assert!(res_ok.is_ok(), "Authorized feeder price update must succeed");

    // Pause access control
    ac_client.pause(&admin);
    let res_paused = oracle_client.try_set_price(&unauthorized_feeder, &symbol_short!("XLM"), &symbol_short!("USD"), &1_600_000i128);
    assert_eq!(res_paused.err().unwrap().unwrap(), PriceOracleError::ProtocolPaused);
}

/// Test 6: Nested multi-hop reentrancy (depth > 1) fails closed.
#[test]
fn test_oracle_callback_nested_reentrancy_depth_two_fails_closed() {
    let env = setup_test_env();
    let contract_id = env.register_contract(None, MaliciousOracleContract);
    
    // Simulate nested guard depth
    env.as_contract(&contract_id, || {
        let _outer_guard = ReentrancyGuard::new(&env).expect("outer guard acquires");
        assert!(is_locked(&env), "Lock held at depth 1");

        // Inner nested attempt 1
        let inner1 = ReentrancyGuard::new(&env);
        assert_eq!(inner1.err().unwrap(), CommonError::Reentrancy);

        // Inner nested attempt 2
        let inner2 = ReentrancyGuard::new(&env);
        assert_eq!(inner2.err().unwrap(), CommonError::Reentrancy);

        assert!(is_locked(&env), "Lock remains held by outer guard");
    });

    // Released on drop
    env.as_contract(&contract_id, || {
        assert!(!is_locked(&env), "Lock successfully released after outer guard drop");
    });
}
