#![no_std]

use kora_shared::{
    errors::KoraError,
    events,
    reentrancy::ReentrancyGuard,
    validation::require_valid_fee_bps,
};
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env};

// ── Storage TTL constants (~31 days in ledgers) ───────────────────────────────
const PERSISTENT_BUMP_AMOUNT: u32 = 535_680;
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 535_680 / 2;

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Admin address — persistent so it survives ledger archival.
    Admin,
    /// Protocol fee in basis points — persistent for durability.
    FeeBps,
    /// Accumulated fees per token (informational).
    Collected(Address),
    /// Whitelisted token flag.
    WhitelistedToken(Address),
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct TreasuryContract;

#[contractimpl]
impl TreasuryContract {
    /// One-time initialization. Sets admin and protocol fee.
    pub fn initialize(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }
        require_valid_fee_bps(fee_bps)?;
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().extend_ttl(
            &DataKey::Admin,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().set(&DataKey::FeeBps, &fee_bps);
        env.storage().persistent().extend_ttl(
            &DataKey::FeeBps,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::treasury_initialized(&env, &admin, fee_bps);
        Ok(())
    }

    /// Update protocol fee. Admin only.
    pub fn set_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_valid_fee_bps(fee_bps)?;

        let old_bps: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::FeeBps)
            .unwrap_or(50);

        env.storage().persistent().set(&DataKey::FeeBps, &fee_bps);
        Self::bump_persistent(&env, &DataKey::FeeBps);

