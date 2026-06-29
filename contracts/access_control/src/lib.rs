#![no_std]

use kora_shared::{
    errors::KoraError,
    events,
    reentrancy::ReentrancyGuard,
    types::{MultisigConfig, ParameterKey, ParameterProposal},
    validation::UPGRADE_TIMELOCK_DELAY,
};
use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env, Vec};

/// Timelock delay between a parameter proposal reaching quorum and being executable.
/// Mirrors the B1 upgrade timelock (~24h) so parameter changes get the same cooling-off period.
const GOVERNANCE_TIMELOCK_DELAY: u64 = UPGRADE_TIMELOCK_DELAY;

// ── TTL constants (~30 days) ──────────────────────────────────────────────────
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Admin address — persistent so it survives ledger archival.
    Admin,
    /// Pending new-admin address (two-step transfer).
    PendingAdmin,
    /// Protocol pause flag — persistent so pause state is never silently lost.
    Paused,
    /// Per-address role mapping.
    Role(Address),
    /// Pending upgrade proposal: (wasm_hash, proposed_at_timestamp).
    UpgradeProposal,
    /// Multisig configuration (threshold + signer set).
    MultisigConfig,
    /// A pending protocol-parameter governance proposal, keyed by id.
    ParameterProposal(u64),
    /// Monotonic counter for the next parameter-proposal id.
    NextParamProposalId,
    /// The current governed value of a protocol parameter.
    Parameter(ParameterKey),
}

const PROPOSAL_TTL_LEDGERS: u64 = 120_960; // ~7 days at ~5s/ledger

// ── Role enum ─────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Role {
    Admin,
    Operator,
    Verifier,
    None,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct AccessControlContract;

