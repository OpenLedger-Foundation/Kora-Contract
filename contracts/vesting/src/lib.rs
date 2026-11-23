#![no_std]

use kora_shared::{
    errors::CommonError,
    events,
    validation::{require_non_negative_amount, require_not_self, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env};

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum VestingError {
    AlreadyInitialized = 1,
    NotAdmin = 2,
    NotInitialized = 3,
    InvalidAddress = 4,
    InvalidAmount = 5,
    InvalidVestingSchedule = 6,
    GrantAlreadyExists = 7,
    GrantNotFound = 8,
    InsufficientVestedBalance = 9,
    GrantAlreadyRevoked = 10,
    ArithmeticOverflow = 11,
    NoUpgradeProposed = 12,
    UpgradeTimelockNotElapsed = 13,
}

impl From<CommonError> for VestingError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => VestingError::InvalidAmount,
            CommonError::InvalidAddress => VestingError::InvalidAddress,
            CommonError::ArithmeticOverflow => VestingError::ArithmeticOverflow,
            _ => VestingError::InvalidAmount,
        }
    }
}

// ── Storage TTL constants (~31 days in ledgers) ───────────────────────────────
const PERSISTENT_BUMP_AMOUNT: u32 = 535_680;
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 535_680 / 2;

// ── Types ─────────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug)]
pub struct VestingGrant {
    pub beneficiary: Address,
    pub total_amount: i128,
    pub start_time: u64,
    pub cliff_duration: u64,
    pub vesting_duration: u64,
    pub released_amount: i128,
    pub revoked: bool,
    pub revoked_at: Option<u64>,
}

#[contracttype]
pub enum DataKey {
    Admin,
    TokenAddress,
    Grant(Address),
    UpgradeProposal,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct VestingContract;

#[contractimpl]
impl VestingContract {
    /// Initialize the vesting contract with an admin and token address.
    pub fn initialize(env: Env, admin: Address, token: Address) -> Result<(), VestingError> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(VestingError::AlreadyInitialized);
        }
        require_not_self(&env, &admin)?;
        require_not_self(&env, &token)?;

        env.storage().persistent().set(&DataKey::Admin, &admin);
        Self::bump_persistent(&env, &DataKey::Admin);

        env.storage().persistent().set(&DataKey::TokenAddress, &token);
        Self::bump_persistent(&env, &DataKey::TokenAddress);

        events::vesting_initialized(&env, &admin, &token);

