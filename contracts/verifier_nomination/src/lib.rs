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
    InCoolDown = 13,
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
    CoolDown,
    SlashedAt(Address),
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
        Self::assert_not_in_cool_down(&env, &candidate)?;
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

    pub fn get_stake(env: Env, nomination_id: u64, supporter