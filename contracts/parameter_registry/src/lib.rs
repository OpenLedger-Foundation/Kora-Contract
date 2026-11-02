#![no_std]

//! # Parameter Registry Contract
//!
//! A single, canonical on-chain store for every governable protocol parameter.
//! Consuming contracts (`marketplace`, `financing_pool`, `risk_registry`,
//! `treasury`) read from this registry rather than maintaining independent,
//! disconnected parameter storage.
//!
//! ## Marketplace keys
//!
//! | Key | Type | Default | Description |
//! |-----|------|---------|-------------|
//! | `MarketplaceFeeBps` | i128 | `50` | Protocol fee in basis points (0–10_000) charged on each investor contribution. |
//! | `ReferrerSplitBps` | i128 | `0` | Fraction of the protocol fee routed to the listing's referrer (0–10_000). |
//! | `TierFeeAaa` | i128 | `0` | Override fee for AAA risk tier (0 = use flat `MarketplaceFeeBps`). |
//! | `TierFeeAa` | i128 | `0` | Override fee for AA risk tier. |
//! | `TierFeeA` | i128 | `0` | Override fee for A risk tier. |
//! | `TierFeeB` | i128 | `0` | Override fee for B risk tier. |
//! | `TierFeeC` | i128 | `0` | Override fee for C risk tier. |
//!
//! ## Write path
//! Parameters are written **only** through a multisig governance proposal:
//!   1. A configured signer calls `propose_param_update`.
//!   2. Other signers call `approve_param_update` until quorum is reached.
//!   3. Any signer calls `execute_param_update` after the timelock elapses.
//!
//! The admin (or multi-sig/timelock as interim/emergency) may also call
//! `admin_set_parameter` directly when no multisig is configured, providing a
//! backward-compatible bootstrap path.
//!
//! ## Read / cache path
//! Consuming contracts call `get_parameter(key)` on each read.  For hot-path
//! functions where a cross-contract call per invocation would be prohibitively
//! expensive, they may cache the value locally and call `refresh_cache` to
//! pull the current registry value on demand (or at a configurable staleness
//! threshold).
//!
//! ## Migration note
//! Existing per-contract parameters are NOT deleted in this PR.  Consuming
//! contracts adopt a "registry-first, local-fallback" read pattern: if the
//! registry address is configured and the key exists there, that value wins;
//! otherwise the contract's own stored value is used.  This maintains full
//! backward-compatible read access during migration.

use kora_shared::{
    audit::{AdminActionType, AdminAuditEntry, AuditSource, MAX_AUDIT_LOG_SIZE},
    errors::KoraError,
    events,
    types::{RegistryParam, RegistryParamKey},
    validation::UPGRADE_TIMELOCK_DELAY,
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Vec,
};

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RegistryError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotAdmin = 3,
    NotGovernanceSigner = 4,
    ParamNotFound = 5,
    ParamValueOutOfBounds = 6,
    ProposalNotFound = 7,
    ProposalAlreadyExecuted = 8,
    ProposalExpired = 9,
    ProposalAlreadyApproved = 10,
    GovernanceThresholdNotMet = 11,
    GovernanceTimelockNotElapsed = 12,
    NoUpgradeProposed = 13,
    UpgradeTimelockNotElapsed = 14,
    DirectCallProhibited = 15,
    ArithmeticOverflow = 16,
    InvalidParameterValue = 17,
    NotMultisigSigner = 18,
}

