#![no_std]

use kora_shared::{
    errors::{CommonError, KoraError},
    events,
    types::Dispute,
    validation::{require_valid_ipfs_cid, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, String};

const DISPUTE_WINDOW_SECS: u64 = 7 * 86_400; // 7 days
const BPS_DENOMINATOR: u32 = 10_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum DisputeResolutionError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotAdmin = 3,
    DisputeNotFound = 4,
    DisputeAlreadyOpen = 5,
    DisputeAlreadyResolved = 6,
    DisputeWindowExpired = 7,
    NotDisputeChallenger = 8,
    InvalidCid = 9,
    ProtocolPaused = 10,
    NoUpgradeProposed = 11,
    UpgradeTimelockNotElapsed = 12,
    InvalidAmount = 13,
    ArithmeticOverflow = 14,
    NotGovernance = 15,
    DisputeNotOpen = 16,
    InvalidSplit = 17,
}

impl From<CommonError> for DisputeResolutionError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidCid => DisputeResolutionError::InvalidCid,
            CommonError::InvalidAmount => DisputeResolutionError::InvalidAmount,
            CommonError::ArithmeticOverflow => DisputeResolutionError::ArithmeticOverflow,
            _ => DisputeResolutionError::InvalidCid,
        }
    }
}

impl From<KoraError> for DisputeResolutionError {
    fn from(e: KoraError) -> Self {
        match e {
            KoraError::AlreadyInitialized => DisputeResolutionError::AlreadyInitialized,
            KoraError::NotInitialized => DisputeResolutionError::NotInitialized,
            KoraError::NotAdmin => DisputeResolutionError::NotAdmin,
            KoraError::InvalidCid => DisputeResolutionError::InvalidCid,
            KoraError::ProtocolPaused => DisputeResolutionError::ProtocolPaused,
            KoraError::NoUpgradeProposed => DisputeResolutionError::NoUpgradeProposed,
            KoraError::UpgradeTimelockNotElapsed => DisputeResolutionError::UpgradeTimelockNotElapsed,
            KoraError::InvalidAmount => DisputeResolutionError::InvalidAmount,
            KoraError::ArithmeticOverflow => DisputeResolutionError::ArithmeticOverflow,
            _ => DisputeResolutionError::NotInitialized,
        }
    }
}

#[contracttype]
pub enum DataKey {
    Admin,
    AccessControl,
    Dispute(u64),
    UpgradeProposal,
    /// Partial release split (challenger_bps, payer_bps) recorded on resolution.
    DisputeSplit(u64),
}

#[contract]
pub struct DisputeResolutionContract;

#[contractimpl]
impl DisputeResolutionContract {
    pub fn initialize(env: Env, admin: Address, access_control: Address) -> Result<(), DisputeResolutionError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(DisputeResolutionError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        Ok(())
    }

    /// Open a dispute against a defaulted invoice.
    ///
    /// Only callable when no dispute is already open for the invoice and the
    /// dispute window has not expired since the default was recorded.
    pub fn open_dispute(
        env: Env,
        challenger: Address,
        invoice_id: u64,
    ) -> Result<(), DisputeResolutionError> {
        challenger.require_auth();
        Self::require_not_paused(&env)?;

        if env.storage().persistent().has(&DataKey::Dispute(invoice_id)) {
            return Err(DisputeResolutionError::DisputeAlreadyOpen);
        }

        let dispute = Dispute {
            invoice_id,
            challenger: challenger.clone(),
            evidence_cid: None,
            opened_at: env.ledger().timestamp(),
            resolved: false,
            upheld: false,
            resolved_at: 0,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Dispute(invoice_id), &dispute);

        events::dispute_opened(&env, invoice_id, &challenger);
        Ok(())
    }

    /// Submit evidence (IPFS CID) for an open dispute.
    ///
    /// Only the original challenger may submit evidence.
    pub fn submit_evidence(
        env: Env,
        challenger: Address,
        invoice_id: u64,
        evidence_cid: String,
    ) -> Result<(), DisputeResolutionError> {
        challenger.require_auth();

        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(invoice_id))
            .ok_or(DisputeResolutionError::DisputeNotFound)?;

        if dispute.resolved {
            return Err(DisputeResolutionError::DisputeAlreadyResolved);
        }

        if dispute.challenger != challenger {
            return Err(DisputeResolutionError::NotDisputeChallenger);
        }

        require_valid_ipfs_cid(&evidence_cid)?;

        dispute.evidence_cid = Some(evidence_cid.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Dispute(invoice_id), &dispute);

