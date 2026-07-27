#![no_std]

use kora_shared::{
    audit::{AdminActionType, AdminAuditEntry, AuditSource, MAX_AUDIT_LOG_SIZE},
    errors::CommonError,
    events,
    reentrancy::ReentrancyGuard,
    types::{AdminAction, MultisigConfig, ParameterKey, ParameterProposal, Proposal, RecoveryProposal},
    validation::UPGRADE_TIMELOCK_DELAY,
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Bytes, BytesN, Env, IntoVal, Vec};

// ── Errors ───────────────────────────────────────────────────────────────────

/// Local error enum for `access_control`. Soroban's `#[contracterror]` macro caps an
/// error enum at 50 variants, so each contract now owns its own small enum instead of
/// sharing one giant `AccessControlError` across all 7 contracts.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AccessControlError {
    AlreadyApproved = 1,
    AlreadyInitialized = 2,
    AlreadyPaused = 3,
    AlreadyVoted = 4,
    ArithmeticOverflow = 5,
    GovernanceThresholdNotMet = 6,
    GovernanceTimelockNotElapsed = 7,
    InvalidAddress = 8,
    InvalidParameterValue = 9,
    InvalidThreshold = 10,
    MultisigNotConfigured = 11,
    NoUpgradeProposed = 12,
    NotAdmin = 13,
    NotInitialized = 14,
    NotMultisigSigner = 15,
    NotPaused = 16,
    ParameterProposalAlreadyExecuted = 17,
    ParameterProposalNotFound = 18,
    ProposalAlreadyExecuted = 19,
    ProposalExpired = 20,
    ProposalNotFound = 21,
    Reentrancy = 22,
    RoleNotAssigned = 23,
    SignerNotFound = 24,
    ThresholdNotMet = 25,
    Unauthorized = 26,
    UpgradeTimelockNotElapsed = 27,
}

impl From<CommonError> for AccessControlError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAddress => AccessControlError::InvalidAddress,
            CommonError::ArithmeticOverflow => AccessControlError::ArithmeticOverflow,
            CommonError::Reentrancy => AccessControlError::Reentrancy,
            _ => AccessControlError::InvalidParameterValue,
        }
    }
}

/// Timelock delay between a parameter proposal reaching quorum and being executable.
/// Mirrors the B1 upgrade timelock (~24h) so parameter changes get the same cooling-off period.
const GOVERNANCE_TIMELOCK_DELAY: u64 = UPGRADE_TIMELOCK_DELAY;

/// Long timelock for multisig recovery proposals (30 days at ~5s/ledger).
/// Gives legitimate signer set ample opportunity to object if recovery is illegitimate.
const RECOVERY_TIMELOCK_DELAY: u64 = 518_400; // ~30 days at ~5s/ledger