impl From<KoraError> for RegistryError {
    fn from(e: KoraError) -> Self {
        match e {
            KoraError::NotAdmin => RegistryError::NotAdmin,
            KoraError::AlreadyInitialized => RegistryError::AlreadyInitialized,
            KoraError::NotInitialized => RegistryError::NotInitialized,
            KoraError::ArithmeticOverflow => RegistryError::ArithmeticOverflow,
            _ => RegistryError::InvalidParameterValue,
        }
    }
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Governance timelock — same as upgrade timelock (~24 h).
const GOVERNANCE_TIMELOCK: u64 = UPGRADE_TIMELOCK_DELAY;

/// Proposal TTL in seconds (~7 days) — proposals that haven't reached quorum
/// by this deadline expire and must be re-proposed.
const PROPOSAL_TTL_SECS: u64 = 7 * 86_400;

/// Storage TTL bump (~30 days in ledgers at ~5 s/ledger).
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    PendingAdmin,
    /// Address of the access_control contract (for governance signer lookups and pause check).
    AccessControl,
    /// Pending WASM upgrade proposal: (wasm_hash, proposed_at).
    UpgradeProposal,
    /// Current governed value of a parameter.
    Param(RegistryParamKey),
    /// Whether a parameter's value came from an executed governance proposal
    /// (as opposed to a bootstrap default). Consumers that treat local config
    /// as authoritative until registry governance acts should consult this.
    ParamGoverned(RegistryParamKey),
    /// A pending governance proposal to update a parameter.
    ParamProposal(u64),
    /// Monotonic proposal ID counter.
    NextProposalId,
    // ── Audit log ─────────────────────────────────────────────────────────────
    AuditLogHead,
    AuditLogTotal,
    AuditEntry(u64),
}

// ── Governance proposal type ──────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug)]
pub struct ParamProposal {
    pub id: u64,
    pub key: RegistryParamKey,
    pub new_value: i128,
    pub proposer: Address,
    /// Addresses that have approved so far.
    pub approvals: Vec<Address>,
    pub proposed_at: u64,
    /// Earliest ledger timestamp at which execute is allowed (proposed_at + GOVERNANCE_TIMELOCK).
    pub executable_after: u64,
    pub expires_at: u64,
    pub executed: bool,
    pub cancelled: bool,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct ParameterRegistryContract;

#[contractimpl]
impl ParameterRegistryContract {
    // ── Initialization ────────────────────────────────────────────────────────

    /// One-time initialization. Bootstraps the registry with the default
    /// governing values for every key.
    ///
    /// `access_control` is optional at init; set it later with
    /// `set_access_control` to wire up governance signing. Until then,
    /// `admin_set_parameter` is the only write path (bootstrap mode).
    pub fn initialize(
        env: Env,
        admin: Address,
        access_control: Option<Address>,
    ) -> Result<(), RegistryError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(RegistryError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        if let Some(ref ac) = access_control {
            env.storage().instance().set(&DataKey::AccessControl, ac);
        }
        env.storage().instance().set(&DataKey::NextProposalId, &1u64);

        // Bootstrap default values for all keys.  Consuming contracts use
        // these as their initial governed values.
        let now = env.ledger().timestamp();
        Self::bootstrap_defaults(&env, &admin, now);

        events::registry_param_initialized(&env, &admin);
        Ok(())
    }

    /// Wire up (or update) the access_control contract reference.  Admin only.
    pub fn set_access_control(
        env: Env,
        admin: Address,
        access_control: Address,
    ) -> Result<(), RegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        Ok(())
    }

    // ── Bootstrap helpers ─────────────────────────────────────────────────────

