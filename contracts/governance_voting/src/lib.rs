#![no_std]

use kora_shared::errors::CommonError;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, vec, Address, Env, IntoVal, String, Symbol,
    Val, Vec,
};

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum GovernanceError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotAdmin = 3,
    InvalidAddress = 4,
    ProposalNotFound = 5,
    ProposalExpired = 6,
    ProposalAlreadyExecuted = 7,
    AlreadyVoted = 8,
    InsufficientVotingWeight = 9,
    InvalidProposalType = 10,
    InvalidVotingMode = 11,
    CircularDelegation = 12,
    ArithmeticOverflow = 13,
    InvalidVoiceCredits = 14,
    QuadraticVotingNotAllowed = 15,
    DelegationNotFound = 16,
    Reentrancy = 17,
    VestingNotConfigured = 18,
    VestingContractError = 19,
}

impl From<CommonError> for GovernanceError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAddress => GovernanceError::InvalidAddress,
            CommonError::ArithmeticOverflow => GovernanceError::ArithmeticOverflow,
            CommonError::Reentrancy => GovernanceError::Reentrancy,
            _ => GovernanceError::InvalidAddress,
        }
    }
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default proposal duration: ~7 days in seconds
const DEFAULT_PROPOSAL_DURATION: u64 = 604_800;

/// Storage TTL constants (~31 days in ledgers)
const PERSISTENT_BUMP_AMOUNT: u32 = 535_680;
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 535_680 / 2;

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Admin address
    Admin,
    /// Access control contract reference
    AccessControl,
    /// Next proposal ID counter
    NextProposalId,
    /// Proposal data by ID
    Proposal(u64),
    /// Vote record: (proposal_id, voter) -> VoteRecord
    VoteRecord(u64, Address),
    /// Delegation mapping: delegator -> delegate
    Delegation(Address),
    /// Reverse delegation index: delegate -> Vec<delegator>
    DelegatedBy(Address),
    /// Voting weight/stake for an address
    VotingWeight(Address),
    /// Vesting contract address for vested-balance weight computation
    Vesting,
    /// Reentrancy guard
    ReentrancyLock,
}

// ── Data Structures ───────────────────────────────────────────────────────────

/// Voting mode for a proposal
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VotingMode {
    /// Standard stake-weighted voting
    Standard,
    /// Quadratic voting (for community sentiment proposals)
    Quadratic,
}

/// Proposal type/category
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProposalType {
    /// Financial decisions (treasury allocation, grants)
    Financial,
    /// Community sentiment (protocol direction, non-financial)
    Sentiment,
    /// Protocol upgrade
    Upgrade,
    /// Parameter change
    ParameterChange,
}

/// Vote choice
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VoteChoice {
    For,
    Against,
    Abstain,
}

/// A governance proposal
#[contracttype]
#[derive(Clone, Debug)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address,
    pub title: String,
    pub description: String,
    pub proposal_type: ProposalType,
    pub voting_mode: VotingMode,
    pub votes_for: i128,
    pub votes_against: i128,
    pub votes_abstain: i128,
    pub created_at: u64,
    pub expires_at: u64,
    pub executed: bool,
    pub quorum_required: i128,
}

/// Individual vote record
#[contracttype]
#[derive(Clone, Debug)]
pub struct VoteRecord {
    pub voter: Address,
    pub choice: VoteChoice,
    pub weight: i128,
    pub voice_credits_spent: i128, // For quadratic voting
    pub voted_at: u64,
}

/// Delegation record
#[contracttype]
#[derive(Clone, Debug)]
pub struct DelegationRecord {
    pub delegator: Address,
    pub delegate: Address,
    pub delegated_at: u64,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct GovernanceVotingContract;

#[contractimpl]
impl GovernanceVotingContract {
    /// Initialize the governance contract
    ///
    /// **Parameters:**
    /// - `admin` — The admin address
    /// - `access_control` — The access control contract address
    ///
    /// **Errors:**
    /// - `GovernanceError::AlreadyInitialized` — Contract already initialized
    /// - `GovernanceError::InvalidAddress` — Invalid admin or access control address
    pub fn initialize(
        env: Env,
        admin: Address,
        access_control: Address,
    ) -> Result<(), GovernanceError> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(GovernanceError::AlreadyInitialized);
        }

