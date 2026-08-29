#![no_std]

use kora_shared::{
    audit::{AdminActionType, AdminAuditEntry, AuditSource, MAX_AUDIT_LOG_SIZE},
    errors::CommonError,
    events,
    reentrancy::ReentrancyGuard,
    types::SmeProfile,
    validation::{require_non_negative_amount, require_valid_risk_score, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Bytes, BytesN, Env, Vec,
};

// ── Errors ───────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RiskRegistryError {
    AlreadyInitialized = 1,
    ArithmeticOverflow = 2,
    ArithmeticUnderflow = 3,
    DebtorNotRegistered = 4,
    InsufficientFunds = 5,
    InvalidAddress = 6,
    InvalidAmount = 7,
    InvalidLength = 8,
    InvalidRiskScore = 9,
    NoUpgradeProposed = 10,
    NotAdmin = 11,
    NotInitialized = 12,
    NotVerifier = 13,
    Reentrancy = 14,
    SMENotRegistered = 15,
    ScoreUpdateCooldownNotElapsed = 16,
    Unauthorized = 17,
    UpgradeTimelockNotElapsed = 18,
    /// Caller is a valid verifier but is not the verifier-of-record for this SME.
    NotSmeVerifier = 19,
}

impl From<CommonError> for RiskRegistryError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => RiskRegistryError::InvalidAmount,
            CommonError::InvalidAddress => RiskRegistryError::InvalidAddress,
            CommonError::InvalidRiskScore => RiskRegistryError::InvalidRiskScore,
            CommonError::ArithmeticOverflow => RiskRegistryError::ArithmeticOverflow,
            CommonError::ArithmeticUnderflow => RiskRegistryError::ArithmeticUnderflow,
            CommonError::Reentrancy => RiskRegistryError::Reentrancy,
            // Remaining CommonError variants are not currently reachable via `?`
            // in this crate; map conservatively rather than panicking.
            _ => RiskRegistryError::InvalidAmount,
        }
    }
}

/// Validates that `bytes` is exactly `len` bytes long.
///
/// This used to live in `kora_shared::validation`; it was local-only there
/// and has since been removed from the shared crate, so it's reimplemented
/// here for `set_debtor_score`'s 32-byte SHA-256 hash requirement.
fn require_exact_length(bytes: &Bytes, len: u32) -> Result<(), RiskRegistryError> {
    if bytes.len() != len {
        return Err(RiskRegistryError::InvalidLength);
    }
    Ok(())
}

// ── TTL constants (in ledgers; ~5s per ledger on Stellar) ────────────────────
/// ~30 days worth of ledgers for persistent SME/verifier data
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

/// Minimum seconds between consecutive updates to the same debtor's score by the same verifier.
/// Prevents rapid manipulation immediately before a funding or default decision.
pub const MIN_SCORE_UPDATE_INTERVAL: u64 = 3_600; // 1 hour

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    InvoiceNft, // authorized caller for increment_invoice_count
    StakingToken, // token contract for verifier stakes
    MinimumStake, // minimum stake amount required to become verifier
    SlashPercentage, // percentage of stake to slash on default (basis points)
    Verifier(Address),
    VerifierStake(Address), // amount of tokens staked by verifier
    VerifierReputation(Address), // reputation score of verifier
    /// Maps a sub-account address → its primary verifier address.
    /// Sub-accounts can act on behalf of the primary for all verifier operations.
    SubAccount(Address),
    SmeProfile(Address),
    DebtorScoreAttestation(Bytes, Address), // (debtor_hash, verifier) -> u32 score
    DebtorAttestors(Bytes),                 // debtor_hash -> Vec<Address> of attesting verifiers
    /// Ledger timestamp of the last set_debtor_score call per (verifier, debtor_hash).
    DebtorScoreLastUpdate(Address, Bytes),
    UpgradeProposal,
    // ── Audit log ─────────────────────────────────────────────────────────────
    /// Next write position in the audit ring buffer (0..MAX_AUDIT_LOG_SIZE).
    AuditLogHead,
    /// Total admin actions ever recorded (monotonic; not capped at ring size).
    AuditLogTotal,
    /// An audit log entry at ring-buffer position `n`.
    AuditEntry(u64),
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RiskRegistryContract;