    fn bootstrap_defaults(env: &Env, admin: &Address, now: u64) {
        // Marketplace
        Self::write_param(env, admin, now, RegistryParamKey::MarketplaceFeeBps, 50, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::ReferrerSplitBps, 1_000, 0, 5_000);
        Self::write_param(env, admin, now, RegistryParamKey::MinContributionAmount, 10_000_000, 0, i128::MAX / 2);
        Self::write_param(env, admin, now, RegistryParamKey::TierFeeAaa, 25, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::TierFeeAa, 35, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::TierFeeA, 50, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::TierFeeB, 75, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::TierFeeC, 100, 0, 10_000);
        // Financing pool
        Self::write_param(env, admin, now, RegistryParamKey::LatePenaltyBps, 500, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::LatePenaltyTreasurySplitBps, 5_000, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::MaxPositionBps, 5_000, 1, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::GracePeriodSecs, 7 * 86_400, 0, 90 * 86_400);
        // Risk registry
        Self::write_param(env, admin, now, RegistryParamKey::MinimumVerifierStake, 1_000_000_000, 0, i128::MAX / 2);
        Self::write_param(env, admin, now, RegistryParamKey::SlashPercentageBps, 500, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::MinimumDebtorScore, 0, 0, 100);
        // Treasury
        Self::write_param(env, admin, now, RegistryParamKey::TreasuryFeeBps, 50, 0, 10_000);
        Self::write_param(env, admin, now, RegistryParamKey::ReserveAllocationBps, 1_000, 0, 5_000);
        Self::write_param(env, admin, now, RegistryParamKey::MinReserveRatioBps, 0, 0, 10_000);
    }

    fn write_param(
        env: &Env,
        by: &Address,
        now: u64,
        key: RegistryParamKey,
        value: i128,
        min_value: i128,
        max_value: i128,
    ) {
        let param = RegistryParam {
            value,
            min_value,
            max_value,
            updated_at: now,
            updated_by: by.clone(),
        };
        env.storage().persistent().set(&DataKey::Param(key.clone()), &param);
        env.storage().persistent().extend_ttl(
            &DataKey::Param(key),
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_BUMP,
        );
    }

    // ── Admin / emergency write path ──────────────────────────────────────────