#[contractimpl]
impl AccessControlContract {
    /// One-time initialization. Sets the admin and initializes the paused flag.
    pub fn initialize(env: Env, admin: Address) -> Result<(), KoraError> {
        // Guard: prevent re-initialization
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }
        kora_shared::validation::require_not_self(&env, &admin)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage()
            .persistent()
            .set(&DataKey::Role(admin.clone()), &Role::Admin);
        Self::bump_persistent(&env, &DataKey::Role(admin));
        Ok(())
    }

    // ── Pause / Unpause ───────────────────────────────────────────────────────

    /// Pause the entire protocol. Admin only. Fails if already paused.
    pub fn pause(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if env
            .storage()
            .instance()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
        {
            return Err(KoraError::AlreadyPaused);
        }
        let _guard = ReentrancyGuard::new(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        events::protocol_paused(&env, &admin);
        Ok(())
    }

    /// Unpause the protocol. Admin only. Fails if not currently paused.
    pub fn unpause(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if !env
            .storage()
            .instance()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
        {
            return Err(KoraError::NotPaused);
        }
        let _guard = ReentrancyGuard::new(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        events::protocol_unpaused(&env, &admin);
        Ok(())
    }

    // ── Role management ───────────────────────────────────────────────────────

    /// Assign a role to an address. Admin only.
    /// - Cannot grant `Role::Admin` (use `propose_admin` + `accept_admin`).
    /// - Cannot grant `Role::None` (use `revoke_role`).
    /// - Cannot grant a role to the current admin address.
    pub fn grant_role(
        env: Env,
        admin: Address,
        target: Address,
        role: Role,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if role == Role::Admin {
            return Err(KoraError::Unauthorized);
        }
        if role == Role::None {
            return Err(KoraError::Unauthorized);
        }
        // Prevent silently overwriting the admin's own role entry
        if target == admin {
            return Err(KoraError::Unauthorized);
        }
        env.storage()
            .persistent()
            .set(&DataKey::Role(target.clone()), &role);
        Self::bump_persistent(&env, &DataKey::Role(target.clone()));
        events::role_granted(&env, &admin, &target);
        Ok(())
    }

    /// Revoke a role from an address. Admin only.
    /// - Cannot revoke the admin's own role.
    /// - Fails if the target has no role assigned.
    pub fn revoke_role(env: Env, admin: Address, target: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let current_role = env
            .storage()
            .persistent()
            .get::<_, Role>(&DataKey::Role(target.clone()))
            .unwrap_or(Role::None);

        if current_role == Role::Admin {
            return Err(KoraError::Unauthorized);
        }
        if current_role == Role::None {
            return Err(KoraError::RoleNotAssigned);
        }
        // Use remove() to reclaim storage rather than writing Role::None
        env.storage()
            .persistent()
            .remove(&DataKey::Role(target.clone()));
        events::role_revoked(&env, &admin, &target);
        Ok(())
    }

    // ── Admin transfer ────────────────────────────────────────────────────────

    // ── Admin transfer (two-step) ─────────────────────────────────────────────

    /// Step 1: current admin proposes a new admin. Does not transfer yet.
    /// - Cannot propose self.
    /// - Cannot propose an address already holding a non-Admin role (must revoke first).
    pub fn propose_admin(
        env: Env,
        current_admin: Address,
        new_admin: Address,
    ) -> Result<(), KoraError> {
        current_admin.require_auth();
        Self::require_admin(&env, &current_admin)?;
        if current_admin == new_admin {
            return Err(KoraError::InvalidAddress);
        }
        kora_shared::validation::require_not_self(&env, &new_admin)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage()
            .persistent()
            .set(&DataKey::Role(new_admin.clone()), &Role::Admin);
        env.storage()
            .persistent()
            .set(&DataKey::Role(current_admin), &Role::None);
        // Guard: new_admin must not already hold a role (Operator/Verifier)
        // to prevent silent role overwrite.
        let existing = env
            .storage()
            .persistent()
            .get::<_, Role>(&DataKey::Role(new_admin.clone()))
            .unwrap_or(Role::None);
        if existing != Role::None && existing != Role::Admin {
            return Err(KoraError::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage()
            .persistent()
            .set(&DataKey::Role(new_admin.clone()), &Role::Admin);
        Self::bump_persistent(&env, &DataKey::Role(new_admin.clone()));
        env.storage()
            .persistent()
            .remove(&DataKey::Role(current_admin.clone()));
        events::admin_transferred(&env, &current_admin, &new_admin);
        Ok(())
    }

    /// Cancel a pending admin proposal. Current admin only.
    pub fn cancel_admin_proposal(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if !env.storage().instance().has(&DataKey::PendingAdmin) {
            return Err(KoraError::NoPendingAdminProposal);
        }
        env.storage().instance().remove(&DataKey::PendingAdmin);
        events::admin_proposal_cancelled(&env, &admin);
        Ok(())
    }

    // ── Multisig ──────────────────────────────────────────────────────────────

    /// Configure the N-of-M multisig. Admin only. Once configured, admin
    /// actions must go through propose → approve → execute.
    pub fn configure_multisig(
        env: Env,
        admin: Address,
        signers: Vec<Address>,
        threshold: u32,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let signer_count = signers.len();
        if threshold == 0 || threshold > signer_count {
            return Err(KoraError::InvalidThreshold);
        }

        let config = MultisigConfig { threshold, signers };
        env.storage()
            .persistent()
            .set(&DataKey::MultisigConfig, &config);
        Self::bump_persistent(&env, &DataKey::MultisigConfig);

        if !env.storage().persistent().has(&DataKey::NextProposalId) {
            env.storage()
                .persistent()
                .set(&DataKey::NextProposalId, &1u64);
        }

        events::multisig_configured(&env, threshold, signer_count);
        Ok(())
    }

    /// Propose a new admin action. Caller must be a signer.
    pub fn propose_action(
        env: Env,
        proposer: Address,
        action: AdminAction,
    ) -> Result<u64, KoraError> {
        proposer.require_auth();
        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &proposer)?;

        let proposal_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::NextProposalId)
            .unwrap_or(1);

        let mut approvals = Vec::new(&env);
        approvals.push_back(proposer.clone());

        let proposal = Proposal {
            id: proposal_id,
            action,
            proposer: proposer.clone(),
            approvals,
            executed: false,
            created_at: env.ledger().timestamp(),
            expires_at: env.ledger().timestamp() + PROPOSAL_TTL_LEDGERS,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::Proposal(proposal_id));

        env.storage().persistent().set(
            &DataKey::NextProposalId,
            &(proposal_id
                .checked_add(1)
                .ok_or(KoraError::ArithmeticOverflow)?),
        );

        events::action_proposed(&env, proposal_id, &proposer);
        Ok(proposal_id)
    }

    /// Approve an existing proposal. Caller must be a signer who hasn't
    /// already approved this proposal.
    pub fn approve_action(env: Env, approver: Address, proposal_id: u64) -> Result<(), KoraError> {
        approver.require_auth();
        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &approver)?;

        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .ok_or(KoraError::ProposalNotFound)?;

        if proposal.executed {
            return Err(KoraError::ProposalAlreadyExecuted);
        }
        if env.ledger().timestamp() > proposal.expires_at {
            return Err(KoraError::ProposalExpired);
        }

        for i in 0..proposal.approvals.len() {
            if proposal.approvals.get(i).ok_or(KoraError::Unauthorized)? == approver {
                return Err(KoraError::AlreadyApproved);
            }
        }

        proposal.approvals.push_back(approver.clone());

        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::Proposal(proposal_id));

        events::action_approved(&env, proposal_id, &approver, proposal.approvals.len());
        Ok(())
    }

    /// Execute a proposal once the approval threshold is met.
    /// Any signer can call execute.
    pub fn execute_action(env: Env, executor: Address, proposal_id: u64) -> Result<(), KoraError> {
        executor.require_auth();
        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &executor)?;

        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .ok_or(KoraError::ProposalNotFound)?;

        if proposal.executed {
            return Err(KoraError::ProposalAlreadyExecuted);
        }
        if env.ledger().timestamp() > proposal.expires_at {
            return Err(KoraError::ProposalExpired);
        }
        if proposal.approvals.len() < config.threshold {
            return Err(KoraError::ThresholdNotMet);
        }

        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);

        match proposal.action {
            AdminAction::Pause => {
                env.storage().instance().set(&DataKey::Paused, &true);
                events::protocol_paused(&env, &executor);
            }
            AdminAction::Unpause => {
                env.storage().instance().set(&DataKey::Paused, &false);
                events::protocol_unpaused(&env, &executor);
            }
            AdminAction::GrantRole(target, role_val) => {
                let role = match role_val {
                    1 => Role::Operator,
                    2 => Role::Verifier,
                    _ => return Err(KoraError::Unauthorized),
                };
                env.storage()
                    .persistent()
                    .set(&DataKey::Role(target.clone()), &role);
                Self::bump_persistent(&env, &DataKey::Role(target.clone()));
                events::role_granted(&env, &executor, &target);
            }
            AdminAction::RevokeRole(target) => {
                env.storage()
                    .persistent()
                    .remove(&DataKey::Role(target.clone()));
                events::role_revoked(&env, &executor, &target);
            }
            AdminAction::TransferAdmin(new_admin) => {
                env.storage().instance().set(&DataKey::Admin, &new_admin);
                env.storage()
                    .persistent()
                    .set(&DataKey::Role(new_admin.clone()), &Role::Admin);
                Self::bump_persistent(&env, &DataKey::Role(new_admin.clone()));
                events::admin_transferred(&env, &executor, &new_admin);
            }
        }

        events::action_executed(&env, proposal_id, &executor);
        Ok(())
    }

    /// Get a proposal by ID.
    pub fn get_proposal(env: Env, proposal_id: u64) -> Result<Proposal, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .ok_or(KoraError::ProposalNotFound)
    }

    /// Get the current multisig configuration.
    pub fn get_multisig_config(env: Env) -> Result<MultisigConfig, KoraError> {
        Self::load_multisig_config(&env)
    }

    // ── Parameter Governance ─────────────────────────────────────────────────────

    /// Propose a change to a tunable protocol parameter.
    ///
    /// Gated by the B2 multisig signer set: only a configured signer may propose, and the
    /// proposer's vote is recorded automatically. Execution additionally requires a quorum of
    /// signer votes (B2 threshold) and a B1-style timelock to elapse.
    pub fn propose_parameter_change(
        env: Env,
        proposer: Address,
        key: ParameterKey,
        new_value: u32,
    ) -> Result<u64, KoraError> {
        proposer.require_auth();

        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &proposer)?;
        Self::require_valid_parameter(&key, new_value)?;

        let proposal_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::NextParamProposalId)
            .unwrap_or(1);

        let mut approvals = Vec::new(&env);
        approvals.push_back(proposer.clone());

        let proposal = ParameterProposal {
            id: proposal_id,
            key,
            new_value,
            proposer: proposer.clone(),
            approvals,
            created_at: env.ledger().timestamp(),
            executed: false,
        };

        env.storage()
            .persistent()
            .set(&DataKey::ParameterProposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::ParameterProposal(proposal_id));
        env.storage().persistent().set(
            &DataKey::NextParamProposalId,
            &(proposal_id
                .checked_add(1)
                .ok_or(KoraError::ArithmeticOverflow)?),
        );

        events::action_proposed(&env, proposal_id, &proposer);
        Ok(proposal_id)
    }

    /// Vote in favour of a pending parameter-change proposal. Multisig signers only.
    pub fn vote_parameter_change(
        env: Env,
        signer: Address,
        proposal_id: u64,
    ) -> Result<(), KoraError> {
        signer.require_auth();

        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &signer)?;

        let mut proposal: ParameterProposal = env
            .storage()
            .persistent()
            .get(&DataKey::ParameterProposal(proposal_id))
            .ok_or(KoraError::ParameterProposalNotFound)?;

        if proposal.executed {
            return Err(KoraError::ParameterProposalAlreadyExecuted);
        }
        for i in 0..proposal.approvals.len() {
            if proposal.approvals.get(i).unwrap() == signer {
                return Err(KoraError::AlreadyVoted);
            }
        }

        proposal.approvals.push_back(signer.clone());
        let count = proposal.approvals.len();

        env.storage()
            .persistent()
            .set(&DataKey::ParameterProposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::ParameterProposal(proposal_id));

        events::action_approved(&env, proposal_id, &signer, count);
        Ok(())
    }

    /// Execute a parameter-change proposal once it has reached the multisig threshold (B2) and the
    /// governance timelock has elapsed (B1). Commits the new value on-chain.
    pub fn execute_parameter_change(
        env: Env,
        caller: Address,
        proposal_id: u64,
    ) -> Result<(), KoraError> {
        caller.require_auth();

        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &caller)?;

        let mut proposal: ParameterProposal = env
            .storage()
            .persistent()
            .get(&DataKey::ParameterProposal(proposal_id))
            .ok_or(KoraError::ParameterProposalNotFound)?;

        if proposal.executed {
            return Err(KoraError::ParameterProposalAlreadyExecuted);
        }
        if proposal.approvals.len() < config.threshold {
            return Err(KoraError::GovernanceThresholdNotMet);
        }
        if env.ledger().timestamp() < proposal.created_at + GOVERNANCE_TIMELOCK_DELAY {
            return Err(KoraError::GovernanceTimelockNotElapsed);
        }

        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::ParameterProposal(proposal_id), &proposal);

        env.storage().persistent().set(
            &DataKey::Parameter(proposal.key.clone()),
            &proposal.new_value,
        );
        Self::bump_persistent(&env, &DataKey::Parameter(proposal.key.clone()));

        events::action_executed(&env, proposal_id, &caller);
        Ok(())
    }

    /// Read the current governed value of a parameter, if one has been executed.
    pub fn get_parameter(env: Env, key: ParameterKey) -> Option<u32> {
        env.storage().persistent().get(&DataKey::Parameter(key))
    }

    /// Read a parameter-change proposal by id.
    pub fn get_parameter_proposal(
        env: Env,
        proposal_id: u64,
    ) -> Result<ParameterProposal, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::ParameterProposal(proposal_id))
            .ok_or(KoraError::ParameterProposalNotFound)
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    /// Returns `true` if the protocol is currently paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Returns the role assigned to `address`, or `Role::None` if unassigned.
    pub fn get_role(env: Env, address: Address) -> Role {
        env.storage()
            .persistent()
            .get(&DataKey::Role(address))
            .unwrap_or(Role::None)
    }

    /// Returns `true` if `address` holds the given `role`.
    pub fn has_role(env: Env, address: Address, role: Role) -> bool {
        let assigned: Role = env
            .storage()
            .persistent()
            .get(&DataKey::Role(address))
            .unwrap_or(Role::None);
        assigned == role
    }

    /// Returns the current admin address.
    pub fn get_admin(env: Env) -> Result<Address, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(KoraError::NotInitialized)
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(
            &DataKey::UpgradeProposal,
            &(new_wasm_hash.clone(), env.ledger().timestamp()),
        );
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        Ok(())
    }

    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(KoraError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(KoraError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        events::upgrade_executed(&env, &admin, &wasm_hash);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Read the paused flag from persistent storage.
    fn read_paused(env: &Env) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

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

    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_BUMP);
    }

    fn load_multisig_config(env: &Env) -> Result<MultisigConfig, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::MultisigConfig)
            .ok_or(KoraError::MultisigNotConfigured)
    }

    fn require_signer(config: &MultisigConfig, caller: &Address) -> Result<(), KoraError> {
        for i in 0..config.signers.len() {
            if &config.signers.get(i).ok_or(KoraError::Unauthorized)? == caller {
                return Ok(());
            }
        }
        Err(KoraError::SignerNotFound)
    }

    /// Validate a proposed parameter value against its allowed range.
    fn require_valid_parameter(key: &ParameterKey, value: u32) -> Result<(), KoraError> {
        let ok = match key {
            ParameterKey::FeeBps | ParameterKey::LatePenaltyBps => value <= 10_000,
            ParameterKey::MaxRiskScore => value <= 100,
        };
        if ok {
            Ok(())
        } else {
            Err(KoraError::InvalidParameterValue)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use kora_shared::errors::KoraError;
    use soroban_sdk::{
        testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation},
        Address, Env, IntoVal, Symbol,
    };

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Deploy and initialize with mock_all_auths for convenience.
    fn setup() -> (Env, Address, AccessControlContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);
        (env, admin, client)
    }

    /// Deploy without initializing (for pre-init tests).
    fn deploy_uninit() -> (Env, AccessControlContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &contract_id);
        (env, client)
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_success() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        assert!(client.try_initialize(&admin).is_ok());
        // Admin is stored correctly
        assert_eq!(client.get_admin(), admin);
        // Admin role is set
        assert_eq!(client.get_role(&admin), Role::Admin);
        // Protocol starts unpaused
        assert!(!client.is_paused());
    }

    #[test]
    fn test_initialize_already_initialized_returns_correct_error() {
        let (_, admin, client) = setup();
        let result = client.try_initialize(&admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::AlreadyInitialized);
    }

    #[test]
    fn test_initialize_second_admin_ignored() {
        // A second initialize with a different admin must fail — original admin unchanged
        let (env, admin, client) = setup();
        let attacker = Address::generate(&env);
        let _ = client.try_initialize(&attacker);
        assert_eq!(client.get_admin(), admin);
    }

    // ── pause ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_pause_sets_paused_flag() {
        let (_, admin, client) = setup();
        assert!(!client.is_paused());
        client.pause(&admin);
        assert!(client.is_paused());
    }

    #[test]
    fn test_pause_requires_admin_auth() {
        let (env, admin, client) = setup();
        // Use mock_auths to verify the exact auth requirement
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "pause",
                args: (&admin,).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert!(client.try_pause(&admin).is_ok());
    }

    #[test]
    fn test_pause_non_admin_returns_not_admin() {
        let (env, _, client) = setup();
        let stranger = Address::generate(&env);
        let result = client.try_pause(&stranger);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_pause_already_paused_returns_correct_error() {
        let (_, admin, client) = setup();
        client.pause(&admin);
        let result = client.try_pause(&admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::AlreadyPaused);
    }

    #[test]
    fn test_pause_state_unchanged_after_double_pause() {
        // After a failed second pause, the contract must still be paused
        let (_, admin, client) = setup();
        client.pause(&admin);
        let _ = client.try_pause(&admin);
        assert!(client.is_paused());
    }

    // ── unpause ───────────────────────────────────────────────────────────────

    #[test]
    fn test_unpause_clears_paused_flag() {
        let (_, admin, client) = setup();
        client.pause(&admin);
        client.unpause(&admin);
        assert!(!client.is_paused());
    }

    #[test]
    fn test_unpause_requires_admin_auth() {
        let (env, admin, client) = setup();
        client.pause(&admin);
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "unpause",
                args: (&admin,).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert!(client.try_unpause(&admin).is_ok());
    }

    #[test]
    fn test_unpause_non_admin_returns_not_admin() {
        let (env, admin, client) = setup();
        client.pause(&admin);
        let stranger = Address::generate(&env);
        let result = client.try_unpause(&stranger);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_unpause_when_not_paused_returns_correct_error() {
        let (_, admin, client) = setup();
        let result = client.try_unpause(&admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotPaused);
    }

    #[test]
    fn test_unpause_state_unchanged_after_failed_unpause() {
        // After a failed unpause (not paused), state must still be unpaused
        let (_, admin, client) = setup();
        let _ = client.try_unpause(&admin);
        assert!(!client.is_paused());
    }

    #[test]
    fn test_pause_unpause_cycle_multiple_times() {
        let (_, admin, client) = setup();
        for _ in 0..5 {
            client.pause(&admin);
            assert!(client.is_paused());
            client.unpause(&admin);
            assert!(!client.is_paused());
        }
    }

    // ── grant_role ────────────────────────────────────────────────────────────

    #[test]
    fn test_grant_role_operator_success() {
        let (env, admin, client) = setup();
        let operator = Address::generate(&env);
        client.grant_role(&admin, &operator, &Role::Operator);
        assert_eq!(client.get_role(&operator), Role::Operator);
    }

    #[test]
    fn test_grant_role_verifier_success() {
        let (env, admin, client) = setup();
        let verifier = Address::generate(&env);
        client.grant_role(&admin, &verifier, &Role::Verifier);
        assert_eq!(client.get_role(&verifier), Role::Verifier);
    }

    #[test]
    fn test_grant_role_requires_admin_auth() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "grant_role",
                args: (&admin, &target, &Role::Verifier).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert!(client
            .try_grant_role(&admin, &target, &Role::Verifier)
            .is_ok());
    }

    #[test]
    fn test_grant_role_non_admin_returns_not_admin() {
        let (env, _, client) = setup();
        let stranger = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_grant_role(&stranger, &target, &Role::Verifier);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_grant_role_admin_variant_returns_unauthorized() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let result = client.try_grant_role(&admin, &target, &Role::Admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_grant_role_none_variant_returns_unauthorized() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let result = client.try_grant_role(&admin, &target, &Role::None);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_grant_role_to_self_returns_unauthorized() {
        let (_, admin, client) = setup();
        let result = client.try_grant_role(&admin, &admin, &Role::Operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_grant_role_state_unchanged_after_failed_grant() {
        // After a rejected grant, the target must still have no role
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let _ = client.try_grant_role(&admin, &target, &Role::Admin);
        assert_eq!(client.get_role(&target), Role::None);
    }

    #[test]
    fn test_grant_role_override_operator_to_verifier() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &Role::Operator);
        client.grant_role(&admin, &user, &Role::Verifier);
        assert_eq!(client.get_role(&user), Role::Verifier);
    }

    #[test]
    fn test_grant_role_override_verifier_to_operator() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &Role::Verifier);
        client.grant_role(&admin, &user, &Role::Operator);
        assert_eq!(client.get_role(&user), Role::Operator);
    }

    #[test]
    fn test_grant_role_same_role_twice_idempotent() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &Role::Verifier);
        client.grant_role(&admin, &user, &Role::Verifier);
        assert_eq!(client.get_role(&user), Role::Verifier);
    }

    #[test]
    fn test_grant_role_multiple_users_independent() {
        let (env, admin, client) = setup();
        let v1 = Address::generate(&env);
        let v2 = Address::generate(&env);
        let op = Address::generate(&env);
        client.grant_role(&admin, &v1, &Role::Verifier);
        client.grant_role(&admin, &v2, &Role::Verifier);
        client.grant_role(&admin, &op, &Role::Operator);
        assert_eq!(client.get_role(&v1), Role::Verifier);
        assert_eq!(client.get_role(&v2), Role::Verifier);
        assert_eq!(client.get_role(&op), Role::Operator);
        // Revoking one does not affect others
        client.revoke_role(&admin, &v1);
        assert_eq!(client.get_role(&v1), Role::None);
        assert_eq!(client.get_role(&v2), Role::Verifier);
    }

    // ── revoke_role ───────────────────────────────────────────────────────────

    #[test]
    fn test_revoke_role_success() {
        let (env, admin, client) = setup();
        let operator = Address::generate(&env);
        client.grant_role(&admin, &operator, &Role::Operator);
        client.revoke_role(&admin, &operator);
        assert_eq!(client.get_role(&operator), Role::None);
    }

    #[test]
    fn test_revoke_role_requires_admin_auth() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        client.grant_role(&admin, &target, &Role::Operator);
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "revoke_role",
                args: (&admin, &target).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert!(client.try_revoke_role(&admin, &target).is_ok());
    }

    #[test]
    fn test_revoke_role_non_admin_returns_not_admin() {
        let (env, admin, client) = setup();
        let operator = Address::generate(&env);
        let stranger = Address::generate(&env);
        client.grant_role(&admin, &operator, &Role::Operator);
        let result = client.try_revoke_role(&stranger, &operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_revoke_role_admin_returns_unauthorized() {
        let (_, admin, client) = setup();
        let result = client.try_revoke_role(&admin, &admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_revoke_role_not_assigned_returns_correct_error() {
        let (env, admin, client) = setup();
        let stranger = Address::generate(&env);
        let result = client.try_revoke_role(&admin, &stranger);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::RoleNotAssigned);
    }

    #[test]
    fn test_revoke_role_state_unchanged_after_failed_revoke() {
        // After a failed revoke (no role), target still has no role
        let (env, admin, client) = setup();
        let stranger = Address::generate(&env);
        let _ = client.try_revoke_role(&admin, &stranger);
        assert_eq!(client.get_role(&stranger), Role::None);
    }

    #[test]
    fn test_revoke_role_twice_fails_second_time() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &Role::Verifier);
        client.revoke_role(&admin, &user);
        // Second revoke must fail — role is already gone
        let result = client.try_revoke_role(&admin, &user);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::RoleNotAssigned);
    }

    #[test]
    fn test_revoke_then_re_grant() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &Role::Verifier);
        client.revoke_role(&admin, &user);
        client.grant_role(&admin, &user, &Role::Operator);
        assert_eq!(client.get_role(&user), Role::Operator);
    }

    // ── propose_admin / accept_admin ──────────────────────────────────────────

    #[test]
    fn test_transfer_admin_success() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);
        assert_eq!(client.get_admin(), new_admin);
        assert_eq!(client.get_role(&new_admin), Role::Admin);
        assert_eq!(client.get_role(&admin), Role::None);
    }

    #[test]
    fn test_transfer_admin_requires_current_admin_auth() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        env.mock_auths(&[soroban_sdk::testutils::MockAuth {
            address: &admin,
            invoke: &soroban_sdk::testutils::MockAuthInvoke {
                contract: &client.address,
                fn_name: "propose_admin",
                args: (&admin, &new_admin).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert!(client.try_propose_admin(&admin, &new_admin).is_ok());
    }

    #[test]
    fn test_transfer_admin_non_admin_returns_not_admin() {
        let (env, _, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_propose_admin(&stranger, &new_admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_transfer_admin_to_self_returns_invalid_address() {
        let (_, admin, client) = setup();
        let result = client.try_propose_admin(&admin, &admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAddress);
    }

    #[test]
    fn test_transfer_admin_to_operator_returns_unauthorized() {
        let (env, admin, client) = setup();
        let operator = Address::generate(&env);
        client.grant_role(&admin, &operator, &Role::Operator);
        let result = client.try_propose_admin(&admin, &operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_transfer_admin_to_verifier_returns_unauthorized() {
        let (env, admin, client) = setup();
        let verifier = Address::generate(&env);
        client.grant_role(&admin, &verifier, &Role::Verifier);
        let result = client.try_propose_admin(&admin, &verifier);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_transfer_admin_state_unchanged_after_failed_transfer() {
        let (env, admin, client) = setup();
        let _ = client.try_propose_admin(&admin, &admin);
        assert_eq!(client.get_admin(), admin);
        assert_eq!(client.get_role(&admin), Role::Admin);
    }

    #[test]
    fn test_transfer_admin_old_admin_loses_all_privileges() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);
        assert!(client.try_pause(&admin).is_err());
        let target = Address::generate(&env);
        assert!(client
            .try_grant_role(&admin, &target, &Role::Verifier)
            .is_err());
        // Old admin cannot transfer admin again
        assert!(client.try_transfer_admin(&admin, &target).is_err());
    }

    #[test]
    fn test_transfer_admin_new_admin_has_full_privileges() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);
        client.pause(&new_admin);
        assert!(client.is_paused());
        client.unpause(&new_admin);
        let target = Address::generate(&env);
        client.grant_role(&new_admin, &target, &Role::Verifier);
        assert_eq!(client.get_role(&target), Role::Verifier);
    }

    #[test]
    fn test_transfer_admin_chain_a_to_b_to_c() {
        let (env, admin_a, client) = setup();
        let admin_b = Address::generate(&env);
        let admin_c = Address::generate(&env);
        client.propose_admin(&admin_a, &admin_b);
        client.accept_admin(&admin_b);
        assert_eq!(client.get_admin(), admin_b);
        client.propose_admin(&admin_b, &admin_c);
        client.accept_admin(&admin_c);
        assert_eq!(client.get_admin(), admin_c);
        assert_eq!(client.get_role(&admin_a), Role::None);
        assert_eq!(client.get_role(&admin_b), Role::None);
        assert_eq!(client.get_role(&admin_c), Role::Admin);
    }

    #[test]
    fn test_transfer_admin_to_clean_address_succeeds() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        assert_eq!(client.get_role(&new_admin), Role::None);
        assert!(client.try_propose_admin(&admin, &new_admin).is_ok());
    }

    #[test]
    fn test_accept_admin_wrong_caller_fails() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        let impostor = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        assert!(client.try_accept_admin(&impostor).is_err());
    }

    #[test]
    fn test_cancel_admin_proposal_blocks_accept() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.cancel_admin_proposal(&admin);
        assert!(client.try_accept_admin(&new_admin).is_err());
    }

    // ── get_admin ─────────────────────────────────────────────────────────────

    #[test]
    fn test_pause_before_init_returns_not_initialized() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        let result = client.try_pause(&admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotInitialized);
    }

    #[test]
    fn test_initialize_self_as_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &contract_id);
        // Passing the contract's own address as admin must be rejected
        let result = client.try_initialize(&contract_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_transfer_admin_to_self_contract_rejected() {
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let result = client.try_propose_admin(&admin, &contract_id);
        assert!(result.is_err());
    }
}
#[test]
fn test_role_override() {
    fn test_grant_role_before_init_returns_not_initialized() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_grant_role(&admin, &target, &Role::Verifier);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotInitialized);
    }

    #[test]
    fn test_revoke_role_before_init_returns_not_initialized() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_revoke_role(&admin, &target);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotInitialized);
    }

    #[test]
    fn test_transfer_admin_before_init_returns_not_initialized() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_propose_admin(&admin, &new_admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotInitialized);
    }

    #[test]
    fn test_get_role_falls_back_to_admin_when_role_key_missing() {
        let (env, admin, client) = setup();
        env.storage()
            .persistent()
            .remove(&DataKey::Role(admin.clone()));
        assert_eq!(client.get_role(&admin), Role::Admin);
    }

    #[test]
    fn test_get_admin_before_init_returns_not_initialized() {
        let (_, client) = deploy_uninit();
        let result = client.try_get_admin();
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotInitialized);
    }

    #[test]
    fn test_get_admin_returns_correct_address() {
        let (_, admin, client) = setup();
        assert_eq!(client.get_admin(), admin);
    }

    // ── get_role ──────────────────────────────────────────────────────────────

    #[test]
    fn test_get_role_unknown_address_returns_none() {
        let (env, _, client) = setup();
        let unknown = Address::generate(&env);
        assert_eq!(client.get_role(&unknown), Role::None);
    }

    #[test]
    fn test_get_role_admin_returns_admin() {
        let (_, admin, client) = setup();
        assert_eq!(client.get_role(&admin), Role::Admin);
    }

    // ── is_paused ─────────────────────────────────────────────────────────────

    #[test]
    fn test_is_paused_default_false() {
        let (_, _, client) = setup();
        assert!(!client.is_paused());
    }

    #[test]
    fn test_is_paused_reflects_state_correctly() {
        let (_, admin, client) = setup();
        assert!(!client.is_paused());
        client.pause(&admin);
        assert!(client.is_paused());
        client.unpause(&admin);
        assert!(!client.is_paused());
    }

    // ── cross-function interaction ────────────────────────────────────────────

    #[test]
    fn test_revoke_role_then_transfer_admin_to_that_address_succeeds() {
        // After revoking a role, the address is clean and can receive admin
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &Role::Operator);
        client.revoke_role(&admin, &user);
        assert_eq!(client.get_role(&user), Role::None);
        assert!(client.try_propose_admin(&admin, &user).is_ok());
        client.accept_admin(&user);
        assert_eq!(client.get_admin(), user);
    }

    #[test]
    fn test_pause_does_not_affect_role_state() {
        let (env, admin, client) = setup();
        let verifier = Address::generate(&env);
        client.grant_role(&admin, &verifier, &Role::Verifier);
        client.pause(&admin);
        // Roles are unaffected by pause state
        assert_eq!(client.get_role(&verifier), Role::Verifier);
        assert_eq!(client.get_role(&admin), Role::Admin);
    }

    #[test]
    fn test_grant_and_revoke_do_not_affect_pause_state() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        client.pause(&admin);
        client.grant_role(&admin, &user, &Role::Verifier);
        assert!(client.is_paused()); // pause state unchanged
        client.revoke_role(&admin, &user);
        assert!(client.is_paused()); // still paused
    }

    // ── Additional admin transfer tests ──────────────────────────────────────

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);
        assert_eq!(client.get_admin(), new_admin);
        assert_eq!(client.get_role(&new_admin), Role::Admin);
        assert_eq!(client.get_role(&admin), Role::None);
    }

    #[test]
    fn test_transfer_admin_self_rejected() {
        let (_, admin, client) = setup();
        assert!(client.try_propose_admin(&admin, &admin).is_err());
    }

    #[test]
    fn test_non_admin_cannot_transfer_admin() {
        let (env, _admin, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        assert!(client.try_propose_admin(&stranger, &new_admin).is_err());
    }

    // ── has_role view ─────────────────────────────────────────────────────────

    #[test]
    fn test_has_role_returns_false_for_unassigned() {
        let (env, _admin, client) = setup();
        let user = Address::generate(&env);
        assert!(!client.has_role(&user, &Role::Verifier));
        assert!(!client.has_role(&user, &Role::Operator));
    }

    #[test]
    fn test_has_role_returns_true_after_grant() {
        let (env, admin, client) = setup();
        let user = Address::generate(&env);
        client.grant_role(&admin, &user, &Role::Verifier);
        assert!(client.has_role(&user, &Role::Verifier));
        assert!(!client.has_role(&user, &Role::Operator));
    }

    #[test]
    fn test_initialize_already_initialized_fails() {
        let (_, admin, client) = setup();
        let result = client.try_initialize(&admin);
        assert!(result.is_err());
    }
}

    #[test]
    fn test_interleaved_pause_and_role_operations_remain_independent() {
        // Long interleaved sequence: grant → pause → revoke → unpause → re-grant
        // Verify that pause state and role assignments never affect each other
        let (env, admin, client) = setup();
        let target1 = Address::generate(&env);
        let target2 = Address::generate(&env);

        // Phase 1: Grant roles
        client.grant_role(&admin, &target1, &Role::Verifier);
        client.grant_role(&admin, &target2, &Role::Operator);
        assert!(client.has_role(&target1, &Role::Verifier));
        assert!(client.has_role(&target2, &Role::Operator));
        assert!(!client.is_paused());

        // Phase 2: Pause protocol
        client.pause(&admin);
        assert!(client.is_paused());
        assert!(client.has_role(&target1, &Role::Verifier), "Role should survive pause");
        assert!(client.has_role(&target2, &Role::Operator), "Role should survive pause");

        // Phase 3: Revoke roles while paused
        client.revoke_role(&admin, &target1);
        assert!(client.is_paused(), "Pause state should persist after revoke");
        assert!(!client.has_role(&target1, &Role::Verifier));
        assert!(client.has_role(&target2, &Role::Operator), "Other role should be unaffected");

        // Phase 4: Unpause protocol
        client.unpause(&admin);
        assert!(!client.is_paused());
        assert!(!client.has_role(&target1, &Role::Verifier), "Revoked role should stay revoked");
        assert!(client.has_role(&target2, &Role::Operator), "Role should survive unpause");

        // Phase 5: Re-grant role after unpause
        client.grant_role(&admin, &target1, &Role::Verifier);
        assert!(!client.is_paused(), "Pause state should remain unpaused");
        assert!(client.has_role(&target1, &Role::Verifier), "Re-granted role should be assigned");
        assert!(client.has_role(&target2, &Role::Operator), "Other role should be unaffected");
    }
}
