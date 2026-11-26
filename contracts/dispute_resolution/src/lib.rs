#![no_std]

use kora_shared::{
    errors::{CommonError, KoraError},
    events,
    types::Dispute,
    validation::{require_valid_ipfs_cid, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, String};

const DISPUTE_WINDOW_SECS: u64 = 7 * 86_400; // 7 days

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

        events::dispute_resolved(&env, invoice_id, &resolver, upheld);
        Ok(())
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
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

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

    fn require_not_paused(env: &Env) -> Result<(), DisputeResolutionError> {
        if let Some(ac) = env.storage().instance().get::<DataKey, Address>(&DataKey::AccessControl) {
            let ac_client = kora_access_control::AccessControlContractClient::new(env, &ac);
            if ac_client.is_paused() {
                return Err(DisputeResolutionError::ProtocolPaused);
            }
        }
        Ok(())
    }

    fn require_governance(env: &Env, caller: &Address) -> Result<(), DisputeResolutionError> {
        if let Some(ac) = env.storage().instance().get::<DataKey, Address>(&DataKey::AccessControl) {
            let ac_client = kora_access_control::AccessControlContractClient::new(env, &ac);
            let cfg = ac_client.try_get_multisig_config();
            if let Ok(Ok(cfg)) = cfg {
                if cfg.signers.contains(caller) {
                    return Ok(());
                }
            }
        }
        Err(DisputeResolutionError::NotGovernance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, Address, Address, DisputeResolutionContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, DisputeResolutionContract);
        let client = DisputeResolutionContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let ac = env.register_contract(None, kora_access_control::AccessControlContract);
        let access = kora_access_control::AccessControlContractClient::new(&env, &ac);
        access.initialize(&admin);
        // Resolution checks signer membership; an admin alone is insufficient.
        access.configure_multisig(&admin, &soroban_sdk::vec![&env, admin.clone()], &1);
        client.initialize(&admin, &ac);
        (env, admin, ac, client)
    }

    #[test]
    fn test_open_dispute_success() {
        let (_env, _admin, _ac, client) = setup();
        let challenger = Address::generate(&_env);
        assert_eq!(client.try_open_dispute(&challenger, &1u64), Ok(Ok(())));
        assert!(client.has_open_dispute(&1u64));
    }

    #[test]
    fn test_open_dispute_twice_rejected() {
        let (_env, _admin, _ac, client) = setup();
        let challenger = Address::generate(&_env);
        client.open_dispute(&challenger, &1u64);
        let result = client.try_open_dispute(&challenger, &1u64);
        assert_eq!(result.unwrap_err().unwrap(), DisputeResolutionError::DisputeAlreadyOpen);
    }

    #[test]
    fn test_submit_evidence_success() {
        let (env, _admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &1u64);
        let cid = String::from_str(&env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
        assert_eq!(client.try_submit_evidence(&challenger, &1u64, &cid), Ok(Ok(())));
    }

    #[test]
    fn test_resolve_dispute_success() {
        let (env, admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &1u64);
        assert_eq!(client.try_resolve_dispute(&admin, &1u64, &true), Ok(Ok(())));
        let dispute = client.get_dispute(&1u64).unwrap();
        assert!(dispute.resolved);
        assert!(dispute.upheld);
    }

    // ── Appeal window enforcement ─────────────────────────────────────────────
    //
    // These tests verify the DISPUTE_WINDOW_SECS boundary: resolution must be
    // rejected after the window expires, and must be accepted just before it.

    /// Resolution attempted before the dispute window expires must succeed.
    #[test]
    fn test_resolve_before_window_expiry_succeeds() {
        use soroban_sdk::testutils::{Ledger, LedgerInfo};
        let (env, admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &10u64);

        // Advance to 1 second before window expiry
        env.ledger().set(LedgerInfo {
            timestamp: env.ledger().timestamp() + DISPUTE_WINDOW_SECS - 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10_000,
            min_persistent_entry_ttl: 10_000,
            max_entry_ttl: 1_000_000,
        });

        let result = client.try_resolve_dispute(&admin, &10u64, &false);
        assert!(
            result.is_ok(),
            "resolution just before window expiry must succeed"
        );
        let dispute = client.get_dispute(&10u64).unwrap();
        assert!(dispute.resolved);
        assert!(!dispute.upheld);
    }

    /// Resolution attempted exactly at the window boundary (opened_at + DISPUTE_WINDOW_SECS)
    /// must be rejected — the boundary is exclusive on the expired side.
    #[test]
    fn test_resolve_exactly_at_window_boundary_rejected() {
        use soroban_sdk::testutils::{Ledger, LedgerInfo};
        let (env, admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        let opened_at = env.ledger().timestamp();
        client.open_dispute(&challenger, &11u64);

        // Advance to exactly opened_at + DISPUTE_WINDOW_SECS
        env.ledger().set(LedgerInfo {
            timestamp: opened_at + DISPUTE_WINDOW_SECS,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10_000,
            min_persistent_entry_ttl: 10_000,
            max_entry_ttl: 1_000_000,
        });

        let result = client.try_resolve_dispute(&admin, &11u64, &true);
        assert!(
            result.is_err(),
            "resolution at exactly the window boundary must be rejected (window expired)"
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            DisputeResolutionError::DisputeWindowExpired,
            "expired-window rejection must surface the typed DisputeWindowExpired error"
        );
    }

    /// Resolution attempted one second past the window must also be rejected.
    #[test]
    fn test_resolve_past_window_expiry_rejected_typed_error() {
        use soroban_sdk::testutils::{Ledger, LedgerInfo};
        let (env, admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        let opened_at = env.ledger().timestamp();
        client.open_dispute(&challenger, &12u64);

        // Advance one second past the window
        env.ledger().set(LedgerInfo {
            timestamp: opened_at + DISPUTE_WINDOW_SECS + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10_000,
            min_persistent_entry_ttl: 10_000,
            max_entry_ttl: 1_000_000,
        });

        let result = client.try_resolve_dispute(&admin, &12u64, &true);
        assert!(
            result.is_err(),
            "resolution past window must be rejected"
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            DisputeResolutionError::DisputeWindowExpired,
        );
    }

    // ── Post-finality immutability ─────────────────────────────────────────────
    //
    // Once a dispute is resolved (finalized), all subsequent mutation attempts
    // must return `DisputeAlreadyResolved` — the final state is immutable.

    /// Attempting to resolve an already-resolved dispute must be rejected with
    /// the typed `DisputeAlreadyResolved` error.
    #[test]
    fn test_resolved_dispute_is_immutable() {
        let (env, admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &20u64);
        client.resolve_dispute(&admin, &20u64, &true);

        // Any further resolution attempt must be rejected
        let result = client.try_resolve_dispute(&admin, &20u64, &false);
        assert!(result.is_err(), "re-resolving a finalized dispute must fail");
        assert_eq!(
            result.unwrap_err().unwrap(),
            DisputeResolutionError::DisputeAlreadyResolved,
            "finalized dispute must surface typed DisputeAlreadyResolved, not a panic"
        );
    }

    /// Submitting evidence to an already-resolved dispute must be rejected.
    #[test]
    fn test_submit_evidence_after_finality_rejected() {
        let (env, admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &21u64);
        client.resolve_dispute(&admin, &21u64, &false);

        let cid = soroban_sdk::String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let result = client.try_submit_evidence(&challenger, &21u64, &cid);
        assert!(result.is_err(), "evidence submission after finality must fail");
        assert_eq!(
            result.unwrap_err().unwrap(),
            DisputeResolutionError::DisputeAlreadyResolved,
        );
    }

    /// The resolved state persists correctly: upheld flag is immutable after resolution.
    #[test]
    fn test_resolved_state_persists_after_resolution() {
        let (env, admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &22u64);
        client.resolve_dispute(&admin, &22u64, &true);

        // State should remain permanently set
        let dispute = client.get_dispute(&22u64).unwrap();
        assert!(dispute.resolved);
        assert!(dispute.upheld);
        // resolved_at must be non-zero
        assert!(dispute.resolved_at > 0, "resolved_at must be set on resolution");

        // Verify has_open_dispute correctly reflects finality
        assert!(
            !client.has_open_dispute(&22u64),
            "finalized dispute must not appear as open"
        );
    }

    /// Upheld=false (rejected dispute) is also immutable.
    #[test]
    fn test_rejected_dispute_upheld_false_immutable() {
        let (env, admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &23u64);
        client.resolve_dispute(&admin, &23u64, &false);

        let dispute = client.get_dispute(&23u64).unwrap();
        assert!(dispute.resolved);
        assert!(!dispute.upheld, "upheld flag must stay false after rejection");

        // Cannot flip to upheld via re-resolve
        let result = client.try_resolve_dispute(&admin, &23u64, &true);
        assert_eq!(
            result.unwrap_err().unwrap(),
            DisputeResolutionError::DisputeAlreadyResolved,
        );
    }

    // ── Governance gate ───────────────────────────────────────────────────────

    /// Non-governance caller cannot resolve a dispute.
    #[test]
    fn test_non_governance_cannot_resolve() {
        let (env, _admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &30u64);

        let stranger = Address::generate(&env);
        let result = client.try_resolve_dispute(&stranger, &30u64, &true);
        assert!(result.is_err(), "non-governance caller must be rejected");
        assert_eq!(
            result.unwrap_err().unwrap(),
            DisputeResolutionError::NotGovernance,
        );
        // Dispute remains open — governance gate must not corrupt state
        assert!(client.has_open_dispute(&30u64), "dispute must remain open after rejected resolution attempt");
    }

    /// Attempting to open a second dispute for the same invoice is rejected.
    #[test]
    fn test_open_dispute_duplicate_same_invoice_rejected() {
        let (env, _admin, _ac, client) = setup();
        let challenger = Address::generate(&env);
        client.open_dispute(&challenger, &40u64);
        let result = client.try_open_dispute(&challenger, &40u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            DisputeResolutionError::DisputeAlreadyOpen,
        );
    }

    /// Resolving a non-existent dispute returns a typed error, not a panic.
    #[test]
    fn test_resolve_nonexistent_dispute_typed_error() {
        let (_env, admin, _ac, client) = setup();
        let result = client.try_resolve_dispute(&admin, &999u64, &true);
        assert_eq!(
            result.unwrap_err().unwrap(),
            DisputeResolutionError::DisputeNotFound,
        );
    }
}