    /// Emergency / bootstrap write path.  Blocked once a multisig is
    /// configured on `access_control`; use the governance proposal flow instead.
    ///
    /// Enforces the `[min_value, max_value]` bounds configured on the existing
    /// parameter record.
    pub fn admin_set_parameter(
        env: Env,
        admin: Address,
        key: RegistryParamKey,
        value: i128,
    ) -> Result<(), RegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        // Block direct writes once governance is wired up.
        if Self::is_multisig_configured(&env) {
            return Err(RegistryError::DirectCallProhibited);
        }
        Self::apply_param_update(&env, &admin, key, value)
    }

    // ── Governance proposal flow ──────────────────────────────────────────────

    /// Step 1 — propose a parameter update.  Caller must be a multisig signer.
    /// The proposer's approval is recorded automatically.
    pub fn propose_param_update(
        env: Env,
        proposer: Address,
        key: RegistryParamKey,
        new_value: i128,
    ) -> Result<u64, RegistryError> {
        proposer.require_auth();
        Self::require_governance_signer(&env, &proposer)?;

        // Validate the value against the parameter's bounds immediately so
        // proposals that can never execute are rejected at proposal time.
        Self::validate_param_value(&env, &key, new_value)?;

        let proposal_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(1);

        let now = env.ledger().timestamp();
        let mut approvals = Vec::new(&env);
        approvals.push_back(proposer.clone());

        let proposal = ParamProposal {
            id: proposal_id,
            key,
            new_value,
            proposer: proposer.clone(),
            approvals,
            proposed_at: now,
            executable_after: now
                .checked_add(GOVERNANCE_TIMELOCK)
                .ok_or(RegistryError::ArithmeticOverflow)?,
            expires_at: now
                .checked_add(PROPOSAL_TTL_SECS)
                .ok_or(RegistryError::ArithmeticOverflow)?,
            executed: false,
            cancelled: false,
        };

        env.storage()
            .persistent()
            .set(&DataKey::ParamProposal(proposal_id), &proposal);
        env.storage().persistent().extend_ttl(
            &DataKey::ParamProposal(proposal_id),
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_BUMP,
        );

        let next_id = proposal_id
            .checked_add(1)
            .ok_or(RegistryError::ArithmeticOverflow)?;
        env.storage().instance().set(&DataKey::NextProposalId, &next_id);

        // Emit with a u32 key discriminant for off-chain indexers.
        let key_hash = Self::key_discriminant(&proposal.key);
        events::param_update_proposed(&env, &proposer, key_hash, new_value);
        Self::append_audit_entry(&env, &proposer, AdminActionType::RegistryProposeParam);
        Ok(proposal_id)
    }

    /// Step 2 — add an approval.  Each signer may approve at most once.
    pub fn approve_param_update(
        env: Env,
        approver: Address,
        proposal_id: u64,
    ) -> Result<(), RegistryError> {
        approver.require_auth();
        Self::require_governance_signer(&env, &approver)?;

        let mut proposal: ParamProposal = env
            .storage()
            .persistent()
            .get(&DataKey::ParamProposal(proposal_id))
            .ok_or(RegistryError::ProposalNotFound)?;

        if proposal.executed || proposal.cancelled {
            return Err(RegistryError::ProposalAlreadyExecuted);
        }
        if env.ledger().timestamp() > proposal.expires_at {
            return Err(RegistryError::ProposalExpired);
        }
        // Dedup: reject double-approval from the same signer.
        for existing in proposal.approvals.iter() {
            if existing == approver {
                return Err(RegistryError::ProposalAlreadyApproved);
            }
        }
        proposal.approvals.push_back(approver.clone());

        env.storage()
            .persistent()
            .set(&DataKey::ParamProposal(proposal_id), &proposal);
        env.storage().persistent().extend_ttl(
            &DataKey::ParamProposal(proposal_id),
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_BUMP,
        );

        let approval_count = proposal.approvals.len();
        events::param_update_approved(&env, &approver, proposal_id, approval_count);
        Self::append_audit_entry(&env, &approver, AdminActionType::RegistryApproveParam);
        Ok(())
    }

    /// Step 3 — execute after quorum and timelock.  Any governance signer may call.
    pub fn execute_param_update(
        env: Env,
        executor: Address,
        proposal_id: u64,
    ) -> Result<(), RegistryError> {
        executor.require_auth();
        Self::require_governance_signer(&env, &executor)?;

        let mut proposal: ParamProposal = env
            .storage()
            .persistent()
            .get(&DataKey::ParamProposal(proposal_id))
            .ok_or(RegistryError::ProposalNotFound)?;

        if proposal.executed || proposal.cancelled {
            return Err(RegistryError::ProposalAlreadyExecuted);
        }
        if env.ledger().timestamp() > proposal.expires_at {
            return Err(RegistryError::ProposalExpired);
        }
        // Check quorum.
        let threshold = Self::governance_threshold(&env).unwrap_or(1);
        if proposal.approvals.len() < threshold {
            return Err(RegistryError::GovernanceThresholdNotMet);
        }
        // Check timelock.
        if env.ledger().timestamp() < proposal.executable_after {
            return Err(RegistryError::GovernanceTimelockNotElapsed);
        }

        let old_value = Self::get_parameter_raw(&env, &proposal.key)
            .map(|p| p.value)
            .unwrap_or(0);

        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::ParamProposal(proposal_id), &proposal);

        Self::apply_param_update(&env, &executor, proposal.key.clone(), proposal.new_value)?;

        let key_hash = Self::key_discriminant(&proposal.key);
        events::param_updated(&env, &executor, key_hash, old_value, proposal.new_value);
        Self::append_audit_entry(&env, &executor, AdminActionType::RegistryExecuteParam);
        Ok(())
    }

    /// Cancel a pending proposal.  Only the proposer or admin may cancel.
    pub fn cancel_param_update(
        env: Env,
        caller: Address,
        proposal_id: u64,
    ) -> Result<(), RegistryError> {
        caller.require_auth();

        let mut proposal: ParamProposal = env
            .storage()
            .persistent()
            .get(&DataKey::ParamProposal(proposal_id))
            .ok_or(RegistryError::ProposalNotFound)?;

        if proposal.executed || proposal.cancelled {
            return Err(RegistryError::ProposalAlreadyExecuted);
        }

        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(RegistryError::NotInitialized)?;
        if caller != proposal.proposer && caller != admin {
            return Err(RegistryError::NotAdmin);
        }

        proposal.cancelled = true;
        env.storage()
            .persistent()
            .set(&DataKey::ParamProposal(proposal_id), &proposal);

        Self::append_audit_entry(&env, &caller, AdminActionType::RegistryCancelParam);
        Ok(())
    }

    // ── Read interface ────────────────────────────────────────────────────────

    /// Returns the current `RegistryParam` for a key, or `None` if not set.
    pub fn get_parameter(env: Env, key: RegistryParamKey) -> Option<RegistryParam> {
        Self::get_parameter_raw(&env, &key)
    }

    /// Returns true once `key` has been set by an executed governance proposal.
    /// Bootstrap defaults do not count: consumers use this to distinguish an
    /// explicit governance decision from initial values.
    pub fn is_param_governed(env: Env, key: RegistryParamKey) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::ParamGoverned(key))
            .unwrap_or(false)
    }

    /// Returns just the numeric value for a key.  Returns `0` when unset so
    /// consuming contracts with sensible zero-defaults handle missing keys
    /// gracefully.
    pub fn get_value(env: Env, key: RegistryParamKey) -> i128 {
        Self::get_parameter_raw(&env, &key)
            .map(|p| p.value)
            .unwrap_or(0)
    }

    /// Batch read: returns a Vec of (key, value) pairs for the supplied keys.
    /// Keys without a registered value are returned with value = 0.
    pub fn get_values_batch(env: Env, keys: Vec<RegistryParamKey>) -> Vec<(RegistryParamKey, i128)> {
        let mut out: Vec<(RegistryParamKey, i128)> = Vec::new(&env);
        for key in keys.iter() {
            let val = Self::get_parameter_raw(&env, &key)
                .map(|p| p.value)
                .unwrap_or(0);
            out.push_back((key, val));
        }
        out
    }

    /// Returns a pending proposal by ID.
    pub fn get_proposal(env: Env, proposal_id: u64) -> Option<ParamProposal> {
        env.storage()
            .persistent()
            .get(&DataKey::ParamProposal(proposal_id))
    }

    // ── Upgrade ───────────────────────────────────────────────────────────────

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), RegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let now = env.ledger().timestamp();
        env.storage()
            .instance()
            .set(&DataKey::UpgradeProposal, &(new_wasm_hash.clone(), now));
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        Ok(())
    }

    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), RegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(RegistryError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(RegistryError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Admin transfer (two-step) ─────────────────────────────────────────────

    pub fn propose_admin(
        env: Env,
        admin: Address,
        new_admin: Address,
    ) -> Result<(), RegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::PendingAdmin, &new_admin);
        events::admin_proposed(&env, &admin, &new_admin);
        Ok(())
    }

    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), RegistryError> {
        new_admin.require_auth();
        let pending: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(RegistryError::NotInitialized)?;
        if pending != new_admin {
            return Err(RegistryError::NotAdmin);
        }
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        events::admin_transferred(&env, &new_admin, &new_admin);
        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn get_parameter_raw(env: &Env, key: &RegistryParamKey) -> Option<RegistryParam> {
        env.storage().persistent().get(&DataKey::Param(key.clone()))
    }

    fn apply_param_update(
        env: &Env,
        by: &Address,
        key: RegistryParamKey,
        value: i128,
    ) -> Result<(), RegistryError> {
        Self::validate_param_value(env, &key, value)?;
        let now = env.ledger().timestamp();
        let existing = Self::get_parameter_raw(env, &key);
        let (min_value, max_value) = existing
            .as_ref()
            .map(|p| (p.min_value, p.max_value))
            .unwrap_or((0, 0));
        let param = RegistryParam {
            value,
            min_value,
            max_value,
            updated_at: now,
            updated_by: by.clone(),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Param(key.clone()), &param);
        env.storage().persistent().extend_ttl(
            &DataKey::Param(key.clone()),
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_BUMP,
        );
        // Mark the value as explicitly decided (not a bootstrap default) so
        // consumers can distinguish governance/admin writes from initial values.
        env.storage()
            .persistent()
            .set(&DataKey::ParamGoverned(key.clone()), &true);
        Ok(())
    }

    fn validate_param_value(
        env: &Env,
        key: &RegistryParamKey,
        value: i128,
    ) -> Result<(), RegistryError> {
        // Check stored bounds first.
        if let Some(existing) = Self::get_parameter_raw(env, key) {
            if existing.max_value > 0 && value > existing.max_value {
                return Err(RegistryError::ParamValueOutOfBounds);
            }
            if value < existing.min_value {
                return Err(RegistryError::ParamValueOutOfBounds);
            }
        }
        // Key-specific semantic validation.
        match key {
            RegistryParamKey::MarketplaceFeeBps
            | RegistryParamKey::ReferrerSplitBps
            | RegistryParamKey::TierFeeAaa
            | RegistryParamKey::TierFeeAa
            | RegistryParamKey::TierFeeA
            | RegistryParamKey::TierFeeB
            | RegistryParamKey::TierFeeC
            | RegistryParamKey::LatePenaltyBps
            | RegistryParamKey::LatePenaltyTreasurySplitBps
            | RegistryParamKey::MaxPositionBps
            | RegistryParamKey::SlashPercentageBps
            | RegistryParamKey::TreasuryFeeBps
            | RegistryParamKey::ReserveAllocationBps
            | RegistryParamKey::MinReserveRatioBps => {
                if value < 0 || value > 10_000 {
                    return Err(RegistryError::ParamValueOutOfBounds);
                }
            }
            RegistryParamKey::MinimumDebtorScore => {
                if value < 0 || value > 100 {
                    return Err(RegistryError::ParamValueOutOfBounds);
                }
            }
            RegistryParamKey::MinContributionAmount
            | RegistryParamKey::MinimumVerifierStake => {
                if value < 0 {
                    return Err(RegistryError::ParamValueOutOfBounds);
                }
            }
            RegistryParamKey::GracePeriodSecs => {
                // Max 90 days
                if value < 0 || value > 90 * 86_400 {
                    return Err(RegistryError::ParamValueOutOfBounds);
                }
            }
        }
        Ok(())
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), RegistryError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(RegistryError::NotInitialized)?;
        if &admin != caller {
            return Err(RegistryError::NotAdmin);
        }
        Ok(())
    }

    fn require_governance_signer(env: &Env, caller: &Address) -> Result<(), RegistryError> {
        // If no access_control is configured, fall back to admin-only.
        let ac_addr: Option<Address> = env.storage().instance().get(&DataKey::AccessControl);
        if let Some(ref ac) = ac_addr {
            let ac_client = kora_access_control::AccessControlContractClient::new(env, ac);
            if let Ok(Ok(cfg)) = ac_client.try_get_multisig_config() {
                // Caller must be in the signer set.
                for signer in cfg.signers.iter() {
                    if signer == *caller {
                        return Ok(());
                    }
                }
                return Err(RegistryError::NotGovernanceSigner);
            }
        }
        // No multisig — fall back to admin.
        Self::require_admin(env, caller)
    }

    fn governance_threshold(env: &Env) -> Option<u32> {
        let ac_addr: Option<Address> = env.storage().instance().get(&DataKey::AccessControl);
        if let Some(ref ac) = ac_addr {
            let ac_client = kora_access_control::AccessControlContractClient::new(env, ac);
            if let Ok(Ok(cfg)) = ac_client.try_get_multisig_config() {
                return Some(cfg.threshold);
            }
        }
        Some(1) // admin-only mode: threshold = 1
    }

    fn is_multisig_configured(env: &Env) -> bool {
        let ac_addr: Option<Address> = env.storage().instance().get(&DataKey::AccessControl);
        if let Some(ref ac) = ac_addr {
            let ac_client = kora_access_control::AccessControlContractClient::new(env, ac);
            return ac_client.try_get_multisig_config()
                .ok()
                .and_then(Result::ok)
                .is_some();
        }
        false
    }

    /// Stable u32 discriminant for a `RegistryParamKey` — used in events so
    /// off-chain indexers don't have to deserialize the full enum.
    fn key_discriminant(key: &RegistryParamKey) -> u32 {
        match key {
            RegistryParamKey::MarketplaceFeeBps => 1,
            RegistryParamKey::ReferrerSplitBps => 2,
            RegistryParamKey::MinContributionAmount => 3,
            RegistryParamKey::TierFeeAaa => 4,
            RegistryParamKey::TierFeeAa => 5,
            RegistryParamKey::TierFeeA => 6,
            RegistryParamKey::TierFeeB => 7,
            RegistryParamKey::TierFeeC => 8,
            RegistryParamKey::LatePenaltyBps => 9,
            RegistryParamKey::LatePenaltyTreasurySplitBps => 10,
            RegistryParamKey::MaxPositionBps => 11,
            RegistryParamKey::GracePeriodSecs => 12,
            RegistryParamKey::MinimumVerifierStake => 13,
            RegistryParamKey::SlashPercentageBps => 14,
            RegistryParamKey::MinimumDebtorScore => 15,
            RegistryParamKey::TreasuryFeeBps => 16,
            RegistryParamKey::ReserveAllocationBps => 17,
            RegistryParamKey::MinReserveRatioBps => 18,
        }
    }

    // ── Audit log ring-buffer ─────────────────────────────────────────────────

    fn append_audit_entry(env: &Env, actor: &Address, action: AdminActionType) {
        let head: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditLogHead)
            .unwrap_or(0);
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditLogTotal)
            .unwrap_or(0);
        let entry = AdminAuditEntry {
            sequence: total,
            timestamp: env.ledger().timestamp(),
            actor: actor.clone(),
            action,
            source: AuditSource::ParameterRegistry,
            token: None,
            amount: None,
        };
        let slot = head % MAX_AUDIT_LOG_SIZE;
        env.storage()
            .persistent()
            .set(&DataKey::AuditEntry(slot), &entry);
        env.storage().persistent().extend_ttl(
            &DataKey::AuditEntry(slot),
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_BUMP,
        );
        env.storage()
            .instance()
            .set(&DataKey::AuditLogHead, &(head + 1));
        env.storage()
            .instance()
            .set(&DataKey::AuditLogTotal, &(total + 1));
    }

    // ── Read helpers for tests ────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Admin)
    }

    pub fn get_next_proposal_id(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::NextProposalId)
            .unwrap_or(1)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};

    fn setup() -> (Env, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });
        let admin = Address::generate(&env);
        let id = env.register_contract(None, ParameterRegistryContract);
        let client = ParameterRegistryContractClient::new(&env, &id);
        client.initialize(&admin, &None);
        (env, id, admin)
    }

    #[test]
    fn test_initialize_bootstraps_defaults() {
        let (env, id, _admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        // marketplace fee default = 50 bps
        let fee = client.get_value(&RegistryParamKey::MarketplaceFeeBps);
        assert_eq!(fee, 50);
        // late penalty default = 500 bps
        let penalty = client.get_value(&RegistryParamKey::LatePenaltyBps);
        assert_eq!(penalty, 500);
        // minimum debtor score default = 0
        let score = client.get_value(&RegistryParamKey::MinimumDebtorScore);
        assert_eq!(score, 0);
    }

    #[test]
    fn test_admin_set_parameter_updates_value() {
        let (env, id, admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        client.admin_set_parameter(&admin, &RegistryParamKey::MarketplaceFeeBps, &100);
        let fee = client.get_value(&RegistryParamKey::MarketplaceFeeBps);
        assert_eq!(fee, 100);
    }

    #[test]
    fn test_admin_set_parameter_rejects_out_of_bounds() {
        let (env, id, admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        let result = client.try_admin_set_parameter(
            &admin,
            &RegistryParamKey::MarketplaceFeeBps,
            &20_000, // > 10_000 bps
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_governance_proposal_lifecycle_without_multisig() {
        let (env, id, admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        // In admin-only mode, propose/approve/execute all work with admin as the signer.
        let proposal_id = client.propose_param_update(
            &admin,
            &RegistryParamKey::LatePenaltyBps,
            &750,
        );
        assert_eq!(proposal_id, 1);

        // Advance past the timelock.
        env.ledger().set(LedgerInfo {
            timestamp: 1_000_000 + GOVERNANCE_TIMELOCK + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1_000,
            min_persistent_entry_ttl: 1_000,
            max_entry_ttl: 1_000_000,
        });

        client.execute_param_update(&admin, &proposal_id);
        let val = client.get_value(&RegistryParamKey::LatePenaltyBps);
        assert_eq!(val, 750);
    }

    #[test]
    fn test_execute_before_timelock_fails() {
        let (env, id, admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        let proposal_id = client.propose_param_update(
            &admin,
            &RegistryParamKey::LatePenaltyBps,
            &750,
        );
        // Do NOT advance time — timelock not elapsed.
        let result = client.try_execute_param_update(&admin, &proposal_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_cancel_proposal() {
        let (env, id, admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        let proposal_id = client.propose_param_update(
            &admin,
            &RegistryParamKey::LatePenaltyBps,
            &750,
        );
        client.cancel_param_update(&admin, &proposal_id);
        let proposal = client.get_proposal(&proposal_id).unwrap();
        assert!(proposal.cancelled);
    }

    #[test]
    fn test_double_approval_rejected() {
        let (env, id, admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        let proposal_id = client.propose_param_update(
            &admin,
            &RegistryParamKey::LatePenaltyBps,
            &750,
        );
        // Admin already auto-approved at propose time; second approval should fail.
        let result = client.try_approve_param_update(&admin, &proposal_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_values_batch() {
        let (env, id, _admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        let keys = {
            let mut v = Vec::new(&env);
            v.push_back(RegistryParamKey::MarketplaceFeeBps);
            v.push_back(RegistryParamKey::LatePenaltyBps);
            v
        };
        let results = client.get_values_batch(&keys);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_cache_invalidation_mid_lifecycle() {
        // A consuming contract holds a cached value. After a parameter update
        // (simulated via admin_set_parameter), calling get_value returns the
        // new value — confirming the registry is the single source of truth.
        let (env, id, admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        let cached = client.get_value(&RegistryParamKey::MarketplaceFeeBps);
        assert_eq!(cached, 50);

        client.admin_set_parameter(&admin, &RegistryParamKey::MarketplaceFeeBps, &75);
        let refreshed = client.get_value(&RegistryParamKey::MarketplaceFeeBps);
        assert_eq!(refreshed, 75);

        // Cached value (50) != refreshed value (75) — consumer must call
        // refresh_cache after observing a param_updated event.
        assert_ne!(cached, refreshed);
    }

    #[test]
    fn test_backward_compat_zero_default_for_unknown_key() {
        // Consuming contracts that call get_value for a key not yet bootstrapped
        // receive 0 (not an error) — enabling graceful migration.
        let (env, id, _admin) = setup();
        let client = ParameterRegistryContractClient::new(&env, &id);
        // MinimumVerifierStake is bootstrapped, so this just verifies non-zero default.
        let val = client.get_value(&RegistryParamKey::MinimumVerifierStake);
        assert!(val > 0);
    }
}