        Ok(())
    }

    /// Create a new vesting grant for a beneficiary. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `beneficiary` — The address that will receive vested tokens.
    /// - `total_amount` — Total tokens to vest (must be > 0).
    /// - `start_time` — Unix timestamp when vesting starts.
    /// - `cliff_duration` — Duration in seconds before first vest.
    /// - `vesting_duration` — Total vesting duration in seconds (including cliff).
    ///
    /// **Errors:**
    /// - `VestingError::NotAdmin` — Caller is not the admin.
    /// - `VestingError::InvalidAmount` — total_amount ≤ 0.
    /// - `VestingError::InvalidVestingSchedule` — vesting_duration ≤ cliff_duration.
    /// - `VestingError::GrantAlreadyExists` — A grant already exists for this beneficiary.
    /// - `VestingError::InvalidAddress` — beneficiary is contract address.
    ///
    /// **Security:** Requires `admin.require_auth()`. Tokens must be transferred
    /// to contract separately before creating grants.
    pub fn create_grant(
        env: Env,
        admin: Address,
        beneficiary: Address,
        total_amount: i128,
        start_time: u64,
        cliff_duration: u64,
        vesting_duration: u64,
    ) -> Result<(), VestingError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_not_self(&env, &beneficiary)?;
        require_non_negative_amount(total_amount)?;

        if total_amount <= 0 {
            return Err(VestingError::InvalidAmount);
        }
        if vesting_duration <= cliff_duration {
            return Err(VestingError::InvalidVestingSchedule);
        }

        let key = DataKey::Grant(beneficiary.clone());
        if env.storage().persistent().has(&key) {
            return Err(VestingError::GrantAlreadyExists);
        }

        let grant = VestingGrant {
            beneficiary: beneficiary.clone(),
            total_amount,
            released_amount: 0,
            start_time,
            cliff_duration,
            vesting_duration,
            revoked: false,
            revoked_at: None,
        };

        env.storage().persistent().set(&key, &grant);
        Self::bump_persistent(&env, &key);

        events::vesting_grant_created(
            &env,
            &admin,
            &beneficiary,
            total_amount,
            start_time,
            cliff_duration,
            vesting_duration,
        );

        Ok(())
    }

    /// Release vested tokens to the beneficiary. Callable by anyone.
    ///
    /// **Parameters:**
    /// - `beneficiary` — The address whose vested tokens should be released.
    ///
    /// **Errors:**
    /// - `VestingError::GrantNotFound` — No grant exists for this beneficiary.
    /// - `VestingError::InsufficientVestedBalance` — No vested tokens available to release.
    ///
    /// **Security:** No auth required — anyone can trigger release, but tokens
    /// always go to the beneficiary. Only vested (not unvested) amounts are released.
    pub fn release(env: Env, beneficiary: Address) -> Result<(), VestingError> {
        let key = DataKey::Grant(beneficiary.clone());
        let mut grant: VestingGrant = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VestingError::GrantNotFound)?;

        let vested = Self::calculate_vested(&env, &grant)?;
        let releasable = vested
            .checked_sub(grant.released_amount)
            .ok_or(VestingError::ArithmeticOverflow)?;

        if releasable <= 0 {
            return Err(VestingError::InsufficientVestedBalance);
        }

        grant.released_amount = grant
            .released_amount
            .checked_add(releasable)
            .ok_or(VestingError::ArithmeticOverflow)?;

        env.storage().persistent().set(&key, &grant);
        Self::bump_persistent(&env, &key);

        let token: Address = env
            .storage()
            .persistent()
            .get(&DataKey::TokenAddress)
            .ok_or(VestingError::NotInitialized)?;

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&env.current_contract_address(), &beneficiary, &releasable);

        events::vesting_tokens_released(&env, &beneficiary, releasable);

        Ok(())
    }

    /// Revoke a grant, forfeiting unvested tokens. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `beneficiary` — The beneficiary whose grant should be revoked.
    ///
    /// **Errors:**
    /// - `VestingError::NotAdmin` — Caller is not the admin.
    /// - `VestingError::GrantNotFound` — No grant exists for this beneficiary.
    /// - `VestingError::GrantAlreadyRevoked` — Grant was already revoked.
    ///
    /// **Security:** Requires `admin.require_auth()`. Vested amounts remain
    /// claimable; only unvested future amounts are forfeited. Revoked grants
    /// freeze vesting calculation at the revocation timestamp.
    pub fn revoke_grant(
        env: Env,
        admin: Address,
        beneficiary: Address,
    ) -> Result<(), VestingError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let key = DataKey::Grant(beneficiary.clone());
        let mut grant: VestingGrant = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VestingError::GrantNotFound)?;

        if grant.revoked {
            return Err(VestingError::GrantAlreadyRevoked);
        }

        grant.revoked = true;
        grant.revoked_at = Some(env.ledger().timestamp());

        env.storage().persistent().set(&key, &grant);
        Self::bump_persistent(&env, &key);

        events::vesting_grant_revoked(&env, &admin, &beneficiary);

        Ok(())
    }

    /// Claw back already-released tokens from a beneficiary. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `beneficiary` — The beneficiary whose released tokens are clawed back.
    /// - `amount` — Amount to claw back (must be > 0 and ≤ released_amount).
    ///
    /// **Errors:**
    /// - `VestingError::NotAdmin` — Caller is not the admin.
    /// - `VestingError::GrantNotFound` — No grant exists for this beneficiary.
    /// - `VestingError::InvalidAmount` — amount ≤ 0 or exceeds released balance.
    ///
    /// **Security:** Requires `admin.require_auth()`. Only previously released
    /// tokens can be clawed back; unvested amounts are untouched.
    pub fn clawback(
        env: Env,
        admin: Address,
        beneficiary: Address,
        amount: i128,
    ) -> Result<(), VestingError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if amount <= 0 {
            return Err(VestingError::InvalidAmount);
        }

        let key = DataKey::Grant(beneficiary.clone());
        let mut grant: VestingGrant = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(VestingError::GrantNotFound)?;

        if amount > grant.released_amount {
            return Err(VestingError::InvalidAmount);
        }

        grant.released_amount = grant
            .released_amount
            .checked_sub(amount)
            .ok_or(VestingError::ArithmeticOverflow)?;

        env.storage().persistent().set(&key, &grant);
        Self::bump_persistent(&env, &key);

        let token: Address = env
            .storage()
            .persistent()
            .get(&DataKey::TokenAddress)
            .ok_or(VestingError::NotInitialized)?;

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&beneficiary, &env.current_contract_address(), &amount);

        events::vesting_tokens_clawed_back(&env, &admin, &beneficiary, amount);

        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), VestingError> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(VestingError::NotInitialized)?;
        if &admin != caller {
            return Err(VestingError::NotAdmin);
        }
        Ok(())
    }

    fn calculate_vested(env: &Env, grant: &VestingGrant) -> Result<i128, VestingError> {
        let now = match grant.revoked_at {
            Some(ts) => ts,
            None => env.ledger().timestamp(),
        };

        if now < grant.start_time.saturating_add(grant.cliff_duration) {
            return Ok(0);
        }

        let elapsed = now.saturating_sub(grant.start_time);
        if elapsed >= grant.vesting_duration {
            return Ok(grant.total_amount);
        }

        let vested = (grant.total_amount as i128)
            .checked_mul(elapsed as i128)
            .ok_or(VestingError::ArithmeticOverflow)?
            / (grant.vesting_duration as i128);

        Ok(vested)
    }

    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger};
    use soroban_sdk::Env;

    fn setup() -> (Env, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, VestingContract);
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        let client = VestingContractClient::new(&env, &contract_id);
        client.initialize(&admin, &token);
        (env, contract_id, admin, token)
    }

    fn create_default_grant(env: &Env, contract_id: &Address, admin: &Address, beneficiary: &Address) {
        let client = VestingContractClient::new(env, contract_id);
        client.create_grant(admin, beneficiary, &1_000, &0, &0, &1_000);
    }

    // ── Happy path ────────────────────────────────────────────────────────────

    #[test]
    fn revoke_grant_by_admin_succeeds() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);
        create_default_grant(&env, &contract_id, &admin, &beneficiary);

        let client = VestingContractClient::new(&env, &contract_id);
        client.revoke_grant(&admin, &beneficiary);

        let grant: VestingGrant = env
            .storage()
            .persistent()
            .get(&DataKey::Grant(beneficiary.clone()))
            .unwrap();
        assert!(grant.revoked);
        assert!(grant.revoked_at.is_some());
    }

    #[test]
    fn clawback_by_admin_succeeds() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);
        create_default_grant(&env, &contract_id, &admin, &beneficiary);

        // Simulate released balance directly to avoid token mock dependency.
        let key = DataKey::Grant(beneficiary.clone());
        let mut grant: VestingGrant = env.storage().persistent().get(&key).unwrap();
        grant.released_amount = 500;
        env.storage().persistent().set(&key, &grant);

        let client = VestingContractClient::new(&env, &contract_id);
        // Clawback will attempt token transfer; mock_all_auths covers auth but
        // token contract is not registered, so we only assert auth gating here.
        let result = client.try_clawback(&admin, &beneficiary, &100);
        // Auth passed (no NotAdmin); failure, if any, is from token transfer.
        assert_ne!(result, Err(Ok(VestingError::NotAdmin)));
    }

    // ── Negative auth ─────────────────────────────────────────────────────────

    #[test]
    fn revoke_grant_by_non_admin_rejected() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);
        let attacker = Address::generate(&env);
        create_default_grant(&env, &contract_id, &admin, &beneficiary);

        let client = VestingContractClient::new(&env, &contract_id);
        let result = client.try_revoke_grant(&attacker, &beneficiary);
        assert_eq!(result, Err(Ok(VestingError::NotAdmin)));
    }

    #[test]
    fn clawback_by_non_admin_rejected() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);
        let attacker = Address::generate(&env);
        create_default_grant(&env, &contract_id, &admin, &beneficiary);

        let client = VestingContractClient::new(&env, &contract_id);
        let result = client.try_clawback(&attacker, &beneficiary, &100);
        assert_eq!(result, Err(Ok(VestingError::NotAdmin)));
    }

    #[test]
    fn revoke_grant_missing_grant_rejected() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);

        let client = VestingContractClient::new(&env, &contract_id);
        let result = client.try_revoke_grant(&admin, &beneficiary);
        assert_eq!(result, Err(Ok(VestingError::GrantNotFound)));
    }

    #[test]
    fn clawback_missing_grant_rejected() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);

        let client = VestingContractClient::new(&env, &contract_id);
        let result = client.try_clawback(&admin, &beneficiary, &100);
        assert_eq!(result, Err(Ok(VestingError::GrantNotFound)));
    }

    #[test]
    fn revoke_grant_twice_rejected() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);
        create_default_grant(&env, &contract_id, &admin, &beneficiary);

        let client = VestingContractClient::new(&env, &contract_id);
        client.revoke_grant(&admin, &beneficiary);
        let result = client.try_revoke_grant(&admin, &beneficiary);
        assert_eq!(result, Err(Ok(VestingError::GrantAlreadyRevoked)));
    }

    #[test]
    fn clawback_exceeding_released_rejected() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);
        create_default_grant(&env, &contract_id, &admin, &beneficiary);

        let client = VestingContractClient::new(&env, &contract_id);
        let result = client.try_clawback(&admin, &beneficiary, &1);
        assert_eq!(result, Err(Ok(VestingError::InvalidAmount)));
    }

    #[test]
    fn clawback_zero_amount_rejected() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);
        create_default_grant(&env, &contract_id, &admin, &beneficiary);

        let client = VestingContractClient::new(&env, &contract_id);
        let result = client.try_clawback(&admin, &beneficiary, &0);
        assert_eq!(result, Err(Ok(VestingError::InvalidAmount)));
    }

    // ── Missing config fails closed ───────────────────────────────────────────

    #[test]
    fn revoke_grant_uninitialized_fails_closed() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, VestingContract);
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);

        let client = VestingContractClient::new(&env, &contract_id);
        let result = client.try_revoke_grant(&admin, &beneficiary);
        assert_eq!(result, Err(Ok(VestingError::NotInitialized)));
    }

    #[test]
    fn clawback_uninitialized_fails_closed() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, VestingContract);
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);

        let client = VestingContractClient::new(&env, &contract_id);
        let result = client.try_clawback(&admin, &beneficiary, &100);
        assert_eq!(result, Err(Ok(VestingError::NotInitialized)));
    }

    // ── Revoked grant freezes vesting ─────────────────────────────────────────

    #[test]
    fn revoked_grant_freezes_vesting_at_revocation() {
        let (env, contract_id, admin, _token) = setup();
        let beneficiary = Address::generate(&env);
        let client = VestingContractClient::new(&env, &contract_id);
        client.create_grant(&admin, &beneficiary, &1_000, &0, &0, &1_000);

        env.ledger().set_timestamp(500);
        client.revoke_grant(&admin, &beneficiary);

        // Advance time; vested amount must stay frozen at revocation point.
        env.ledger().set_timestamp(900);
        let grant: VestingGrant = env
            .storage()
            .persistent()
            .get(&DataKey::Grant(beneficiary.clone()))
            .unwrap();
        let vested = VestingContract::calculate_vested(&env, &grant).unwrap();
        assert_eq!(vested, 500);
    }
}