        events::fee_rate_updated(&env, &admin, old_bps, fee_bps);
        Ok(())
    }

    /// Whitelist a token so it can be used in withdraw / emergency_withdraw.
    /// Admin only.
    pub fn whitelist_token(env: Env, admin: Address, token: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedToken(token.clone()), &true);
        Self::bump_persistent(&env, &DataKey::WhitelistedToken(token.clone()));

        events::token_whitelisted(&env, &token);
        Ok(())
    }

    /// Record an incoming fee for a given token. Called by the marketplace after
    /// transferring the fee amount to this contract. Updates the informational
    /// accounting ledger.
    ///
    /// No auth required — the token transfer itself is the proof of payment.
    /// The amount is validated to be > 0 to prevent no-op accounting entries.
    pub fn collect_fee(env: Env, token: Address, amount: i128) -> Result<(), KoraError> {
        if amount <= 0 {
            return Err(KoraError::InvalidAmount);
        }
        Self::require_whitelisted_token(&env, &token)?;

        let key = DataKey::Collected(token.clone());
        let current: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        let new_total = current
            .checked_add(amount)
            .ok_or(KoraError::ArithmeticOverflow)?;

        env.storage().persistent().set(&key, &new_total);
        Self::bump_persistent(&env, &key);

        events::fee_collected(&env, 0, amount, &token);
        Ok(())
    }

    /// Withdraw accumulated fees to a recipient. Admin only.
    /// Protected against reentrancy via RAII ReentrancyGuard.
    pub fn withdraw(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), KoraError> {
        // ── Checks ────────────────────────────────────────────────────────────
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if amount <= 0 {
            return Err(KoraError::InvalidAmount);
        }
        Self::require_whitelisted_token(&env, &token)?;

        // Acquire reentrancy guard — released automatically when _guard drops
        let _guard = ReentrancyGuard::new(&env)?;

        let token_client = token::Client::new(&env, &token);
        let balance = token_client.balance(&env.current_contract_address());

        if balance < amount {
            return Err(KoraError::InsufficientPoolBalance);
        }

        // ── Effects ───────────────────────────────────────────────────────────
        let collected_key = DataKey::Collected(token.clone());
        if let Some(collected) = env
            .storage()
            .persistent()
            .get::<_, i128>(&collected_key)
        {
            let new_collected = collected.saturating_sub(amount);
            env.storage()
                .persistent()
                .set(&collected_key, &new_collected);
            Self::bump_persistent(&env, &collected_key);
        }

        // ── Interactions ──────────────────────────────────────────────────────
        token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        events::fee_withdrawn(&env, &token, amount);
        Ok(())
    }

    /// Emergency drain — withdraw entire token balance. Admin only.
    /// Protected against reentrancy via RAII ReentrancyGuard.
    pub fn emergency_withdraw(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
    ) -> Result<(), KoraError> {
        // ── Checks ────────────────────────────────────────────────────────────
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_whitelisted_token(&env, &token)?;

        // Acquire reentrancy guard — released automatically when _guard drops
        let _guard = ReentrancyGuard::new(&env)?;

        let token_client = token::Client::new(&env, &token);
        let balance = token_client.balance(&env.current_contract_address());

        // ── Interactions ──────────────────────────────────────────────────────
        if balance > 0 {
            token_client.transfer(&env.current_contract_address(), &recipient, &balance);
            events::emergency_withdrawn(&env, &admin, &token, balance);
        }

        Ok(())
    }

    /// Returns the current protocol fee in basis points.
    pub fn get_fee_bps(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::FeeBps)
            .unwrap_or(50)
    }

    /// Returns the live token balance held by this contract.
    pub fn get_balance(env: Env, token: Address) -> i128 {
        token::Client::new(&env, &token).balance(&env.current_contract_address())
    }

    pub fn get_admin(env: Env) -> Result<Address, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(KoraError::NotInitialized)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), KoraError> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(KoraError::NotInitialized)?;
        if &admin != caller {
            return Err(KoraError::NotAdmin);
        }
        Ok(())
    }

    fn require_whitelisted_token(env: &Env, token: &Address) -> Result<(), KoraError> {
        let whitelisted: bool = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false);
        if !whitelisted {
            return Err(KoraError::TokenNotWhitelisted);
        }
        Ok(())
    }

    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage().persistent().extend_ttl(
            key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, Address, TreasuryContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &50u32).unwrap();
        (env, admin, client)
    }

    #[test]
    fn test_initialize_creates_contract() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        assert!(client.try_initialize(&admin, &50u32).is_ok());
        assert_eq!(client.get_fee_bps(), 50);
    }

    #[test]
    fn test_initialize_already_initialized() {
        let (env, admin, client) = setup();
        let result = client.try_initialize(&admin, &50u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_invalid_fee_bps() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        assert!(client.try_initialize(&admin, &10_001u32).is_err());
    }

    #[test]
    fn test_get_fee_bps_after_init() {
        let (_env, _admin, client) = setup();
        assert_eq!(client.get_fee_bps(), 50);
    }

    // ── set_fee_bps ───────────────────────────────────────────────────────────

    #[test]
    fn test_set_fee_bps_success() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &100u32).unwrap();
        assert_eq!(client.get_fee_bps(), 100);
    }

    #[test]
    fn test_set_fee_bps_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        assert!(client.try_set_fee_bps(&non_admin, &100u32).is_err());
    }

    #[test]
    fn test_set_fee_bps_invalid_bps_fails() {
        let (_env, admin, client) = setup();
        assert!(client.try_set_fee_bps(&admin, &10_001u32).is_err());
    }

    #[test]
    fn test_set_fee_bps_zero_allowed() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &0u32).unwrap();
        assert_eq!(client.get_fee_bps(), 0);
    }

    #[test]
    fn test_set_fee_bps_max_allowed() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &10_000u32).unwrap();
        assert_eq!(client.get_fee_bps(), 10_000);
    }

    #[test]
    fn test_set_fee_bps_over_max_fails() {
        let (_env, admin, client) = setup();
        assert!(client.try_set_fee_bps(&admin, &10_001u32).is_err());
    }

    #[test]
    fn test_set_fee_bps_multiple_updates() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &100u32).unwrap();
        assert_eq!(client.get_fee_bps(), 100);
        client.set_fee_bps(&admin, &200u32).unwrap();
        assert_eq!(client.get_fee_bps(), 200);
        client.set_fee_bps(&admin, &50u32).unwrap();
        assert_eq!(client.get_fee_bps(), 50);
    }

    // ── withdraw ──────────────────────────────────────────────────────────────

    #[test]
    fn test_withdraw_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client.try_withdraw(&non_admin, &token, &recipient, &1_000_000i128).is_err());
    }

    #[test]
    fn test_withdraw_zero_amount_fails() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client.try_withdraw(&admin, &token, &recipient, &0i128).is_err());
    }

    #[test]
    fn test_withdraw_with_negative_amount_rejected() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client.try_withdraw(&admin, &token, &recipient, &-1_000i128).is_err());
    }

    #[test]
    fn test_emergency_withdraw_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client.try_emergency_withdraw(&non_admin, &token, &recipient).is_err());
    }

    #[test]
    fn test_get_fee_bps_default_before_init() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        assert_eq!(client.get_fee_bps(), 50);
    }

    #[test]
    fn test_lock_released_after_failed_withdraw() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        // Fails due to token not whitelisted — lock must be released
        let _ = client.try_withdraw(&admin, &token, &recipient, &1_000i128);
        // Subsequent admin operation must succeed (lock not stuck)
        assert!(client.try_set_fee_bps(&admin, &100u32).is_ok());
    }

    #[test]
    fn test_lock_released_after_emergency_withdraw() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        let _ = client.try_emergency_withdraw(&admin, &token, &recipient);
        // Lock must be released regardless of balance
        assert!(client.try_set_fee_bps(&admin, &100u32).is_ok());
    }
}