// ── TTL constants (~30 days) ──────────────────────────────────────────────────
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Admin address — persistent so it survives ledger archival.
    Admin,
    /// Protocol pause flag — persistent so pause state is never silently lost.
    Paused,
    /// Per-address role mapping.
    Role(Address),
    /// Registry of all addresses holding a given role (for enumeration).
    RoleMembers(Role),
    /// Pending upgrade proposal: (wasm_hash, proposed_at_timestamp).
    UpgradeProposal,
    /// Multisig configuration (threshold + signer set).
    MultisigConfig,
    /// Monotonic counter for the next multisig proposal id.
    NextProposalId,
    /// A pending multisig action proposal, keyed by proposal id.
    Proposal(u64),
    /// A pending protocol-parameter governance proposal, keyed by id.
    ParameterProposal(u64),
    /// Monotonic counter for the next parameter-proposal id.
    NextParamProposalId,
    /// The current governed value of a protocol parameter.
    Parameter(ParameterKey),
    /// Multisig recovery proposal, keyed by proposal id.
    RecoveryProposal(u64),
    /// Monotonic counter for the next recovery proposal id.
    NextRecoveryProposalId,
    // ── Audit log ─────────────────────────────────────────────────────────────
    /// Next write position in the audit ring buffer (0..MAX_AUDIT_LOG_SIZE).
    AuditLogHead,
    /// Total admin actions ever recorded (monotonic; not capped at ring size).
    AuditLogTotal,
    /// An audit log entry at ring-buffer position `n`.
    AuditEntry(u64),
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
    ///
    /// **Parameters:**
    /// - `admin` — The address that will become the protocol administrator.
    ///
    /// **Errors:**
    /// - `AccessControlError::AlreadyInitialized` — Contract has already been initialized.
    /// - `AccessControlError::InvalidAddress` — `admin` is the contract's own address.
    ///
    /// **Security:** No auth required on first call (contract is uninitialized). Subsequent
    /// calls revert immediately, preventing privilege escalation.
    pub fn initialize(env: Env, admin: Address) -> Result<(), AccessControlError> {
        // Guard: prevent re-initialization
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(AccessControlError::AlreadyInitialized);
        }
        kora_shared::validation::require_not_self(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Admin, &admin);
        Self::bump_persistent(&env, &DataKey::Admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage()
            .persistent()
            .set(&DataKey::Role(admin.clone()), &Role::Admin);
        Self::bump_persistent(&env, &DataKey::Role(admin));
        Ok(())
    }

    // ── Pause / Unpause ───────────────────────────────────────────────────────

    /// Pause the entire protocol. Admin only. Fails if already paused.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    ///
    /// **Errors:**
    /// - `AccessControlError::Unauthorized` / `AccessControlError::NotAdmin` — Caller is not the admin.
    /// - `AccessControlError::AlreadyPaused` — Protocol is already in the paused state.
    /// - `AccessControlError::Reentrancy` — Reentrancy guard triggered (should never happen in normal flow).
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits `protocol_paused` event.
    pub fn pause(env: Env, admin: Address) -> Result<(), AccessControlError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if env
            .storage()
            .instance()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
        {
            return Err(AccessControlError::AlreadyPaused);
        }
        let _guard = ReentrancyGuard::new(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        events::protocol_paused(&env, &admin);
        Self::append_audit_entry(&env, &admin, AdminActionType::Pause, Bytes::new(&env));
        Ok(())
    }

    /// Unpause the protocol. Admin only. Fails if not currently paused.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    ///
    /// **Errors:**
    /// - `AccessControlError::Unauthorized` / `AccessControlError::NotAdmin` — Caller is not the admin.
    /// - `AccessControlError::NotPaused` — Protocol is not currently paused.
    /// - `AccessControlError::Reentrancy` — Reentrancy guard triggered.
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits `protocol_unpaused` event.
    pub fn unpause(env: Env, admin: Address) -> Result<(), AccessControlError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if !env
            .storage()
            .instance()
            .get::<_, bool>(&DataKey::Paused)
            .unwrap_or(false)
        {
            return Err(AccessControlError::NotPaused);
        }
        let _guard = ReentrancyGuard::new(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        events::protocol_unpaused(&env, &admin);
        Self::append_audit_entry(&env, &admin, AdminActionType::Unpause, Bytes::new(&env));
        Ok(())
    }

    // ── Role management ───────────────────────────────────────────────────────

    /// Assign a role to an address. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `target` — The address to assign the role to.
    /// - `role` — The `Role` to assign (`Operator` or `Verifier`).
    ///
    /// **Errors:**
    /// - `AccessControlError::NotAdmin` — Caller is not the admin.
    /// - `AccessControlError::Unauthorized` — Attempt to grant `Role::Admin` (use `transfer_admin`),
    ///   grant `Role::None` (use `revoke_role`), or grant a role to the current admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. Cannot grant `Role::Admin` directly —
    /// use `transfer_admin` instead. Cannot grant `Role::None` — use `revoke_role` instead.
    /// - Cannot grant `Role::Admin` (use `transfer_admin`).
    /// - Cannot grant `Role::None` (use `revoke_role`).
    /// - Cannot grant a role to the current admin address.
    pub fn grant_role(
        env: Env,
        admin: Address,
        target: Address,
        role: Role,
    ) -> Result<(), AccessControlError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if role == Role::Admin {
            return Err(AccessControlError::Unauthorized);
        }
        if role == Role::None {
            return Err(AccessControlError::Unauthorized);
        }
        Self::validate_grant_role_target(&env, &target, &admin)?;
        env.storage()
            .persistent()
            .set(&DataKey::Role(target.clone()), &role);
        Self::bump_persistent(&env, &DataKey::Role(target.clone()));
        Self::add_to_role_members(&env, &role, &target);
        events::role_granted(&env, &admin, &target);
        let details = (&target, &role).into_val(&env);
        Self::append_audit_entry(&env, &admin, AdminActionType::GrantRole, details);
        Ok(())
    }

    /// Revoke a role from an address. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `target` — The address whose role should be removed.
    ///
    /// **Errors:**
    /// - `AccessControlError::NotAdmin` — Caller is not the admin.
    /// - `AccessControlError::Unauthorized` — Attempt to revoke the admin's own role.
    /// - `AccessControlError::RoleNotAssigned` — Target has no role assigned.
    ///
    /// **Security:** Requires `admin.require_auth()`. Uses `remove()` to reclaim storage
    /// rather than writing `Role::None`.
    /// - Cannot revoke the admin's own role.
    /// - Fails if the target has no role assigned.
    pub fn revoke_role(env: Env, admin: Address, target: Address) -> Result<(), AccessControlError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let current_role = env
            .storage()
            .persistent()
            .get::<_, Role>(&DataKey::Role(target.clone()))
            .unwrap_or(Role::None);

        if current_role == Role::Admin {
            return Err(AccessControlError::Unauthorized);
        }
        if current_role == Role::None {
            return Err(AccessControlError::RoleNotAssigned);
        }
        // Use remove() to reclaim storage rather than writing Role::None
        env.storage()
            .persistent()
            .remove(&DataKey::Role(target.clone()));
        Self::remove_from_role_members(&env, &current_role, &target);
        events::role_revoked(&env, &admin, &target);
        let details = (&target, &current_role).into_val(&env);
        Self::append_audit_entry(&env, &admin, AdminActionType::RevokeRole, details);
        Ok(())
    }

    // ── Admin transfer ────────────────────────────────────────────────────────

    /// Transfer admin to a new address. Current admin must sign.
    ///
    /// **Parameters:**
    /// - `current_admin` — The current admin address.
    /// - `new_admin` — The address to transfer admin rights to.
    ///
    /// **Errors:**
    /// - `AccessControlError::NotAdmin` — Caller is not the current admin.
    /// - `AccessControlError::InvalidAddress` — `new_admin` equals `current_admin` or is the contract itself.
    /// - `AccessControlError::Unauthorized` — `new_admin` already holds an `Operator` or `Verifier` role.
    ///   The caller must revoke that role first.
    ///
    /// **Security:** Requires `current_admin.require_auth()`. Prevents silent role overwrites
    /// by rejecting addresses that already hold a non-None, non-Admin role.
    /// - Cannot transfer to self.
    /// - Cannot transfer to an address that already holds a non-None role
    ///   (would silently overwrite it). The caller must revoke first.
    pub fn transfer_admin(
        env: Env,
        current_admin: Address,
        new_admin: Address,
    ) -> Result<(), AccessControlError> {
        current_admin.require_auth();
        Self::require_admin(&env, &current_admin)?;

        Self::validate_transfer_admin_target(&env, &new_admin, &current_admin)?;

        env.storage().persistent().set(&DataKey::Admin, &new_admin);
        Self::bump_persistent(&env, &DataKey::Admin);
        env.storage()
            .persistent()
            .set(&DataKey::Role(new_admin.clone()), &Role::Admin);
        Self::bump_persistent(&env, &DataKey::Role(new_admin.clone()));
        // Remove old admin's role entry to reclaim storage
        env.storage()
            .persistent()
            .remove(&DataKey::Role(current_admin.clone()));
        events::admin_transferred(&env, &current_admin, &new_admin);
        let details = new_admin.into_val(&env);
        Self::append_audit_entry(&env, &current_admin, AdminActionType::TransferAdmin, details);
        Ok(())
    }

    // ── Multisig ──────────────────────────────────────────────────────────────

    /// Configure the N-of-M multisig. Admin only. Once configured, admin
    /// actions must go through propose → approve → execute.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `signers` — The set of authorized signer addresses (M).
    /// - `threshold` — The minimum number of approvals required to execute (N).
    ///
    /// **Errors:**
    /// - `AccessControlError::NotAdmin` — Caller is not the admin.
    /// - `AccessControlError::InvalidThreshold` — `threshold` is 0 or greater than the number of signers.
    ///
    /// **Security:** Requires `admin.require_auth()`. Once this is called, sensitive admin actions
    /// (pause, role management, admin transfer) must go through the multisig proposal flow.
    pub fn configure_multisig(
        env: Env,
        admin: Address,
        signers: Vec<Address>,
        threshold: u32,
    ) -> Result<(), AccessControlError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let signer_count = signers.len();
        if threshold == 0 || threshold > signer_count {
            return Err(AccessControlError::InvalidThreshold);
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
        let details = (threshold, signer_count as u32).into_val(&env);
        Self::append_audit_entry(&env, &admin, AdminActionType::ConfigureMultisig, details);
        Ok(())
    }

    /// Propose a new admin action. Caller must be a signer.
    ///
    /// **Parameters:**
    /// - `proposer` — A configured multisig signer address.
    /// - `action` — The `AdminAction` to propose (Pause, Unpause, GrantRole, RevokeRole, TransferAdmin).
    ///
    /// **Returns:** The ID of the new proposal.
    ///
    /// **Errors:**
    /// - `AccessControlError::NotMultisigSigner` — Caller is not a configured signer.
    /// - `AccessControlError::ArithmeticOverflow` — Proposal ID counter overflowed (extremely unlikely).
    ///
    /// **Security:** Requires `proposer.require_auth()`. Proposer's vote is recorded automatically.
    /// Proposals expire after ~7 days (`PROPOSAL_TTL_LEDGERS`).
    pub fn propose_action(
        env: Env,
        proposer: Address,
        action: AdminAction,
    ) -> Result<u64, AccessControlError> {
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
                .ok_or(AccessControlError::ArithmeticOverflow)?),
        );

        events::action_proposed(&env, proposal_id, &proposer);
        Ok(proposal_id)
    }

    /// Approve an existing proposal. Caller must be a signer who hasn't
    /// already approved this proposal.
    ///
    /// **Parameters:**
    /// - `approver` — A configured multisig signer address.
    /// - `proposal_id` — The ID of the proposal to approve.
    ///
    /// **Errors:**
    /// - `AccessControlError::NotMultisigSigner` — Caller is not a configured signer.
    /// - `AccessControlError::ProposalNotFound` — No proposal exists with the given ID.
    /// - `AccessControlError::ProposalAlreadyExecuted` — Proposal has already been executed.
    /// - `AccessControlError::ProposalExpired` — Proposal's TTL has elapsed.
    /// - `AccessControlError::AlreadyApproved` — Caller has already voted on this proposal.
    ///
    /// **Security:** Requires `approver.require_auth()`. Each signer may only vote once per proposal.
    pub fn approve_action(env: Env, approver: Address, proposal_id: u64) -> Result<(), AccessControlError> {
        approver.require_auth();
        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &approver)?;

        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .ok_or(AccessControlError::ProposalNotFound)?;

        if proposal.executed {
            return Err(AccessControlError::ProposalAlreadyExecuted);
        }
        if env.ledger().timestamp() > proposal.expires_at {
            return Err(AccessControlError::ProposalExpired);
        }

        for i in 0..proposal.approvals.len() {
            if proposal.approvals.get(i).ok_or(AccessControlError::Unauthorized)? == approver {
                return Err(AccessControlError::AlreadyApproved);
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
    ///
    /// **Parameters:**
    /// - `executor` — A configured multisig signer address.
    /// - `proposal_id` — The ID of the proposal to execute.
    ///
    /// **Errors:**
    /// - `AccessControlError::NotMultisigSigner` — Caller is not a configured signer.
    /// - `AccessControlError::ProposalNotFound` — No proposal exists with the given ID.
    /// - `AccessControlError::ProposalAlreadyExecuted` — Proposal has already been executed.
    /// - `AccessControlError::ProposalExpired` — Proposal's TTL has elapsed.
    /// - `AccessControlError::ThresholdNotMet` — Not enough approvals have been collected yet.
    ///
    /// **Security:** Requires `executor.require_auth()`. Once executed, the proposal is marked
    /// as executed and cannot be re-executed. The proposal's action is applied atomically.
    pub fn execute_action(env: Env, executor: Address, proposal_id: u64) -> Result<(), AccessControlError> {
        executor.require_auth();
        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &executor)?;

        let mut proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .ok_or(AccessControlError::ProposalNotFound)?;

        if proposal.executed {
            return Err(AccessControlError::ProposalAlreadyExecuted);
        }
        if env.ledger().timestamp() > proposal.expires_at {
            return Err(AccessControlError::ProposalExpired);
        }
        if proposal.approvals.len() < config.threshold {
            return Err(AccessControlError::ThresholdNotMet);
        }

        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(proposal_id), &proposal);

        let current_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(AccessControlError::NotInitialized)?;

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
                    _ => return Err(AccessControlError::Unauthorized),
                };
                Self::validate_grant_role_target(&env, &target, &current_admin)?;
                env.storage()
                    .persistent()
                    .set(&DataKey::Role(target.clone()), &role);
                Self::bump_persistent(&env, &DataKey::Role(target.clone()));
                Self::add_to_role_members(&env, &role, &target);
                events::role_granted(&env, &executor, &target);
            }
            AdminAction::RevokeRole(target) => {
                let current_role = env
                    .storage()
                    .persistent()
                    .get::<_, Role>(&DataKey::Role(target.clone()))
                    .unwrap_or(Role::None);
                env.storage()
                    .persistent()
                    .remove(&DataKey::Role(target.clone()));
                if current_role != Role::None {
                    Self::remove_from_role_members(&env, &current_role, &target);
                }
                events::role_revoked(&env, &executor, &target);
            }
            AdminAction::TransferAdmin(new_admin) => {
                Self::validate_transfer_admin_target(&env, &new_admin, &current_admin)?;
                env.storage().persistent().set(&DataKey::Admin, &new_admin);
                Self::bump_persistent(&env, &DataKey::Admin);
                env.storage()
                    .persistent()
                    .set(&DataKey::Role(new_admin.clone()), &Role::Admin);
                Self::bump_persistent(&env, &DataKey::Role(new_admin.clone()));
                events::admin_transferred(&env, &executor, &new_admin);
            }
        }

        events::action_executed(&env, proposal_id, &executor);
        let details = proposal_id.into_val(&env);
        Self::append_audit_entry(&env, &executor, AdminActionType::MultisigExecuteAction, details);
        Ok(())
    }

    /// Get a proposal by ID.
    ///
    /// **Parameters:**
    /// - `proposal_id` — The ID of the proposal to retrieve.
    ///
    /// **Returns:** The full `Proposal` struct, or `AccessControlError::ProposalNotFound`.
    ///
    /// **Security:** Read-only view with no authorization check.
    pub fn get_proposal(env: Env, proposal_id: u64) -> Result<Proposal, AccessControlError> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
            .ok_or(AccessControlError::ProposalNotFound)
    }

    /// Get the current multisig configuration.
    ///
    /// **Returns:** The `MultisigConfig` (threshold + signer set), or
    /// `AccessControlError::MultisigNotConfigured` if multisig has not been set up.
    ///
    /// **Security:** Read-only view with no authorization check.
    pub fn get_multisig_config(env: Env) -> Result<MultisigConfig, AccessControlError> {
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
    ) -> Result<u64, AccessControlError> {
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
                .ok_or(AccessControlError::ArithmeticOverflow)?),
        );

        events::action_proposed(&env, proposal_id, &proposer);
        let details = (proposal.key, proposal.new_value).into_val(&env);
        Self::append_audit_entry(&env, &proposer, AdminActionType::ProposeParameter, details);
        Ok(proposal_id)
    }

    /// Vote in favour of a pending parameter-change proposal. Multisig signers only.
    ///
    /// **Parameters:**
    /// - `signer` — A configured multisig signer address.
    /// - `proposal_id` — The ID of the parameter-change proposal.
    ///
    /// **Errors:**
    /// - `AccessControlError::NotMultisigSigner` — Caller is not a configured signer.
    /// - `AccessControlError::ParameterProposalNotFound` — No proposal exists with the given ID.
    /// - `AccessControlError::ParameterProposalAlreadyExecuted` — Proposal already executed.
    /// - `AccessControlError::AlreadyVoted` — Caller has already cast their vote.
    ///
    /// **Security:** Requires `signer.require_auth()`. Each signer may only vote once.
    pub fn vote_parameter_change(
        env: Env,
        signer: Address,
        proposal_id: u64,
    ) -> Result<(), AccessControlError> {
        signer.require_auth();

        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &signer)?;

        let mut proposal: ParameterProposal = env
            .storage()
            .persistent()
            .get(&DataKey::ParameterProposal(proposal_id))
            .ok_or(AccessControlError::ParameterProposalNotFound)?;

        if proposal.executed {
            return Err(AccessControlError::ParameterProposalAlreadyExecuted);
        }
        for i in 0..proposal.approvals.len() {
            if proposal.approvals.get(i).unwrap() == signer {
                return Err(AccessControlError::AlreadyVoted);
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
    ) -> Result<(), AccessControlError> {
        caller.require_auth();

        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &caller)?;

        let mut proposal: ParameterProposal = env
            .storage()
            .persistent()
            .get(&DataKey::ParameterProposal(proposal_id))
            .ok_or(AccessControlError::ParameterProposalNotFound)?;

        if proposal.executed {
            return Err(AccessControlError::ParameterProposalAlreadyExecuted);
        }
        if proposal.approvals.len() < config.threshold {
            return Err(AccessControlError::GovernanceThresholdNotMet);
        }
        if env.ledger().timestamp() < proposal.created_at + GOVERNANCE_TIMELOCK_DELAY {
            return Err(AccessControlError::GovernanceTimelockNotElapsed);
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
        let details = (proposal.key, proposal.new_value).into_val(&env);
        Self::append_audit_entry(&env, &caller, AdminActionType::ExecuteParameter, details);
        Ok(())
    }

    /// Read the current governed value of a parameter, if one has been executed.
    ///
    /// **Parameters:**
    /// - `key` — The `ParameterKey` to look up (`FeeBps`, `LatePenaltyBps`, or `MaxRiskScore`).
    ///
    /// **Returns:** `Some(value)` if a governance proposal for this key has been executed,
    /// `None` otherwise (callers should fall back to the contract's own initialized default).
    ///
    /// **Security:** Read-only view with no authorization check.
    pub fn get_parameter(env: Env, key: ParameterKey) -> Option<u32> {
        env.storage().persistent().get(&DataKey::Parameter(key))
    }

    /// Read a parameter-change proposal by id.
    ///
    /// **Parameters:**
    /// - `proposal_id` — The ID of the parameter-change proposal.
    ///
    /// **Returns:** The full `ParameterProposal` struct, or `AccessControlError::ParameterProposalNotFound`.
    ///
    /// **Security:** Read-only view with no authorization check.
    pub fn get_parameter_proposal(
        env: Env,
        proposal_id: u64,
    ) -> Result<ParameterProposal, AccessControlError> {
        env.storage()
            .persistent()
            .get(&DataKey::ParameterProposal(proposal_id))
            .ok_or(AccessControlError::ParameterProposalNotFound)
    }

    // ── Signer Recovery ────────────────────────────────────────────────────────

    /// Propose a multisig signer recovery after a long timelock.
    /// Any signer can initiate recovery if quorum becomes unreachable.
    /// Execution requires the recovery timelock (~30 days) to elapse without objections.
    pub fn propose_signer_recovery(
        env: Env,
        proposer: Address,
        new_signers: Vec<Address>,
        new_threshold: u32,
    ) -> Result<u64, AccessControlError> {
        proposer.require_auth();
        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &proposer)?;

        if new_threshold == 0 || new_threshold > new_signers.len() {
            return Err(AccessControlError::InvalidThreshold);
        }

        let proposal_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::NextRecoveryProposalId)
            .unwrap_or(1);

        let proposal = RecoveryProposal {
            id: proposal_id,
            proposer: proposer.clone(),
            new_signers,
            new_threshold,
            created_at: env.ledger().timestamp(),
            objections: Vec::new(&env),
            executed: false,
        };

        env.storage()
            .persistent()
            .set(&DataKey::RecoveryProposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::RecoveryProposal(proposal_id));
        env.storage().persistent().set(
            &DataKey::NextRecoveryProposalId,
            &(proposal_id
                .checked_add(1)
                .ok_or(AccessControlError::ArithmeticOverflow)?),
        );

        events::action_proposed(&env, proposal_id, &proposer);
        let details = (new_threshold, new_signers.len() as u32).into_val(&env);
        Self::append_audit_entry(&env, &proposer, AdminActionType::ProposeParameter, details);
        Ok(proposal_id)
    }

    /// Object to a pending signer recovery. Prevents execution if any signer objects.
    pub fn object_signer_recovery(
        env: Env,
        objector: Address,
        proposal_id: u64,
    ) -> Result<(), AccessControlError> {
        objector.require_auth();
        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &objector)?;

        let mut proposal: RecoveryProposal = env
            .storage()
            .persistent()
            .get(&DataKey::RecoveryProposal(proposal_id))
            .ok_or(AccessControlError::ProposalNotFound)?;

        if proposal.executed {
            return Err(AccessControlError::ProposalAlreadyExecuted);
        }

        for i in 0..proposal.objections.len() {
            if proposal.objections.get(i).ok_or(AccessControlError::Unauthorized)? == objector {
                return Err(AccessControlError::AlreadyApproved);
            }
        }

        proposal.objections.push_back(objector.clone());
        env.storage()
            .persistent()
            .set(&DataKey::RecoveryProposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::RecoveryProposal(proposal_id));

        events::action_approved(&env, proposal_id, &objector, proposal.objections.len());
        Ok(())
    }

    /// Execute a signer recovery after the long timelock and with no objections from current signers.
    pub fn execute_signer_recovery(
        env: Env,
        executor: Address,
        proposal_id: u64,
    ) -> Result<(), AccessControlError> {
        executor.require_auth();
        let config = Self::load_multisig_config(&env)?;
        Self::require_signer(&config, &executor)?;

        let mut proposal: RecoveryProposal = env
            .storage()
            .persistent()
            .get(&DataKey::RecoveryProposal(proposal_id))
            .ok_or(AccessControlError::ProposalNotFound)?;

        if proposal.executed {
            return Err(AccessControlError::ProposalAlreadyExecuted);
        }

        if env.ledger().timestamp() < proposal.created_at + RECOVERY_TIMELOCK_DELAY {
            return Err(AccessControlError::GovernanceTimelockNotElapsed);
        }

        if !proposal.objections.is_empty() {
            return Err(AccessControlError::AlreadyApproved);
        }

        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::RecoveryProposal(proposal_id), &proposal);

        let new_config = MultisigConfig {
            threshold: proposal.new_threshold,
            signers: proposal.new_signers.clone(),
        };
        env.storage()
            .persistent()
            .set(&DataKey::MultisigConfig, &new_config);
        Self::bump_persistent(&env, &DataKey::MultisigConfig);

        events::action_executed(&env, proposal_id, &executor);
        let details = (proposal.new_threshold, proposal.new_signers.len() as u32).into_val(&env);
        Self::append_audit_entry(&env, &executor, AdminActionType::ConfigureMultisig, details);
        Ok(())
    }

    /// Get a recovery proposal by ID.
    pub fn get_recovery_proposal(
        env: Env,
        proposal_id: u64,
    ) -> Result<RecoveryProposal, AccessControlError> {
        env.storage()
            .persistent()
            .get(&DataKey::RecoveryProposal(proposal_id))
            .ok_or(AccessControlError::ProposalNotFound)
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    /// Returns `true` if the protocol is currently paused.
    ///
    /// **Security:** Read-only view. No authorization required. Other contracts should call
    /// this before performing any state-mutating operation.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Returns the role assigned to `address`, or `Role::None` if unassigned.
    ///
    /// **Parameters:**
    /// - `address` — The address to query.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_role(env: Env, address: Address) -> Role {
        let key = DataKey::Role(address.clone());
        if let Some(role) = env.storage().persistent().get::<_, Role>(&key) {
            return role;
        }
        // Defensive fallback: if the per-address role entry is missing (e.g. it was
        // never written, or was pruned/removed out-of-band) but `address` is the
        // current admin, still report `Role::Admin` rather than `Role::None`.
        if let Some(admin) = env.storage().persistent().get::<_, Address>(&DataKey::Admin) {
            if admin == address {
                return Role::Admin;
            }
        }
        Role::None
    }

    /// Returns `true` if `address` holds the given `role`.
    ///
    /// **Parameters:**
    /// - `address` — The address to check.
    /// - `role` — The `Role` to test for.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn has_role(env: Env, address: Address, role: Role) -> bool {
        let assigned: Role = env
            .storage()
            .persistent()
            .get(&DataKey::Role(address))
            .unwrap_or(Role::None);
        assigned == role
    }

    /// Returns the current admin address.
    ///
    /// **Errors:**
    /// - `AccessControlError::NotInitialized` — Contract has not been initialized yet.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_admin(env: Env) -> Result<Address, AccessControlError> {
        env.storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(AccessControlError::NotInitialized)
    }

    /// Return a page of addresses holding a given role.
    /// `page` is 0-indexed; `page_size` is clamped to 1–50.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_role_members(env: Env, role: Role, page: u32, page_size: u32) -> Vec<Address> {
        let page_size = (page_size.max(1).min(50)) as usize;
        let skip = (page as usize).saturating_mul(page_size);
        let mut results = Vec::new(&env);

        if let Some(members) = env.storage().persistent().get::<_, Vec<Address>>(&DataKey::RoleMembers(role)) {
            let mut i = 0;
            for j in skip..members.len() {
                if i >= page_size {
                    break;
                }
                if let Ok(addr) = members.get(j as u32) {
                    results.push_back(addr);
                }
                i += 1;
            }
        }
        results
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    /// Propose a WASM upgrade. Admin only. Begins a 24-hour timelock.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `new_wasm_hash` — The SHA-256 hash of the new WASM binary (32 bytes).
    ///
    /// **Errors:**
    /// - `AccessControlError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. The upgrade cannot be applied until
    /// `UPGRADE_TIMELOCK_DELAY` (24 h) has elapsed via `execute_upgrade`.
    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), AccessControlError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(
            &DataKey::UpgradeProposal,
            &(new_wasm_hash.clone(), env.ledger().timestamp()),
        );
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        let details = new_wasm_hash.clone().into_val(&env);
        Self::append_audit_entry(&env, &admin, AdminActionType::ProposeUpgrade, details);
        Ok(())
    }

    /// Execute a previously proposed WASM upgrade after the 24-hour timelock has elapsed.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    ///
    /// **Errors:**
    /// - `AccessControlError::NotAdmin` — Caller is not the admin.
    /// - `AccessControlError::NoUpgradeProposed` — No upgrade proposal is pending.
    /// - `AccessControlError::UpgradeTimelockNotElapsed` — 24-hour timelock has not yet passed.
    ///
    /// **Security:** Requires `admin.require_auth()`. Clears the proposal before calling
    /// `update_current_contract_wasm` to prevent re-execution.
    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), AccessControlError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(AccessControlError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(AccessControlError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        events::upgrade_executed(&env, &admin, &wasm_hash);
        let details = wasm_hash.clone().into_val(&env);
        Self::append_audit_entry(&env, &admin, AdminActionType::ExecuteUpgrade, details);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Audit Log ─────────────────────────────────────────────────────────────

    /// Return a page of audit log entries, newest first.
    /// `page` is 0-indexed; `page_size` is clamped to 1–50.
    pub fn get_audit_log(env: Env, page: u32, page_size: u32) -> Vec<AdminAuditEntry> {
        let page_size = (page_size.max(1).min(50)) as u64;
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditLogTotal)
            .unwrap_or(0);
        let head: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditLogHead)
            .unwrap_or(0);
        let stored = total.min(MAX_AUDIT_LOG_SIZE);

        let skip = (page as u64).saturating_mul(page_size);
        let mut results = Vec::new(&env);

        let mut i: u64 = 0;
        while i < page_size {
            let offset = skip + i;
            if offset >= stored {
                break;
            }
            // Walk backwards from the most recently written slot.
            let pos = (head + MAX_AUDIT_LOG_SIZE - 1 - offset) % MAX_AUDIT_LOG_SIZE;
            if let Some(entry) = env
                .storage()
                .persistent()
                .get::<DataKey, AdminAuditEntry>(&DataKey::AuditEntry(pos))
            {
                results.push_back(entry);
            }
            i += 1;
        }

        results
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Validate grant_role target: reject if target == admin.
    fn validate_grant_role_target(env: &Env, target: &Address, admin: &Address) -> Result<(), AccessControlError> {
        if target == admin {
            return Err(AccessControlError::Unauthorized);
        }
        Ok(())
    }

    /// Validate transfer_admin target: reject self-transfer and existing non-None/non-Admin roles.
    fn validate_transfer_admin_target(env: &Env, new_admin: &Address, current_admin: &Address) -> Result<(), AccessControlError> {
        if current_admin == new_admin {
            return Err(AccessControlError::InvalidAddress);
        }
        kora_shared::validation::require_not_self(env, new_admin)?;

        let existing = env
            .storage()
            .persistent()
            .get::<_, Role>(&DataKey::Role(new_admin.clone()))
            .unwrap_or(Role::None);
        if existing != Role::None && existing != Role::Admin {
            return Err(AccessControlError::Unauthorized);
        }
        Ok(())
    }

    /// Add an address to the role members registry.
    fn add_to_role_members(env: &Env, role: &Role, address: &Address) {
        let key = DataKey::RoleMembers(role.clone());
        let mut members: Vec<Address> = env.storage().persistent().get(&key).unwrap_or(Vec::new(env));
        // Check if already present to avoid duplicates
        let mut found = false;
        for i in 0..members.len() {
            if members.get(i).ok_or(AccessControlError::Unauthorized).unwrap() == address {
                found = true;
                break;
            }
        }
        if !found {
            members.push_back(address.clone());
            env.storage().persistent().set(&key, &members);
            Self::bump_persistent(env, &key);
        }
    }

    /// Remove an address from the role members registry.
    fn remove_from_role_members(env: &Env, role: &Role, address: &Address) {
        let key = DataKey::RoleMembers(role.clone());
        if let Some(mut members) = env.storage().persistent().get::<_, Vec<Address>>(&key) {
            let mut found = false;
            for i in 0..members.len() {
                if members.get(i).ok_or(AccessControlError::Unauthorized).unwrap() == address {
                    // Swap with last and pop to remove efficiently
                    let last = members.pop_back();
                    if i < members.len() {
                        members.set(i, last.unwrap());
                    }
                    found = true;
                    break;
                }
            }
            if found {
                if members.is_empty() {
                    env.storage().persistent().remove(&key);
                } else {
                    env.storage().persistent().set(&key, &members);
                    Self::bump_persistent(env, &key);
                }
            }
        }
    }

    /// Append one entry to the ring-buffer audit log and emit the canonical event.
    fn append_audit_entry(env: &Env, actor: &Address, action: AdminActionType, details: soroban_sdk::Bytes) {
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditLogTotal)
            .unwrap_or(0);
        let head: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditLogHead)
            .unwrap_or(0);

        let entry = AdminAuditEntry {
            sequence: total,
            timestamp: env.ledger().timestamp(),
            actor: actor.clone(),
            action,
            source: AuditSource::AccessControl,
            details,
        };

        env.storage()
            .persistent()
            .set(&DataKey::AuditEntry(head), &entry);
        Self::bump_persistent(env, &DataKey::AuditEntry(head));

        events::admin_action_audited(env, &entry);

        let next_head = (head + 1) % MAX_AUDIT_LOG_SIZE;
        env.storage()
            .instance()
            .set(&DataKey::AuditLogHead, &next_head);
        env.storage()
            .instance()
            .set(&DataKey::AuditLogTotal, &(total + 1));
    }

    /// Read the paused flag from persistent storage.
    fn read_paused(env: &Env) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), AccessControlError> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(AccessControlError::NotInitialized)?;
        if &admin != caller {
            return Err(AccessControlError::NotAdmin);
        }
        Ok(())
    }

    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_BUMP);
    }

    fn load_multisig_config(env: &Env) -> Result<MultisigConfig, AccessControlError> {
        env.storage()
            .persistent()
            .get(&DataKey::MultisigConfig)
            .ok_or(AccessControlError::MultisigNotConfigured)
    }

    fn require_signer(config: &MultisigConfig, caller: &Address) -> Result<(), AccessControlError> {
        for i in 0..config.signers.len() {
            if &config.signers.get(i).ok_or(AccessControlError::Unauthorized)? == caller {
                return Ok(());
            }
        }
        Err(AccessControlError::SignerNotFound)
    }

    /// Validate a proposed parameter value against its allowed range.
    fn require_valid_parameter(key: &ParameterKey, value: u32) -> Result<(), AccessControlError> {
        let ok = match key {
            ParameterKey::FeeBps | ParameterKey::LatePenaltyBps => value <= 10_000,
            ParameterKey::MaxRiskScore => value <= 100,
        };
        if ok {
            Ok(())
        } else {
            Err(AccessControlError::InvalidParameterValue)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::AlreadyInitialized);
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
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotAdmin);
    }

    #[test]
    fn test_pause_already_paused_returns_correct_error() {
        let (_, admin, client) = setup();
        client.pause(&admin);
        let result = client.try_pause(&admin);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::AlreadyPaused);
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
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotAdmin);
    }

    #[test]
    fn test_unpause_when_not_paused_returns_correct_error() {
        let (_, admin, client) = setup();
        let result = client.try_unpause(&admin);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotPaused);
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
                args: (&admin, &target, Role::Verifier).into_val(&env),
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
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotAdmin);
    }

    #[test]
    fn test_grant_role_admin_variant_returns_unauthorized() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let result = client.try_grant_role(&admin, &target, &Role::Admin);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::Unauthorized);
    }

    #[test]
    fn test_grant_role_none_variant_returns_unauthorized() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        let result = client.try_grant_role(&admin, &target, &Role::None);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::Unauthorized);
    }

    #[test]
    fn test_grant_role_to_self_returns_unauthorized() {
        let (_, admin, client) = setup();
        let result = client.try_grant_role(&admin, &admin, &Role::Operator);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::Unauthorized);
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
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotAdmin);
    }

    #[test]
    fn test_revoke_role_admin_returns_unauthorized() {
        let (_, admin, client) = setup();
        let result = client.try_revoke_role(&admin, &admin);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::Unauthorized);
    }

    #[test]
    fn test_revoke_role_not_assigned_returns_correct_error() {
        let (env, admin, client) = setup();
        let stranger = Address::generate(&env);
        let result = client.try_revoke_role(&admin, &stranger);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::RoleNotAssigned);
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
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::RoleNotAssigned);
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

    // ── transfer_admin ────────────────────────────────────────────────────────

    #[test]
    fn test_transfer_admin_success() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
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
                fn_name: "transfer_admin",
                args: (&admin, &new_admin).into_val(&env),
                sub_invokes: &[],
            },
        }]);
        assert!(client.try_transfer_admin(&admin, &new_admin).is_ok());
    }

    #[test]
    fn test_transfer_admin_non_admin_returns_not_admin() {
        let (env, _, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_transfer_admin(&stranger, &new_admin);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotAdmin);
    }

    #[test]
    fn test_transfer_admin_to_self_returns_invalid_address() {
        let (_, admin, client) = setup();
        let result = client.try_transfer_admin(&admin, &admin);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::InvalidAddress);
    }

    #[test]
    fn test_transfer_admin_to_operator_returns_unauthorized() {
        let (env, admin, client) = setup();
        let operator = Address::generate(&env);
        client.grant_role(&admin, &operator, &Role::Operator);
        let result = client.try_transfer_admin(&admin, &operator);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::Unauthorized);
    }

    #[test]
    fn test_transfer_admin_to_verifier_returns_unauthorized() {
        let (env, admin, client) = setup();
        let verifier = Address::generate(&env);
        client.grant_role(&admin, &verifier, &Role::Verifier);
        let result = client.try_transfer_admin(&admin, &verifier);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::Unauthorized);
    }

    #[test]
    fn test_transfer_admin_state_unchanged_after_failed_transfer() {
        // After a rejected transfer, original admin must still be admin
        let (env, admin, client) = setup();
        let _ = client.try_transfer_admin(&admin, &admin);
        assert_eq!(client.get_admin(), admin);
        assert_eq!(client.get_role(&admin), Role::Admin);
    }

    #[test]
    fn test_transfer_admin_old_admin_loses_all_privileges() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        // Old admin cannot pause
        assert!(client.try_pause(&admin).is_err());
        // Old admin cannot grant roles
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
        client.transfer_admin(&admin, &new_admin);
        // New admin can pause
        client.pause(&new_admin);
        assert!(client.is_paused());
        // New admin can unpause
        client.unpause(&new_admin);
        // New admin can grant roles
        let target = Address::generate(&env);
        client.grant_role(&new_admin, &target, &Role::Verifier);
        assert_eq!(client.get_role(&target), Role::Verifier);
    }

    #[test]
    fn test_transfer_admin_chain_a_to_b_to_c() {
        let (env, admin_a, client) = setup();
        let admin_b = Address::generate(&env);
        let admin_c = Address::generate(&env);
        client.transfer_admin(&admin_a, &admin_b);
        assert_eq!(client.get_admin(), admin_b);
        client.transfer_admin(&admin_b, &admin_c);
        assert_eq!(client.get_admin(), admin_c);
        assert_eq!(client.get_role(&admin_a), Role::None);
        assert_eq!(client.get_role(&admin_b), Role::None);
        assert_eq!(client.get_role(&admin_c), Role::Admin);
    }

    #[test]
    fn test_transfer_admin_to_clean_address_succeeds() {
        // Transfer to an address with no prior role must succeed
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        assert_eq!(client.get_role(&new_admin), Role::None);
        assert!(client.try_transfer_admin(&admin, &new_admin).is_ok());
    }

    // ── get_admin ─────────────────────────────────────────────────────────────

    #[test]
    fn test_pause_before_init_returns_not_initialized() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        let result = client.try_pause(&admin);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotInitialized);
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
        let result = client.try_transfer_admin(&admin, &contract_id);
        assert!(result.is_err());
    }
    #[test]
    fn test_grant_role_before_init_returns_not_initialized() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_grant_role(&admin, &target, &Role::Verifier);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotInitialized);
    }

    #[test]
    fn test_revoke_role_before_init_returns_not_initialized() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_revoke_role(&admin, &target);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotInitialized);
    }

    #[test]
    fn test_transfer_admin_before_init_returns_not_initialized() {
        let (env, client) = deploy_uninit();
        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_transfer_admin(&admin, &new_admin);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotInitialized);
    }

    #[test]
    fn test_get_role_falls_back_to_admin_when_role_key_missing() {
        let (env, admin, client) = setup();
        env.as_contract(&client.address, || {
            env.storage()
                .persistent()
                .remove(&DataKey::Role(admin.clone()));
        });
        assert_eq!(client.get_role(&admin), Role::Admin);
    }

    #[test]
    fn test_get_admin_before_init_returns_not_initialized() {
        let (_, client) = deploy_uninit();
        let result = client.try_get_admin();
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::NotInitialized);
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
        assert!(client.try_transfer_admin(&admin, &user).is_ok());
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

    // ── Admin transfer ────────────────────────────────────────────────────────

    #[test]
    fn test_transfer_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);

        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.get_admin(), new_admin);
        assert_eq!(client.get_role(&new_admin), Role::Admin);
        assert_eq!(client.get_role(&admin), Role::None);
    }

    #[test]
    fn test_transfer_admin_self_rejected() {
        let (_, admin, client) = setup();
        assert!(client.try_transfer_admin(&admin, &admin).is_err());
    }

    #[test]
    fn test_non_admin_cannot_transfer_admin() {
        let (env, _admin, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        assert!(client.try_transfer_admin(&stranger, &new_admin).is_err());
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

    // ── Multisig validation tests ─────────────────────────────────────────────

    #[test]
    fn test_multisig_grant_role_to_admin_rejected() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let prop_id = client.propose_action(&signer1, AdminAction::GrantRole(admin.clone(), 1));
        client.approve_action(&signer2, prop_id);
        let result = client.try_execute_action(&signer1, prop_id);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::Unauthorized);
    }

    #[test]
    fn test_multisig_transfer_admin_to_self_rejected() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let prop_id = client.propose_action(&signer1, AdminAction::TransferAdmin(admin.clone()));
        client.approve_action(&signer2, prop_id);
        let result = client.try_execute_action(&signer1, prop_id);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::InvalidAddress);
    }

    #[test]
    fn test_multisig_transfer_admin_to_operator_rejected() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let operator = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers.clone(), 2);

        client.grant_role(&admin, &operator, &Role::Operator);
        assert_eq!(client.get_role(&operator), Role::Operator);

        let prop_id = client.propose_action(&signer1, AdminAction::TransferAdmin(operator.clone()));
        client.approve_action(&signer2, prop_id);
        let result = client.try_execute_action(&signer1, prop_id);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::Unauthorized);
    }

    #[test]
    fn test_multisig_grant_role_valid_succeeds() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let target = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let prop_id = client.propose_action(&signer1, AdminAction::GrantRole(target.clone(), 1));
        client.approve_action(&signer2, prop_id);
        assert!(client.try_execute_action(&signer1, prop_id).is_ok());
        assert_eq!(client.get_role(&target), Role::Operator);
    }

    #[test]
    fn test_multisig_transfer_admin_valid_succeeds() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let prop_id = client.propose_action(&signer1, AdminAction::TransferAdmin(new_admin.clone()));
        client.approve_action(&signer2, prop_id);
        assert!(client.try_execute_action(&signer1, prop_id).is_ok());
        assert_eq!(client.get_admin(), new_admin);
    }

    // ── Role registry tests ───────────────────────────────────────────────────

    #[test]
    fn test_get_role_members_empty_initially() {
        let (_, _, client) = setup();
        let members = client.get_role_members(&Role::Operator, 0, 50);
        assert_eq!(members.len(), 0);
    }

    #[test]
    fn test_get_role_members_after_grant() {
        let (env, admin, client) = setup();
        let op1 = Address::generate(&env);
        let op2 = Address::generate(&env);
        let verifier = Address::generate(&env);

        client.grant_role(&admin, &op1, &Role::Operator);
        client.grant_role(&admin, &op2, &Role::Operator);
        client.grant_role(&admin, &verifier, &Role::Verifier);

        let ops = client.get_role_members(&Role::Operator, 0, 50);
        assert_eq!(ops.len(), 2);

        let vers = client.get_role_members(&Role::Verifier, 0, 50);
        assert_eq!(vers.len(), 1);
    }

    #[test]
    fn test_get_role_members_pagination() {
        let (env, admin, client) = setup();
        for i in 0..5 {
            let addr = Address::generate(&env);
            client.grant_role(&admin, &addr, &Role::Operator);
        }

        let page0 = client.get_role_members(&Role::Operator, 0, 2);
        assert_eq!(page0.len(), 2);

        let page1 = client.get_role_members(&Role::Operator, 1, 2);
        assert_eq!(page1.len(), 2);

        let page2 = client.get_role_members(&Role::Operator, 2, 2);
        assert_eq!(page2.len(), 1);

        let page3 = client.get_role_members(&Role::Operator, 3, 2);
        assert_eq!(page3.len(), 0);
    }

    #[test]
    fn test_get_role_members_after_revoke() {
        let (env, admin, client) = setup();
        let op1 = Address::generate(&env);
        let op2 = Address::generate(&env);

        client.grant_role(&admin, &op1, &Role::Operator);
        client.grant_role(&admin, &op2, &Role::Operator);
        assert_eq!(client.get_role_members(&Role::Operator, 0, 50).len(), 2);

        client.revoke_role(&admin, &op1);
        let members = client.get_role_members(&Role::Operator, 0, 50);
        assert_eq!(members.len(), 1);
    }

    #[test]
    fn test_get_role_members_grant_revoke_regrant() {
        let (env, admin, client) = setup();
        let addr = Address::generate(&env);

        client.grant_role(&admin, &addr, &Role::Operator);
        assert_eq!(client.get_role_members(&Role::Operator, 0, 50).len(), 1);

        client.revoke_role(&admin, &addr);
        assert_eq!(client.get_role_members(&Role::Operator, 0, 50).len(), 0);

        client.grant_role(&admin, &addr, &Role::Verifier);
        let vers = client.get_role_members(&Role::Verifier, 0, 50);
        assert_eq!(vers.len(), 1);

        let ops = client.get_role_members(&Role::Operator, 0, 50);
        assert_eq!(ops.len(), 0);
    }

    #[test]
    fn test_get_role_members_multisig_grant() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let target = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let prop_id = client.propose_action(&signer1, AdminAction::GrantRole(target.clone(), 1));
        client.approve_action(&signer2, prop_id);
        client.execute_action(&signer1, prop_id);

        let members = client.get_role_members(&Role::Operator, 0, 50);
        assert_eq!(members.len(), 1);
    }

    #[test]
    fn test_get_role_members_multisig_revoke() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let target = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let prop_id = client.propose_action(&signer1, AdminAction::GrantRole(target.clone(), 1));
        client.approve_action(&signer2, prop_id);
        client.execute_action(&signer1, prop_id);
        assert_eq!(client.get_role_members(&Role::Operator, 0, 50).len(), 1);

        let prop_id2 = client.propose_action(&signer1, AdminAction::RevokeRole(target.clone()));
        client.approve_action(&signer2, prop_id2);
        client.execute_action(&signer1, prop_id2);

        assert_eq!(client.get_role_members(&Role::Operator, 0, 50).len(), 0);
    }

    // ── Audit payload tests ───────────────────────────────────────────────────

    #[test]
    fn test_audit_log_grant_role_has_payload() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        client.grant_role(&admin, &target, &Role::Operator);

        let entries = client.get_audit_log(0, 50);
        assert!(entries.len() > 0);
        let grant_entry = entries.get(0).unwrap();
        assert_eq!(grant_entry.action, AdminActionType::GrantRole);
        assert!(grant_entry.details.len() > 0, "Payload should be present");
    }

    #[test]
    fn test_audit_log_transfer_admin_has_payload() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);

        let entries = client.get_audit_log(0, 50);
        assert!(entries.len() > 0);
        let transfer_entry = entries.get(0).unwrap();
        assert_eq!(transfer_entry.action, AdminActionType::TransferAdmin);
        assert!(transfer_entry.details.len() > 0, "Payload should be present");
    }

    #[test]
    fn test_audit_log_revoke_role_has_payload() {
        let (env, admin, client) = setup();
        let target = Address::generate(&env);
        client.grant_role(&admin, &target, &Role::Verifier);
        client.revoke_role(&admin, &target);

        let entries = client.get_audit_log(0, 50);
        assert!(entries.len() > 0);
        let revoke_entry = entries.get(0).unwrap();
        assert_eq!(revoke_entry.action, AdminActionType::RevokeRole);
        assert!(revoke_entry.details.len() > 0, "Payload should be present");
    }

    #[test]
    fn test_audit_log_pause_has_payload() {
        let (_, admin, client) = setup();
        client.pause(&admin);

        let entries = client.get_audit_log(0, 50);
        assert!(entries.len() > 0);
        let pause_entry = entries.get(0).unwrap();
        assert_eq!(pause_entry.action, AdminActionType::Pause);
        // Pause has empty payload but still has the details field
        assert!(pause_entry.details.len() == 0);
    }

    // ── Signer recovery tests ─────────────────────────────────────────────────

    #[test]
    fn test_propose_signer_recovery_success() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let new_signer1 = Address::generate(&env);
        let new_signer2 = Address::generate(&env);
        let new_signers = vec![&env, new_signer1, new_signer2];

        let prop_id = client.propose_signer_recovery(&signer1, new_signers, 2);
        assert!(prop_id > 0);
        let proposal = client.get_recovery_proposal(prop_id).unwrap();
        assert_eq!(proposal.proposer, signer1);
        assert!(!proposal.executed);
        assert_eq!(proposal.objections.len(), 0);
    }

    #[test]
    fn test_object_signer_recovery_success() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let new_signer1 = Address::generate(&env);
        let new_signer2 = Address::generate(&env);
        let new_signers = vec![&env, new_signer1, new_signer2];

        let prop_id = client.propose_signer_recovery(&signer1, new_signers, 2);
        assert!(client.try_object_signer_recovery(&signer2, prop_id).is_ok());

        let proposal = client.get_recovery_proposal(prop_id).unwrap();
        assert_eq!(proposal.objections.len(), 1);
    }

    #[test]
    fn test_execute_signer_recovery_before_timelock_fails() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let new_signer1 = Address::generate(&env);
        let new_signer2 = Address::generate(&env);
        let new_signers = vec![&env, new_signer1, new_signer2];

        let prop_id = client.propose_signer_recovery(&signer1, new_signers, 2);
        let result = client.try_execute_signer_recovery(&signer1, prop_id);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::GovernanceTimelockNotElapsed);
    }

    #[test]
    fn test_execute_signer_recovery_fails_if_objections_exist() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let new_signer1 = Address::generate(&env);
        let new_signer2 = Address::generate(&env);
        let new_signers = vec![&env, new_signer1, new_signer2];

        let prop_id = client.propose_signer_recovery(&signer1, new_signers, 2);
        client.object_signer_recovery(&signer2, prop_id);

        let proposal = client.get_recovery_proposal(prop_id).unwrap();
        assert_eq!(proposal.objections.len(), 1);

        let result = client.try_execute_signer_recovery(&signer1, prop_id);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::AlreadyApproved);
    }

    #[test]
    fn test_propose_signer_recovery_invalid_threshold() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let new_signer1 = Address::generate(&env);
        let new_signers = vec![&env, new_signer1];

        let result = client.try_propose_signer_recovery(&signer1, new_signers.clone(), 2);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::InvalidThreshold);

        let result = client.try_propose_signer_recovery(&signer1, new_signers, 0);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::InvalidThreshold);
    }

    #[test]
    fn test_object_recovery_twice_fails() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let new_signer1 = Address::generate(&env);
        let new_signer2 = Address::generate(&env);
        let new_signers = vec![&env, new_signer1, new_signer2];

        let prop_id = client.propose_signer_recovery(&signer1, new_signers, 2);
        assert!(client.try_object_signer_recovery(&signer2, prop_id).is_ok());

        let result = client.try_object_signer_recovery(&signer2, prop_id);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::AlreadyApproved);
    }

    #[test]
    fn test_non_signer_cannot_propose_recovery() {
        let (env, admin, client) = setup();
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);
        let signers = vec![&env, signer1.clone(), signer2.clone()];
        client.configure_multisig(&admin, signers, 2);

        let stranger = Address::generate(&env);
        let new_signer1 = Address::generate(&env);
        let new_signer2 = Address::generate(&env);
        let new_signers = vec![&env, new_signer1, new_signer2];

        let result = client.try_propose_signer_recovery(&stranger, new_signers, 2);
        assert_eq!(result.unwrap_err().unwrap(), AccessControlError::SignerNotFound);
    }
}