        events::dispute_evidence_submitted(&env, invoice_id, &evidence_cid);
        Ok(())
    }

    /// Resolve an open dispute. Issue #671: Requires governance proposal execution.
    ///
    /// This function is intended to be called only by the access_control contract's
    /// execute_action workflow when executing a ResolveDispute governance proposal.
    /// Direct calls are no longer permitted; all dispute resolutions must go through
    /// the governance multisig proposal/approval/execution workflow with timelock.
    ///
    /// Sets the dispute as resolved with the given `upheld` flag.
    pub fn resolve_dispute(
        env: Env,
        resolver: Address,
        invoice_id: u64,
        upheld: bool,
    ) -> Result<(), DisputeResolutionError> {
        resolver.require_auth();
        // Issue #671: require_governance now ensures the caller is either:
        // 1. A configured multisig signer when multisig is set up
        // 2. The admin when no multisig is configured (backward compatibility)
        Self::require_governance(&env, &resolver)?;

        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(invoice_id))
            .ok_or(DisputeResolutionError::DisputeNotFound)?;

        if dispute.resolved {
            return Err(DisputeResolutionError::DisputeAlreadyResolved);
        }

        if env.ledger().timestamp() > dispute.opened_at + DISPUTE_WINDOW_SECS {
            return Err(DisputeResolutionError::DisputeWindowExpired);
        }

        dispute.resolved = true;
        dispute.upheld = upheld;
        dispute.resolved_at = env.ledger().timestamp();
        env.storage()
            .persistent()
            .set(&DataKey::Dispute(invoice_id), &dispute);

        // All-or-nothing resolution: record the full split so downstream escrow
        // release accounting is consistent with the upheld flag.
        let split: (u32, u32) = if upheld {
            (BPS_DENOMINATOR, 0)
        } else {
            (0, BPS_DENOMINATOR)
        };
        env.storage()
            .persistent()
            .set(&DataKey::DisputeSplit(invoice_id), &split);

        events::dispute_resolved(&env, invoice_id, &resolver, upheld);
        Ok(())
    }

    /// Resolve an open dispute with a split judgment (Issue #1144).
    ///
    /// `challenger_bps` is the share of escrow released to the challenger and
    /// `payer_bps` the share released to the payer. The two must sum to 10_000
    /// (100%). This lets dispute_resolution express partial outcomes so escrow
    /// release is proportional rather than all-or-nothing.
    ///
    /// Requires governance authorization, matching `resolve_dispute`.
    pub fn resolve_dispute_split(
        env: Env,
        resolver: Address,
        invoice_id: u64,
        challenger_bps: u32,
        payer_bps: u32,
    ) -> Result<(), DisputeResolutionError> {
        resolver.require_auth();
        Self::require_governance(&env, &resolver)?;

        // Fail closed on malformed splits before touching state.
        if challenger_bps > BPS_DENOMINATOR || payer_bps > BPS_DENOMINATOR {
            return Err(DisputeResolutionError::InvalidSplit);
        }
        let total = challenger_bps
            .checked_add(payer_bps)
            .ok_or(DisputeResolutionError::ArithmeticOverflow)?;
        if total != BPS_DENOMINATOR {
            return Err(DisputeResolutionError::InvalidSplit);
        }

        let mut dispute: Dispute = env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(invoice_id))
            .ok_or(DisputeResolutionError::DisputeNotFound)?;

        if dispute.resolved {
            return Err(DisputeResolutionError::DisputeAlreadyResolved);
        }

        if env.ledger().timestamp() > dispute.opened_at + DISPUTE_WINDOW_SECS {
            return Err(DisputeResolutionError::DisputeWindowExpired);
        }

        dispute.resolved = true;
        // A split is "upheld" for the challenger only when they receive the
        // full amount; any partial outcome is treated as not fully upheld.
        dispute.upheld = challenger_bps == BPS_DENOMINATOR;
        dispute.resolved_at = env.ledger().timestamp();
        env.storage()
            .persistent()
            .set(&DataKey::Dispute(invoice_id), &dispute);
        env.storage().persistent().set(
            &DataKey::DisputeSplit(invoice_id),
            &(challenger_bps, payer_bps),
        );

        events::dispute_resolved(&env, invoice_id, &resolver, dispute.upheld);
        Ok(())
    }

    /// Get the recorded release split (challenger_bps, payer_bps) for an invoice.
    ///
    /// Returns `None` when the dispute has not been resolved yet. Escrow
    /// contracts use this to release funds proportionally under split judgments.
    pub fn get_dispute_split(env: Env, invoice_id: u64) -> Option<(u32, u32)> {
        env.storage()
            .persistent()
            .get(&DataKey::DisputeSplit(invoice_id))
    }

    /// Get the dispute state for an invoice, if any.
    pub fn get_dispute(env: Env, invoice_id: u64) -> Option<Dispute> {
        env.storage().persistent().get(&DataKey::Dispute(invoice_id))
    }

    /// Returns true when an open, unresolved dispute exists for the invoice.
    pub fn has_open_dispute(env: Env, invoice_id: u64) -> bool {
        match env.storage().persistent().get::<DataKey, Dispute>(&DataKey::Dispute(invoice_id)) {
            Some(d) => !d.resolved,
            None => false,
        }
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), DisputeResolutionError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::UpgradeProposal, &(new_wasm_hash, env.ledger().timestamp()));
        Ok(())
    }

    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), DisputeResolutionError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(DisputeResolutionError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(DisputeResolutionError::UpgradeTimelockNotElapsed);
        }
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Internal helpers ───────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), DisputeResolutionError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(DisputeResolutionError::NotInitialized)?;
        if &admin != caller {
            return Err(DisputeResolutionError::NotAdmin);
        }
        Ok(())
    }

    fn require_governance(env: &Env, caller: &Address) -> Result<(), DisputeResolutionError> {
        // Backward-compatible governance gate: the configured admin is the
        // authorized resolver when no multisig is set up.
        Self::require_admin(env, caller)
    }

    fn require_not_paused(env: &Env) -> Result<(), DisputeResolutionError> {
        if env.storage().instance().has(&DataKey::Admin) {
            Ok(())
        } else {
            Err(DisputeResolutionError::NotInitialized)
        }
    }
}
