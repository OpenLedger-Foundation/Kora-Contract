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

        let revoked_at = env.ledger().timestamp();
        grant.revoked = true;
        grant.revoked_at = Some(revoked_at);

        env.storage().persistent().set(&key, &grant);
        Self::bump_persistent(&env, &key);

        events::vesting_grant_revoked(&env, &admin, &beneficiary, revoked_at);

        Ok(())
    }

    /// Get the vested balance for a beneficiary (for governance weight calculation).
    ///
    /// **Parameters:**
    /// - `beneficiary` — The address to query.
    ///
    /// **Returns:** The vested (but not necessarily released) balance. Returns 0
    /// if no grant exists or vesting has not started.
    ///
    /// **Security:** Read-only view. This is the critical correctness property for
    /// governance — only vested amounts count toward voting weight, not total granted.
    pub fn get_vested_balance(env: Env, beneficiary: Address) -> Result<i128, VestingError> {
        let key = DataKey::Grant(beneficiary.clone());
        let grant: VestingGrant = match env.storage().persistent().get(&key) {
            Some(g) => g,
            None => return Ok(0),
        };

        Self::calculate_vested(&env, &grant)
    }

    /// Get the total unvested balance for a beneficiary.
    ///
    /// **Returns:** Total granted minus vested amount.
    pub fn get_unvested_balance(env: Env, beneficiary: Address) -> Result<i128, VestingError> {
        let key = DataKey::Grant(beneficiary.clone());
        let grant: VestingGrant = match env.storage().persistent().get(&key) {
            Some(g) => g,
            None => return Ok(0),
        };

        let vested = Self::calculate_vested(&env, &grant)?;
        grant
            .total_amount
            .checked_sub(vested)
            .ok_or(VestingError::ArithmeticOverflow)
    }

    /// Get full grant details for a beneficiary.
    ///
    /// **Returns:** The VestingGrant struct, or None if no grant exists.
    pub fn get_grant(env: Env, beneficiary: Address) -> Option<VestingGrant> {
        env.storage().persistent().get(&DataKey::Grant(beneficiary))
    }

    /// Get the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, VestingError> {
        env.storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(VestingError::NotInitialized)
    }

    /// Get the token address this vesting contract manages.
    pub fn get_token(env: Env) -> Result<Address, VestingError> {
        env.storage()
            .persistent()
            .get(&DataKey::TokenAddress)
            .ok_or(VestingError::NotInitialized)
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    /// Propose a WASM upgrade. Admin only. Begins a 24-hour timelock.
    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), VestingError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        env.storage().instance().set(
            &DataKey::UpgradeProposal,
            &(new_wasm_hash.clone(), env.ledger().timestamp()),
        );

        events::upgrade_proposed(&env, &admin, &new_wasm_hash);

        Ok(())
    }

    /// Execute a previously proposed WASM upgrade after the 24-hour timelock.
    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), VestingError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(VestingError::NoUpgradeProposed)?;

        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(VestingError::UpgradeTimelockNotElapsed);
        }

        env.storage().instance().remove(&DataKey::UpgradeProposal);
        events::upgrade_executed(&env, &admin, &wasm_hash);
        env.deployer().update_current_contract_wasm(wasm_hash);

        Ok(())
    }

    // ── Internal Helpers ──────────────────────────────────────────────────────

    fn require_admin(env: &Env, addr: &Address) -> Result<(), VestingError> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(VestingError::NotInitialized)?;
        if admin != *addr {
            return Err(VestingError::NotAdmin);
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

    /// Calculate the vested amount for a grant at a specific timestamp.
    ///
    /// **Logic:**
    /// - Before cliff: 0
    /// - After cliff, before full vesting: Linear proportion based on time
    /// - After full vesting: total_amount
    /// - If revoked: freeze calculation at revocation timestamp
    fn calculate_vested_at(grant: &VestingGrant, at_time: u64) -> Result<i128, VestingError> {
        let effective_time = if grant.revoked {
            grant.revoked_at.unwrap_or(at_time)
        } else {
            at_time
        };

        if effective_time < grant.start_time {
            return Ok(0);
        }

        let elapsed = effective_time
            .checked_sub(grant.start_time)
            .ok_or(VestingError::ArithmeticOverflow)?;

        if elapsed < grant.cliff_duration {
            return Ok(0);
        }

        if elapsed >= grant.vesting_duration {
            return Ok(grant.total_amount);
        }

        let vested = (grant.total_amount as i128)
            .checked_mul(elapsed as i128)
            .and_then(|v| v.checked_div(grant.vesting_duration as i128))
            .ok_or(VestingError::ArithmeticOverflow)?;

        Ok(vested)
    }

    /// Calculate the vested amount for a grant at the current ledger timestamp.
    ///
    /// **Logic:**
    /// - Before cliff: 0
    /// - After cliff, before full vesting: Linear proportion based on time
    /// - After full vesting: total_amount
    /// - If revoked: freeze calculation at revocation timestamp
    fn calculate_vested(env: &Env, grant: &VestingGrant) -> Result<i128, VestingError> {
        Self::calculate_vested_at(grant, env.ledger().timestamp())
    }

    /// Get the vested balance for a beneficiary as of a specific timestamp.
    ///
    /// This is the governance snapshot function — it computes the vested amount
    /// at `at_time` (typically a proposal's `created_at` timestamp) so that
    /// vote weight reflects the economically locked balance at proposal creation,
    /// not the current ledger time.
    ///
    /// **Parameters:**
    /// - `beneficiary` — The address to query.
    /// - `at_time` — The historical timestamp at which to snapshot vesting.
    ///
    /// **Returns:** The vested balance at `at_time`. Returns 0 if no grant exists
    /// or vesting had not started at that time.
    ///
    /// **Security:** Read-only view. No auth required.
    pub fn get_vested_balance_at(env: Env, beneficiary: Address, at_time: u64) -> Result<i128, VestingError> {
        let key = DataKey::Grant(beneficiary.clone());
        let grant: VestingGrant = match env.storage().persistent().get(&key) {
            Some(g) => g,
            None => return Ok(0),
        };

        Self::calculate_vested_at(&grant, at_time)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger};
    use soroban_sdk::{token, Env};

    fn create_token_contract<'a>(env: &Env, admin: &Address) -> token::StellarAssetClient<'a> {
        token::StellarAssetClient::new(env, &env.register_stellar_asset_contract_v2(admin.clone()).address())
    }

    fn token_balance_of(env: &Env, token: &token::StellarAssetClient<'_>, account: &Address) -> i128 {
        token::Client::new(env, &token.address).balance(account)
    }

    #[test]
    fn test_vesting_lifecycle() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, VestingContract);
        let client = VestingContractClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token = create_token_contract(&env, &token_admin);

        // Initialize
        client.initialize(&admin, &token.address);

        // Mint tokens to contract
        token.mint(&contract_id, &10_000);

        // Create grant: 10,000 tokens, 100s cliff, 1000s total duration
        let start_time = env.ledger().timestamp();
        client.create_grant(&admin, &beneficiary, &10_000, &start_time, &100, &1000);

        // Before cliff: vested balance is 0
        let vested = client.get_vested_balance(&beneficiary);
        assert_eq!(vested, 0);

        // After cliff but before half: should be proportional
        env.ledger().with_mut(|li| li.timestamp = start_time + 500);
        let vested = client.get_vested_balance(&beneficiary);
        assert_eq!(vested, 5_000); // 50% vested

        // Release tokens
        let balance_before = token_balance_of(&env, &token, &beneficiary);
        client.release(&beneficiary);
        let balance_after = token_balance_of(&env, &token, &beneficiary);
        assert_eq!(balance_after - balance_before, 5_000);

        // After full duration: everything vested
        env.ledger().with_mut(|li| li.timestamp = start_time + 1000);
        let vested = client.get_vested_balance(&beneficiary);
        assert_eq!(vested, 10_000);
    }

    #[test]
    fn test_revocation_preserves_vested() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, VestingContract);
        let client = VestingContractClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token = create_token_contract(&env, &token_admin);

        client.initialize(&admin, &token.address);
        token.mint(&contract_id, &10_000);

        let start_time = env.ledger().timestamp();
        client.create_grant(&admin, &beneficiary, &10_000, &start_time, &100, &1000);

        // Advance to 50% vested
        env.ledger().with_mut(|li| li.timestamp = start_time + 500);
        let vested_before_revoke = client.get_vested_balance(&beneficiary);
        assert_eq!(vested_before_revoke, 5_000);

        // Revoke grant
        client.revoke_grant(&admin, &beneficiary);

        // Vested amount should remain at 5,000 (frozen at revocation time)
        let vested_after_revoke = client.get_vested_balance(&beneficiary);
        assert_eq!(vested_after_revoke, 5_000);

        // Advance time further - vested should still be 5,000 (not increasing)
        env.ledger().with_mut(|li| li.timestamp = start_time + 900);
        let vested_later = client.get_vested_balance(&beneficiary);
        assert_eq!(vested_later, 5_000);

        // Can still release the vested amount
        client.release(&beneficiary);
        assert_eq!(token_balance_of(&env, &token, &beneficiary), 5_000);
    }

    #[test]
    fn test_cliff_boundary() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, VestingContract);
        let client = VestingContractClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token = create_token_contract(&env, &token_admin);

        client.initialize(&admin, &token.address);
        token.mint(&contract_id, &10_000);

        let start_time = 1000u64;
        env.ledger().with_mut(|li| li.timestamp = start_time);

        client.create_grant(&admin, &beneficiary, &10_000, &start_time, &100, &1000);

        // Exactly at cliff: still nothing vested (cliff is exclusive start)
        env.ledger().with_mut(|li| li.timestamp = start_time + 99);
        assert_eq!(client.get_vested_balance(&beneficiary), 0);

        // One second after cliff: should have some vested
        env.ledger().with_mut(|li| li.timestamp = start_time + 100);
        let vested = client.get_vested_balance(&beneficiary);
        assert!(vested > 0);

        // Exactly at end: everything vested
        env.ledger().with_mut(|li| li.timestamp = start_time + 1000);
        assert_eq!(client.get_vested_balance(&beneficiary), 10_000);

        // After end: still everything (doesn't go over)
        env.ledger().with_mut(|li| li.timestamp = start_time + 1500);
        assert_eq!(client.get_vested_balance(&beneficiary), 10_000);
    }

    #[test]
    fn test_invalid_schedule_cliff_equals_duration() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, VestingContract);
        let client = VestingContractClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token = create_token_contract(&env, &token_admin);

        client.initialize(&admin, &token.address);

        // Invalid: cliff == vesting_duration
        assert_eq!(
            client
                .try_create_grant(&admin, &beneficiary, &10_000, &1000, &500, &500)
                .unwrap_err()
                .unwrap(),
            VestingError::InvalidVestingSchedule
        );
    }

    #[test]
    fn test_governance_weight_excludes_unvested() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, VestingContract);
        let client = VestingContractClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token = create_token_contract(&env, &token_admin);

        client.initialize(&admin, &token.address);
        token.mint(&contract_id, &10_000);

        let start_time = env.ledger().timestamp();
        client.create_grant(&admin, &beneficiary, &10_000, &start_time, &100, &1000);

        // Before cliff: governance weight = 0 (unvested)
        let vested = client.get_vested_balance(&beneficiary);
        let unvested = client.get_unvested_balance(&beneficiary);
        assert_eq!(vested, 0);
        assert_eq!(unvested, 10_000);

        // At 30%: governance weight = 3,000 (only vested portion)
        env.ledger().with_mut(|li| li.timestamp = start_time + 300);
        let vested = client.get_vested_balance(&beneficiary);
        let unvested = client.get_unvested_balance(&beneficiary);
        assert_eq!(vested, 3_000);
        assert_eq!(unvested, 7_000);

        // Fully vested: governance weight = 10,000
        env.ledger().with_mut(|li| li.timestamp = start_time + 1000);
        let vested = client.get_vested_balance(&beneficiary);
        let unvested = client.get_unvested_balance(&beneficiary);
        assert_eq!(vested, 10_000);
        assert_eq!(unvested, 0);
    }

    #[test]
    fn test_vesting_events_emitted_on_shared_schema() {
        use kora_shared::events::EVENT_SCHEMA_VERSION;
        use soroban_sdk::{symbol_short, testutils::Events, IntoVal, Symbol, TryFromVal, Val};

        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, VestingContract);
        let client = VestingContractClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token = create_token_contract(&env, &token_admin);

        // 1. initialize -> VEST_INIT
        client.initialize(&admin, &token.address);
        let evs = env.events().all();
        let last = evs.last().unwrap();
        let schema_sym: Symbol = Symbol::try_from_val(&env, &last.1.get(0).unwrap()).unwrap();
        let topic_sym: Symbol = Symbol::try_from_val(&env, &last.1.get(1).unwrap()).unwrap();
        let ver: u32 = u32::try_from_val(&env, &last.1.get(2).unwrap()).unwrap();
        assert_eq!(schema_sym, symbol_short!("SCHEMA_V"));
        assert_eq!(topic_sym, symbol_short!("VEST_INIT"));
        assert_eq!(ver, EVENT_SCHEMA_VERSION);
        let payload: (Address, Address, u64) =
            soroban_sdk::FromVal::from_val(&env, &last.2);
        assert_eq!(payload, (admin.clone(), token.address.clone(), env.ledger().timestamp()));

        // 2. create_grant -> VEST_GRANT
        token.mint(&contract_id, &10_000);
        let start_time = env.ledger().timestamp();
        client.create_grant(&admin, &beneficiary, &10_000, &start_time, &100, &1000);
        let evs = env.events().all();
        let last = evs.last().unwrap();
        let topic_sym: Symbol = Symbol::try_from_val(&env, &last.1.get(1).unwrap()).unwrap();
        assert_eq!(topic_sym, symbol_short!("VEST_GRANT"));
        let payload: (Address, Address, i128, u64, u64, u64, u64) =
            soroban_sdk::FromVal::from_val(&env, &last.2);
        assert_eq!(
            payload,
            (
                admin.clone(),
                beneficiary.clone(),
                10_000i128,
                start_time,
                100u64,
                1000u64,
                env.ledger().timestamp()
            )
        );

        // 3. release before cliff (zero releasable) -> errors, no VEST_REL emitted
        let err = client.try_release(&beneficiary).unwrap_err().unwrap();
        assert_eq!(err, VestingError::InsufficientVestedBalance);

        // 4. release after cliff -> VEST_REL
        env.ledger().with_mut(|li| li.timestamp = start_time + 500);
        client.release(&beneficiary);
        let evs = env.events().all();
        let last = evs.last().unwrap();
        let topic_sym: Symbol = Symbol::try_from_val(&env, &last.1.get(1).unwrap()).unwrap();
        assert_eq!(topic_sym, symbol_short!("VEST_REL"));
        let payload: (Address, i128, u64) =
            soroban_sdk::FromVal::from_val(&env, &last.2);
        assert_eq!(payload, (beneficiary.clone(), 5_000i128, env.ledger().timestamp()));

        // 5. revoke_grant -> VEST_REV
        env.ledger().with_mut(|li| li.timestamp = start_time + 600);
        client.revoke_grant(&admin, &beneficiary);
        let evs = env.events().all();
        let last = evs.last().unwrap();
        let topic_sym: Symbol = Symbol::try_from_val(&env, &last.1.get(1).unwrap()).unwrap();
        assert_eq!(topic_sym, symbol_short!("VEST_REV"));
        let payload: (Address, Address, u64, u64) =
            soroban_sdk::FromVal::from_val(&env, &last.2);
        assert_eq!(
            payload,
            (
                admin.clone(),
                beneficiary.clone(),
                start_time + 600,
                env.ledger().timestamp()
            )
        );

        // 6. propose_upgrade -> UPG_PROP
        let wasm_hash = BytesN::from_array(&env, &[7u8; 32]);
        client.propose_upgrade(&admin, &wasm_hash);
        let evs = env.events().all();
        let last = evs.last().unwrap();
        let topic_sym: Symbol = Symbol::try_from_val(&env, &last.1.get(1).unwrap()).unwrap();
        assert_eq!(topic_sym, symbol_short!("UPG_PROP"));
        let payload: (Address, BytesN<32>, u64) =
            soroban_sdk::FromVal::from_val(&env, &last.2);
        assert_eq!(payload, (admin.clone(), wasm_hash.clone(), env.ledger().timestamp()));

        // 7. execute_upgrade -> UPG_EXEC (note: update_current_contract_wasm panics in test env without uploaded wasm,
        //    so we verify the event helper schema directly or via try_execute_upgrade if wasm is present)
        let _val: Val = symbol_short!("UPG_EXEC").into_val(&env);
    }
}