        kora_shared::validation::require_not_self(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &access_control)?;

        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::AccessControl, &access_control);
        env.storage().persistent().set(&DataKey::NextProposalId, &0u64);

        Self::bump_persistent(&env, &DataKey::Admin);
        Self::bump_persistent(&env, &DataKey::AccessControl);
        Self::bump_persistent(&env, &DataKey::NextProposalId);

        Ok(())
    }

    /// Set voting weight for an address (admin only)
    ///
    /// **Parameters:**
    /// - `admin` — Admin address
    /// - `address` — Address to set weight for
    /// - `weight` — Voting weight (stake amount)
    ///
    /// **Security:** Requires `admin.require_auth()`
    pub fn set_voting_weight(
        env: Env,
        admin: Address,
        address: Address,
        weight: i128,
    ) -> Result<(), GovernanceError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if weight < 0 {
            return Err(GovernanceError::InsufficientVotingWeight);
        }

        let key = DataKey::VotingWeight(address.clone());
        env.storage().persistent().set(&key, &weight);
        Self::bump_persistent(&env, &key);

        Ok(())
    }

    /// Set the vesting contract address (admin only).
    ///
    /// Once set, vote weight is computed from the beneficiary's vested balance
    /// at the proposal's `created_at` snapshot timestamp, instead of using the
    /// manually set `VotingWeight` storage.
    ///
    /// **Parameters:**
    /// - `admin` — Admin address
    /// - `vesting_address` — The vesting contract address
    ///
    /// **Security:** Requires `admin.require_auth()`
    pub fn set_vesting_contract(
        env: Env,
        admin: Address,
        vesting_address: Address,
    ) -> Result<(), GovernanceError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        kora_shared::validation::require_not_self(&env, &vesting_address)?;

        env.storage()
            .persistent()
            .set(&DataKey::Vesting, &vesting_address);
        Self::bump_persistent(&env, &DataKey::Vesting);

        Ok(())
    }

    /// Get the vesting contract address, or None if not configured.
    pub fn get_vesting_contract(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Vesting)
    }

    /// Get voting weight for an address (includes delegated weight if the address is a delegate).
    ///
    /// When a vesting contract is configured, the base weight is the beneficiary's
    /// vested balance as of the current ledger timestamp. Otherwise it falls back
    /// to the manually set `VotingWeight` storage.
    ///
    /// **Returns:** Total voting weight (own weight + delegated weight)
    pub fn get_voting_weight(env: Env, address: Address) -> i128 {
        // Get base weight — from vesting contract if configured, otherwise from storage
        let base_weight = Self::get_base_weight(&env, &address);

        // Get delegated weight if this address is a delegate
        let delegated_weight = Self::get_delegated_weight(&env, &address);

        base_weight.saturating_add(delegated_weight)
    }

    /// Delegate voting weight to another address
    ///
    /// **Parameters:**
    /// - `delegator` — Address delegating their vote
    /// - `delegate` — Address receiving the delegation
    ///
    /// **Rules:**
    /// - Single-level delegation only (no transitive chains)
    /// - Delegator retains ownership of underlying stake
    /// - Can be revoked at any time
    /// - If delegator votes directly, their vote takes precedence
    ///
    /// **Security:** Requires `delegator.require_auth()`
    pub fn delegate(
        env: Env,
        delegator: Address,
        delegate: Address,
    ) -> Result<(), GovernanceError> {
        delegator.require_auth();

        // Prevent self-delegation
        if delegator == delegate {
            return Err(GovernanceError::InvalidAddress);
        }

        // Prevent circular delegation: check if delegate has delegated to delegator
        if let Some(delegate_target) = env
            .storage()
            .persistent()
            .get::<_, Address>(&DataKey::Delegation(delegate.clone()))
        {
            if delegate_target == delegator {
                return Err(GovernanceError::CircularDelegation);
            }
        }

        // Remove from previous delegate's index if exists
        if let Some(old_delegate) = env
            .storage()
            .persistent()
            .get::<_, Address>(&DataKey::Delegation(delegator.clone()))
        {
            Self::remove_from_delegated_by_index(&env, &old_delegate, &delegator);
        }

        // Set new delegation
        let delegation_key = DataKey::Delegation(delegator.clone());
        env.storage()
            .persistent()
            .set(&delegation_key, &delegate);
        Self::bump_persistent(&env, &delegation_key);

        // Update reverse index
        Self::add_to_delegated_by_index(&env, &delegate, &delegator);

        Ok(())
    }

    /// Revoke delegation
    ///
    /// **Parameters:**
    /// - `delegator` — Address revoking their delegation
    ///
    /// **Security:** Requires `delegator.require_auth()`
    pub fn revoke_delegation(env: Env, delegator: Address) -> Result<(), GovernanceError> {
        delegator.require_auth();

        let delegation_key = DataKey::Delegation(delegator.clone());
        
        let delegate: Address = env
            .storage()
            .persistent()
            .get(&delegation_key)
            .ok_or(GovernanceError::DelegationNotFound)?;

        // Remove from reverse index
        Self::remove_from_delegated_by_index(&env, &delegate, &delegator);

        // Remove delegation
        env.storage().persistent().remove(&delegation_key);

        Ok(())
    }

    /// Get the delegate for an address
    ///
    /// **Returns:** The delegate address, or None if not delegated
    pub fn get_delegate(env: Env, delegator: Address) -> Option<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::Delegation(delegator))
    }

    /// Create a new proposal
    ///
    /// **Parameters:**
    /// - `proposer` — Address creating the proposal
    /// - `title` — Proposal title
    /// - `description` — Proposal description
    /// - `proposal_type` — Type of proposal
    /// - `voting_mode` — Voting mode (Standard or Quadratic)
    /// - `quorum_required` — Minimum votes needed for validity
    /// - `duration` — Duration in seconds (0 = use default)
    ///
    /// **Rules:**
    /// - Quadratic voting only allowed for Sentiment proposals
    /// - Financial proposals must use Standard voting
    ///
    /// **Security:** Requires `proposer.require_auth()`
    pub fn create_proposal(
        env: Env,
        proposer: Address,
        title: String,
        description: String,
        proposal_type: ProposalType,
        voting_mode: VotingMode,
        quorum_required: i128,
        duration: u64,
    ) -> Result<u64, GovernanceError> {
        proposer.require_auth();

        // Validate voting mode for proposal type
        match (&proposal_type, &voting_mode) {
            (ProposalType::Financial, VotingMode::Quadratic) => {
                return Err(GovernanceError::QuadraticVotingNotAllowed);
            }
            _ => {}
        }

        let proposal_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::NextProposalId)
            .unwrap_or(0);

        let created_at = env.ledger().timestamp();
        let proposal_duration = if duration > 0 {
            duration
        } else {
            DEFAULT_PROPOSAL_DURATION
        };
        let expires_at = created_at
            .checked_add(proposal_duration)
            .ok_or(GovernanceError::ArithmeticOverflow)?;

        let proposal = Proposal {
            id: proposal_id,
            proposer: proposer.clone(),
            title,
            description,
            proposal_type,
            voting_mode,
            votes_for: 0,
            votes_against: 0,
            votes_abstain: 0,
            created_at,
            expires_at,
            executed: false,
            quorum_required,
        };

        let proposal_key = DataKey::Proposal(proposal_id);
        env.storage().persistent().set(&proposal_key, &proposal);
        Self::bump_persistent(&env, &proposal_key);

        let next_id = proposal_id
            .checked_add(1)
            .ok_or(GovernanceError::ArithmeticOverflow)?;
        env.storage()
            .persistent()
            .set(&DataKey::NextProposalId, &next_id);
        Self::bump_persistent(&env, &DataKey::NextProposalId);

        Ok(proposal_id)
    }

    /// Cast a vote on a proposal
    ///
    /// **Parameters:**
    /// - `voter` — Address casting the vote
    /// - `proposal_id` — ID of the proposal
    /// - `choice` — Vote choice (For, Against, Abstain)
    /// - `voice_credits` — For quadratic voting: number of voice credits to spend
    ///
    /// **Rules:**
    /// - If voter has delegated, cannot vote directly (delegation takes precedence unless revoked)
    /// - Actually: Direct vote takes precedence over delegation
    /// - For quadratic voting: actual votes = sqrt(voice_credits_spent)
    /// - For standard voting: uses full voting weight
    ///
    /// **Security:** Requires `voter.require_auth()`
    pub fn vote(
        env: Env,
        voter: Address,
        proposal_id: u64,
        choice: VoteChoice,
        voice_credits: i128,
    ) -> Result<(), GovernanceError> {
        voter.require_auth();

        // Get proposal
        let proposal_key = DataKey::Proposal(proposal_id);
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .ok_or(GovernanceError::ProposalNotFound)?;

        // Check if expired
        if env.ledger().timestamp() > proposal.expires_at {
            return Err(GovernanceError::ProposalExpired);
        }

        // Check if already executed
        if proposal.executed {
            return Err(GovernanceError::ProposalAlreadyExecuted);
        }

        // Check if already voted
        let vote_record_key = DataKey::VoteRecord(proposal_id, voter.clone());
        if env.storage().persistent().has(&vote_record_key) {
            return Err(GovernanceError::AlreadyVoted);
        }

        // Get voter's base voting weight — from vesting contract if configured
        let base_weight = Self::get_base_weight_at_snapshot(&env, &voter, proposal.created_at)?;

        if base_weight <= 0 {
            return Err(GovernanceError::InsufficientVotingWeight);
        }

        // Calculate effective vote weight based on voting mode
        let (effective_weight, credits_spent) = match proposal.voting_mode {
            VotingMode::Standard => {
                // Standard: use full weight, ignore voice_credits parameter
                (base_weight, 0)
            }
            VotingMode::Quadratic => {
                // Quadratic: votes = sqrt(voice_credits_spent)
                if voice_credits <= 0 {
                    return Err(GovernanceError::InvalidVoiceCredits);
                }
                if voice_credits > base_weight {
                    return Err(GovernanceError::InvalidVoiceCredits);
                }
                
                let votes = Self::integer_sqrt(voice_credits);
                (votes, voice_credits)
            }
        };

        // Record the vote
        let vote_record = VoteRecord {
            voter: voter.clone(),
            choice: choice.clone(),
            weight: effective_weight,
            voice_credits_spent: credits_spent,
            voted_at: env.ledger().timestamp(),
        };

        env.storage()
            .persistent()
            .set(&vote_record_key, &vote_record);
        Self::bump_persistent(&env, &vote_record_key);

        // Update proposal tallies
        match choice {
            VoteChoice::For => {
                proposal.votes_for = proposal
                    .votes_for
                    .checked_add(effective_weight)
                    .ok_or(GovernanceError::ArithmeticOverflow)?;
            }
            VoteChoice::Against => {
                proposal.votes_against = proposal
                    .votes_against
                    .checked_add(effective_weight)
                    .ok_or(GovernanceError::ArithmeticOverflow)?;
            }
            VoteChoice::Abstain => {
                proposal.votes_abstain = proposal
                    .votes_abstain
                    .checked_add(effective_weight)
                    .ok_or(GovernanceError::ArithmeticOverflow)?;
            }
        }

        env.storage().persistent().set(&proposal_key, &proposal);
        Self::bump_persistent(&env, &proposal_key);

        // ── Emit vote event with weight basis ──────────────────────────────────
        //
        // Topics:
        //   "voteCast"  — event discriminator
        //   "SCHEMA_V"  — schema version (stable across all events)
        //
        // Fields (tuple):
        //   [0] proposal_id  (u64)
        //   [1] voter        (Address)
        //   [2] weight       (i128) — the effective weight used
        //   [3] voice_credits (i128)
        //   [4] choice       (u32 — 0=For,1=Against,2=Abstain)
        //   [5] snapshot_at  (u64) — proposal.created_at snapshot timestamp
        //   [6] schema_v     (u32)
        {
            let topics: Vec<Symbol> = vec![
                &env,
                Symbol::new(&env, "voteCast"),
                Symbol::new(&env, "SCHEMA_V"),
            ];
            let data: Vec<Val> = vec![
                &env,
                proposal_id.into_val(&env),
                voter.into_val(&env),
                effective_weight.into_val(&env),
                credits_spent.into_val(&env),
                (match choice {
                    VoteChoice::For => 0u32,
                    VoteChoice::Against => 1u32,
                    VoteChoice::Abstain => 2u32,
                }).into_val(&env),
                proposal.created_at.into_val(&env),
                kora_shared::events::EVENT_SCHEMA_VERSION.into_val(&env),
            ];
            env.events().publish(topics, data);
        }

        Ok(())
    }

    /// Get proposal details
    ///
    /// **Returns:** The proposal, or None if not found
    pub fn get_proposal(env: Env, proposal_id: u64) -> Option<Proposal> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
    }

    /// Get vote record for a voter on a proposal
    ///
    /// **Returns:** The vote record, or None if not voted
    pub fn get_vote_record(
        env: Env,
        proposal_id: u64,
        voter: Address,
    ) -> Option<VoteRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::VoteRecord(proposal_id, voter))
    }

    /// Execute a proposal (admin only)
    ///
    /// **Parameters:**
    /// - `admin` — Admin address
    /// - `proposal_id` — ID of the proposal to execute
    ///
    /// **Rules:**
    /// - Proposal must have reached quorum
    /// - Proposal must not be expired
    /// - Proposal must not already be executed
    ///
    /// **Security:** Requires `admin.require_auth()`
    pub fn execute_proposal(
        env: Env,
        admin: Address,
        proposal_id: u64,
    ) -> Result<(), GovernanceError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let proposal_key = DataKey::Proposal(proposal_id);
        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&proposal_key)
            .ok_or(GovernanceError::ProposalNotFound)?;

        if proposal.executed {
            return Err(GovernanceError::ProposalAlreadyExecuted);
        }

        if env.ledger().timestamp() > proposal.expires_at {
            return Err(GovernanceError::ProposalExpired);
        }

        // Check if quorum reached
        let total_votes = proposal
            .votes_for
            .checked_add(proposal.votes_against)
            .and_then(|sum| sum.checked_add(proposal.votes_abstain))
            .ok_or(GovernanceError::ArithmeticOverflow)?;

        if total_votes < proposal.quorum_required {
            return Err(GovernanceError::InsufficientVotingWeight);
        }

        proposal.executed = true;
        env.storage().persistent().set(&proposal_key, &proposal);
        Self::bump_persistent(&env, &proposal_key);

        Ok(())
    }

    // ── Internal Helpers ──────────────────────────────────────────────────────

    /// Get the base (own) weight for an address without delegated weight.
    ///
    /// When a vesting contract is configured, the base weight is the beneficiary's
    /// vested balance at the current ledger timestamp. Otherwise it reads from the
    /// manually set `VotingWeight` storage.
    fn get_base_weight(env: &Env, address: &Address) -> i128 {
        let vesting_address: Option<Address> = env.storage().persistent().get(&DataKey::Vesting);

        if let Some(vesting_addr) = vesting_address {
            // Read current vested balance from vesting contract
            let now = env.ledger().timestamp();
            let result: i128 = env.invoke_contract(
                &vesting_addr,
                &Symbol::new(env, "get_vested_balance_at"),
                vec![
                    env,
                    address.clone().into_val(env),
                    now.into_val(env),
                ],
            );
            result
        } else {
            // Fall back to manually set weight
            env.storage()
                .persistent()
                .get(&DataKey::VotingWeight(address.clone()))
                .unwrap_or(0)
        }
    }

    /// Get the base weight at a specific snapshot timestamp (from vesting if
    /// configured, otherwise falls back to the manual `VotingWeight` storage).
    ///
    /// When a vesting contract is configured, calls `get_vested_balance_at` to
    /// compute the beneficiary's vested balance at the given snapshot time.
    /// Otherwise reads the manually set weight (backward-compatible fallback).
    fn get_base_weight_at_snapshot(
        env: &Env,
        address: &Address,
        snapshot_at: u64,
    ) -> Result<i128, GovernanceError> {
        let vesting_address: Option<Address> = env.storage().persistent().get(&DataKey::Vesting);

        if let Some(vesting_addr) = vesting_address {
            let result: i128 = env.invoke_contract(
                &vesting_addr,
                &Symbol::new(env, "get_vested_balance_at"),
                vec![
                    env,
                    address.clone().into_val(env),
                    snapshot_at.into_val(env),
                ],
            );
            Ok(result)
        } else {
            // Fall back to manually set weight (backward compatible)
            let weight: i128 = env.storage()
                .persistent()
                .get(&DataKey::VotingWeight(address.clone()))
                .unwrap_or(0);
            Ok(weight)
        }
    }

    fn require_admin(env: &Env, admin: &Address) -> Result<(), GovernanceError> {
        let stored_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(GovernanceError::NotInitialized)?;

        if stored_admin != *admin {
            return Err(GovernanceError::NotAdmin);
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

    fn get_delegated_weight(env: &Env, delegate: &Address) -> i128 {
        let delegators: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::DelegatedBy(delegate.clone()))
            .unwrap_or(Vec::new(env));

        let mut total_weight: i128 = 0;

        for delegator in delegators.iter() {
            let weight = Self::get_base_weight(env, &delegator);
            total_weight = total_weight.saturating_add(weight);
        }

        total_weight
    }

    fn add_to_delegated_by_index(env: &Env, delegate: &Address, delegator: &Address) {
        let key = DataKey::DelegatedBy(delegate.clone());
        let mut delegators: Vec<Address> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(env));

        // Check if already in list
        let mut found = false;
        for existing in delegators.iter() {
            if existing == *delegator {
                found = true;
                break;
            }
        }

        if !found {
            delegators.push_back(delegator.clone());
            env.storage().persistent().set(&key, &delegators);
            Self::bump_persistent(env, &key);
        }
    }

    fn remove_from_delegated_by_index(env: &Env, delegate: &Address, delegator: &Address) {
        let key = DataKey::DelegatedBy(delegate.clone());
        let delegators: Vec<Address> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(env));

        let mut new_delegators = Vec::new(env);
        for existing in delegators.iter() {
            if existing != *delegator {
                new_delegators.push_back(existing);
            }
        }

        if new_delegators.len() > 0 {
            env.storage().persistent().set(&key, &new_delegators);
            Self::bump_persistent(env, &key);
        } else {
            env.storage().persistent().remove(&key);
        }
    }

    /// Integer square root using binary search (for quadratic voting)
    ///
    /// Returns floor(sqrt(n))
    fn integer_sqrt(n: i128) -> i128 {
        if n < 0 {
            return 0;
        }
        if n == 0 {
            return 0;
        }
        if n == 1 {
            return 1;
        }

        // Binary search for the square root
        let mut low: i128 = 1;
        let mut high: i128 = n;
        let mut result: i128 = 1;

        while low <= high {
            let mid = low + (high - low) / 2;
            let square = mid.saturating_mul(mid);

            if square == n {
                return mid;
            } else if square < n {
                low = mid + 1;
                result = mid;
            } else {
                high = mid - 1;
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    #[test]
    fn test_initialize() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);

        client.initialize(&admin, &access_control);

        // Should fail on second initialization
        let result = client.try_initialize(&admin, &access_control);
        assert!(result.is_err());
    }

    #[test]
    fn test_delegation() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let delegator = Address::generate(&env);
        let delegate = Address::generate(&env);

        client.initialize(&admin, &access_control);

        // Set voting weight
        client.set_voting_weight(&admin, &delegator, &100);

        // Delegate votes
        client.delegate(&delegator, &delegate);

        // Check delegation
        let result = client.get_delegate(&delegator);
        assert_eq!(result, Some(delegate.clone()));

        // Check delegate's weight includes delegated amount
        let delegate_weight = client.get_voting_weight(&delegate);
        assert_eq!(delegate_weight, 100);
    }

    #[test]
    fn test_revoke_delegation() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let delegator = Address::generate(&env);
        let delegate = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &delegator, &100);

        // Delegate and then revoke
        client.delegate(&delegator, &delegate);
        client.revoke_delegation(&delegator);

        // Check delegation removed
        let result = client.get_delegate(&delegator);
        assert_eq!(result, None);

        // Check delegate's weight back to zero
        let delegate_weight = client.get_voting_weight(&delegate);
        assert_eq!(delegate_weight, 0);
    }

    #[test]
    fn test_circular_delegation_prevented() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let addr_a = Address::generate(&env);
        let addr_b = Address::generate(&env);

        client.initialize(&admin, &access_control);

        // A delegates to B
        client.delegate(&addr_a, &addr_b);

        // B tries to delegate to A - should fail
        let result = client.try_delegate(&addr_b, &addr_a);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_proposal_standard_voting() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);

        client.initialize(&admin, &access_control);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test Proposal"),
            &String::from_str(&env, "Description"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &1000,
            &0,
        );

        assert_eq!(proposal_id, 0);

        let proposal = client.get_proposal(&proposal_id);
        assert!(proposal.is_some());
    }

    #[test]
    fn test_quadratic_voting_only_for_sentiment() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);

        client.initialize(&admin, &access_control);

        // Try to create financial proposal with quadratic voting - should fail
        let result = client.try_create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Quadratic,
            &1000,
            &0,
        );

        assert!(result.is_err());

        // Sentiment proposal with quadratic voting - should succeed
        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &1000,
            &0,
        );

        assert_eq!(proposal_id, 0);
    }

    #[test]
    fn test_standard_voting() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &500);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &100,
            &0,
        );

        // Vote
        client.vote(&voter, &proposal_id, &VoteChoice::For, &0);

        let proposal = client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 500); // Full weight
    }

    #[test]
    fn test_quadratic_voting() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &100);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &10,
            &0,
        );

        // Spend 49 voice credits -> sqrt(49) = 7 votes
        client.vote(&voter, &proposal_id, &VoteChoice::For, &49);

        let proposal = client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 7);
    }

    #[test]
    fn test_integer_sqrt() {
        assert_eq!(GovernanceVotingContract::integer_sqrt(0), 0);
        assert_eq!(GovernanceVotingContract::integer_sqrt(1), 1);
        assert_eq!(GovernanceVotingContract::integer_sqrt(4), 2);
        assert_eq!(GovernanceVotingContract::integer_sqrt(9), 3);
        assert_eq!(GovernanceVotingContract::integer_sqrt(16), 4);
        assert_eq!(GovernanceVotingContract::integer_sqrt(25), 5);
        assert_eq!(GovernanceVotingContract::integer_sqrt(49), 7);
        assert_eq!(GovernanceVotingContract::integer_sqrt(100), 10);
        assert_eq!(GovernanceVotingContract::integer_sqrt(99), 9); // Floor
        assert_eq!(GovernanceVotingContract::integer_sqrt(101), 10); // Floor
    }

    #[test]
    fn test_already_voted() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &100);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &10,
            &0,
        );

        client.vote(&voter, &proposal_id, &VoteChoice::For, &0);

        // Try to vote again
        let result = client.try_vote(&voter, &proposal_id, &VoteChoice::Against, &0);
        assert!(result.is_err());
    }

    // ── Comprehensive Delegation Tests ───────────────────────────────────────

    #[test]
    fn test_delegation_then_direct_vote_precedence() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let delegator = Address::generate(&env);
        let delegate = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &delegator, &100);

        // Delegate votes
        client.delegate(&delegator, &delegate);

        // Create proposal
        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &10,
            &0,
        );

        // Delegator votes directly (takes precedence)
        client.vote(&delegator, &proposal_id, &VoteChoice::For, &0);

        let proposal = client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 100);

        // Delegate cannot vote with delegated weight since delegator voted directly
        // (In this implementation, delegator's direct vote uses their own weight)
    }

    #[test]
    fn test_revocation_mid_vote() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let delegator1 = Address::generate(&env);
        let delegator2 = Address::generate(&env);
        let delegate = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &delegator1, &100);
        client.set_voting_weight(&admin, &delegator2, &150);
        client.set_voting_weight(&admin, &delegate, &50); // Delegate's own weight

        // Both delegate to same delegate
        client.delegate(&delegator1, &delegate);
        client.delegate(&delegator2, &delegate);

        // Verify delegate has combined weight
        let delegate_weight = client.get_voting_weight(&delegate);
        assert_eq!(delegate_weight, 300); // 50 + 100 + 150

        // Create proposal
        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &10,
            &0,
        );

        // Delegator1 revokes mid-vote
        client.revoke_delegation(&delegator1);

        // Check delegate's weight decreased
        let delegate_weight = client.get_voting_weight(&delegate);
        assert_eq!(delegate_weight, 200); // 50 + 150 (delegator1's 100 removed)
    }

    #[test]
    fn test_large_delegate_fan_in() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let delegate = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &delegate, &1000); // Delegate's own weight

        // Create 50 delegators
        let mut total_delegated: i128 = 0;
        for i in 1..=50 {
            let delegator = Address::generate(&env);
            let weight = i * 10;
            client.set_voting_weight(&admin, &delegator, &weight);
            client.delegate(&delegator, &delegate);
            total_delegated += weight;
        }

        // Verify delegate has all delegated weight
        let delegate_weight = client.get_voting_weight(&delegate);
        assert_eq!(delegate_weight, 1000 + total_delegated); // Own + delegated
    }

    #[test]
    fn test_re_delegation() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let delegator = Address::generate(&env);
        let delegate1 = Address::generate(&env);
        let delegate2 = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &delegator, &100);

        // Delegate to delegate1
        client.delegate(&delegator, &delegate1);
        assert_eq!(client.get_voting_weight(&delegate1), 100);
        assert_eq!(client.get_voting_weight(&delegate2), 0);

        // Re-delegate to delegate2
        client.delegate(&delegator, &delegate2);
        assert_eq!(client.get_voting_weight(&delegate1), 0);
        assert_eq!(client.get_voting_weight(&delegate2), 100);
    }

    #[test]
    fn test_self_delegation_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let delegator = Address::generate(&env);

        client.initialize(&admin, &access_control);

        // Try to delegate to self
        let result = client.try_delegate(&delegator, &delegator);
        assert!(result.is_err());
    }

    #[test]
    fn test_delegation_without_voting_weight() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let delegator = Address::generate(&env);
        let delegate = Address::generate(&env);

        client.initialize(&admin, &access_control);
        // No voting weight set for delegator

        // Delegation should succeed even without weight
        client.delegate(&delegator, &delegate);
        
        // But delegate won't gain any weight
        assert_eq!(client.get_voting_weight(&delegate), 0);
    }

    #[test]
    fn test_multiple_delegators_to_same_delegate() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let delegator1 = Address::generate(&env);
        let delegator2 = Address::generate(&env);
        let delegator3 = Address::generate(&env);
        let delegate = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &delegator1, &100);
        client.set_voting_weight(&admin, &delegator2, &200);
        client.set_voting_weight(&admin, &delegator3, &300);

        // All delegate to same address
        client.delegate(&delegator1, &delegate);
        client.delegate(&delegator2, &delegate);
        client.delegate(&delegator3, &delegate);

        // Verify combined weight
        let total_weight = client.get_voting_weight(&delegate);
        assert_eq!(total_weight, 600);
    }

    // ── Comprehensive Quadratic Voting Tests ──────────────────────────────────

    #[test]
    fn test_quadratic_cost_calculation() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &10000);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &10,
            &0,
        );

        // Test various credit amounts and their corresponding votes
        // 1 credit -> 1 vote
        // 4 credits -> 2 votes
        // 9 credits -> 3 votes
        // 16 credits -> 4 votes
        // 25 credits -> 5 votes

        client.vote(&voter, &proposal_id, &VoteChoice::For, &25);

        let proposal = client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 5); // sqrt(25) = 5
    }

    #[test]
    fn test_quadratic_voice_credits_exceed_weight_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &100); // Only 100 weight

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &10,
            &0,
        );

        // Try to spend more credits than available weight
        let result = client.try_vote(&voter, &proposal_id, &VoteChoice::For, &101);
        assert!(result.is_err());
    }

    #[test]
    fn test_quadratic_zero_credits_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &100);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &10,
            &0,
        );

        // Try to spend zero credits
        let result = client.try_vote(&voter, &proposal_id, &VoteChoice::For, &0);
        assert!(result.is_err());
    }

    #[test]
    fn test_quadratic_negative_credits_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &100);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &10,
            &0,
        );

        // Try to spend negative credits
        let result = client.try_vote(&voter, &proposal_id, &VoteChoice::For, &-10);
        assert!(result.is_err());
    }

    #[test]
    fn test_quadratic_mode_selection_per_proposal() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);

        client.initialize(&admin, &access_control);

        // Create standard voting proposal
        let proposal1_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Standard"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &10,
            &0,
        );

        // Create quadratic voting proposal
        let proposal2_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Quadratic"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &10,
            &0,
        );

        let proposal1 = client.get_proposal(&proposal1_id).unwrap();
        let proposal2 = client.get_proposal(&proposal2_id).unwrap();

        assert_eq!(proposal1.voting_mode, VotingMode::Standard);
        assert_eq!(proposal2.voting_mode, VotingMode::Quadratic);
    }

    #[test]
    fn test_integer_sqrt_comprehensive() {
        // Test edge cases and various values
        assert_eq!(GovernanceVotingContract::integer_sqrt(-1), 0);
        assert_eq!(GovernanceVotingContract::integer_sqrt(0), 0);
        assert_eq!(GovernanceVotingContract::integer_sqrt(1), 1);
        assert_eq!(GovernanceVotingContract::integer_sqrt(2), 1);
        assert_eq!(GovernanceVotingContract::integer_sqrt(3), 1);
        assert_eq!(GovernanceVotingContract::integer_sqrt(4), 2);
        assert_eq!(GovernanceVotingContract::integer_sqrt(8), 2);
        assert_eq!(GovernanceVotingContract::integer_sqrt(9), 3);
        assert_eq!(GovernanceVotingContract::integer_sqrt(15), 3);
        assert_eq!(GovernanceVotingContract::integer_sqrt(16), 4);
        assert_eq!(GovernanceVotingContract::integer_sqrt(24), 4);
        assert_eq!(GovernanceVotingContract::integer_sqrt(25), 5);
        assert_eq!(GovernanceVotingContract::integer_sqrt(49), 7);
        assert_eq!(GovernanceVotingContract::integer_sqrt(50), 7);
        assert_eq!(GovernanceVotingContract::integer_sqrt(63), 7);
        assert_eq!(GovernanceVotingContract::integer_sqrt(64), 8);
        assert_eq!(GovernanceVotingContract::integer_sqrt(100), 10);
        assert_eq!(GovernanceVotingContract::integer_sqrt(10000), 100);
        assert_eq!(GovernanceVotingContract::integer_sqrt(1000000), 1000);
    }

    #[test]
    fn test_quadratic_voting_multiple_voters() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter1 = Address::generate(&env);
        let voter2 = Address::generate(&env);
        let voter3 = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter1, &100);
        client.set_voting_weight(&admin, &voter2, &100);
        client.set_voting_weight(&admin, &voter3, &100);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &10,
            &0,
        );

        // voter1 spends 25 credits -> 5 votes
        client.vote(&voter1, &proposal_id, &VoteChoice::For, &25);
        
        // voter2 spends 16 credits -> 4 votes
        client.vote(&voter2, &proposal_id, &VoteChoice::For, &16);
        
        // voter3 spends 36 credits -> 6 votes against
        client.vote(&voter3, &proposal_id, &VoteChoice::Against, &36);

        let proposal = client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_for, 9); // 5 + 4
        assert_eq!(proposal.votes_against, 6);
    }

    #[test]
    fn test_proposal_execution_requires_quorum() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &50);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &100, // Quorum = 100
            &0,
        );

        // Vote with only 50 weight
        client.vote(&voter, &proposal_id, &VoteChoice::For, &0);

        // Try to execute - should fail (insufficient quorum)
        let result = client.try_execute_proposal(&admin, &proposal_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_proposal_execution_success() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &150);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &100, // Quorum = 100
            &0,
        );

        // Vote with 150 weight (exceeds quorum)
        client.vote(&voter, &proposal_id, &VoteChoice::For, &0);

        // Execute - should succeed
        client.execute_proposal(&admin, &proposal_id);

        let proposal = client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.executed);
    }

    #[test]
    fn test_vote_record_tracking() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &100);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Sentiment,
            &VotingMode::Quadratic,
            &10,
            &0,
        );

        client.vote(&voter, &proposal_id, &VoteChoice::For, &49);

        let vote_record = client.get_vote_record(&proposal_id, &voter).unwrap();
        assert_eq!(vote_record.voter, voter);
        assert_eq!(vote_record.choice, VoteChoice::For);
        assert_eq!(vote_record.weight, 7); // sqrt(49) = 7
        assert_eq!(vote_record.voice_credits_spent, 49);
    }

    #[test]
    fn test_abstain_vote() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &100);

        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "Desc"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &10,
            &0,
        );

        client.vote(&voter, &proposal_id, &VoteChoice::Abstain, &0);

        let proposal = client.get_proposal(&proposal_id).unwrap();
        assert_eq!(proposal.votes_abstain, 100);
        assert_eq!(proposal.votes_for, 0);
        assert_eq!(proposal.votes_against, 0);
    }
}