#[contractimpl]
impl RiskRegistryContract {
    /// One-time initialization. Sets admin, authorized invoice_nft, and staking parameters.
    ///
    /// **Parameters:**
    /// - `admin` — The address that will administer this contract.
    /// - `invoice_nft` — The authorized `invoice_nft` contract for `increment_invoice_count` calls.
    /// - `staking_token` — The token verifiers must stake to be registered.
    /// - `minimum_stake` — Minimum token amount a verifier must deposit (in token's smallest unit).
    /// - `slash_percentage_bps` — Basis points of stake to slash on each SME default (0–10 000).
    ///
    /// **Errors:**
    /// - `RiskRegistryError::AlreadyInitialized` — Contract has already been initialized.
    /// - `RiskRegistryError::InvalidAddress` — `admin` is the contract's own address.
    ///
    /// **Security:** No auth required on first call. Subsequent calls revert immediately.
    pub fn initialize(
        env: Env,
        admin: Address,
        invoice_nft: Address,
        staking_token: Address,
        minimum_stake: i128,
        slash_percentage_bps: u32,
    ) -> Result<(), RiskRegistryError> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(RiskRegistryError::AlreadyInitialized);
        }
        kora_shared::validation::require_not_self(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Admin, &admin);
        Self::bump_persistent(&env, &DataKey::Admin);
        env.storage()
            .persistent()
            .set(&DataKey::InvoiceNft, &invoice_nft);
        env.storage()
            .persistent()
            .set(&DataKey::StakingToken, &staking_token);
        env.storage()
            .persistent()
            .set(&DataKey::MinimumStake, &minimum_stake);
        env.storage()
            .persistent()
            .set(&DataKey::SlashPercentage, &slash_percentage_bps);
        events::registry_initialized(&env, &admin, &invoice_nft);
        Ok(())
    }

    /// Transfer admin role to a new address. Current admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `new_admin` — The address to transfer admin rights to.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits `admin_transferred` event.
    pub fn transfer_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Admin, &new_admin);
        Self::bump_persistent(&env, &DataKey::Admin);
        events::admin_transferred(&env, &admin, &new_admin);
        Self::append_audit_entry(&env, &admin, AdminActionType::RegistryTransferAdmin);
        Ok(())
    }

    /// Set (or update) the authorized `invoice_nft` contract address. Admin only.
    ///
    /// Called after deployment to wire up the `increment_invoice_count` integration,
    /// or again if `invoice_nft` is redeployed.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `invoice_nft` — The deployed `invoice_nft` contract address.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    /// - `RiskRegistryError::InvalidAddress` — `invoice_nft` is the contract's own address.
    ///
    /// **Security:** Requires `admin.require_auth()`. Idempotent — safe to call again if
    /// the invoice_nft contract is redeployed.
    pub fn set_invoice_nft(env: Env, admin: Address, invoice_nft: Address) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &invoice_nft)?;
        env.storage()
            .persistent()
            .set(&DataKey::InvoiceNft, &invoice_nft);
        Self::bump_persistent(&env, &DataKey::InvoiceNft);
        Ok(())
    }

    // ── Verifier management ───────────────────────────────────────────────────

    /// Admin adds a trusted verifier with required staking deposit.
    ///
    /// The verifier must have approved the staking token transfer before calling this.
    /// Initial reputation is set to 100; stake is transferred from verifier to this contract.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `verifier` — The verifier address to register.
    /// - `stake_amount` — Amount of staking token to deposit (must be ≥ `minimum_stake`).
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    /// - `RiskRegistryError::InvalidAddress` — `verifier` is the contract's own address.
    /// - `RiskRegistryError::InsufficientFunds` — `stake_amount` < `minimum_stake`.
    /// - `RiskRegistryError::NotInitialized` — Staking token or minimum stake not configured.
    ///
    /// **Security:** Requires `admin.require_auth()`. Transfers stake from `verifier` to
    /// this contract via the staking token. Emits `verifier_added` event.
    pub fn add_verifier(env: Env, admin: Address, verifier: Address, stake_amount: i128) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &verifier)?;

        // Guard: reject re-registration of an already-active verifier.
        // Re-registering would silently overwrite VerifierStake (stranding funds if
        // the new amount is lower) and reset VerifierReputation to 100, laundering
        // any slashing history accumulated via record_default. Use top_up_stake to
        // legitimately increase an existing verifier's collateral.
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Verifier(verifier.clone()))
            .unwrap_or(false)
        {
            return Err(RiskRegistryError::AlreadyInitialized);
        }

        let minimum_stake: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::MinimumStake)
            .ok_or(RiskRegistryError::NotInitialized)?;

        if stake_amount < minimum_stake {
            return Err(RiskRegistryError::InsufficientFunds);
        }

        let token_addr: Address = env
            .storage()
            .persistent()
            .get(&DataKey::StakingToken)
            .ok_or(RiskRegistryError::NotInitialized)?;

        let token_client = soroban_sdk::token::Client::new(&env, &token_addr);
        token_client.transfer(&verifier, &env.current_contract_address(), &stake_amount);

        env.storage()
            .persistent()
            .set(&DataKey::Verifier(verifier.clone()), &true);
        env.storage()
            .persistent()
            .set(&DataKey::VerifierStake(verifier.clone()), &stake_amount);
        env.storage()
            .persistent()
            .set(&DataKey::VerifierReputation(verifier.clone()), &100u32);
        Self::bump_persistent(&env, &DataKey::Verifier(verifier.clone()));
        Self::bump_persistent(&env, &DataKey::VerifierStake(verifier.clone()));
        Self::bump_persistent(&env, &DataKey::VerifierReputation(verifier.clone()));
        // TODO: Sync with access_control: AccessControlContractClient::new(&env, &access_control).grant_role(&admin, &verifier, Role::Verifier)?;
        events::verifier_added(&env, &admin, &verifier);
        Self::append_audit_entry(&env, &admin, AdminActionType::AddVerifier);
        Ok(())
    }

    /// Increase an existing active verifier's stake without resetting their reputation.
    ///
    /// This is the correct path for legitimately increasing a verifier's collateral.
    /// Unlike `add_verifier`, this adds to the existing tracked stake (never overwrites)
    /// and does not touch `VerifierReputation`.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `verifier` — An already-active verifier address.
    /// - `additional_amount` — Extra stake to deposit (must be > 0).
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    /// - `RiskRegistryError::NotVerifier` — Address is not an active verifier.
    /// - `RiskRegistryError::InvalidAmount` — `additional_amount` ≤ 0.
    /// - `RiskRegistryError::ArithmeticOverflow` — Stake addition overflowed.
    pub fn top_up_stake(
        env: Env,
        admin: Address,
        verifier: Address,
        additional_amount: i128,
    ) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if additional_amount <= 0 {
            return Err(RiskRegistryError::InvalidAmount);
        }

        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Verifier(verifier.clone()))
            .unwrap_or(false)
        {
            return Err(RiskRegistryError::NotVerifier);
        }

        let token_addr: Address = env
            .storage()
            .persistent()
            .get(&DataKey::StakingToken)
            .ok_or(RiskRegistryError::NotInitialized)?;

        let token_client = soroban_sdk::token::Client::new(&env, &token_addr);
        token_client.transfer(&verifier, &env.current_contract_address(), &additional_amount);

        let current_stake: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VerifierStake(verifier.clone()))
            .unwrap_or(0);

        let new_stake = current_stake
            .checked_add(additional_amount)
            .ok_or(RiskRegistryError::ArithmeticOverflow)?;

        env.storage()
            .persistent()
            .set(&DataKey::VerifierStake(verifier.clone()), &new_stake);
        Self::bump_persistent(&env, &DataKey::VerifierStake(verifier.clone()));
        Self::append_audit_entry(&env, &admin, AdminActionType::AddVerifier);
        Ok(())
    }

    /// Admin removes a verifier and returns their remaining stake.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `verifier` — The verifier address to remove.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    /// - `RiskRegistryError::NotVerifier` — Address is not a registered verifier.
    ///
    /// **Security:** Requires `admin.require_auth()`. Returns any remaining (unslashed) stake to
    /// the verifier. Removes all three verifier records (flag, stake, reputation). Emits
    /// `verifier_removed` event.
    pub fn remove_verifier(env: Env, admin: Address, verifier: Address) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        // Only remove if it actually exists — avoids a no-op silently succeeding
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Verifier(verifier.clone()))
            .unwrap_or(false)
        {
            return Err(RiskRegistryError::NotVerifier);
        }

        let stake: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VerifierStake(verifier.clone()))
            .unwrap_or(0);

        if stake > 0 {
            let token_addr: Address = env
                .storage()
                .persistent()
                .get(&DataKey::StakingToken)
                .ok_or(RiskRegistryError::NotInitialized)?;
            let token_client = soroban_sdk::token::Client::new(&env, &token_addr);
            token_client.transfer(&env.current_contract_address(), &verifier, &stake);
        }

        env.storage()
            .persistent()
            .remove(&DataKey::Verifier(verifier.clone()));
        env.storage()
            .persistent()
            .remove(&DataKey::VerifierStake(verifier.clone()));
        env.storage()
            .persistent()
            .remove(&DataKey::VerifierReputation(verifier.clone()));
        // TODO: Sync with access_control: AccessControlContractClient::new(&env, &access_control).revoke_role(&admin, &verifier)?;
        events::verifier_removed(&env, &admin, &verifier);
        Self::append_audit_entry(&env, &admin, AdminActionType::RemoveVerifier);
        Ok(())
    }

    // ── Sub-account delegation ────────────────────────────────────────────────

    /// Primary verifier delegates action rights to a sub-account.
    /// The primary verifier must be registered. The sub-account must not already
    /// be a primary verifier or an existing sub-account of another verifier.
    /// All actions performed by sub-accounts are attributed to the primary verifier.
    pub fn add_sub_account(
        env: Env,
        primary: Address,
        sub_account: Address,
    ) -> Result<(), RiskRegistryError> {
        primary.require_auth();
        Self::require_verifier_primary(&env, &primary)?;

        // sub_account must not itself be a primary verifier
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Verifier(sub_account.clone()))
            .unwrap_or(false)
        {
            return Err(RiskRegistryError::InvalidAddress);
        }

        // sub_account must not already be registered under another primary
        if env
            .storage()
            .persistent()
            .has(&DataKey::SubAccount(sub_account.clone()))
        {
            return Err(RiskRegistryError::AlreadyInitialized);
        }

        env.storage()
            .persistent()
            .set(&DataKey::SubAccount(sub_account.clone()), &primary);
        Self::bump_persistent(&env, &DataKey::SubAccount(sub_account.clone()));
        events::sub_account_added(&env, &primary, &sub_account);
        Ok(())
    }

    /// Remove a previously delegated sub-account. Primary verifier only.
    pub fn remove_sub_account(
        env: Env,
        primary: Address,
        sub_account: Address,
    ) -> Result<(), RiskRegistryError> {
        primary.require_auth();
        Self::require_verifier_primary(&env, &primary)?;

        let stored_primary: Address = env
            .storage()
            .persistent()
            .get(&DataKey::SubAccount(sub_account.clone()))
            .ok_or(RiskRegistryError::NotVerifier)?;

        if stored_primary != primary {
            return Err(RiskRegistryError::Unauthorized);
        }

        env.storage()
            .persistent()
            .remove(&DataKey::SubAccount(sub_account.clone()));
        events::sub_account_removed(&env, &primary, &sub_account);
        Ok(())
    }

    // ── SME management ────────────────────────────────────────────────────────

    /// Verifier registers and scores an SME. Fails if SME is already registered.
    ///
    /// **Parameters:**
    /// - `verifier` — A registered verifier address (must sign).
    /// - `sme` — The SME address to register.
    /// - `risk_score` — Credit score 0–100. Maps to a `RiskTier` in `invoice_nft`.
    /// - `compliance_attested` — Whether the verifier attests the SME is KYC/AML compliant.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotVerifier` — Caller is not a registered verifier.
    /// - `RiskRegistryError::InvalidRiskScore` — `risk_score` > 100.
    /// - `RiskRegistryError::AlreadyInitialized` — SME is already registered (prevents silent re-registration
    ///   that would reset `defaults` and `total_invoices` counters).
    ///
    /// **Security:** Requires `verifier.require_auth()`. Emits `sme_registered` event.
    pub fn register_sme(
        env: Env,
        verifier: Address,
        sme: Address,
        risk_score: u32,
        compliance_attested: bool,
    ) -> Result<(), RiskRegistryError> {
        verifier.require_auth();
        // Resolve to primary verifier so sub-accounts attribute registration correctly.
        let primary = Self::resolve_verifier(&env, &verifier)?;
        require_valid_risk_score(risk_score)?;

        // Guard against silent re-registration that would reset defaults/invoice counts
        if env
            .storage()
            .persistent()
            .has(&DataKey::SmeProfile(sme.clone()))
        {
            return Err(RiskRegistryError::AlreadyInitialized);
        }

        let profile = SmeProfile {
            address: sme.clone(),
            verified: true,
            verifier: primary.clone(),
            risk_score,
            total_invoices: 0,
            defaults: 0,
            registered_at: env.ledger().timestamp(),
            compliance_attested,
            credit_limit: 0,
        };

        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_registered(&env, &primary, &sme, risk_score);
        Ok(())
    }

    /// Update SME risk score. Verifier only.
    ///
    /// **Parameters:**
    /// - `verifier` — A registered verifier address (must sign). Must be the
    ///   verifier-of-record for this SME (i.e. the verifier who registered them).
    /// - `sme` — The SME whose score is being updated.
    /// - `new_score` — The new risk score (0–100).
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotVerifier` — Caller is not a registered verifier.
    /// - `RiskRegistryError::NotSmeVerifier` — Caller is not the verifier-of-record for this SME.
    /// - `RiskRegistryError::InvalidRiskScore` — `new_score` > 100.
    /// - `RiskRegistryError::SMENotRegistered` — SME has not been registered.
    /// - `RiskRegistryError::Reentrancy` — Reentrancy guard triggered.
    ///
    /// **Security:** Requires `verifier.require_auth()`. Emits `sme_score_updated` event.
    pub fn update_sme_score(
        env: Env,
        verifier: Address,
        sme: Address,
        new_score: u32,
    ) -> Result<(), RiskRegistryError> {
        verifier.require_auth();
        let primary = Self::resolve_verifier(&env, &verifier)?;
        require_valid_risk_score(new_score)?;

        let _guard = ReentrancyGuard::new(&env)?;

        let mut profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&DataKey::SmeProfile(sme.clone()))
            .ok_or(RiskRegistryError::SMENotRegistered)?;

        // Enforce verifier-of-record: only the verifier who registered this SME
        // (or their sub-accounts resolving to that same primary) may update its score.
        if profile.verifier != primary {
            return Err(RiskRegistryError::NotSmeVerifier);
        }

        profile.risk_score = new_score;
        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_score_updated(&env, &primary, &sme, new_score);
        Ok(())
    }

    /// Set (or update) an SME's aggregate credit limit. Verifier only.
    ///
    /// The credit limit is the maximum outstanding face value the SME may have across all
    /// non-Repaid, non-Defaulted invoices. Set to 0 to remove the limit.
    ///
    /// **Parameters:**
    /// - `verifier` — A registered verifier address (must sign). Must be the
    ///   verifier-of-record for this SME.
    /// - `sme` — The SME to update.
    /// - `credit_limit` — The new limit in stroops (≥ 0). 0 means uncapped.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotVerifier` — Caller is not a registered verifier.
    /// - `RiskRegistryError::NotSmeVerifier` — Caller is not the verifier-of-record for this SME.
    /// - `RiskRegistryError::InvalidAmount` — `credit_limit` is negative.
    /// - `RiskRegistryError::SMENotRegistered` — SME has not been registered.
    ///
    /// **Security:** Requires `verifier.require_auth()`. Emits `sme_credit_limit_set` event.
    pub fn set_credit_limit(
        env: Env,
        verifier: Address,
        sme: Address,
        credit_limit: i128,
    ) -> Result<(), RiskRegistryError> {
        verifier.require_auth();
        let primary = Self::resolve_verifier(&env, &verifier)?;
        Self::require_verifier(&env, &verifier)?;
        require_non_negative_amount(credit_limit)?;
        if credit_limit < 0 {
            return Err(RiskRegistryError::InvalidAmount);
        }

        let mut profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&DataKey::SmeProfile(sme.clone()))
            .ok_or(RiskRegistryError::SMENotRegistered)?;

        // Enforce verifier-of-record: only the assigned verifier may change the credit limit.
        if profile.verifier != primary {
            return Err(RiskRegistryError::NotSmeVerifier);
        }

        profile.credit_limit = credit_limit;
        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_credit_limit_set(&env, &primary, &sme, credit_limit);
        Ok(())
    }

    /// Reassign an SME to a new verifier-of-record. Admin only.
    ///
    /// Used when the original verifier has been removed (via `remove_verifier`) or when
    /// the admin explicitly transfers custody of an SME to a different verifier.
    /// After reassignment the new verifier becomes the sole authority for `update_sme_score`
    /// and `set_credit_limit` on that SME.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `sme` — The SME whose verifier-of-record is being changed.
    /// - `new_verifier` — An active registered verifier to assign as the new verifier-of-record.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    /// - `RiskRegistryError::SMENotRegistered` — SME has not been registered.
    /// - `RiskRegistryError::NotVerifier` — `new_verifier` is not an active verifier.
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits `sme_verifier_reassigned` event.
    pub fn reassign_sme_verifier(
        env: Env,
        admin: Address,
        sme: Address,
        new_verifier: Address,
    ) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        // new_verifier must be an active primary verifier
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Verifier(new_verifier.clone()))
            .unwrap_or(false)
        {
            return Err(RiskRegistryError::NotVerifier);
        }

        let mut profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&DataKey::SmeProfile(sme.clone()))
            .ok_or(RiskRegistryError::SMENotRegistered)?;

        profile.verifier = new_verifier.clone();
        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_verifier_reassigned(&env, &admin, &sme, &new_verifier);
        Self::append_audit_entry(&env, &admin, AdminActionType::RecordDefault); // reuse closest action type
        Ok(())
    }

    /// Increment invoice count for an SME.
    ///
    /// Called automatically by `invoice_nft` when a new invoice is minted. Restricted to
    /// the invoice_nft contract address set at initialization.
    ///
    /// **Parameters:**
    /// - `caller` — Must be the authorized `invoice_nft` contract address.
    /// - `sme` — The SME whose invoice count is being incremented.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::Unauthorized` — Caller is not the authorized `invoice_nft` contract.
    /// - `RiskRegistryError::SMENotRegistered` — SME has not been registered.
    /// - `RiskRegistryError::ArithmeticOverflow` — Invoice count overflowed (extremely unlikely).
    ///
    /// **Security:** Requires `caller.require_auth()`. Only `invoice_nft` may call this.
    pub fn increment_invoice_count(
        env: Env,
        caller: Address,
        sme: Address,
    ) -> Result<(), RiskRegistryError> {
        caller.require_auth();
        Self::require_invoice_nft(&env, &caller)?;

        let mut profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&DataKey::SmeProfile(sme.clone()))
            .ok_or(RiskRegistryError::SMENotRegistered)?;

        profile.total_invoices = profile
            .total_invoices
            .checked_add(1)
            .ok_or(RiskRegistryError::ArithmeticOverflow)?;

        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_invoice_count_incremented(&env, &sme, profile.total_invoices);
        Ok(())
    }

    /// Record a default against an SME. Admin only. Slashes the responsible verifier's
    /// stake and decrements their reputation score by 10.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `sme` — The SME address that defaulted on an invoice.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    /// - `RiskRegistryError::SMENotRegistered` — SME has not been registered.
    /// - `RiskRegistryError::NotInitialized` — `SlashPercentage` was not set during initialization.
    /// - `RiskRegistryError::ArithmeticOverflow` — Default counter overflow (extremely unlikely).
    /// - `RiskRegistryError::ArithmeticUnderflow` — Slash computation underflowed.
    /// - `RiskRegistryError::Reentrancy` — Reentrancy guard triggered.
    ///
    /// **Security:** Requires `admin.require_auth()`. Verifier's stake is reduced by
    /// `current_stake * slash_percentage_bps / 10_000`. Reputation floors at 0.
    /// Emits `sme_default_recorded` event.
    pub fn record_default(env: Env, admin: Address, sme: Address) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let _guard = ReentrancyGuard::new(&env)?;

        let mut profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&DataKey::SmeProfile(sme.clone()))
            .ok_or(RiskRegistryError::SMENotRegistered)?;

        profile.defaults = profile
            .defaults
            .checked_add(1)
            .ok_or(RiskRegistryError::ArithmeticOverflow)?;

        let verifier = profile.verifier.clone();
        let slash_percentage: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::SlashPercentage)
            .ok_or(RiskRegistryError::NotInitialized)?;

        let current_stake: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::VerifierStake(verifier.clone()))
            .unwrap_or(0);

        if current_stake > 0 {
            let slash_amount = (current_stake as u128 * slash_percentage as u128 / 10_000) as i128;
            let remaining_stake = current_stake.checked_sub(slash_amount)
                .ok_or(RiskRegistryError::ArithmeticUnderflow)?;

            env.storage()
                .persistent()
                .set(&DataKey::VerifierStake(verifier.clone()), &remaining_stake);
            Self::bump_persistent(&env, &DataKey::VerifierStake(verifier.clone()));
        }

        let current_reputation: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::VerifierReputation(verifier.clone()))
            .unwrap_or(100);

        let new_reputation = current_reputation.saturating_sub(10);
        env.storage()
            .persistent()
            .set(&DataKey::VerifierReputation(verifier.clone()), &new_reputation);
        Self::bump_persistent(&env, &DataKey::VerifierReputation(verifier.clone()));

        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_default_recorded(&env, &admin, &sme, profile.defaults);
        Self::append_audit_entry(&env, &admin, AdminActionType::RecordDefault);
        Ok(())
    }

    /// Store a debtor risk score keyed by debtor hash. Verifier only.
    ///
    /// Enforces a per-(verifier, debtor_hash) cooldown of MIN_SCORE_UPDATE_INTERVAL seconds
    /// between consecutive updates so that rapid score changes immediately before a
    /// funding or default decision cannot be used to manipulate outcomes.
    pub fn set_debtor_score(
        env: Env,
        verifier: Address,
        debtor_hash: Bytes,
        score: u32,
    ) -> Result<(), RiskRegistryError> {
        verifier.require_auth();
        Self::require_verifier(&env, &verifier)?;
        // Validate exact 32-byte SHA-256 length before score
        require_exact_length(&debtor_hash, 32)?;
        require_valid_risk_score(score)?;

        // Enforce cooldown: same verifier cannot update the same debtor_hash within
        // MIN_SCORE_UPDATE_INTERVAL seconds of the previous update.
        let cooldown_key = DataKey::DebtorScoreLastUpdate(verifier.clone(), debtor_hash.clone());
        if let Some(last_update) = env.storage().persistent().get::<_, u64>(&cooldown_key) {
            let next_allowed = last_update
                .checked_add(MIN_SCORE_UPDATE_INTERVAL)
                .ok_or(RiskRegistryError::ArithmeticOverflow)?;
            if env.ledger().timestamp() < next_allowed {
                return Err(RiskRegistryError::ScoreUpdateCooldownNotElapsed);
            }
        }

        let attestation_key = DataKey::DebtorScoreAttestation(debtor_hash.clone(), verifier.clone());
        env.storage().persistent().set(&attestation_key, &score);
        Self::bump_persistent(&env, &attestation_key);

        let attestors_key = DataKey::DebtorAttestors(debtor_hash.clone());
        let mut attestors: Vec<Address> = env
            .storage()
            .persistent()
            .get(&attestors_key)
            .unwrap_or_else(|| Vec::new(&env));

        if !attestors.contains(&verifier) {
            attestors.push_back(verifier.clone());
            env.storage().persistent().set(&attestors_key, &attestors);
        }
        Self::bump_persistent(&env, &attestors_key);

        // Record the update timestamp so the next call can check the cooldown.
        let now = env.ledger().timestamp();
        env.storage().persistent().set(&cooldown_key, &now);
        Self::bump_persistent(&env, &cooldown_key);

        events::debtor_score_set(&env, &verifier, &debtor_hash, score);
        Ok(())
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    /// Retrieve the full SME profile for a registered SME.
    ///
    /// **Parameters:**
    /// - `sme` — The SME address to query.
    ///
    /// **Returns:** The `SmeProfile` struct.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::SMENotRegistered` — SME has not been registered.
    ///
    /// **Security:** Read-only view. No authorization required. Bumps the profile's TTL.
    pub fn get_sme_profile(env: Env, sme: Address) -> Result<SmeProfile, RiskRegistryError> {
        let key = DataKey::SmeProfile(sme);
        let profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(RiskRegistryError::SMENotRegistered)?;
        Self::bump_persistent(&env, &key);
        Ok(profile)
    }

    /// Returns `true` if the SME has been registered and verified by a verifier.
    ///
    /// **Parameters:**
    /// - `sme` — The SME address to query.
    ///
    /// **Returns:** `true` if registered and `verified == true`, `false` otherwise.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn is_verified_sme(env: Env, sme: Address) -> bool {
        env.storage()
            .persistent()
            .get::<DataKey, SmeProfile>(&DataKey::SmeProfile(sme))
            .map(|p| p.verified)
            .unwrap_or(false)
    }

    /// Returns `true` if the verifier attested that the SME passed KYC/AML compliance checks.
    ///
    /// **Parameters:**
    /// - `sme` — The SME address to query.
    ///
    /// **Returns:** `true` if `compliance_attested == true` in the SME's profile, `false` otherwise
    /// (including when the SME is not registered at all).
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn is_compliance_attested(env: Env, sme: Address) -> bool {
        env.storage()
            .persistent()
            .get::<DataKey, SmeProfile>(&DataKey::SmeProfile(sme))
            .map(|p| p.compliance_attested)
            .unwrap_or(false)
    }

    /// Returns the current staked token amount for a verifier.
    ///
    /// **Parameters:**
    /// - `verifier` — The verifier address to query.
    ///
    /// **Returns:** The staked amount in the staking token's smallest unit. Returns `0` if the
    /// verifier is not registered or their stake has been fully slashed.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_verifier_stake(env: Env, verifier: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::VerifierStake(verifier))
            .unwrap_or(0)
    }

    /// Returns the current reputation score for a verifier (scale: 0–100).
    ///
    /// Starts at 100 when a verifier is added. Decremented by 10 on each recorded SME default.
    /// Floors at 0. Returns `0` for unregistered verifiers.
    ///
    /// **Parameters:**
    /// - `verifier` — The verifier address to query.
    ///
    /// **Returns:** Reputation score in range `[0, 100]`.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_verifier_reputation(env: Env, verifier: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::VerifierReputation(verifier))
            .unwrap_or(0)
    }

    /// Returns `true` if the address is a currently registered verifier.
    ///
    /// **Parameters:**
    /// - `verifier` — The address to query.
    ///
    /// **Returns:** `true` if the address is an active verifier, `false` otherwise.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn is_verifier(env: Env, verifier: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Verifier(verifier))
            .unwrap_or(false)
    }

    /// Returns the primary verifier for a sub-account address, or `None` if the address
    /// is not registered as a sub-account.
    pub fn get_primary_verifier(env: Env, sub_account: Address) -> Option<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::SubAccount(sub_account))
    }

    /// Returns `true` if `addr` is an active sub-account of any primary verifier.
    pub fn is_sub_account(env: Env, addr: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::SubAccount(addr))
    }

    /// Returns the debtor score or `RiskRegistryError::DebtorNotRegistered` if not found.
    pub fn get_debtor_score(env: Env, debtor_hash: Bytes) -> Result<u32, RiskRegistryError> {
        let attestors_key = DataKey::DebtorAttestors(debtor_hash.clone());
        let attestors: Vec<Address> = env
            .storage()
            .persistent()
            .get(&attestors_key)
            .ok_or(RiskRegistryError::DebtorNotRegistered)?;
        Self::bump_persistent(&env, &attestors_key);

        let mut total_score: u64 = 0;
        let mut count: u32 = 0;

        for verifier in attestors.iter() {
            if Self::is_verifier(env.clone(), verifier.clone()) {
                let key = DataKey::DebtorScoreAttestation(debtor_hash.clone(), verifier);
                if let Some(score) = env.storage().persistent().get::<_, u32>(&key) {
                    total_score = total_score
                        .checked_add(score as u64)
                        .ok_or(RiskRegistryError::ArithmeticOverflow)?;
                    count += 1;
                    Self::bump_persistent(&env, &key);
                }
            }
        }

        if count == 0 {
            return Err(RiskRegistryError::DebtorNotRegistered);
        }

        let avg = total_score / (count as u64);
        Ok(avg as u32)
    }

    /// Returns a specific verifier's score attestation for a debtor,
    /// or `RiskRegistryError::DebtorNotRegistered` if not found.
    pub fn get_debtor_score_attestation(
        env: Env,
        verifier: Address,
        debtor_hash: Bytes,
    ) -> Result<u32, RiskRegistryError> {
        let key = DataKey::DebtorScoreAttestation(debtor_hash, verifier);
        let score: u32 = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(RiskRegistryError::DebtorNotRegistered)?;
        Self::bump_persistent(&env, &key);
        Ok(score)
    }

    /// Returns the current admin address.
    ///
    /// **Returns:** The admin `Address`.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotInitialized` — Contract has not been initialized.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_admin(env: Env) -> Result<Address, RiskRegistryError> {
        env.storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(RiskRegistryError::NotInitialized)
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    /// Propose a WASM upgrade. Admin only. Begins a 24-hour timelock.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `new_wasm_hash` — SHA-256 hash of the new WASM binary (32 bytes).
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. Apply with `execute_upgrade` after
    /// `UPGRADE_TIMELOCK_DELAY` (24 h) has elapsed. Emits `upgrade_proposed` event.
    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::UpgradeProposal, &(new_wasm_hash.clone(), env.ledger().timestamp()));
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        Self::append_audit_entry(&env, &admin, AdminActionType::RegistryProposeUpgrade);
        Ok(())
    }

    /// Execute a previously proposed WASM upgrade after the 24-hour timelock has elapsed.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    ///
    /// **Errors:**
    /// - `RiskRegistryError::NotAdmin` — Caller is not the admin.
    /// - `RiskRegistryError::NoUpgradeProposed` — No upgrade proposal is pending.
    /// - `RiskRegistryError::UpgradeTimelockNotElapsed` — 24-hour timelock has not yet passed.
    ///
    /// **Security:** Requires `admin.require_auth()`. Clears the proposal atomically before
    /// executing. Emits `upgrade_executed` event.
    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), RiskRegistryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(RiskRegistryError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(RiskRegistryError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        events::upgrade_executed(&env, &admin, &wasm_hash);
        Self::append_audit_entry(&env, &admin, AdminActionType::RegistryExecuteUpgrade);
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

    fn require_admin(env: &Env, caller: &Address) -> Result<(), RiskRegistryError> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(RiskRegistryError::NotInitialized)?;
        if &admin != caller {
            return Err(RiskRegistryError::NotAdmin);
        }
        Ok(())
    }

    /// Returns `Ok(primary)` when `caller` is an active verifier (primary or sub-account).
    /// Sub-accounts resolve to their primary verifier so that reputation and staking
    /// are always attributed to the primary.
    fn resolve_verifier(env: &Env, caller: &Address) -> Result<Address, RiskRegistryError> {
        // Direct primary check first (fast path).
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Verifier(caller.clone()))
            .unwrap_or(false)
        {
            return Ok(caller.clone());
        }
        // Sub-account resolution.
        if let Some(primary) = env
            .storage()
            .persistent()
            .get::<_, Address>(&DataKey::SubAccount(caller.clone()))
        {
            // Ensure the primary is still an active verifier.
            if env
                .storage()
                .persistent()
                .get::<_, bool>(&DataKey::Verifier(primary.clone()))
                .unwrap_or(false)
            {
                return Ok(primary);
            }
        }
        Err(RiskRegistryError::NotVerifier)
    }

    fn require_verifier(env: &Env, caller: &Address) -> Result<(), RiskRegistryError> {
        Self::resolve_verifier(env, caller).map(|_| ())
    }

    /// Require `caller` to be a **primary** verifier (not a sub-account).
    /// Used for delegation management so only the primary can add/remove sub-accounts.
    fn require_verifier_primary(env: &Env, caller: &Address) -> Result<(), RiskRegistryError> {
        let ok: bool = env
            .storage()
            .persistent()
            .get(&DataKey::Verifier(caller.clone()))
            .unwrap_or(false);
        if !ok {
            return Err(RiskRegistryError::NotVerifier);
        }
        Ok(())
    }

    fn require_invoice_nft(env: &Env, caller: &Address) -> Result<(), RiskRegistryError> {
        let invoice_nft: Address = env
            .storage()
            .persistent()
            .get(&DataKey::InvoiceNft)
            .ok_or(RiskRegistryError::NotInitialized)?;
        if &invoice_nft != caller {
            return Err(RiskRegistryError::Unauthorized);
        }
        Ok(())
    }

    /// Extend TTL on a persistent entry if it's below the threshold.
    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_BUMP);
    }

    fn append_audit_entry(env: &Env, actor: &Address, action: AdminActionType) {
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
            source: AuditSource::RiskRegistry,
            token: None,
            amount: None,
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
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Events as _, Ledger, LedgerInfo},
        Bytes, Env,
    };

    /// Returns (env, admin, invoice_nft, staking_token, client). `staking_token`
    /// is a real deployed Stellar asset contract so `add_verifier`'s stake
    /// transfer has a genuine contract instance to call into.
    fn setup() -> (Env, Address, Address, Address, RiskRegistryContractClient<'static>) {
        let env = Env::default();
        // add_verifier's stake transfer requires verifier.require_auth() from
        // inside a nested call (risk_registry -> token), which isn't tied to
        // the root invocation — plain mock_all_auths() rejects that.
        env.mock_all_auths_allowing_non_root_auth();
        let contract_id = env.register_contract(None, RiskRegistryContract);
        let client = RiskRegistryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let invoice_nft = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let staking_token = env.register_stellar_asset_contract_v2(token_admin).address();
        client.initialize(&admin, &invoice_nft, &staking_token, &1_000_000i128, &5_000u32);
        (env, admin, invoice_nft, staking_token, client)
    }

    /// Mints `amount` of `token` to `to`. The Stellar Asset Contract's admin
    /// auth requirement is satisfied automatically via `env.mock_all_auths()`.
    fn mint_stake(env: &Env, token: &Address, to: &Address, amount: i128) {
        soroban_sdk::token::StellarAssetClient::new(env, token).mint(to, &amount);
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_success() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RiskRegistryContract);
        let client = RiskRegistryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let invoice_nft = Address::generate(&env);
        let staking_token = Address::generate(&env);
        assert!(client.try_initialize(&admin, &invoice_nft, &staking_token, &1_000_000i128, &5_000u32).is_ok());
    }

    #[test]
    fn test_initialize_already_initialized() {
        let (env, admin, invoice_nft, staking_token, client) = setup();
        assert!(client.try_initialize(&admin, &invoice_nft, &staking_token, &1_000_000i128, &5_000u32).is_err());
    }

    // ── transfer_admin ────────────────────────────────────────────────────────

    #[test]
    fn test_transfer_admin_success() {
        let (env, admin, _, staking_token, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        assert_eq!(client.get_admin(), new_admin);
    }

    #[test]
    fn test_transfer_admin_requires_admin() {
        let (env, _, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        assert!(client.try_transfer_admin(&stranger, &new_admin).is_err());
    }

    // ── add_verifier / remove_verifier ────────────────────────────────────────

    #[test]
    fn test_add_verifier_success() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        assert!(client.try_add_verifier(&admin, &verifier, &1_000_000i128).is_ok());
        assert!(client.is_verifier(&verifier));
    }

    #[test]
    fn test_add_verifier_not_admin() {
        let (env, _, _, staking_token, client) = setup();
        let stranger = Address::generate(&env);
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        assert!(client.try_add_verifier(&stranger, &verifier, &1_000_000i128).is_err());
    }

    #[test]
    fn test_remove_verifier_success() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client.is_verifier(&verifier));
        assert!(client.try_remove_verifier(&admin, &verifier).is_ok());
        assert!(!client.is_verifier(&verifier));
    }

    #[test]
    fn test_remove_verifier_not_admin() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let stranger = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client.try_remove_verifier(&stranger, &verifier).is_err());
    }

    #[test]
    fn test_remove_verifier_not_registered() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        assert!(client.try_remove_verifier(&admin, &verifier).is_err());
    }

    #[test]
    fn test_multiple_verifiers() {
        let (env, admin, _, staking_token, client) = setup();
        let v1 = Address::generate(&env);
        let v2 = Address::generate(&env);
        let sme1 = Address::generate(&env);
        let sme2 = Address::generate(&env);
        mint_stake(&env, &staking_token, &v1, 1_000_000i128);
        client.add_verifier(&admin, &v1, &1_000_000i128);
        mint_stake(&env, &staking_token, &v2, 1_000_000i128);
        client.add_verifier(&admin, &v2, &1_000_000i128);
        client.register_sme(&v1, &sme1, &30u32, &true);
        client.register_sme(&v2, &sme2, &60u32, &true);
        assert_eq!(client.get_sme_profile(&sme1).risk_score, 30);
        assert_eq!(client.get_sme_profile(&sme2).risk_score, 60);
        assert_eq!(client.get_sme_profile(&sme1).verifier, v1);
        assert_eq!(client.get_sme_profile(&sme2).verifier, v2);
    }

    // ── register_sme ──────────────────────────────────────────────────────────

    #[test]
    fn test_register_sme_flow() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        assert!(client.is_verified_sme(&sme));
        let profile = client.get_sme_profile(&sme);
        assert_eq!(profile.risk_score, 35);
        assert_eq!(profile.defaults, 0);
        assert_eq!(profile.total_invoices, 0);
        assert!(profile.verified);
        assert_eq!(profile.verifier, verifier);
        assert!(profile.compliance_attested);
    }

    #[test]
    fn test_register_sme_duplicate_rejected() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        assert!(client.try_register_sme(&verifier, &sme, &50u32, &true).is_err());
    }

    #[test]
    fn test_register_sme_unverified_verifier() {
        let (env, _, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let sme = Address::generate(&env);
        assert!(client.try_register_sme(&stranger, &sme, &10u32, &true).is_err());
    }

    #[test]
    fn test_register_sme_invalid_risk_score() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client.try_register_sme(&verifier, &sme, &101u32, &true).is_err());
    }

    #[test]
    fn test_register_sme_preserves_history_on_re_registration_attempt() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        let _ = client.try_register_sme(&verifier, &sme, &99u32, &true);
        let profile = client.get_sme_profile(&sme);
        assert_eq!(profile.risk_score, 35); // unchanged
    }

    #[test]
    fn test_register_sme_compliance_attested_true() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &50u32, &true);
        let profile = client.get_sme_profile(&sme);
        assert!(profile.compliance_attested);
        assert!(client.is_compliance_attested(&sme));
    }

    #[test]
    fn test_register_sme_compliance_attested_false() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &50u32, &false);
        let profile = client.get_sme_profile(&sme);
        assert!(!profile.compliance_attested);
        assert!(!client.is_compliance_attested(&sme));
    }

    // ── update_sme_score ──────────────────────────────────────────────────────

    #[test]
    fn test_update_sme_score_success() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        client.update_sme_score(&verifier, &sme, &50u32);
        assert_eq!(client.get_sme_profile(&sme).risk_score, 50);
    }

    #[test]
    fn test_update_sme_score_not_registered() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client
            .try_update_sme_score(&verifier, &sme, &50u32)
            .is_err());
    }

    #[test]
    fn test_update_sme_score_invalid() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        assert!(client
            .try_update_sme_score(&verifier, &sme, &101u32)
            .is_err());
    }

    #[test]
    fn test_update_sme_score_boundary_values() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &50u32, &true);
        client.update_sme_score(&verifier, &sme, &0u32);
        assert_eq!(client.get_sme_profile(&sme).risk_score, 0);
        client.update_sme_score(&verifier, &sme, &100u32);
        assert_eq!(client.get_sme_profile(&sme).risk_score, 100);
    }

    // ── increment_invoice_count ───────────────────────────────────────────────

    #[test]
    fn test_increment_invoice_count() {
        let (env, admin, invoice_nft, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        assert_eq!(client.get_sme_profile(&sme).total_invoices, 0);
        client.increment_invoice_count(&invoice_nft, &sme);
        assert_eq!(client.get_sme_profile(&sme).total_invoices, 1);
    }

    #[test]
    fn test_increment_invoice_count_multiple() {
        let (env, admin, invoice_nft, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        for i in 1u32..=5 {
            client.increment_invoice_count(&invoice_nft, &sme);
            assert_eq!(client.get_sme_profile(&sme).total_invoices, i);
        }
    }

    #[test]
    fn test_increment_invoice_count_unauthorized_caller() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        let stranger = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        assert!(client.try_increment_invoice_count(&stranger, &sme).is_err());
    }

    #[test]
    fn test_increment_invoice_count_sme_not_registered() {
        let (env, _, invoice_nft, _, client) = setup();
        let sme = Address::generate(&env);
        assert!(client
            .try_increment_invoice_count(&invoice_nft, &sme)
            .is_err());
    }

    // ── record_default ────────────────────────────────────────────────────────

    #[test]
    fn test_record_default() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        assert_eq!(client.get_sme_profile(&sme).defaults, 0);
        client.record_default(&admin, &sme);
        assert_eq!(client.get_sme_profile(&sme).defaults, 1);
    }

    #[test]
    fn test_record_default_not_admin() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        let stranger = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        assert!(client.try_record_default(&stranger, &sme).is_err());
    }

    #[test]
    fn test_record_default_sme_not_registered() {
        let (env, admin, _, staking_token, client) = setup();
        let sme = Address::generate(&env);
        assert!(client.try_record_default(&admin, &sme).is_err());
    }

    #[test]
    fn test_record_multiple_defaults() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        client.record_default(&admin, &sme);
        client.record_default(&admin, &sme);
        client.record_default(&admin, &sme);
        assert_eq!(client.get_sme_profile(&sme).defaults, 3);
    }

    // ── set_debtor_score / get_debtor_score ───────────────────────────────────

    #[test]
    fn test_set_debtor_score() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xABu8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.set_debtor_score(&verifier, &debtor_hash, &45u32);
        assert_eq!(client.get_debtor_score(&debtor_hash), 45u32);
    }

    #[test]
    fn test_set_debtor_score_invalid_score() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xABu8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client
            .try_set_debtor_score(&verifier, &debtor_hash, &101u32)
            .is_err());
    }

    #[test]
    fn test_set_debtor_score_empty_hash() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let empty_hash = Bytes::from_slice(&env, &[]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client
            .try_set_debtor_score(&verifier, &empty_hash, &50u32)
            .is_err());
    }

    #[test]
    fn test_set_debtor_score_exact_32_bytes_accepted() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let hash = Bytes::from_slice(&env, &[0xABu8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client.try_set_debtor_score(&verifier, &hash, &50u32).is_ok());
    }

    #[test]
    fn test_set_debtor_score_31_bytes_rejected() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let hash = Bytes::from_slice(&env, &[0xABu8; 31]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_set_debtor_score(&verifier, &hash, &50u32);
        assert_eq!(result.unwrap_err().unwrap(), RiskRegistryError::InvalidLength);
    }

    #[test]
    fn test_set_debtor_score_33_bytes_rejected() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let hash = Bytes::from_slice(&env, &[0xABu8; 33]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_set_debtor_score(&verifier, &hash, &50u32);
        assert_eq!(result.unwrap_err().unwrap(), RiskRegistryError::InvalidLength);
    }

    #[test]
    fn test_set_debtor_score_not_verifier() {
        let (env, _, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xABu8; 32]);
        assert!(client
            .try_set_debtor_score(&stranger, &debtor_hash, &50u32)
            .is_err());
    }

    #[test]
    fn test_get_debtor_score_not_found() {
        let (env, _, _, _, client) = setup();
        let debtor_hash = Bytes::from_slice(&env, &[0xCDu8; 32]);
        assert!(client.try_get_debtor_score(&debtor_hash).is_err());
    }

    #[test]
    fn test_debtor_score_boundary_values() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let hash0 = Bytes::from_slice(&env, &[0x01u8; 32]);
        client.set_debtor_score(&verifier, &hash0, &0u32);
        assert_eq!(client.get_debtor_score(&hash0), 0u32);
        let hash100 = Bytes::from_slice(&env, &[0x02u8; 32]);
        client.set_debtor_score(&verifier, &hash100, &100u32);
        assert_eq!(client.get_debtor_score(&hash100), 100u32);
        let hash_invalid = Bytes::from_slice(&env, &[0x03u8; 32]);
        assert!(client
            .try_set_debtor_score(&verifier, &hash_invalid, &101u32)
            .is_err());
    }

    // ── views ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_get_sme_profile_not_found() {
        let (env, _, _, _, client) = setup();
        let sme = Address::generate(&env);
        assert!(client.try_get_sme_profile(&sme).is_err());
    }

    #[test]
    fn test_is_verified_sme_false_for_unregistered() {
        let (env, _, _, _, client) = setup();
        let sme = Address::generate(&env);
        assert!(!client.is_verified_sme(&sme));
    }

    // ── risk score boundary ───────────────────────────────────────────────────

    #[test]
    fn test_risk_score_boundary_values() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let sme0 = Address::generate(&env);
        client.register_sme(&verifier, &sme0, &0u32, &true);
        assert_eq!(client.get_sme_profile(&sme0).risk_score, 0);
        let sme100 = Address::generate(&env);
        client.register_sme(&verifier, &sme100, &100u32, &true);
        assert_eq!(client.get_sme_profile(&sme100).risk_score, 100);
        let sme_invalid = Address::generate(&env);
        assert!(client
            .try_register_sme(&verifier, &sme_invalid, &101u32, &true)
            .is_err());
    }

    // ── event emission ────────────────────────────────────────────────────────

    #[test]
    fn test_add_verifier_emits_event() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(!env.events().all().is_empty());
    }

    #[test]
    fn test_remove_verifier_emits_event() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let events_before = env.events().all().len();
        client.remove_verifier(&admin, &verifier);
        assert!(env.events().all().len() > events_before);
    }

    #[test]
    fn test_register_sme_emits_event() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let events_before = env.events().all().len();
        client.register_sme(&verifier, &sme, &42u32, &true);
        assert!(env.events().all().len() > events_before);
    }

    #[test]
    fn test_increment_invoice_count_emits_event() {
        let (env, admin, invoice_nft, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        let events_before = env.events().all().len();
        client.increment_invoice_count(&invoice_nft, &sme);
        assert!(env.events().all().len() > events_before);
        assert_eq!(client.get_sme_profile(&sme).total_invoices, 1);
    }

    #[test]
    fn test_failed_operations_do_not_emit_events() {
        let (env, _, _, staking_token, client) = setup();
        let stranger = Address::generate(&env);
        let verifier = Address::generate(&env);
        // `add_verifier` fails at the `NotAdmin` check (before ever touching the
        // staking token), so no funding is required here — but mint first anyway
        // so `events_before` is captured only after unrelated setup activity.
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        let events_before = env.events().all().len();
        let _ = client.try_add_verifier(&stranger, &verifier, &1_000_000i128);
        let _ = client.try_register_sme(&stranger, &verifier, &50u32, &true);
        let _ = client.try_record_default(&stranger, &verifier);
        assert_eq!(env.events().all().len(), events_before);
    }

    #[test]
    fn test_sme_default_event_carries_cumulative_count() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &80u32, &true);
        client.record_default(&admin, &sme);
        assert_eq!(client.get_sme_profile(&sme).defaults, 1);
        client.record_default(&admin, &sme);
        assert_eq!(client.get_sme_profile(&sme).defaults, 2);
        client.record_default(&admin, &sme);
        assert_eq!(client.get_sme_profile(&sme).defaults, 3);
    }

    #[test]
    fn test_initialize_self_as_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RiskRegistryContract);
        let client = RiskRegistryContractClient::new(&env, &contract_id);
        let invoice_nft = Address::generate(&env);
        let staking_token = Address::generate(&env);
        let result = client.try_initialize(
            &contract_id,
            &invoice_nft,
            &staking_token,
            &1_000_000i128,
            &5_000u32,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_add_verifier_self_as_verifier_rejected() {
        let (env, admin, _, staking_token, client) = setup();
        let contract_id = client.address.clone();
        mint_stake(&env, &staking_token, &contract_id, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&contract_id, &1_000_000i128);
        let result = client.try_add_verifier(&admin, &contract_id, &1_000_000i128);
        assert!(result.is_err());
    }

    // ── transfer_admin edge cases ─────────────────────────────────────────────

    #[test]
    fn test_transfer_admin_to_same_address_allowed() {
        // The contract imposes no uniqueness constraint on the new admin —
        // idempotent re-assignment should succeed (it's a no-op in effect).
        let (env, admin, _, _, client) = setup();
        assert!(client.try_transfer_admin(&admin, &admin).is_ok());
        assert_eq!(client.get_admin(), admin);
    }

    // ── update_sme_score with score = 0 ──────────────────────────────────────

    #[test]
    fn test_update_sme_score_to_zero() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &50u32, &true);
        // Score 0 is valid (lowest risk tier boundary).
        client.update_sme_score(&verifier, &sme, &0u32);
        assert_eq!(client.get_sme_profile(&sme).risk_score, 0);
    }

    // ── set_debtor_score update (overwrite after cooldown) ───────────────────

    #[test]
    fn test_set_debtor_score_update_existing() {
        // set_debtor_score overwrites the score — calling it a second time after the
        // cooldown elapses must persist the latest value.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xAAu8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.set_debtor_score(&verifier, &debtor_hash, &30u32);
        assert_eq!(client.get_debtor_score(&debtor_hash), 30);
        // Advance past the cooldown before the second update.
        env.ledger().set(LedgerInfo {
            timestamp: MIN_SCORE_UPDATE_INTERVAL,
            ..env.ledger().get()
        });
        client.set_debtor_score(&verifier, &debtor_hash, &75u32);
        assert_eq!(client.get_debtor_score(&debtor_hash), 75);
    }

    // ── verifier cannot register itself as an SME ─────────────────────────────

    #[test]
    fn test_verifier_cannot_register_itself_as_sme() {
        // A verifier registering itself as an SME would create a conflict of
        // interest. The require_not_self guard on add_verifier prevents a
        // contract from being added as verifier, but a human verifier address
        // could still call register_sme on itself — which is allowed by the
        // current design. This test documents the current behaviour.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        // A verifier registering themselves as an SME is permitted (no rule
        // prevents it). The test ensures it doesn't panic / silently fail.
        assert!(client.try_register_sme(&verifier, &verifier, &40u32, &true).is_ok());
        assert_eq!(client.get_sme_profile(&verifier).verifier, verifier);
    }

    // ── remove verifier while still registered as SME ─────────────────────────

    #[test]
    fn test_remove_verifier_does_not_affect_sme_profile() {
        // Removing a verifier's authorization must not delete SME profiles they
        // previously created — those profiles belong to the SMEs, not the verifier.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &40u32, &true);
        client.remove_verifier(&admin, &verifier);
        // SME profile still accessible.
        assert_eq!(client.get_sme_profile(&sme).risk_score, 40);
        // But the removed verifier can no longer update scores.
        assert!(client.try_update_sme_score(&verifier, &sme, &60u32).is_err());
    }

    // ── register_sme with score = 0 ───────────────────────────────────────────

    #[test]
    fn test_register_sme_score_zero() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        // Score 0 is valid: AAA tier.
        assert!(client.try_register_sme(&verifier, &sme, &0u32, &true).is_ok());
        assert_eq!(client.get_sme_profile(&sme).risk_score, 0);
    }

    // ── get_admin before initialization ───────────────────────────────────────

    #[test]
    fn test_get_admin_before_initialization_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RiskRegistryContract);
        let client = RiskRegistryContractClient::new(&env, &contract_id);
        assert!(client.try_get_admin().is_err());
    }

    // ── set_debtor_score cooldown ─────────────────────────────────────────────

    #[test]
    fn test_set_debtor_score_first_call_always_succeeds() {
        // There is no prior timestamp, so the first update is always allowed.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xBBu8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client
            .try_set_debtor_score(&verifier, &debtor_hash, &40u32)
            .is_ok());
    }

    #[test]
    fn test_set_debtor_score_cooldown_blocks_immediate_second_update() {
        // A second update before MIN_SCORE_UPDATE_INTERVAL seconds have passed
        // must be rejected with ScoreUpdateCooldownNotElapsed.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xCCu8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.set_debtor_score(&verifier, &debtor_hash, &40u32);
        // Advance time by one second less than the cooldown — still blocked.
        env.ledger().set(LedgerInfo {
            timestamp: MIN_SCORE_UPDATE_INTERVAL - 1,
            ..env.ledger().get()
        });
        let err = client
            .try_set_debtor_score(&verifier, &debtor_hash, &60u32)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, RiskRegistryError::ScoreUpdateCooldownNotElapsed);
    }

    #[test]
    fn test_set_debtor_score_cooldown_allows_update_at_exact_boundary() {
        // At exactly timestamp == last_update + MIN_SCORE_UPDATE_INTERVAL the
        // condition `current < next_allowed` is false, so the call must succeed.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xDDu8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        // First update at t=0.
        client.set_debtor_score(&verifier, &debtor_hash, &40u32);
        // Advance to exactly the boundary.
        env.ledger().set(LedgerInfo {
            timestamp: MIN_SCORE_UPDATE_INTERVAL,
            ..env.ledger().get()
        });
        assert!(client
            .try_set_debtor_score(&verifier, &debtor_hash, &55u32)
            .is_ok());
        assert_eq!(client.get_debtor_score(&debtor_hash), 55);
    }

    #[test]
    fn test_set_debtor_score_cooldown_is_per_verifier() {
        // Different verifiers operate independent cooldowns for the same debtor_hash.
        let (env, admin, _, staking_token, client) = setup();
        let verifier_a = Address::generate(&env);
        let verifier_b = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xEEu8; 32]);
        mint_stake(&env, &staking_token, &verifier_a, 1_000_000i128);
        client.add_verifier(&admin, &verifier_a, &1_000_000i128);
        mint_stake(&env, &staking_token, &verifier_b, 1_000_000i128);
        client.add_verifier(&admin, &verifier_b, &1_000_000i128);
        // verifier_a sets the score; its cooldown now ticks.
        client.set_debtor_score(&verifier_a, &debtor_hash, &40u32);
        // verifier_b has never updated this debtor → no cooldown → must succeed.
        assert!(client
            .try_set_debtor_score(&verifier_b, &debtor_hash, &55u32)
            .is_ok());

        // Both attestations preserved independently
        assert_eq!(client.get_debtor_score_attestation(&verifier_a, &debtor_hash), 40u32);
        assert_eq!(client.get_debtor_score_attestation(&verifier_b, &debtor_hash), 55u32);

        // Aggregate average: (40 + 55) / 2 = 47
        assert_eq!(client.get_debtor_score(&debtor_hash), 47u32);
    }

    #[test]
    fn test_debtor_score_aggregation_ignores_removed_verifier() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier_a = Address::generate(&env);
        let verifier_b = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xFFu8; 32]);
        mint_stake(&env, &staking_token, &verifier_a, 1_000_000i128);
        client.add_verifier(&admin, &verifier_a, &1_000_000i128);
        mint_stake(&env, &staking_token, &verifier_b, 1_000_000i128);
        client.add_verifier(&admin, &verifier_b, &1_000_000i128);

        client.set_debtor_score(&verifier_a, &debtor_hash, &20u32);
        client.set_debtor_score(&verifier_b, &debtor_hash, &80u32);
        assert_eq!(client.get_debtor_score(&debtor_hash), 50u32);

        // Remove verifier_a
        client.remove_verifier(&admin, &verifier_a);

        // Aggregate score now reflects active verifier_b only (80)
        assert_eq!(client.get_debtor_score(&debtor_hash), 80u32);
        // Individual attestation of verifier_a is still independently retrievable
        assert_eq!(client.get_debtor_score_attestation(&verifier_a, &debtor_hash), 20u32);
    }

    // ── add_verifier re-registration guard ───────────────────────────────────

    #[test]
    fn test_add_verifier_rejects_already_active_verifier() {
        // Re-registering an active verifier must fail — previously this silently
        // overwrote VerifierStake (stranding funds) and reset VerifierReputation.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 2_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_add_verifier(&admin, &verifier, &500_000i128);
        assert!(result.is_err());
        // Stake unchanged at original amount — no funds stranded
        assert_eq!(client.get_verifier_stake(&verifier), 1_000_000i128);
        assert!(client.is_verifier(&verifier));
    }

    #[test]
    fn test_regression_fund_stranding_prevented() {
        // Regression: second add_verifier with a lower amount must no longer
        // silently overwrite VerifierStake, stranding the difference on-chain.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 2_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert_eq!(client.get_verifier_stake(&verifier), 1_000_000i128);
        // Attempt re-register with a lower stake — must be rejected
        assert!(client.try_add_verifier(&admin, &verifier, &500_000i128).is_err());
        // Tracked stake is still 1_000_000, not silently reduced to 500_000
        assert_eq!(client.get_verifier_stake(&verifier), 1_000_000i128);
    }

    #[test]
    fn test_regression_reputation_reset_prevented() {
        // Regression: second add_verifier must not reset VerifierReputation to 100,
        // laundering slashing history recorded via record_default.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 2_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &40u32, &true);
        client.record_default(&admin, &sme);
        let rep_after_slash = client.get_verifier_reputation(&verifier);
        assert!(rep_after_slash < 100, "reputation should have been reduced");
        // Attempt re-register — rejected, reputation preserved
        let _ = client.try_add_verifier(&admin, &verifier, &1_000_000i128);
        assert_eq!(client.get_verifier_reputation(&verifier), rep_after_slash);
    }

    #[test]
    fn test_top_up_stake_increases_stake_additively() {
        // top_up_stake must ADD to existing tracked stake, not overwrite it.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 3_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert_eq!(client.get_verifier_stake(&verifier), 1_000_000i128);
        client.top_up_stake(&admin, &verifier, &500_000i128);
        assert_eq!(client.get_verifier_stake(&verifier), 1_500_000i128);
    }

    #[test]
    fn test_top_up_stake_preserves_reputation() {
        // top_up_stake must leave VerifierReputation untouched.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 3_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &40u32, &true);
        client.record_default(&admin, &sme);
        let rep = client.get_verifier_reputation(&verifier);
        assert!(rep < 100);
        client.top_up_stake(&admin, &verifier, &500_000i128);
        assert_eq!(client.get_verifier_reputation(&verifier), rep);
    }

    #[test]
    fn test_top_up_stake_requires_active_verifier() {
        let (env, admin, _, staking_token, client) = setup();
        let stranger = Address::generate(&env);
        mint_stake(&env, &staking_token, &stranger, 1_000_000i128);
        assert!(client.try_top_up_stake(&admin, &stranger, &500_000i128).is_err());
    }

    #[test]
    fn test_top_up_stake_rejects_zero_amount() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client.try_top_up_stake(&admin, &verifier, &0i128).is_err());
    }

    // ── add_verifier re-registration guard ───────────────────────────────────

    #[test]
    fn test_add_verifier_rejects_already_active_verifier() {
        // Re-registering an active verifier must fail — previously this silently
        // overwrote VerifierStake (stranding funds) and reset VerifierReputation.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 2_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_add_verifier(&admin, &verifier, &500_000i128);
        assert!(result.is_err());
        // Stake unchanged at original amount — no funds stranded
        assert_eq!(client.get_verifier_stake(&verifier), 1_000_000i128);
        assert!(client.is_verifier(&verifier));
    }

    #[test]
    fn test_regression_fund_stranding_prevented() {
        // Regression: second add_verifier with a lower amount must no longer
        // silently overwrite VerifierStake, stranding the difference on-chain.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 2_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert_eq!(client.get_verifier_stake(&verifier), 1_000_000i128);
        // Attempt re-register with a lower stake — must be rejected
        assert!(client.try_add_verifier(&admin, &verifier, &500_000i128).is_err());
        // Tracked stake is still 1_000_000, not silently reduced to 500_000
        assert_eq!(client.get_verifier_stake(&verifier), 1_000_000i128);
    }

    #[test]
    fn test_regression_reputation_reset_prevented() {
        // Regression: second add_verifier must not reset VerifierReputation to 100,
        // laundering slashing history recorded via record_default.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 2_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &40u32, &true);
        client.record_default(&admin, &sme);
        let rep_after_slash = client.get_verifier_reputation(&verifier);
        assert!(rep_after_slash < 100, "reputation should have been reduced");
        // Attempt re-register — rejected, reputation preserved
        let _ = client.try_add_verifier(&admin, &verifier, &1_000_000i128);
        assert_eq!(client.get_verifier_reputation(&verifier), rep_after_slash);
    }

    #[test]
    fn test_top_up_stake_increases_stake_additively() {
        // top_up_stake must ADD to existing tracked stake, not overwrite it.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 3_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert_eq!(client.get_verifier_stake(&verifier), 1_000_000i128);
        client.top_up_stake(&admin, &verifier, &500_000i128);
        assert_eq!(client.get_verifier_stake(&verifier), 1_500_000i128);
    }

    #[test]
    fn test_top_up_stake_preserves_reputation() {
        // top_up_stake must leave VerifierReputation untouched.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 3_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &40u32, &true);
        client.record_default(&admin, &sme);
        let rep = client.get_verifier_reputation(&verifier);
        assert!(rep < 100);
        client.top_up_stake(&admin, &verifier, &500_000i128);
        assert_eq!(client.get_verifier_reputation(&verifier), rep);
    }

    #[test]
    fn test_top_up_stake_requires_active_verifier() {
        let (env, admin, _, staking_token, client) = setup();
        let stranger = Address::generate(&env);
        mint_stake(&env, &staking_token, &stranger, 1_000_000i128);
        assert!(client.try_top_up_stake(&admin, &stranger, &500_000i128).is_err());
    }

    #[test]
    fn test_top_up_stake_rejects_zero_amount() {
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        assert!(client.try_top_up_stake(&admin, &verifier, &0i128).is_err());
    }

    // ── set_credit_limit (require_non_negative_amount guard) ─────────────────

    #[test]
    fn test_set_credit_limit_negative_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RiskRegistryContract);
        let client = RiskRegistryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let invoice_nft = Address::generate(&env);
        let staking_token = Address::generate(&env);
        client.initialize(&admin, &invoice_nft, &staking_token, &1_000_000i128, &5_000u32);

        // Seed a verifier directly into storage to exercise set_credit_limit's
        // validation in isolation from the staking-token flow of add_verifier.
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .set(&DataKey::Verifier(verifier.clone()), &true);
        });

        let result = client.try_set_credit_limit(&verifier, &sme, &-1i128);
        assert_eq!(result.unwrap_err().unwrap(), RiskRegistryError::InvalidAmount);
    }

    // ── Issue #483: Stake lockup enforcement tests ──────────────────────────────

    #[test]
    fn test_remove_verifier_blocks_full_stake_return_with_open_sme() {
        // When a verifier registers an SME with open invoices and is then removed,
        // their stake should not be fully returned if they still have outstanding
        // vouched exposure (i.e., SMEs they verified with active invoices).
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        let initial_stake = 1_000_000i128;

        mint_stake(&env, &staking_token, &verifier, initial_stake);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, initial_stake);
        client.add_verifier(&admin, &verifier, &initial_stake);
        client.register_sme(&verifier, &sme, &50u32, &true);

        // At this point, the verifier has an open SME. When removed, full stake
        // should be blocked/withheld (not returned) because the SME has unresolved
        // exposure. The current implementation incorrectly returns full stake.
        // This test documents the required fix: remove_verifier should enforce
        // a time-based or exposure-based lockup.
        let result = client.try_remove_verifier(&admin, &verifier);
        // TODO: After implementing stake lockup, this should fail with a "stake locked" error.
        // For now, this documents the vulnerable behavior.
        if result.is_ok() {
            // If removal succeeds (current behavior), verify that stake was returned
            // (which is the vulnerability). After the fix, this call should fail instead.
            let _ = result;
        }
    }

    #[test]
    fn test_verifier_stakes_locked_after_registering_sme() {
        // A verifier's stake should be locked/held after registering an SME
        // until either (1) a time-based lockup window expires, or (2) all
        // outstanding vouched exposure is resolved (defaults recorded).
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        let initial_stake = 1_000_000i128;

        mint_stake(&env, &staking_token, &verifier, initial_stake);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, initial_stake);
        client.add_verifier(&admin, &verifier, &initial_stake);
        client.register_sme(&verifier, &sme, &30u32, &true);

        // Immediately after registering an SME, the verifier's stake should be locked.
        // Attempting to remove the verifier should be blocked or the returned amount
        // should be partial, not the full stake. This test documents the expected behavior.
        // Current code returns full stake (vulnerability).
        let initial_verifier_stake = client.get_verifier_stake(&verifier);
        assert_eq!(initial_verifier_stake, initial_stake);
        // TODO: After fix, ensure that remove_verifier either:
        //   - Fails with a "stake locked" error, or
        //   - Returns only a partial amount (withholding locked portion)
    }

    // ── Issue #482: Admin parameter setter tests ────────────────────────────────

    #[test]
    fn test_set_minimum_stake_by_admin() {
        // Admin should be able to update minimum_stake post-initialization
        // to accommodate token price movements or protocol evolution.
        let (env, admin, _, _, client) = setup();
        let new_minimum_stake = 5_000_000i128;

        // TODO: After implementing set_minimum_stake, this should succeed:
        // assert!(client.try_set_minimum_stake(&admin, &new_minimum_stake).is_ok());
        // Verify persistence: a new verifier must meet the updated minimum.
    }

    #[test]
    fn test_set_minimum_stake_requires_admin() {
        // Only the admin can update minimum_stake. Non-admins should be rejected.
        let (env, _, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let new_minimum_stake = 5_000_000i128;

        // TODO: After implementing set_minimum_stake:
        // assert!(client.try_set_minimum_stake(&stranger, &new_minimum_stake).is_err());
    }

    #[test]
    fn test_set_minimum_stake_with_bounds_validation() {
        // set_minimum_stake should enforce reasonable bounds (e.g., non-negative).
        let (env, admin, _, _, client) = setup();
        let negative_stake = -1i128;

        // TODO: After implementing set_minimum_stake:
        // assert!(client.try_set_minimum_stake(&admin, &negative_stake).is_err());
    }

    #[test]
    fn test_set_slash_percentage_by_admin() {
        // Admin should be able to update slash_percentage_bps post-initialization
        // to recalibrate based on observed default rates.
        let (env, admin, _, _, client) = setup();
        let new_slash_percentage = 1_000u32; // 10% instead of 50%

        // TODO: After implementing set_slash_percentage, this should succeed:
        // assert!(client.try_set_slash_percentage(&admin, &new_slash_percentage).is_ok());
        // Verify persistence: record_default should use the new percentage.
    }

    #[test]
    fn test_set_slash_percentage_requires_admin() {
        // Only the admin can update slash_percentage. Non-admins should be rejected.
        let (env, _, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let new_slash_percentage = 1_000u32;

        // TODO: After implementing set_slash_percentage:
        // assert!(client.try_set_slash_percentage(&stranger, &new_slash_percentage).is_err());
    }

    #[test]
    fn test_set_slash_percentage_bounds_validation_above_10000() {
        // slash_percentage_bps must not exceed 10_000 (100%).
        let (env, admin, _, _, client) = setup();
        let invalid_slash_percentage = 10_001u32;

        // TODO: After implementing set_slash_percentage:
        // assert!(client.try_set_slash_percentage(&admin, &invalid_slash_percentage).is_err());
    }

    #[test]
    fn test_set_slash_percentage_bounds_validation_zero_allowed() {
        // slash_percentage_bps = 0 should be allowed (no slashing).
        let (env, admin, _, _, client) = setup();
        let zero_slash_percentage = 0u32;

        // TODO: After implementing set_slash_percentage:
        // assert!(client.try_set_slash_percentage(&admin, &zero_slash_percentage).is_ok());
    }

    #[test]
    fn test_set_slash_percentage_affects_future_defaults() {
        // When slash_percentage is updated, future record_default calls should use
        // the new percentage, not the one from initialization.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        let initial_stake = 10_000_000i128;

        mint_stake(&env, &staking_token, &verifier, initial_stake);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, initial_stake);
        client.add_verifier(&admin, &verifier, &initial_stake);
        client.register_sme(&verifier, &sme, &50u32, &true);

        // TODO: After implementing set_slash_percentage:
        // let new_slash_percentage = 1_000u32; // 10%
        // client.set_slash_percentage(&admin, &new_slash_percentage);
        // client.record_default(&admin, &sme);
        // Verify that the new percentage was applied (10% slashed, not 50%).
    }

    // ── Issue #481: Verifier reputation gating tests ─────────────────────────────

    #[test]
    fn test_register_sme_blocked_below_reputation_threshold() {
        // Verifiers below a minimum reputation threshold should be blocked
        // from registering new SMEs. This prevents low-reputation verifiers
        // from continued fraudulent activity.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);

        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, 1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);

        // Slash the verifier down to a low reputation
        let bad_sme1 = Address::generate(&env);
        client.register_sme(&verifier, &bad_sme1, &50u32, &true);
        for _ in 0..10 {
            client.record_default(&admin, &bad_sme1);
        }
        let reputation = client.get_verifier_reputation(&verifier);
        assert_eq!(reputation, 0); // Slashed below threshold

        // TODO: After implementing reputation gating:
        // Attempting to register another SME should fail:
        // let bad_sme2 = Address::generate(&env);
        // assert!(client.try_register_sme(&verifier, &bad_sme2, &50u32, &true).is_err());
    }

    #[test]
    fn test_update_sme_score_blocked_below_reputation_threshold() {
        // Verifiers below reputation threshold should also be blocked from
        // updating existing SME scores.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);

        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, 1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &50u32, &true);

        // Slash the verifier to 0 reputation
        for _ in 0..10 {
            client.record_default(&admin, &sme);
        }
        let reputation = client.get_verifier_reputation(&verifier);
        assert_eq!(reputation, 0);

        // TODO: After implementing reputation gating:
        // Attempting to update the score should fail:
        // assert!(client.try_update_sme_score(&verifier, &sme, &75u32).is_err());
    }

    #[test]
    fn test_set_credit_limit_blocked_below_reputation_threshold() {
        // Verifiers below reputation threshold should be blocked from
        // setting or updating SME credit limits.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);

        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, 1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &50u32, &true);

        // Slash to 0 reputation
        for _ in 0..10 {
            client.record_default(&admin, &sme);
        }

        // TODO: After implementing reputation gating:
        // assert!(client.try_set_credit_limit(&verifier, &sme, &1_000_000i128).is_err());
    }

    #[test]
    fn test_reputation_preserved_on_verifier_removal_and_re_add() {
        // Reputation history should survive remove_verifier and be preserved
        // (not reset to 100) when the same address is re-added via add_verifier.
        // This prevents a reputation laundering attack.
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);

        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, 1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &50u32, &true);

        // Record defaults to slash reputation to 50
        for _ in 0..5 {
            client.record_default(&admin, &sme);
        }
        assert_eq!(client.get_verifier_reputation(&verifier), 50);

        // Remove the verifier
        client.remove_verifier(&admin, &verifier);
        assert!(!client.is_verifier(&verifier));

        // Re-add the same verifier
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, 1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);

        // TODO: After implementing reputation history preservation:
        // Reputation should still be 50, not reset to 100:
        // assert_eq!(client.get_verifier_reputation(&verifier), 50);
        // Current behavior resets to 100 (vulnerability).
    }

    #[test]
    fn test_reputation_threshold_allows_default_actions() {
        // A verifier at or above the reputation threshold should be able
        // to perform all verifier actions (register_sme, update_sme_score, etc).
        let (env, admin, _, staking_token, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);

        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, 1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);

        // Verifier at reputation 100 (initial) should succeed
        assert!(client.try_register_sme(&verifier, &sme, &50u32, &true).is_ok());
        assert!(client.try_update_sme_score(&verifier, &sme, &75u32).is_ok());
        assert!(client.try_set_credit_limit(&verifier, &sme, &500_000i128).is_ok());
    }

    // ── Issue #480: Debtor score cooldown sub-account bypass tests ──────────────

    #[test]
    fn test_set_debtor_score_cooldown_enforced_per_primary_verifier() {
        // The cooldown must be scoped to the primary verifier, not the raw caller.
        // This prevents bypassing the cooldown by cycling through sub-accounts.
        let (env, admin, _, staking_token, client) = setup();
        let primary = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xAAu8; 32]);

        mint_stake(&env, &staking_token, &primary, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&primary, 1_000_000i128);
        client.add_verifier(&admin, &primary, &1_000_000i128);

        // Primary sets a debtor score
        client.set_debtor_score(&primary, &debtor_hash, &30u32);
        assert_eq!(client.get_debtor_score(&debtor_hash), 30);

        // Verify cooldown blocks immediate re-update
        let err = client
            .try_set_debtor_score(&primary, &debtor_hash, &60u32)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, RiskRegistryError::ScoreUpdateCooldownNotElapsed);

        // TODO: After implementing per-primary-verifier cooldown:
        // Even if we try via a different address (sub-account), the cooldown
        // should still block the update because the resolved primary is the same.
        // Current code keys the cooldown by raw caller, allowing bypass.
    }

    #[test]
    fn test_set_debtor_score_subaccount_cooldown_bypass_blocked() {
        // A primary verifier with sub-accounts should not be able to bypass
        // the cooldown by updating the same debtor via different sub-accounts.
        // The cooldown must be keyed to the resolved primary, not the raw caller.
        let (env, admin, _, staking_token, client) = setup();
        let primary = Address::generate(&env);
        let sub_a = Address::generate(&env);
        let sub_b = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xBBu8; 32]);

        mint_stake(&env, &staking_token, &primary, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&primary, 1_000_000i128);
        client.add_verifier(&admin, &primary, &1_000_000i128);
        client.add_sub_account(&primary, &sub_a);
        client.add_sub_account(&primary, &sub_b);

        // Update debtor score via sub_a
        client.set_debtor_score(&sub_a, &debtor_hash, &40u32);
        assert_eq!(client.get_debtor_score(&debtor_hash), 40);

        // TODO: After implementing per-primary-verifier cooldown:
        // Immediately updating via sub_b should be blocked (same cooldown, different caller).
        // This documents the vulnerability: current code allows this because it keys
        // cooldown by raw caller (sub_a vs sub_b), not by resolved primary.
        // let err = client
        //     .try_set_debtor_score(&sub_b, &debtor_hash, &65u32)
        //     .unwrap_err()
        //     .unwrap();
        // assert_eq!(err, RiskRegistryError::ScoreUpdateCooldownNotElapsed);

        // Current behavior incorrectly allows the bypass:
        if let Ok(_) = client.try_set_debtor_score(&sub_b, &debtor_hash, &65u32) {
            // This documents the vulnerability being tested.
        }
    }

    #[test]
    fn test_set_debtor_score_multiple_subaccounts_unified_cooldown() {
        // When multiple sub-accounts of the same primary all update the same debtor,
        // the cooldown should be enforced per primary, preventing rapid successive
        // updates regardless of which sub-account is used.
        let (env, admin, _, staking_token, client) = setup();
        let primary = Address::generate(&env);
        let sub_1 = Address::generate(&env);
        let sub_2 = Address::generate(&env);
        let sub_3 = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xCCu8; 32]);

        mint_stake(&env, &staking_token, &primary, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&primary, 1_000_000i128);
        client.add_verifier(&admin, &primary, &1_000_000i128);
        client.add_sub_account(&primary, &sub_1);
        client.add_sub_account(&primary, &sub_2);
        client.add_sub_account(&primary, &sub_3);

        // First update via sub_1
        assert!(client.try_set_debtor_score(&sub_1, &debtor_hash, &30u32).is_ok());

        // TODO: After fix, all of these should fail with cooldown error (same primary):
        // assert!(client.try_set_debtor_score(&sub_2, &debtor_hash, &40u32).is_err());
        // assert!(client.try_set_debtor_score(&sub_3, &debtor_hash, &50u32).is_err());
        // assert!(client.try_set_debtor_score(&primary, &debtor_hash, &60u32).is_err());

        // Current code allows bypasses because it keys cooldown by caller, not primary.
    }

    #[test]
    fn test_set_debtor_score_event_attributes_to_primary_verifier() {
        // Events emitted by set_debtor_score should consistently attribute
        // the update to the primary verifier, not the raw caller (sub-account).
        let (env, admin, _, staking_token, client) = setup();
        let primary = Address::generate(&env);
        let sub_account = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xDDu8; 32]);

        mint_stake(&env, &staking_token, &primary, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&primary, 1_000_000i128);
        client.add_verifier(&admin, &primary, &1_000_000i128);
        client.add_sub_account(&primary, &sub_account);

        // Record events before and after
        let events_before = env.events().all().len();
        client.set_debtor_score(&sub_account, &debtor_hash, &45u32);
        let events_after = env.events().all().len();

        // TODO: After fix, ensure emitted event references the primary, not sub_account.
        // For now, this test documents that events may incorrectly attribute
        // to the sub-account caller rather than the resolved primary.
        assert!(events_after > events_before, "Event should have been emitted");
    }

    #[test]
    fn test_primary_and_subaccount_share_single_cooldown() {
        // Primary and its sub-account should share the same cooldown namespace.
        // An update via primary is blocked if a sub-account recently updated,
        // and vice versa.
        let (env, admin, _, staking_token, client) = setup();
        let primary = Address::generate(&env);
        let sub = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xEEu8; 32]);

        mint_stake(&env, &staking_token, &primary, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&primary, 1_000_000i128);
        client.add_verifier(&admin, &primary, &1_000_000i128);
        client.add_sub_account(&primary, &sub);

        // Update via sub-account
        assert!(client.try_set_debtor_score(&sub, &debtor_hash, &35u32).is_ok());

        // TODO: After fix, this should fail (same primary, so same cooldown):
        // let err = client
        //     .try_set_debtor_score(&primary, &debtor_hash, &50u32)
        //     .unwrap_err()
        //     .unwrap();
        // assert_eq!(err, RiskRegistryError::ScoreUpdateCooldownNotElapsed);
    }

    // ── Issue #479: risk_registry protocol-pause integration ─────────────────

    #[test]
    fn test_add_verifier_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that add_verifier should check protocol pause status,
        // currently it does not (issue #479). Once implemented, verifiers should
        // not be addable when access_control has paused the protocol.
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        let result = client.try_add_verifier(&admin, &verifier, &1_000_000i128);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    #[test]
    fn test_remove_verifier_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that remove_verifier should check protocol pause status.
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_remove_verifier(&admin, &verifier);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    #[test]
    fn test_add_sub_account_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that add_sub_account should check protocol pause status.
        let verifier = Address::generate(&env);
        let sub_account = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_add_sub_account(&verifier, &sub_account);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    #[test]
    fn test_remove_sub_account_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that remove_sub_account should check protocol pause status.
        let verifier = Address::generate(&env);
        let sub_account = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.add_sub_account(&verifier, &sub_account);
        let result = client.try_remove_sub_account(&verifier, &sub_account);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    #[test]
    fn test_register_sme_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that register_sme should check protocol pause status.
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_register_sme(&verifier, &sme, &35u32, &true);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    #[test]
    fn test_update_sme_score_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that update_sme_score should check protocol pause status.
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        let result = client.try_update_sme_score(&verifier, &sme, &45u32);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    #[test]
    fn test_set_credit_limit_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that set_credit_limit should check protocol pause status.
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        client.register_sme(&verifier, &sme, &35u32, &true);
        let result = client.try_set_credit_limit(&verifier, &sme, &1_000_000i128);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    #[test]
    fn test_record_default_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that record_default should check protocol pause status.
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0u8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_record_default(&verifier, &debtor_hash, &1_000_000i128);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    #[test]
    fn test_set_debtor_score_blocked_when_protocol_paused() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that set_debtor_score should check protocol pause status.
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0u8; 32]);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&env, &staking_token).mint(&verifier, &1_000_000i128);
        client.add_verifier(&admin, &verifier, &1_000_000i128);
        let result = client.try_set_debtor_score(&verifier, &debtor_hash, &50u32);
        // TODO: Once require_not_paused check is added, verify it's blocked when paused.
        let _ = result;
    }

    // ── Issue #478: Upgrade via access_control's multisig ────────────────────

    #[test]
    fn test_propose_upgrade_bare_admin_rejected_when_multisig_configured() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that propose_upgrade should check if a multisig is
        // configured on access_control and reject the bare-admin path (issue #478).
        // Currently, upgrade can be proposed by any single admin without multisig.
        let wasm_hash = BytesN::<32>::random(&env);
        let result = client.try_propose_upgrade(&admin, &wasm_hash);
        // TODO: Once multisig check is added, when ac has a configured multisig,
        // this bare-admin call should be rejected.
        let _ = result;
    }

    #[test]
    fn test_execute_upgrade_bare_admin_rejected_when_multisig_configured() {
        let (env, admin, _, staking_token, client) = setup();
        // This test documents that execute_upgrade should check if a multisig is
        // configured and reject the bare-admin path when it is (issue #478).
        let wasm_hash = BytesN::<32>::random(&env);
        let result = client.try_execute_upgrade(&admin, &wasm_hash);
        // TODO: Once multisig check is added, when ac has a configured multisig,
        // this bare-admin call should be rejected.
        let _ = result;
    }
}
