#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, token, Address, Env, Vec};

const MAX_NOMINATIONS: u64 = 50;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum NominationError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotAdmin = 3,
    InvalidAmount = 4,
    NotFound = 5,
    Closed = 6,
    StakeCapExceeded = 7,
    NotStale = 8,
    NotNominee = 9,
    NoEligibleCandidate = 10,
    TooManyNominations = 11,
    ArithmeticOverflow = 12,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NominationStatus {
    Open,
    Selected,
    Accepted,
    Declined,
    Expired,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Nomination {
    pub id: u64,
    pub candidate: Address,
    pub total_stake: i128,
    pub created_at: u64,
    pub selected_at: u64,
    pub status: NominationStatus,
    pub supporters: Vec<Address>,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Token,
    MinimumStake,
    PerAddressCap,
    Window,
    NextId,
    Nomination(u64),
    Stake(u64, Address),
}

#[contract]
pub struct VerifierNominationContract;

#[contractimpl]
impl VerifierNominationContract {
    pub fn initialize(
        env: Env,
        admin: Address,
        token_address: Address,
        minimum_stake: i128,
        per_address_cap: i128,
        nomination_window: u64,
    ) -> Result<(), NominationError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(NominationError::AlreadyInitialized);
        }
        if minimum_stake <= 0 || per_address_cap < minimum_stake || nomination_window == 0 {
            return Err(NominationError::InvalidAmount);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token_address);
        env.storage().instance().set(&DataKey::MinimumStake, &minimum_stake);
        env.storage().instance().set(&DataKey::PerAddressCap, &per_address_cap);
        env.storage().instance().set(&DataKey::Window, &nomination_window);
        env.storage().instance().set(&DataKey::NextId, &0u64);
        Ok(())
    }

    pub fn nominate(
        env: Env,
        supporter: Address,
        candidate: Address,
        amount: i128,
    ) -> Result<u64, NominationError> {
        supporter.require_auth();
        let id: u64 = env.storage().instance().get(&DataKey::NextId).ok_or(NominationError::NotInitialized)?;
        if id >= MAX_NOMINATIONS {
            return Err(NominationError::TooManyNominations);
        }
        let next_id = id.checked_add(1).ok_or(NominationError::ArithmeticOverflow)?;
        let nomination = Nomination {
            id,
            candidate,
            total_stake: 0,
            created_at: env.ledger().timestamp(),
            selected_at: 0,
            status: NominationStatus::Open,
            supporters: Vec::new(&env),
        };
        env.storage().persistent().set(&DataKey::Nomination(id), &nomination);
        env.storage().instance().set(&DataKey::NextId, &next_id);
        Self::stake_internal(&env, id, &supporter, amount)?;
        Ok(id)
    }

    pub fn stake(env: Env, supporter: Address, nomination_id: u64, amount: i128) -> Result<(), NominationError> {
        supporter.require_auth();
        Self::stake_internal(&env, nomination_id, &supporter, amount)
    }

    pub fn select_candidate(env: Env) -> Result<u64, NominationError> {
        let count: u64 = env.storage().instance().get(&DataKey::NextId).ok_or(NominationError::NotInitialized)?;
        let minimum_stake: i128 = env.storage().instance().get(&DataKey::MinimumStake).unwrap_or(0);
        let window: u64 = env.storage().instance().get(&DataKey::Window).unwrap_or(0);
        let now = env.ledger().timestamp();
        let mut winner: Option<Nomination> = None;
        for id in 0..count {
            let Some(nomination) = env.storage().persistent().get::<_, Nomination>(&DataKey::Nomination(id)) else {
                continue;
            };
            if nomination.status != NominationStatus::Open
                || nomination.total_stake < minimum_stake
                || now >= nomination.created_at.saturating_add(window)
            {
                continue;
            }
            if winner.as_ref().map_or(true, |current| nomination.total_stake > current.total_stake) {
                winner = Some(nomination);
            }
        }
        let mut selected = winner.ok_or(NominationError::NoEligibleCandidate)?;
        selected.status = NominationStatus::Selected;
        selected.selected_at = now;
        env.storage().persistent().set(&DataKey::Nomination(selected.id), &selected);
        Ok(selected.id)
    }

    pub fn accept_onboarding(env: Env, candidate: Address, nomination_id: u64) -> Result<(), NominationError> {
        candidate.require_auth();
        let mut nomination = Self::load(&env, nomination_id)?;
        if nomination.candidate != candidate {
            return Err(NominationError::NotNominee);
        }
        if nomination.status != NominationStatus::Selected {
            return Err(NominationError::Closed);
        }
        nomination.status = NominationStatus::Accepted;
        env.storage().persistent().set(&DataKey::Nomination(nomination_id), &nomination);
        Ok(())
    }

    pub fn decline_onboarding(env: Env, candidate: Address, nomination_id: u64) -> Result<(), NominationError> {
        candidate.require_auth();
        let mut nomination = Self::load(&env, nomination_id)?;
        if nomination.candidate != candidate {
            return Err(NominationError::NotNominee);
        }
        if nomination.status != NominationStatus::Selected {
            return Err(NominationError::Closed);
        }
        nomination.status = NominationStatus::Declined;
        env.storage().persistent().set(&DataKey::Nomination(nomination_id), &nomination);
        Ok(())
    }

    pub fn withdraw(env: Env, supporter: Address, nomination_id: u64) -> Result<i128, NominationError> {
        supporter.require_auth();
        let nomination = Self::load(&env, nomination_id)?;
        let window: u64 = env.storage().instance().get(&DataKey::Window).unwrap_or(0);
        let stale = nomination.status == NominationStatus::Open
            && env.ledger().timestamp() >= nomination.created_at.saturating_add(window);
        let declined = nomination.status == NominationStatus::Declined;
        let selection_expired = (nomination.status == NominationStatus::Selected
            || nomination.status == NominationStatus::Accepted)
            && env.ledger().timestamp() >= nomination.selected_at.saturating_add(window);
        let already_expired = nomination.status == NominationStatus::Expired;
        if !stale && !declined && !selection_expired && !already_expired {
            return Err(NominationError::NotStale);
        }
        let amount: i128 = env.storage().persistent().get(&DataKey::Stake(nomination_id, supporter.clone())).unwrap_or(0);
        if amount <= 0 {
            return Err(NominationError::InvalidAmount);
        }
        env.storage().persistent().remove(&DataKey::Stake(nomination_id, supporter.clone()));
        let mut updated = nomination;
        updated.total_stake -= amount;
        if stale || selection_expired {
            updated.status = NominationStatus::Expired;
        }
        env.storage().persistent().set(&DataKey::Nomination(nomination_id), &updated);
        let token_address: Address = env.storage().instance().get(&DataKey::Token).ok_or(NominationError::NotInitialized)?;
        token::Client::new(&env, &token_address).transfer(&env.current_contract_address(), &supporter, &amount);
        Ok(amount)
    }

    pub fn get_nomination(env: Env, nomination_id: u64) -> Result<Nomination, NominationError> {
        Self::load(&env, nomination_id)
    }

    pub fn get_stake(env: Env, nomination_id: u64, supporter: Address) -> i128 {
        env.storage().persistent().get(&DataKey::Stake(nomination_id, supporter)).unwrap_or(0)
    }

    pub fn is_eligible(env: Env, nomination_id: u64) -> bool {
        Self::load(&env, nomination_id).map(|n| n.status == NominationStatus::Accepted).unwrap_or(false)
    }

    fn stake_internal(env: &Env, nomination_id: u64, supporter: &Address, amount: i128) -> Result<(), NominationError> {
        if amount <= 0 {
            return Err(NominationError::InvalidAmount);
        }
        let mut nomination = Self::load(env, nomination_id)?;
        if nomination.status != NominationStatus::Open {
            return Err(NominationError::Closed);
        }
        let window: u64 = env.storage().instance().get(&DataKey::Window).unwrap_or(0);
        if env.ledger().timestamp() >= nomination.created_at.saturating_add(window) {
            return Err(NominationError::Closed);
        }
        let cap: i128 = env.storage().instance().get(&DataKey::PerAddressCap).unwrap_or(0);
        let current: i128 = env.storage().persistent().get(&DataKey::Stake(nomination_id, supporter.clone())).unwrap_or(0);
        let new_stake = current.checked_add(amount).ok_or(NominationError::ArithmeticOverflow)?;
        if new_stake > cap {
            return Err(NominationError::StakeCapExceeded);
        }
        nomination.total_stake = nomination.total_stake.checked_add(amount).ok_or(NominationError::ArithmeticOverflow)?;
        if current == 0 {
            nomination.supporters.push_back(supporter.clone());
        }
        let token_address: Address = env.storage().instance().get(&DataKey::Token).ok_or(NominationError::NotInitialized)?;
        token::Client::new(env, &token_address).transfer(supporter, &env.current_contract_address(), &amount);
        env.storage().persistent().set(&DataKey::Stake(nomination_id, supporter.clone()), &new_stake);
        env.storage().persistent().set(&DataKey::Nomination(nomination_id), &nomination);
        Ok(())
    }

    fn load(env: &Env, nomination_id: u64) -> Result<Nomination, NominationError> {
        env.storage().persistent().get(&DataKey::Nomination(nomination_id)).ok_or(NominationError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::{Address as _, Ledger, LedgerInfo}, Env};

    fn setup() -> (Env, Address, VerifierNominationContractClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        env.ledger().set(LedgerInfo {
            timestamp: 100,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });
        let admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
        let contract_id = env.register_contract(None, VerifierNominationContract);
        let client = VerifierNominationContractClient::new(&env, &contract_id);
        client.initialize(&admin, &token, &50, &100, &100);
        (env, token, client, admin)
    }

    fn fund(env: &Env, token: &Address, address: &Address) {
        token::StellarAssetClient::new(env, token).mint(address, &500);
    }

    #[test]
    fn enforces_per_address_cap_for_each_nomination() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);
        let id = client.nominate(&supporter, &candidate, &60);
        assert!(client.try_stake(&supporter, &id, &41).is_err());
        client.stake(&supporter, &id, &40);
        assert_eq!(client.get_stake(&id, &supporter), 100);
    }

    #[test]
    fn permits_withdrawal_when_nomination_window_is_stale() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);
        let id = client.nominate(&supporter, &candidate, &50);
        env.ledger().set(LedgerInfo {
            timestamp: 201,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });
        assert_eq!(client.withdraw(&supporter, &id), 50);
        assert_eq!(token::Client::new(&env, &token).balance(&supporter), 500);
    }

    #[test]
    fn declined_candidate_unlocks_support_and_accepted_candidate_does_not() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);
        let id = client.nominate(&supporter, &candidate, &50);
        client.select_candidate();
        client.accept_onboarding(&candidate, &id);
        assert!(client.try_withdraw(&supporter, &id).is_err());
        // A declined selection releases support immediately.
        let other_candidate = Address::generate(&env);
        let other_id = client.nominate(&supporter, &other_candidate, &50);
        // The first nomination was accepted, so the second is selected from the remaining open set.
        client.select_candidate();
        client.decline_onboarding(&other_candidate, &other_id);
        assert_eq!(client.withdraw(&supporter, &other_id), 50);
    }

    // ── Slash / stake math tests ───────────────────────────────────────────────
    //
    // The nomination contract does not have an on-chain "slash" entrypoint but
    // the stake accounting must be exact.  These tests verify:
    //   • total_stake equals sum of individual supporter stakes
    //   • per-address cap is enforced per nomination (not globally)
    //   • multiple supporters contribute independently
    //   • withdraw returns exactly what was staked, no more

    /// Total stake equals the sum of individual supporter contributions.
    #[test]
    fn stake_math_total_equals_sum_of_contributors() {
        let (env, token, client, _) = setup();
        let s1 = Address::generate(&env);
        let s2 = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &s1);
        fund(&env, &token, &s2);

        let id = client.nominate(&s1, &candidate, &30);
        client.stake(&s2, &id, &40);

        let nom = client.get_nomination(&id);
        assert_eq!(
            nom.total_stake, 70,
            "total_stake must equal 30 + 40 = 70"
        );
        assert_eq!(client.get_stake(&id, &s1), 30);
        assert_eq!(client.get_stake(&id, &s2), 40);
    }

    /// Per-address cap is enforced independently per nomination — a supporter
    /// hitting the cap on one nomination can still stake the full cap on another.
    #[test]
    fn stake_cap_independent_per_nomination() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let c1 = Address::generate(&env);
        let c2 = Address::generate(&env);
        fund(&env, &token, &supporter);

        let id1 = client.nominate(&supporter, &c1, &100); // cap = 100, saturated
        // Trying to add 1 more to id1 must fail
        assert_eq!(
            client.try_stake(&supporter, &id1, &1).unwrap_err().unwrap(),
            NominationError::StakeCapExceeded,
            "exceeding per-address cap must return StakeCapExceeded"
        );

        // But the same supporter can contribute the full cap to a different nomination
        let id2 = client.nominate(&supporter, &c2, &100);
        assert_eq!(
            client.get_stake(&id2, &supporter), 100,
            "per-address cap is independent across nominations"
        );
    }

    /// Withdraw returns exactly the staked amount — not more.
    #[test]
    fn withdraw_returns_exact_staked_amount() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let pre_balance = token::Client::new(&env, &token).balance(&supporter);
        let id = client.nominate(&supporter, &candidate, &75);

        // Advance past window to unlock withdrawal
        env.ledger().set(LedgerInfo {
            timestamp: 201,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        let returned = client.withdraw(&supporter, &id);
        assert_eq!(returned, 75, "withdraw must return exactly the staked 75");
        let post_balance = token::Client::new(&env, &token).balance(&supporter);
        assert_eq!(
            post_balance, pre_balance,
            "supporter balance must be fully restored after withdrawal"
        );
    }

    /// Two supporters can both withdraw their independent shares after the
    /// nomination window expires, and each gets back exactly what they put in.
    #[test]
    fn multiple_supporters_withdraw_independently_after_window() {
        let (env, token, client, _) = setup();
        let s1 = Address::generate(&env);
        let s2 = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &s1);
        fund(&env, &token, &s2);

        let id = client.nominate(&s1, &candidate, &30);
        client.stake(&s2, &id, &40);

        // Advance past window
        env.ledger().set(LedgerInfo {
            timestamp: 201,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        assert_eq!(client.withdraw(&s1, &id), 30);
        assert_eq!(client.withdraw(&s2, &id), 40);

        // No funds should remain staked
        assert_eq!(client.get_stake(&id, &s1), 0);
        assert_eq!(client.get_stake(&id, &s2), 0);
    }

    // ── Early withdraw rejection tests ────────────────────────────────────────

    /// Withdrawal inside an open, unexpired nomination window must fail.
    #[test]
    fn early_withdraw_inside_window_rejected() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let id = client.nominate(&supporter, &candidate, &50);
        // Window is 100s; we are at timestamp 100 (window start) — still inside
        let result = client.try_withdraw(&supporter, &id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            NominationError::NotStale,
            "withdrawal inside window must return typed NotStale"
        );
    }

    /// Withdrawal just before the window boundary is rejected.
    #[test]
    fn early_withdraw_one_second_before_boundary_rejected() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let id = client.nominate(&supporter, &candidate, &50);
        // Nomination created at t=100, window=100 → expires at t=200
        // Advance to t=199 (1 second before expiry)
        env.ledger().set(LedgerInfo {
            timestamp: 199,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        let result = client.try_withdraw(&supporter, &id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            NominationError::NotStale,
            "withdrawal one second before window expiry must be rejected"
        );
    }

    /// Withdrawal at exactly the window boundary (created_at + window) is accepted.
    #[test]
    fn withdraw_at_exact_window_boundary_accepted() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let id = client.nominate(&supporter, &candidate, &50);
        // created_at=100, window=100 → boundary at t=200
        env.ledger().set(LedgerInfo {
            timestamp: 200,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        let result = client.try_withdraw(&supporter, &id);
        assert!(
            result.is_ok(),
            "withdrawal at exactly the window boundary must succeed (nomination is stale)"
        );
        assert_eq!(result.unwrap(), 50);
    }

    /// Withdrawal of a declined nomination is immediately available, before window expiry.
    #[test]
    fn withdraw_after_decline_available_immediately() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let id = client.nominate(&supporter, &candidate, &50);
        client.select_candidate();
        client.decline_onboarding(&candidate, &id);

        // Window has NOT expired (still at t=100); withdrawal is allowed because declined
        let result = client.try_withdraw(&supporter, &id);
        assert!(
            result.is_ok(),
            "withdrawal after decline must succeed even inside the window"
        );
        assert_eq!(result.unwrap(), 50);
    }

    // ── Double nomination / churn tests ───────────────────────────────────────

    /// A supporter can back multiple distinct candidates with separate nominations.
    /// The per-address cap is applied per nomination, not globally.
    #[test]
    fn double_nomination_different_candidates_allowed() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let c1 = Address::generate(&env);
        let c2 = Address::generate(&env);
        fund(&env, &token, &supporter);
        // Each cap is 100; total stake = 200 is within token balance (500)
        let id1 = client.nominate(&supporter, &c1, &50);
        let id2 = client.nominate(&supporter, &c2, &50);

        assert_ne!(id1, id2, "each nomination must have a distinct id");
        assert_eq!(client.get_stake(&id1, &supporter), 50);
        assert_eq!(client.get_stake(&id2, &supporter), 50);
    }

    /// The nomination cap bounds the total number of nominations to MAX_NOMINATIONS (50).
    #[test]
    fn nomination_id_counter_advances_monotonically() {
        let (env, token, client, _) = setup();
        let prev_supporter = Address::generate(&env);
        fund(&env, &token, &prev_supporter);

        // Create a few nominations and verify ids are monotonically increasing
        let c1 = Address::generate(&env);
        let c2 = Address::generate(&env);
        let id1 = client.nominate(&prev_supporter, &c1, &50);
        let id2 = client.nominate(&prev_supporter, &c2, &50);

        assert!(id2 > id1, "nomination ids must increase monotonically");
    }

    // ── Zero-stake nominee edge case ──────────────────────────────────────────

    /// Nominating with zero stake must be rejected with a typed error.
    #[test]
    fn nominate_zero_stake_rejected_typed_error() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let result = client.try_nominate(&supporter, &candidate, &0i128);
        assert_eq!(
            result.unwrap_err().unwrap(),
            NominationError::InvalidAmount,
            "zero-stake nomination must be rejected with typed InvalidAmount"
        );
    }

    /// Staking zero to an existing nomination must also be rejected.
    #[test]
    fn stake_zero_to_existing_nomination_rejected() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let id = client.nominate(&supporter, &candidate, &50);
        let result = client.try_stake(&supporter, &id, &0i128);
        assert_eq!(
            result.unwrap_err().unwrap(),
            NominationError::InvalidAmount,
            "zero-stake call to stake() must be rejected"
        );
    }

    /// Nominating with a negative stake must be rejected.
    #[test]
    fn nominate_negative_stake_rejected() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let result = client.try_nominate(&supporter, &candidate, &-1i128);
        assert_eq!(
            result.unwrap_err().unwrap(),
            NominationError::InvalidAmount,
            "negative-stake nomination must be rejected"
        );
    }

    /// A supporter with zero stake cannot withdraw (nothing to return).
    #[test]
    fn withdraw_zero_stake_returns_invalid_amount() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let non_supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        let id = client.nominate(&supporter, &candidate, &50);

        // Advance past window so the stale condition is met
        env.ledger().set(LedgerInfo {
            timestamp: 201,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        // non_supporter never staked — has 0 stake
        let result = client.try_withdraw(&non_supporter, &id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            NominationError::InvalidAmount,
            "withdrawal of zero stake must return typed InvalidAmount"
        );
    }

    /// select_candidate with no eligible nominations returns a typed error.
    #[test]
    fn select_candidate_no_eligible_returns_typed_error() {
        let (env, _token, client, _) = setup();
        // No nominations created — selection must fail cleanly
        let result = client.try_select_candidate();
        assert_eq!(
            result.unwrap_err().unwrap(),
            NominationError::NoEligibleCandidate,
            "select_candidate with no nominations must return typed NoEligibleCandidate"
        );
    }

    /// Nomination that does not meet minimum stake is not selected.
    #[test]
    fn nomination_below_minimum_stake_not_selectable() {
        let (env, token, client, _) = setup();
        let supporter = Address::generate(&env);
        let candidate = Address::generate(&env);
        fund(&env, &token, &supporter);

        // minimum_stake = 50; stake only 30 < 50
        client.nominate(&supporter, &candidate, &30);
        let result = client.try_select_candidate();
        assert_eq!(
            result.unwrap_err().unwrap(),
            NominationError::NoEligibleCandidate,
            "nomination below minimum stake must not be selectable"
        );
    }
}