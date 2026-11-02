#![no_std]

use kora_access_control::AccessControlContractClient;
use kora_parameter_registry::ParameterRegistryContractClient;
use kora_shared::{
    audit::{chain_checksum, AdminActionType, AuditEntry, MAX_AUDIT_LOG_SIZE},
    errors::{CommonError, KoraError},
    events,
    reentrancy::ReentrancyGuard,
    types::{MultisigConfig, RegistryParamKey},
    validation::{require_non_negative_amount, require_not_self, require_valid_fee_bps, require_within_max_amount, UPGRADE_TIMELOCK_DELAY, MAX_AMOUNT},
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, String, Symbol, Vec};

mod report;

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TreasuryError {
    AlreadyInitialized = 1,
    ArithmeticOverflow = 2,
    InsufficientPoolBalance = 3,
    InvalidAddress = 4,
    InvalidAmount = 5,
    InvalidFeeRate = 6,
    NoCapChangeProposed = 7,
    NoUpgradeProposed = 8,
    NotAdmin = 9,
    NotInitialized = 10,
    Reentrancy = 11,
    TokenNotWhitelisted = 12,
    UpgradeTimelockNotElapsed = 13,
    WithdrawalCapTimelockNotElapsed = 14,
    WithdrawalRateLimitExceeded = 15,
    NotEligibleStakeholder = 16,
    NoShareAvailable = 17,
    InvalidDistributionConfig = 18,
    DistributionProposalNotFound = 19,
    SweepSplitsNotConfigured = 20,
    InvalidSweepSplits = 21,
    InvalidGrant = 22,
    GrantNotFound = 23,
    TokenDecimalsNotRegistered = 24,
    /// `emergency_withdraw` called without a declared emergency (#453).
    EmergencyNotDeclared = 25,
}

impl From<CommonError> for TreasuryError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => TreasuryError::InvalidAmount,
            CommonError::InvalidAddress => TreasuryError::InvalidAddress,
            CommonError::InvalidFeeRate => TreasuryError::InvalidFeeRate,
            CommonError::ArithmeticOverflow => TreasuryError::ArithmeticOverflow,
            CommonError::Reentrancy => TreasuryError::Reentrancy,
            // Any other CommonError variant reachable via a `?` call in this crate
            // falls back to InvalidAmount rather than being silently dropped.
            _ => TreasuryError::InvalidAmount,
        }
    }
}

impl From<KoraError> for TreasuryError {
    fn from(e: KoraError) -> Self {
        match e {
            KoraError::NotAdmin => TreasuryError::NotAdmin,
            KoraError::InvalidAmount => TreasuryError::InvalidAmount,
            KoraError::InvalidAddress => TreasuryError::InvalidAddress,
            KoraError::InvalidFeeRate => TreasuryError::InvalidFeeRate,
            KoraError::ArithmeticOverflow => TreasuryError::ArithmeticOverflow,
            KoraError::Reentrancy => TreasuryError::Reentrancy,
            KoraError::NotInitialized => TreasuryError::NotInitialized,
            KoraError::AlreadyInitialized => TreasuryError::AlreadyInitialized,
            KoraError::SweepSplitsNotConfigured => TreasuryError::SweepSplitsNotConfigured,
            KoraError::InvalidSweepSplits => TreasuryError::InvalidSweepSplits,
            KoraError::InsufficientFunds | KoraError::InsufficientPoolBalance => TreasuryError::InsufficientPoolBalance,
            KoraError::TokenNotWhitelisted => TreasuryError::TokenNotWhitelisted,
            KoraError::UpgradeTimelockNotElapsed => TreasuryError::UpgradeTimelockNotElapsed,
            KoraError::NoUpgradeProposed => TreasuryError::NoUpgradeProposed,
            KoraError::WithdrawalRateLimitExceeded => TreasuryError::WithdrawalRateLimitExceeded,
            _ => TreasuryError::InvalidAmount,
        }
    }
}

/// TTL for treasury multisig proposals — mirrors access_control's PROPOSAL_TTL_LEDGERS (~7 days).
const TREASURY_PROPOSAL_TTL: u64 = 604_800;
const MAX_TOKEN_DECIMALS: u32 = 18;

// ── Storage TTL constants (~31 days in ledgers) ───────────────────────────────
const PERSISTENT_BUMP_AMOUNT: u32 = 535_680;
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 535_680 / 2;

// ── Rate-limit epoch: 24 h in seconds ────────────────────────────────────────
const EPOCH_DURATION: u64 = 86_400;

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    /// Admin address — persistent so it survives ledger archival.
    Admin,
    /// Pending new-admin address (two-step transfer).
    PendingAdmin,
    /// Protocol fee in basis points — persistent for durability.
    FeeBps,
    /// Accumulated fees per token (informational).
    Collected(Address),
    /// Reentrancy guard flag (instance-level).
    WithdrawalLock,
    /// Audit log entry at ring-buffer position `n`.
    AuditEntry(u64),
    /// Next write slot in the audit ring buffer (u64).
    AuditHead,
    /// Total entries ever appended (u64).
    AuditTotal,
    /// Rolling sha256 checksum (BytesN<32>).
    AuditChecksum,
    /// Whitelisted token flag.
    WhitelistedToken(Address),
    /// Pending upgrade proposal: (wasm_hash, proposed_at_timestamp).
    UpgradeProposal,
    /// Maximum total withdrawal allowed per EPOCH_DURATION window for a given
    /// token (0 = uncapped). Keyed by token so unrelated tokens' caps don't
    /// share a single global quota (#452).
    WithdrawalCap(Address),
    /// Pending cap change proposal for a given token: (new_cap, proposed_at).
    WithdrawalCapProposal(Address),
    /// Shared parameter registry used for grant reserve-floor checks.
    ParameterRegistry,
    /// Timestamp when the current rate-limit epoch started, per token.
    EpochStart(Address),
    /// Total withdrawn in the current epoch, per token (resets each epoch).
    EpochWithdrawn(Address),
    /// Address of the `access_control` contract instance (optional — pause
    /// enforcement is skipped when unset, e.g. in unit tests) (#454).
    AccessControl,
    /// Distinct, admin-declared flag gating `emergency_withdraw` (#453).
    EmergencyDeclared,
    /// Next write position in the audit ring buffer (0..MAX_AUDIT_LOG_SIZE).
    AuditLogHead,
    /// Total admin actions ever recorded (monotonic; not capped at ring size).
    AuditLogTotal,
    // ── Recipient allowlist (#457) ───────────────────────────────────────────
    /// Whether `recipient` is a matured, allowed withdrawal destination.
    AllowedRecipient(Address),
    /// Pending recipient proposal: (recipient is implicit in the key, proposed_at timestamp).
    RecipientProposal(Address),
    // ── Insurance / loss reserve (#458) ──────────────────────────────────────
    /// Whether `caller` (typically a `financing_pool` deployment) may call `disburse_from_reserve`.
    AuthorizedReserveCaller(Address),
    /// Reserve balance earmarked per token — a subset of the live token balance,
    /// excluded from `withdraw`/`emergency_withdraw`.
    ReserveBalance(Address),
    /// Portion (bps) of every newly `collect_fee`'d amount routed into the reserve.
    ReserveAllocationBps,
    // ── Multisig quorum gate (#455) ──────────────────────────────────────────
    /// Monotonic counter for the next treasury proposal id.
    NextTreasuryProposalId,
    /// A pending treasury multisig proposal, keyed by proposal id.
    TreasuryProposal(u64),
    /// Shared timelock delay in effect when the proposal was created.
    TreasuryProposalTimelock(u64),
    NextGrantId,
    Grant(u64),
    /// Evidence commitment submitted for a grant milestone.
    GrantMilestoneProof(u64, u32),
    // ── Asset Allocation Policy (Issue #673) ────────────────────────────────
    /// Governed allocation policy for a given asset (target min/max ranges).
    AssetAllocationPolicy(Address),
    // ── Marketplace & Price Oracle ──────────────────────────────────────────
    /// Address of the authorized marketplace contract.
    Marketplace,
    /// Address of the price oracle contract.
    PriceOracle,
    // ── Revenue-sharing distribution (#667) ─────────────────────────────────
    /// Current distribution epoch counter.
    CurrentDistributionEpoch,
    /// Pending distribution proposal.
    DistributionProposal,
    /// Distribution configuration for a given epoch.
    DistributionConfig(u64),
    /// Stakeholder claim record: (stakeholder, epoch, token) -> amount.
    StakeholderClaim(Address, u64, Address),
    /// Distribution pool balance per token.
    DistributionPool(Address),
    // ── Transparency reporting ──────────────────────────────────────────────
    /// Treasury report for a given epoch.
    TreasuryReport(u64),
    /// Last epoch for which a report was generated.
    LastReportedEpoch,
    // ── Fee Sweep Automation (#742) ─────────────────────────────────────────
    /// Configured sweep split destinations: Vec<(Address, u32)> summing to 10000 bps.
    SweepSplit,
    /// Primary destination that receives rounding dust from sweep.
    SweepPrimaryDestination,
    /// Decimal precision used to interpret a token's raw amounts in reports.
    TokenDecimals(Address),
}

/// A highest-risk treasury action gated behind `access_control`'s multisig quorum
/// once one is configured (see `set_access_control`).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreasuryAction {
    Withdraw(Address, Address, i128),
    EmergencyWithdraw(Address, Address),
    SetFeeBps(u32),
    ProposeUpgrade(BytesN<32>),
    ReviewGrant(u64),
    ReleaseGrantMilestone(u64),
    ReclaimGrant(u64),
    SetTimelockDelay(u64),
    SetParameterRegistry(Address),
    SetTokenDecimals(Address, u32),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantProposal {
    pub id: u64,
    pub proposer: Address,
    pub recipient: Address,
    pub token: Address,
    pub scope: String,
    pub milestones: Vec<i128>,
    pub next_milestone: u32,
    pub remaining: i128,
    pub reviewed: bool,
    pub closed: bool,
    pub submitted_at: u64,
    pub last_activity_at: u64,
}

const GRANT_RECLAIM_DELAY: u64 = 90 * 24 * 60 * 60;
const MAX_GRANT_MILESTONES: u32 = 20;

/// A pending treasury action proposal awaiting multisig quorum.
#[contracttype]
#[derive(Clone, Debug)]
pub struct TreasuryProposal {
    pub id: u64,
    pub action: TreasuryAction,
    pub proposer: Address,
    pub approvals: Vec<Address>,
    pub executed: bool,
    pub created_at: u64,
    pub expires_at: u64,
}

/// Revenue-sharing distribution configuration for an epoch.
/// Defines stakeholders and their equal distribution shares.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistributionConfig {
    pub epoch: u64,
    pub stakeholders: Vec<Address>,
    pub created_at: u64,
    pub distribution_bps: u32, // Basis points of fees to distribute (e.g., 5000 = 50%)
}

/// A pending distribution proposal awaiting execution.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DistributionProposal {
    pub epoch: u64,
    pub stakeholders: Vec<Address>,
    pub distribution_bps: u32,
    pub proposed_at: u64,
    pub expires_at: u64,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct TreasuryContract;

#[contractimpl]
impl TreasuryContract {
    /// One-time initialization. Sets admin and protocol fee.
    ///
    /// **Parameters:**
    /// - `admin` — The address that will administer this contract.
    /// - `fee_bps` — Protocol fee in basis points (0–10 000).
    ///
    /// **Errors:**
    /// - `TreasuryError::AlreadyInitialized` — Contract has already been initialized.
    /// - `TreasuryError::InvalidFeeRate` — `fee_bps` > 10 000.
    /// - `TreasuryError::InvalidAddress` — `admin` is the contract's own address.
    ///
    /// **Security:** No auth required on first call. Subsequent calls revert immediately.
    /// Initializes rate-limit state (epoch start, epoch withdrawn, cap = 0 = uncapped).
    pub fn initialize(env: Env, admin: Address, fee_bps: u32) -> Result<(), TreasuryError> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(TreasuryError::AlreadyInitialized);
        }
        require_valid_fee_bps(fee_bps)?;
        kora_shared::validation::require_not_self(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().extend_ttl(
            &DataKey::Admin,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().set(&DataKey::FeeBps, &fee_bps);
        env.storage().persistent().extend_ttl(
            &DataKey::FeeBps,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::treasury_initialized(&env, &admin, fee_bps);
        Self::append_audit_entry_internal(&env, &admin, String::from_str(&env, "initialize"));
        Ok(())
    }

    /// Update protocol fee. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `fee_bps` — New fee in basis points (0–10 000).
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::InvalidFeeRate` — `fee_bps` > 10 000.
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits `fee_rate_updated` event.
    pub fn set_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_no_quorum_required(&env)?;
        Self::do_set_fee_bps(&env, &admin, fee_bps)?;
        Ok(())
    }

    fn do_set_fee_bps(env: &Env, admin: &Address, fee_bps: u32) -> Result<(), KoraError> {
        require_valid_fee_bps(fee_bps)?;
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        Self::append_audit_entry_internal(env, admin, String::from_str(env, "set_fee_bps"));

        let old_bps: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::FeeBps)
            .unwrap_or(50);

        env.storage().persistent().set(&DataKey::FeeBps, &fee_bps);
        Self::bump_persistent(env, &DataKey::FeeBps);

        events::fee_rate_updated(env, admin, old_bps, fee_bps);
        Self::append_audit_entry(
            env,
            admin,
            AdminActionType::SetFeeBps,
            None,
            Some(fee_bps as i128),
        );
        Ok(())
    }

    /// Set or update the price oracle address. Admin only.
    /// The oracle is optional — if not set, `get_total_collected_value` will work but skip unsupported conversions.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `price_oracle` — The price oracle contract address.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn set_price_oracle(env: Env, admin: Address, price_oracle: Address) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        env.storage()
            .persistent()
            .set(&DataKey::PriceOracle, &price_oracle);
        Self::bump_persistent(&env, &DataKey::PriceOracle);
        Self::append_audit_entry(&env, &admin, AdminActionType::SetFeeBps, None, None);
        Ok(())
    }

    /// Whitelist a token so it can be used in `withdraw` / `emergency_withdraw`.
    ///
    /// Admin only. Idempotent — calling it twice for the same token is safe.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `token` — The token contract address to whitelist.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits `token_whitelisted` event.
    pub fn whitelist_token(env: Env, admin: Address, token: Address) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedToken(token.clone()), &true);
        Self::bump_persistent(&env, &DataKey::WhitelistedToken(token.clone()));

        events::token_whitelisted(&env, &admin, &token);
        Self::append_audit_entry(
            &env,
            &admin,
            AdminActionType::WhitelistToken,
            Some(token),
            None,
        );
        Ok(())
    }

    /// Set the authorized marketplace contract address. Admin only. Required
    /// before `refund_fee` can be called by the marketplace. (#450)
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `marketplace` — The marketplace contract address to authorize.
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn set_marketplace(env: Env, admin: Address, marketplace: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .set(&DataKey::Marketplace, &marketplace);
        Self::bump_persistent(&env, &DataKey::Marketplace);
        Ok(())
    }

    /// Returns the configured marketplace contract address, if set.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_marketplace(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Marketplace)
    }

    /// Record an incoming fee for a given token. Called by the marketplace after
    /// transferring the fee amount to this contract. Updates the informational
    /// accounting ledger and earmarks a reserve portion per the effective reserve
    /// allocation (read from parameter_registry when configured).
    ///
    /// **Parameters:**
    /// - `token` — The token address the fee was paid in (must be whitelisted).
    /// - `amount` — The fee amount (must be > 0).
    ///
    /// **Errors:**
    /// - `TreasuryError::InvalidAmount` — `amount` is ≤ 0.
    /// - `TreasuryError::TokenNotWhitelisted` — Token has not been whitelisted.
    /// - `TreasuryError::ArithmeticOverflow` — Running total would overflow.
    pub fn collect_fee(env: Env, token: Address, amount: i128) -> Result<(), TreasuryError> {
        if amount <= 0 {
            return Err(TreasuryError::InvalidAmount);
        }
        Self::require_whitelisted_token(&env, &token)?;

        // Read reserve allocation (registry-aware)
        let reserve_bps = Self::get_reserve_allocation_bps(env.clone());
        if reserve_bps > 0 && reserve_bps <= 10_000 {
            let reserve_amount = amount.checked_mul(reserve_bps as i128).ok_or(TreasuryError::ArithmeticOverflow)?
                .checked_div(10_000).ok_or(TreasuryError::ArithmeticOverflow)?;
            if reserve_amount > 0 {
                let current_reserve: i128 = env.storage().persistent().get(&DataKey::ReserveBalance(token.clone())).unwrap_or(0);
                let new_reserve = current_reserve.checked_add(reserve_amount).ok_or(TreasuryError::ArithmeticOverflow)?;
                env.storage().persistent().set(&DataKey::ReserveBalance(token.clone()), &new_reserve);
                Self::bump_persistent(&env, &DataKey::ReserveBalance(token.clone()));
            }
        }

        // Update informational ledger
        let current: i128 = env.storage().persistent().get(&DataKey::Collected(token.clone())).unwrap_or(0);
        let new_total = current.checked_add(amount).ok_or(TreasuryError::ArithmeticOverflow)?;
        env.storage().persistent().set(&DataKey::Collected(token.clone()), &new_total);
        Self::bump_persistent(&env, &DataKey::Collected(token.clone()));
        events::fee_collected(&env, &env.current_contract_address(), 0, amount, &token);
        Ok(())
    }

    /// Record fees collected with an associated invoice ID (marketplace-originated).
    pub fn collect_fee_with_invoice(env: Env, token: Address, amount: i128, invoice_id: u64) -> Result<(), TreasuryError> {
        if amount <= 0 {
            return Err(TreasuryError::InvalidAmount);
        }
        Self::require_whitelisted_token(&env, &token)?;

        let reserve_bps = Self::get_reserve_allocation_bps(env.clone());
        if reserve_bps > 0 && reserve_bps <= 10_000 {
            let reserve_amount = amount.checked_mul(reserve_bps as i128).ok_or(TreasuryError::ArithmeticOverflow)?
                .checked_div(10_000).ok_or(TreasuryError::ArithmeticOverflow)?;
            if reserve_amount > 0 {
                let current_reserve: i128 = env.storage().persistent().get(&DataKey::ReserveBalance(token.clone())).unwrap_or(0);
                let new_reserve = current_reserve.checked_add(reserve_amount).ok_or(TreasuryError::ArithmeticOverflow)?;
                env.storage().persistent().set(&DataKey::ReserveBalance(token.clone()), &new_reserve);
                Self::bump_persistent(&env, &DataKey::ReserveBalance(token.clone()));
            }
        }

        let current: i128 = env.storage().persistent().get(&DataKey::Collected(token.clone())).unwrap_or(0);
        let new_total = current.checked_add(amount).ok_or(TreasuryError::ArithmeticOverflow)?;
        env.storage().persistent().set(&DataKey::Collected(token.clone()), &new_total);
        Self::bump_persistent(&env, &DataKey::Collected(token.clone()));
        events::fee_collected(&env, &env.current_contract_address(), invoice_id, amount, &token);
        Ok(())
    }

    /// Claw back a previously collected protocol fee to an investor on a
    /// failed/cancelled listing refund. Only the authorized marketplace may
    /// call this. Transfers `amount` of `token` from the treasury to
    /// `investor` and decrements the `Collected` accounting ledger, including
    /// any referrer-split portion (the full fee was collected to the treasury).
    /// Fails with `InsufficientPoolBalance` when the treasury token balance
    /// cannot cover the clawback (e.g. fees already spent via grants).
    pub fn refund_fee(
        env: Env,
        caller: Address,
        token: Address,
        amount: i128,
        investor: Address,
    ) -> Result<(), TreasuryError> {
        if amount <= 0 {
            return Err(TreasuryError::InvalidAmount);
        }
        let marketplace: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Marketplace)
            .ok_or(TreasuryError::NotInitialized)?;
        if caller != marketplace {
            return Err(TreasuryError::NotAdmin);
        }
        Self::require_whitelisted_token(&env, &token)?;
        let live_balance = Self::get_balance_internal(&env, &token);
        if amount > live_balance {
            return Err(TreasuryError::InsufficientPoolBalance);
        }
        let collected: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::Collected(token.clone()))
            .unwrap_or(0);
        let new_collected = collected
            .checked_sub(amount)
            .ok_or(TreasuryError::ArithmeticOverflow)?;
        env.storage()
            .persistent()
            .set(&DataKey::Collected(token.clone()), &new_collected);
        Self::bump_persistent(&env, &DataKey::Collected(token.clone()));
        Self::transfer_token(&env, &marketplace, &token, &investor, amount);
        events::fee_reversed(&env, &caller, 0, &investor, amount, &token);
        Ok(())
    }

    /// Withdraw accumulated fees to a recipient. Admin only.
    ///
    /// The withdrawable balance excludes the loss-reserve amount per token
    /// (reserve floor is read from parameter_registry when configured).
    pub fn withdraw(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_no_quorum_required(&env)?;
        Self::do_withdraw(&env, &admin, &token, &recipient, amount)?;
        Ok(())
    }

    fn do_withdraw(
        env: &Env,
        admin: &Address,
        token: &Address,
        recipient: &Address,
        amount: i128,
    ) -> Result<(), TreasuryError> {
        if amount <= 0 || amount > MAX_AMOUNT {
            return Err(TreasuryError::InvalidAmount);
        }
        Self::require_whitelisted_token(env, token)?;
        Self::require_not_paused(env)?;
        Self::enforce_rate_limit(env, token, amount)?;
        Self::require_allowed_recipient(env, recipient)?;

        let _guard = ReentrancyGuard::new(env).map_err(|_| TreasuryError::Reentrancy)?;

        // Read reserve balance (excluded from withdrawable amount)
        let reserve: i128 = env.storage().persistent().get(&DataKey::ReserveBalance(token.clone())).unwrap_or(0);
        let live_balance = Self::get_balance_internal(env, token);
        let spendable = live_balance.checked_sub(reserve).ok_or(TreasuryError::ArithmeticOverflow)?;
        if amount > spendable {
            return Err(TreasuryError::InsufficientPoolBalance);
        }

        let collected: i128 = env.storage().persistent().get(&DataKey::Collected(token.clone())).unwrap_or(0);
        let new_collected = collected.checked_sub(amount).ok_or(TreasuryError::ArithmeticOverflow)?;
        env.storage().persistent().set(&DataKey::Collected(token.clone()), &new_collected);
        Self::bump_persistent(env, &DataKey::Collected(token.clone()));

        Self::record_withdrawal(env, token, amount);
        Self::transfer_token(env, admin, token, recipient, amount);

        events::fee_withdrawn(env, admin, token, amount);
        Self::append_audit_entry(
            env,
            admin,
            AdminActionType::Withdraw,
            Some(token.clone()),
            Some(amount),
        );
        Ok(())
    }

    fn do_emergency_withdraw(
        env: &Env,
        admin: &Address,
        token: &Address,
        recipient: &Address,
    ) -> Result<(), KoraError> {
        Self::require_whitelisted_token(env, token)?;
        Self::require_allowed_recipient(env, recipient)?;

        let _guard = ReentrancyGuard::new(env)?;

        let token_client = token::Client::new(env, token);
        let balance = token_client.balance(&env.current_contract_address());
        let reserved: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::ReserveBalance(token.clone()))
            .unwrap_or(0);
        let spendable = balance.saturating_sub(reserved);

        if spendable > 0 {
            token_client.transfer(&env.current_contract_address(), recipient, &spendable);
            events::emergency_withdrawn(env, admin, token, spendable);
        }
        Self::append_audit_entry(
            env,
            admin,
            AdminActionType::EmergencyWithdraw,
            Some(token.clone()),
            Some(spendable),
        );
        Ok(())
    }

    /// Map a `TreasuryError` from the direct-withdraw path into the governance
    /// execution error space (`KoraError`). Variants mirror 1:1.
    fn map_withdraw_err(e: TreasuryError) -> KoraError {
        match e {
            TreasuryError::InvalidAmount => KoraError::InvalidAmount,
            TreasuryError::TokenNotWhitelisted => KoraError::TokenNotWhitelisted,
            TreasuryError::ArithmeticOverflow => KoraError::ArithmeticOverflow,
            TreasuryError::InsufficientPoolBalance => KoraError::InsufficientPoolBalance,
            TreasuryError::Reentrancy => KoraError::Reentrancy,
            TreasuryError::NotAdmin => KoraError::NotAdmin,
            TreasuryError::NotInitialized => KoraError::NotInitialized,
            TreasuryError::WithdrawalRateLimitExceeded => KoraError::WithdrawalRateLimitExceeded,
            _ => KoraError::InvalidAmount,
        }
    }

    /// Emergency drain of a token's spendable balance to a recipient. Admin only.
    ///
    /// Requires a previously declared emergency (see `declare_emergency`, #453).
    /// Unlike `withdraw`, this path is uncapped and ignores rate limits; the
    /// reserve floor is still excluded from the drained amount.
    pub fn emergency_withdraw(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
    ) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_no_quorum_required(&env)?;
        let declared: bool = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyDeclared)
            .unwrap_or(false);
        if !declared {
            return Err(TreasuryError::EmergencyNotDeclared);
        }
        Self::do_emergency_withdraw(&env, &admin, &token, &recipient)?;
        Ok(())
    }

    fn get_balance_internal(env: &Env, token: &Address) -> i128 {
        let token_client = token::Client::new(env, token);
        token_client.balance(&env.current_contract_address())
    }

    /// Declare a treasury emergency, unlocking `emergency_withdraw`. Admin only.
    ///
    /// A distinct, auditable action from ordinary withdrawals — required before the
    /// uncapped drain path in `emergency_withdraw` becomes callable (#453). Stays in
    /// effect until `revoke_emergency` is called.
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits `emergency_declared` event.
    pub fn declare_emergency(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::EmergencyDeclared, &true);
        events::emergency_declared(&env, &admin);
        Self::append_audit_entry(&env, &admin, AdminActionType::DeclareEmergency, None, None);
        Ok(())
    }

    /// Revoke a previously declared emergency, re-locking `emergency_withdraw`. Admin only.
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits `emergency_revoked` event.
    pub fn revoke_emergency(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::EmergencyDeclared, &false);
        events::emergency_revoked(&env, &admin);
        Self::append_audit_entry(&env, &admin, AdminActionType::RevokeEmergency, None, None);
        Ok(())
    }

    /// Returns whether a treasury emergency is currently declared.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn is_emergency_declared(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::EmergencyDeclared)
            .unwrap_or(false)
    }

    /// Returns whether the protocol is currently paused, as seen by this treasury
    /// instance's configured `access_control` reference (`false` if unset).
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn is_paused(env: Env) -> bool {
        Self::require_not_paused(&env).is_err()
    }

    // ── Withdrawal cap management ─────────────────────────────────────────────

    /// Propose a new withdrawal cap for a specific token. Takes effect after
    /// `UPGRADE_TIMELOCK_DELAY` (24 h). Each whitelisted token has its own independent
    /// rolling cap and epoch (#452) — proposing a cap for one token never affects any
    /// other token's quota.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `token` — The token this cap applies to.
    /// - `new_cap` — The new rolling 24 h withdrawal limit in the token's smallest unit.
    ///   Set to 0 to remove the limit.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::InvalidAmount` — `new_cap` is negative.
    ///
    /// **Security:** Requires `admin.require_auth()`. The proposal must be executed via
    /// `execute_withdrawal_cap` after the timelock elapses.
    pub fn propose_withdrawal_cap(
        env: Env,
        admin: Address,
        token: Address,
        new_cap: i128,
    ) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_non_negative_amount(new_cap)?;
        if new_cap < 0 {
            return Err(TreasuryError::InvalidAmount);
        }
        env.storage().instance().set(
            &DataKey::WithdrawalCapProposal(token.clone()),
            &(new_cap, env.ledger().timestamp()),
        );
        events::withdrawal_cap_proposed(&env, &admin, &token, new_cap);
        Self::append_audit_entry(&env, &admin, AdminActionType::ProposeWithdrawalCap, Some(token), Some(new_cap));
        Ok(())
    }

    /// Execute a previously proposed withdrawal cap change for a token after the
    /// timelock elapses.
    ///
    /// Admin only. Applies the new rolling 24-hour withdrawal limit that was previously
    /// queued via `propose_withdrawal_cap`, atomically clearing the pending proposal.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `token` — The token whose proposed cap should be applied.
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    /// - `KoraError::NoUpgradeProposed` — No withdrawal cap proposal is pending.
    /// - `KoraError::UpgradeTimelockNotElapsed` — 24-hour timelock has not yet passed.
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::NoCapChangeProposed` — No withdrawal cap proposal is pending.
    /// - `TreasuryError::WithdrawalCapTimelockNotElapsed` — 24-hour timelock has not yet passed.
    ///
    /// **Security:** Requires `admin.require_auth()`. Clears the proposal atomically before
    /// applying the new cap. Emits `withdrawal_cap_updated` event.
    pub fn execute_withdrawal_cap(env: Env, admin: Address, token: Address) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let proposal_key = DataKey::WithdrawalCapProposal(token.clone());
        let (new_cap, proposed_at): (i128, u64) = env
            .storage()
            .instance()
            .get(&proposal_key)
            .ok_or(TreasuryError::NoCapChangeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(TreasuryError::WithdrawalCapTimelockNotElapsed);
        }
        let cap_key = DataKey::WithdrawalCap(token.clone());
        let old_cap: i128 = env.storage().instance().get(&cap_key).unwrap_or(0);
        env.storage().instance().set(&cap_key, &new_cap);
        env.storage().instance().remove(&proposal_key);
        events::withdrawal_cap_updated(&env, &admin, &token, old_cap, new_cap);
        Self::append_audit_entry(&env, &admin, AdminActionType::ExecuteWithdrawalCap, Some(token), Some(new_cap));
        Ok(())
    }

    /// Returns the current rolling 24-hour withdrawal cap for a token, in its smallest unit.
    ///
    /// A value of `0` means the cap is disabled (uncapped) for that token. A positive value
    /// is the maximum total amount that can be withdrawn within any 24-hour epoch. Each
    /// token's cap is fully independent of every other token's (#452).
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_withdrawal_cap(env: Env, token: Address) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::WithdrawalCap(token))
            .unwrap_or(0)
    }

    /// Returns the current protocol fee in basis points (e.g., 50 = 0.5%).
    ///
    /// Defaults to 50 bps if the contract has not yet been initialized or if the
    /// fee has never been explicitly set.
    ///
    /// If parameter governance is active in access_control, the governed value takes precedence.
    /// **Security:** Read-only view. No authorization required.
    pub fn get_fee_bps(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::FeeBps)
            .unwrap_or(50)
    }

    /// Returns the live token balance held by this contract for the given token.
    ///
    /// **Parameters:**
    /// - `token` — The token contract address to query.
    ///
    /// **Returns:** The current balance in the token's smallest unit (stroops for XLM-based tokens).
    /// Returns `0` if the contract holds none of the requested token.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_balance(env: Env, token: Address) -> i128 {
        token::Client::new(&env, &token).balance(&env.current_contract_address())
    }

    /// Return up to the most recent `limit` audit entries (newest first).
    pub fn get_audit_log(env: Env, limit: u32) -> soroban_sdk::Vec<AuditEntry> {
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditTotal)
            .unwrap_or(0);
        let head: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditHead)
            .unwrap_or(0);

        let available = total.min(MAX_AUDIT_LOG_SIZE) as u32;
        let count = (limit as u32).min(available);

        let mut result = soroban_sdk::Vec::new(&env);
        for i in 0..count {
            // Walk backwards from the last written slot
            let offset = (i as u64 + 1) % MAX_AUDIT_LOG_SIZE;
            let slot = (head + MAX_AUDIT_LOG_SIZE - offset) % MAX_AUDIT_LOG_SIZE;
            if let Some(entry) = env
                .storage()
                .persistent()
                .get::<DataKey, AuditEntry>(&DataKey::AuditEntry(slot))
            {
                result.push_back(entry);
            }
        }
        result
    }

    /// Rolling sha256 checksum committing the full admin-action history.
    /// Verifiable on-chain; survives ring-buffer wraparound.
    pub fn get_audit_checksum(env: Env) -> BytesN<32> {
        env.storage()
            .instance()
            .get(&DataKey::AuditChecksum)
            .unwrap_or_else(|| BytesN::from_array(&env, &[0u8; 32]))
    }

    /// Total number of audit entries ever appended (never resets).
    pub fn get_audit_total(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::AuditTotal)
            .unwrap_or(0)
    }

    // ── Transparency Reports ──────────────────────────────────────────────────

    /// Generate and store a treasury transparency report for the current epoch.
    /// Summarizes inflows (fees collected), outflows (withdrawals), and ending balances per token.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `epoch` — The epoch identifier for this report.
    /// - `whitelisted_tokens` — Vector of token addresses to include in the report.
    ///
    /// **Returns:** The generated report.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::TokenDecimalsNotRegistered` — A token has no registered decimal precision.
    ///
    /// **Security:** Requires `admin.require_auth()`. Report is persisted for historical tracking.
    pub fn generate_transparency_report(
        env: Env,
        admin: Address,
        epoch: u64,
        whitelisted_tokens: Vec<Address>,
    ) -> Result<report::TreasuryReport, TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let mut report = report::TreasuryReport::new(&env, epoch);

        // Aggregate data for each whitelisted token
        for i in 0..whitelisted_tokens.len() {
            if let Some(token) = whitelisted_tokens.get(i) {
                let decimals: u32 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::TokenDecimals(token.clone()))
                    .ok_or(TreasuryError::TokenDecimalsNotRegistered)?;

                // Get accumulated inflows (fees collected for this token)
                let inflows: i128 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::Collected(token.clone()))
                    .unwrap_or(0);

                // Get current balance (ending balance)
                let balance = token::Client::new(&env, &token).balance(&env.current_contract_address());

                // For now, outflows is calculated as (inflows - ending_balance)
                // In a more complete implementation, we would track explicit withdrawal events
                let outflows = (inflows - balance).max(0);

                report.set_token_balance(token.clone(), decimals, inflows, outflows, balance);
            }
        }

        // Store the report for historical reference
        env.storage()
            .persistent()
            .set(&DataKey::TreasuryReport(epoch), &report);
        Self::bump_persistent(&env, &DataKey::TreasuryReport(epoch));

        // Update the last reported epoch
        env.storage()
            .persistent()
            .set(&DataKey::LastReportedEpoch, &epoch);
        Self::bump_persistent(&env, &DataKey::LastReportedEpoch);

        Ok(report)
    }

    /// Retrieve a previously generated transparency report by epoch.
    ///
    /// **Parameters:**
    /// - `epoch` — The epoch identifier of the report to retrieve.
    ///
    /// **Returns:** The report if it exists, None otherwise.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_transparency_report(env: Env, epoch: u64) -> Option<report::TreasuryReport> {
        env.storage()
            .persistent()
            .get(&DataKey::TreasuryReport(epoch))
    }

    /// Get the most recent epoch for which a report was generated.
    ///
    /// **Returns:** The epoch number, or 0 if no reports have been generated yet.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_last_reported_epoch(env: Env) -> u64 {
        env.storage()
            .persistent()
            .get(&DataKey::LastReportedEpoch)
            .unwrap_or(0)
    }

    /// Set the decimal precision registered for a token.
    ///
    /// Direct updates require the admin and are disabled when treasury multisig
    /// quorum is configured. In that case, use `SetTokenDecimals` through the
    /// treasury proposal workflow.
    pub fn set_token_decimals(
        env: Env,
        admin: Address,
        token: Address,
        decimals: u32,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_no_quorum_required(&env)?;
        Self::update_token_decimals(&env, &admin, &token, decimals)
    }

    /// Return the registered number of decimal places for a token, if configured.
    pub fn get_token_decimals(env: Env, token: Address) -> Option<u32> {
        env.storage()
            .persistent()
            .get(&DataKey::TokenDecimals(token))
    }

    fn update_token_decimals(
        env: &Env,
        actor: &Address,
        token: &Address,
        decimals: u32,
    ) -> Result<(), KoraError> {
        if decimals > MAX_TOKEN_DECIMALS {
            return Err(KoraError::InvalidParameterValue);
        }
        env.storage()
            .persistent()
            .set(&DataKey::TokenDecimals(token.clone()), &decimals);
        Self::bump_persistent(env, &DataKey::TokenDecimals(token.clone()));
        Self::append_audit_entry(
            env,
            actor,
            AdminActionType::SetTokenDecimals,
            Some(token.clone()),
            Some(i128::from(decimals)),
        );
        Ok(())
    }

    // ── Views ─────────────────────────────────────────────────────────────────
    /// Returns the running total of fees collected for a given token (informational ledger).
    ///
    /// This counter is maintained by `collect_fee` and decremented by `withdraw`. It is
    /// informational and does not gate any operation — actual spendable funds are determined
    /// by `get_balance`.
    ///
    /// **Parameters:**
    /// - `token` — The token contract address to query.
    ///
    /// **Returns:** Total fees collected in the token's smallest unit. Returns `0` if no
    /// fees have been collected for this token yet.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_collected(env: Env, token: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Collected(token))
            .unwrap_or(0)
    }

    /// Returns the current admin address.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotInitialized` — Contract has not been initialized.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_admin(env: Env) -> Result<Address, TreasuryError> {
        env.storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(TreasuryError::NotInitialized)
    }

    /// Get the total value of collected fees for given tokens in a reference currency.
    /// Converts each token's collected balance via the price oracle and sums the result.
    ///
    /// **Parameters:**
    /// - `tokens` — Vec of token contract addresses to aggregate.
    /// - `token_symbols` — Vec of corresponding token symbols (must match length of `tokens`).
    /// - `reference_currency` — The target currency symbol for valuation (e.g., "USDC").
    /// - `ref_decimals` — Decimal places of the reference currency (typically 6 or 7).
    ///
    /// **Returns:** Total collected fee revenue across all tokens, converted to reference currency.
    /// Tokens for which no oracle price is available are skipped gracefully (0 contribution).
    /// Returns 0 if no oracle is configured or all balances are zero.
    ///
    /// **Errors:**
    /// - `TreasuryError::InvalidAmount` — Token and symbol vecs have mismatched lengths.
    /// - `TreasuryError::TokenDecimalsNotRegistered` — A nonzero token balance lacks registered decimals.
    ///
    /// **Security:** Read-only view. No authorization required. Price oracle errors (staleness, missing prices)
    /// are silently tolerated — conversions are skipped for unavailable price pairs.
    ///
    pub fn get_total_collected_value(
        env: Env,
        tokens: Vec<Address>,
        token_symbols: Vec<Symbol>,
        reference_currency: Symbol,
        ref_decimals: u32,
    ) -> Result<i128, TreasuryError> {
        if tokens.len() != token_symbols.len() {
            return Err(TreasuryError::InvalidAmount);
        }

        // If no oracle is configured, return 0 (cannot perform conversions)
        let oracle_addr: Option<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::PriceOracle);
        let oracle_addr = match oracle_addr {
            Some(addr) => addr,
            None => return Ok(0),
        };

        let oracle_client = kora_price_oracle::PriceOracleContractClient::new(&env, &oracle_addr);
        let mut total: i128 = 0;

        for i in 0..tokens.len() {
            let token_addr = tokens.get(i).unwrap();
            let symbol = token_symbols.get(i).unwrap();

            let collected: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::Collected(token_addr.clone()))
                .unwrap_or(0);

            if collected == 0 {
                continue;
            }

            let decimals: u32 = env
                .storage()
                .persistent()
                .get(&DataKey::TokenDecimals(token_addr.clone()))
                .ok_or(TreasuryError::TokenDecimalsNotRegistered)?;

            // Attempt conversion; if it fails (missing price, stale, etc.), skip this token
            let converted = match oracle_client.try_convert_with_decimals(
                &collected,
                &symbol,
                &reference_currency,
                &decimals,
                &ref_decimals,
            ) {
                Ok(Ok(val)) => val,
                _ => 0, // Skip this token's contribution if conversion fails
            };

            total = total
                .checked_add(converted)
                .unwrap_or(i128::MAX); // Saturate on overflow
        }

        Ok(total)
    }

    // ── Admin transfer (two-step) ──────────────────────────────────────────────

    /// Step 1: current admin proposes a new admin. Does not transfer yet.
    pub fn propose_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if admin == new_admin {
            return Err(KoraError::InvalidAddress);
        }
        kora_shared::validation::require_not_self(&env, &new_admin)?;
        env.storage().persistent().set(&DataKey::PendingAdmin, &new_admin);
        Self::bump_persistent(&env, &DataKey::PendingAdmin);
        events::admin_proposed(&env, &admin, &new_admin);
        Ok(())
    }

    /// Step 2: proposed admin accepts, completing the transfer.
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), KoraError> {
        new_admin.require_auth();
        let pending: Address = env
            .storage()
            .persistent()
            .get(&DataKey::PendingAdmin)
            .ok_or(KoraError::NoPendingAdminProposal)?;
        if pending != new_admin {
            return Err(KoraError::NotPendingAdmin);
        }
        let current_admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .unwrap_or(new_admin.clone());
        env.storage().persistent().set(&DataKey::Admin, &new_admin);
        Self::bump_persistent(&env, &DataKey::Admin);
        env.storage().persistent().remove(&DataKey::PendingAdmin);
        events::admin_transferred(&env, &current_admin, &new_admin);
        Ok(())
    }

    /// Cancel a pending admin proposal. Current admin only.
    pub fn cancel_admin_proposal(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if !env.storage().persistent().has(&DataKey::PendingAdmin) {
            return Err(KoraError::NoPendingAdminProposal);
        }
        env.storage().persistent().remove(&DataKey::PendingAdmin);
        events::admin_proposal_cancelled(&env, &admin);
        Ok(())
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    /// Propose a WASM upgrade. Admin only. Begins a 24-hour timelock.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `new_wasm_hash` — SHA-256 hash of the new WASM binary (32 bytes).
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. Apply with `execute_upgrade` after
    /// `UPGRADE_TIMELOCK_DELAY` (24 h) has elapsed.
    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::do_propose_upgrade(&env, &admin, &new_wasm_hash)?;
        Ok(())
    }

    fn do_propose_upgrade(
        env: &Env,
        admin: &Address,
        new_wasm_hash: &BytesN<32>,
    ) -> Result<(), KoraError> {
        env.storage().instance().set(
            &DataKey::UpgradeProposal,
            &(new_wasm_hash.clone(), env.ledger().timestamp()),
        );
        events::upgrade_proposed(env, admin, new_wasm_hash);
        Self::append_audit_entry(
            env,
            admin,
            AdminActionType::TreasuryProposeUpgrade,
            None,
            None,
        );
        Ok(())
    }

    /// Execute a previously proposed WASM upgrade after the 24-hour timelock has elapsed.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::NoUpgradeProposed` — No upgrade proposal is pending.
    /// - `TreasuryError::UpgradeTimelockNotElapsed` — 24-hour timelock has not yet passed.
    ///
    /// **Security:** Requires `admin.require_auth()`. Clears the proposal atomically before executing.
    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(TreasuryError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(TreasuryError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        events::upgrade_executed(&env, &admin, &wasm_hash);
        Self::append_audit_entry(
            &env,
            &admin,
            AdminActionType::TreasuryExecuteUpgrade,
            None,
            None,
        );
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Audit Log ─────────────────────────────────────────────────────────────

    /// Internal audit log entry append with rolling checksum.
    fn append_audit_entry_internal(env: &Env, actor: &Address, action: String) {
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditTotal)
            .unwrap_or(0);
        let head: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditHead)
            .unwrap_or(0);

        let timestamp = env.ledger().timestamp();
        let sequence = total;

        let entry = AuditEntry {
            action: action.clone(),
            actor: actor.clone(),
            timestamp,
            sequence,
        };

        // 1. Update rolling checksum before writing (so the checksum covers this entry)
        let prev_checksum: BytesN<32> = env
            .storage()
            .instance()
            .get(&DataKey::AuditChecksum)
            .unwrap_or_else(|| BytesN::from_array(env, &[0u8; 32]));
        let new_checksum = chain_checksum(env, &prev_checksum, &entry);
        env.storage()
            .instance()
            .set(&DataKey::AuditChecksum, &new_checksum);

        // 2. Emit the canonical ADM_AUDIT event
        events::adm_audit(env, sequence, action, actor, timestamp);

        // 3. If this write will overwrite an existing slot, emit checkpoint first
        if total > 0 && total % MAX_AUDIT_LOG_SIZE == 0 {
            // The slot `head` currently holds the oldest entry in the window
            if let Some(old_entry) = env
                .storage()
                .persistent()
                .get::<DataKey, AuditEntry>(&DataKey::AuditEntry(head))
            {
                events::audit_checkpoint(
                    env,
                    total,
                    new_checksum.clone(),
                    old_entry.action,
                    &old_entry.actor,
                    old_entry.timestamp,
                    old_entry.sequence,
                );
            }
        }

        // 4. Write entry into ring buffer
        env.storage()
            .persistent()
            .set(&DataKey::AuditEntry(head), &entry);

        // 5. Advance head and total
        let next_head = (head + 1) % MAX_AUDIT_LOG_SIZE;
        env.storage()
            .instance()
            .set(&DataKey::AuditHead, &next_head);
        env.storage()
            .instance()
            .set(&DataKey::AuditTotal, &(total + 1));
    }

    // ── Recipient allowlist (#457) ───────────────────────────────────────────

    /// Propose a new allowed withdrawal recipient. Takes effect after
    /// `UPGRADE_TIMELOCK_DELAY` (24 h), mirroring `propose_withdrawal_cap`.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn propose_recipient(env: Env, admin: Address, recipient: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().set(
            &DataKey::RecipientProposal(recipient.clone()),
            &env.ledger().timestamp(),
        );
        Self::bump_persistent(&env, &DataKey::RecipientProposal(recipient.clone()));
        events::recipient_proposed(&env, &admin, &recipient);
        Self::append_audit_entry(
            &env,
            &admin,
            AdminActionType::ProposeRecipient,
            Some(recipient),
            None,
        );
        Ok(())
    }

    /// Execute a previously proposed recipient after the timelock elapses, adding it
    /// to the allowlist that `withdraw`/`emergency_withdraw` recipients must belong to.
    ///
    /// **Errors:**
    /// - `KoraError::NoRecipientProposed` — No proposal pending for this address.
    /// - `KoraError::RecipientTimelockNotElapsed` — 24-hour timelock has not yet passed.
    pub fn execute_recipient(env: Env, admin: Address, recipient: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let proposed_at: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::RecipientProposal(recipient.clone()))
            .ok_or(KoraError::NoRecipientProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(KoraError::RecipientTimelockNotElapsed);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::RecipientProposal(recipient.clone()));
        env.storage()
            .persistent()
            .set(&DataKey::AllowedRecipient(recipient.clone()), &true);
        Self::bump_persistent(&env, &DataKey::AllowedRecipient(recipient.clone()));
        events::recipient_allowed(&env, &admin, &recipient);
        Self::append_audit_entry(
            &env,
            &admin,
            AdminActionType::ExecuteRecipient,
            Some(recipient),
            None,
        );
        Ok(())
    }

    /// Returns whether `recipient` is a matured, allowlisted withdrawal destination.
    pub fn is_recipient_allowed(env: Env, recipient: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::AllowedRecipient(recipient))
            .unwrap_or(false)
    }

    // ── Insurance / loss reserve (#458) ──────────────────────────────────────

    /// Set the portion (bps, 0–10 000) of every newly `collect_fee`'d amount that is
    /// earmarked into the per-token loss reserve instead of the freely withdrawable pool.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn set_reserve_allocation_bps(env: Env, admin: Address, bps: u32) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_valid_fee_bps(bps)?;
        env.storage()
            .persistent()
            .set(&DataKey::ReserveAllocationBps, &bps);
        Self::bump_persistent(&env, &DataKey::ReserveAllocationBps);
        Self::append_audit_entry(
            &env,
            &admin,
            AdminActionType::SetReserveAllocation,
            None,
            Some(bps as i128),
        );
        Ok(())
    }

    /// Authorize (or deauthorize) an address — typically a `financing_pool` deployment —
    /// to call `disburse_from_reserve`.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn set_reserve_caller(
        env: Env,
        admin: Address,
        caller: Address,
        authorized: bool,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .set(&DataKey::AuthorizedReserveCaller(caller.clone()), &authorized);
        Self::bump_persistent(&env, &DataKey::AuthorizedReserveCaller(caller.clone()));
        Self::append_audit_entry(
            &env,
            &admin,
            AdminActionType::SetReserveCaller,
            Some(caller),
            Some(if authorized { 1 } else { 0 }),
        );
        Ok(())
    }

    /// Draw down the token's earmarked loss reserve to partially reimburse investors on a
    /// recorded default. Callable only by an address previously authorized via
    /// `set_reserve_caller` (e.g. `financing_pool`).
    ///
    /// **Errors:**
    /// - `KoraError::ReserveCallerNotAuthorized` — `caller` is not an authorized reserve caller.
    /// - `KoraError::InvalidAmount` — `amount` is ≤ 0.
    /// - `KoraError::InsufficientReserveBalance` — `amount` exceeds the token's reserve balance.
    ///
    /// **Security:** Requires `caller.require_auth()` — a genuine contract-to-contract auth
    /// check, since `financing_pool` calls this programmatically.
    pub fn disburse_from_reserve(
        env: Env,
        caller: Address,
        token: Address,
        amount: i128,
        recipient: Address,
    ) -> Result<(), KoraError> {
        caller.require_auth();
        let authorized: bool = env
            .storage()
            .persistent()
            .get(&DataKey::AuthorizedReserveCaller(caller.clone()))
            .unwrap_or(false);
        if !authorized {
            return Err(KoraError::ReserveCallerNotAuthorized);
        }
        if amount <= 0 {
            return Err(KoraError::InvalidAmount);
        }
        require_within_max_amount(amount)?;

        let reserve_key = DataKey::ReserveBalance(token.clone());
        let reserve_balance: i128 = env.storage().persistent().get(&reserve_key).unwrap_or(0);
        if amount > reserve_balance {
            return Err(KoraError::InsufficientReserveBalance);
        }

        let _guard = ReentrancyGuard::new(&env)?;

        env.storage()
            .persistent()
            .set(&reserve_key, &(reserve_balance - amount));
        Self::bump_persistent(&env, &reserve_key);

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        events::reserve_disbursed(&env, &caller, &token, &recipient, amount);
        Self::append_audit_entry(
            &env,
            &caller,
            AdminActionType::DisburseFromReserve,
            Some(token),
            Some(amount),
        );
        Ok(())
    }

    /// Returns the token's current loss-reserve balance (excluded from admin withdrawals).
    pub fn get_reserve_balance(env: Env, token: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::ReserveBalance(token))
            .unwrap_or(0)
    }

    /// Returns the current reserve allocation rate in basis points.
    pub fn get_reserve_allocation_bps(env: Env) -> u32 {
        // Registry-aware: an explicitly governed registry value takes precedence
        // over the local setting; bootstrap defaults do not override local config.
        if let Some(reg) = env
            .storage()
            .persistent()
            .get::<_, Address>(&DataKey::ParameterRegistry)
        {
            let client = ParameterRegistryContractClient::new(&env, &reg);
            if client.is_param_governed(&RegistryParamKey::ReserveAllocationBps) {
                let v = client.get_value(&RegistryParamKey::ReserveAllocationBps);
                if v >= 0 && v <= 10_000 {
                    return v as u32;
                }
            }
        }
        env.storage()
            .persistent()
            .get(&DataKey::ReserveAllocationBps)
            .unwrap_or(0)
    }

    /// Returns whether `caller` is authorized to call `disburse_from_reserve`.
    pub fn is_reserve_caller(env: Env, caller: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::AuthorizedReserveCaller(caller))
            .unwrap_or(false)
    }

    // ── Multisig quorum gate (#455) ──────────────────────────────────────────

    /// Link this treasury to an `access_control` deployment. When that deployment has a
    /// multisig configured with `threshold > 1`, treasury's highest-risk functions
    /// (`withdraw`, `emergency_withdraw`, `set_fee_bps`, `propose_upgrade`) can no longer be
    /// called directly — they must go through `propose_treasury_action` →
    /// `approve_treasury_action` → `execute_treasury_action` instead.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn set_access_control(env: Env, admin: Address, access_control: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_not_self(&env, &access_control)?;
        env.storage()
            .persistent()
            .set(&DataKey::AccessControl, &access_control);
        Self::bump_persistent(&env, &DataKey::AccessControl);
        Self::append_audit_entry(
            &env,
            &admin,
            AdminActionType::SetAccessControl,
            None,
            None,
        );
        Ok(())
    }

    /// Returns the linked `access_control` address, if configured.
    pub fn get_access_control(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::AccessControl)
    }

    /// Set the parameter registry used to enforce the governed grant reserve floor.
    pub fn set_parameter_registry(
        env: Env,
        admin: Address,
        registry: Address,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_no_quorum_required(&env)?;
        Self::update_parameter_registry(&env, &registry)
    }

    fn update_parameter_registry(env: &Env, registry: &Address) -> Result<(), KoraError> {
        require_not_self(env, registry)?;
        let parameter = ParameterRegistryContractClient::new(env, registry)
            .get_parameter(&RegistryParamKey::MinReserveRatioBps)
            .ok_or(KoraError::InvalidParameterValue)?;
        if parameter.value < 0 || parameter.value > 10_000 {
            return Err(KoraError::InvalidParameterValue);
        }
        env.storage()
            .persistent()
            .set(&DataKey::ParameterRegistry, registry);
        Self::bump_persistent(env, &DataKey::ParameterRegistry);
        Ok(())
    }

    /// Returns the configured parameter registry, if set.
    pub fn get_parameter_registry(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::ParameterRegistry)
    }

    /// Propose one of treasury's highest-risk actions. Caller must be a signer configured
    /// in the linked `access_control` multisig. The proposer's approval is recorded
    /// automatically, matching `access_control::propose_action`.
    ///
    /// **Errors:**
    /// - `KoraError::MultisigNotConfigured` — No `access_control` multisig is configured.
    /// - `KoraError::SignerNotFound` — Caller is not a configured signer.
    pub fn propose_treasury_action(
        env: Env,
        proposer: Address,
        action: TreasuryAction,
    ) -> Result<u64, KoraError> {
        proposer.require_auth();
        let config = Self::load_signer_config(&env)?;
        Self::require_signer(&config, &proposer)?;

        let proposal_id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::NextTreasuryProposalId)
            .unwrap_or(1);

        let mut approvals = Vec::new(&env);
        approvals.push_back(proposer.clone());

        let proposal = TreasuryProposal {
            id: proposal_id,
            action,
            proposer: proposer.clone(),
            approvals,
            executed: false,
            created_at: env.ledger().timestamp(),
            expires_at: env.ledger().timestamp() + TREASURY_PROPOSAL_TTL,
        };

        env.storage()
            .persistent()
            .set(&DataKey::TreasuryProposal(proposal_id), &proposal);
        let timelock_delay = kora_shared::timelock::get_delay(&env);
        env.storage().persistent().set(
            &DataKey::TreasuryProposalTimelock(proposal_id),
            &timelock_delay,
        );
        Self::bump_persistent(&env, &DataKey::TreasuryProposal(proposal_id));
        Self::bump_persistent(&env, &DataKey::TreasuryProposalTimelock(proposal_id));
        env.storage().persistent().set(
            &DataKey::NextTreasuryProposalId,
            &(proposal_id
                .checked_add(1)
                .ok_or(KoraError::ArithmeticOverflow)?),
        );

        Self::append_audit_entry(
            &env,
            &proposer,
            AdminActionType::ProposeTreasuryAction,
            None,
            Some(proposal_id as i128),
        );
        Ok(proposal_id)
    }

    /// Approve a pending treasury proposal. Caller must be a configured signer who has not
    /// already voted on this proposal.
    pub fn approve_treasury_action(env: Env, approver: Address, proposal_id: u64) -> Result<(), KoraError> {
        approver.require_auth();
        let config = Self::load_signer_config(&env)?;
        Self::require_signer(&config, &approver)?;

        let mut proposal: TreasuryProposal = env
            .storage()
            .persistent()
            .get(&DataKey::TreasuryProposal(proposal_id))
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
            .set(&DataKey::TreasuryProposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::TreasuryProposal(proposal_id));

        Self::append_audit_entry(
            &env,
            &approver,
            AdminActionType::ApproveTreasuryAction,
            None,
            Some(proposal_id as i128),
        );
        Ok(())
    }

    /// Execute a treasury proposal once its approval threshold is met, the timelock has elapsed,
    /// and it has not expired, applying the underlying action ('withdraw' / 'emergency_withdraw' / 'set_fee_bps' / 'propose_upgrade').
    ///
    /// **Errors:**
    /// - `KoraError::ThresholdNotMet` — Not enough approvals collected yet.
    /// - `KoraError::GovernanceTimelockNotElapsed` — Governance timelock has not yet passed.
    pub fn execute_treasury_action(env: Env, executor: Address, proposal_id: u64) -> Result<(), KoraError> {
        executor.require_auth();
        let config = Self::load_signer_config(&env)?;
        Self::require_signer(&config, &executor)?;

        let mut proposal: TreasuryProposal = env
            .storage()
            .persistent()
            .get(&DataKey::TreasuryProposal(proposal_id))
            .ok_or(KoraError::ProposalNotFound)?;

        if proposal.executed {
            return Err(KoraError::ProposalAlreadyExecuted);
        }
        if env.ledger().timestamp() > proposal.expires_at {
            return Err(KoraError::ProposalExpired);
        }
        if (proposal.approvals.len()) < config.threshold {
            return Err(KoraError::ThresholdNotMet);
        }
        let proposed_delay: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::TreasuryProposalTimelock(proposal_id))
            .unwrap_or_else(|| kora_shared::timelock::get_delay(&env));
        let required_delay = proposed_delay.max(kora_shared::timelock::get_delay(&env));
        let executable_at = proposal
            .created_at
            .checked_add(required_delay)
            .ok_or(KoraError::ArithmeticOverflow)?;
        if env.ledger().timestamp() < executable_at {
            return Err(KoraError::GovernanceTimelockNotElapsed);
        }

        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::TreasuryProposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::TreasuryProposal(proposal_id));

        match proposal.action {
            TreasuryAction::Withdraw(token, recipient, amount) => {
                Self::do_withdraw(&env, &executor, &token, &recipient, amount)
                    .map_err(Self::map_withdraw_err)?;
            }
            TreasuryAction::EmergencyWithdraw(token, recipient) => {
                Self::do_emergency_withdraw(&env, &executor, &token, &recipient)?;
            }
            TreasuryAction::SetFeeBps(fee_bps) => {
                Self::do_set_fee_bps(&env, &executor, fee_bps)?;
            }
            TreasuryAction::ProposeUpgrade(wasm_hash) => {
                Self::do_propose_upgrade(&env, &executor, &wasm_hash)?;
            }
            TreasuryAction::ReviewGrant(grant_id) => Self::review_grant(&env, grant_id)?,
            TreasuryAction::ReleaseGrantMilestone(grant_id) => {
                Self::release_grant_milestone(&env, grant_id)?;
            }
            TreasuryAction::ReclaimGrant(grant_id) => Self::reclaim_grant(&env, grant_id)?,
            TreasuryAction::SetTimelockDelay(delay) => {
                if delay < UPGRADE_TIMELOCK_DELAY {
                    return Err(KoraError::InvalidParameterValue);
                }
                kora_shared::timelock::set_delay(&env, delay);
            }
            TreasuryAction::SetParameterRegistry(registry) => {
                Self::update_parameter_registry(&env, &registry)?;
            }
            TreasuryAction::SetTokenDecimals(token, decimals) => {
                Self::update_token_decimals(&env, &executor, &token, decimals)?;
            }
        }

        Self::append_audit_entry(
            &env,
            &executor,
            AdminActionType::ExecuteTreasuryAction,
            None,
            Some(proposal_id as i128),
        );
        Ok(())
    }

    /// Get a treasury proposal by ID.
    pub fn get_treasury_proposal(env: Env, proposal_id: u64) -> Result<TreasuryProposal, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::TreasuryProposal(proposal_id))
            .ok_or(KoraError::ProposalNotFound)
    }

    /// Submit a public grant request. Each milestone becomes a separate
    /// quorum-and-timelock-gated treasury action after reviewers approve the grant.
    pub fn submit_grant_proposal(
        env: Env,
        proposer: Address,
        recipient: Address,
        token: Address,
        scope: String,
        milestones: Vec<i128>,
    ) -> Result<u64, TreasuryError> {
        proposer.require_auth();
        if scope.len() == 0 || scope.len() > 512 || milestones.len() == 0
            || milestones.len() > MAX_GRANT_MILESTONES
        {
            return Err(TreasuryError::InvalidGrant);
        }
        Self::require_whitelisted_token(&env, &token)?;
        let mut requested = 0i128;
        for amount in milestones.iter() {
            if amount <= 0 {
                return Err(TreasuryError::InvalidGrant);
            }
            requested = requested.checked_add(amount).ok_or(TreasuryError::ArithmeticOverflow)?;
        }
        let id: u64 = env.storage().persistent().get(&DataKey::NextGrantId).unwrap_or(1);
        let now = env.ledger().timestamp();
        let grant = GrantProposal {
            id,
            proposer,
            recipient,
            token,
            scope,
            milestones,
            next_milestone: 0,
            remaining: requested,
            reviewed: false,
            closed: false,
            submitted_at: now,
            last_activity_at: now,
        };
        env.storage().persistent().set(&DataKey::Grant(id), &grant);
        env.storage().persistent().set(
            &DataKey::NextGrantId,
            &id.checked_add(1).ok_or(TreasuryError::ArithmeticOverflow)?,
        );
        Ok(id)
    }

    pub fn get_grant_proposal(env: Env, grant_id: u64) -> Result<GrantProposal, TreasuryError> {
        env.storage().persistent().get(&DataKey::Grant(grant_id)).ok_or(TreasuryError::GrantNotFound)
    }

    /// Submit an evidence hash for the grant's next milestone. The recipient must
    /// authenticate; a quorum-approved release action is the review of that evidence.
    pub fn submit_grant_milestone_proof(
        env: Env,
        recipient: Address,
        grant_id: u64,
        proof_hash: BytesN<32>,
    ) -> Result<(), KoraError> {
        recipient.require_auth();
        let mut grant: GrantProposal = env.storage().persistent()
            .get(&DataKey::Grant(grant_id))
            .ok_or(KoraError::ProposalNotFound)?;
        if grant.recipient != recipient || !grant.reviewed || grant.closed
            || grant.next_milestone >= grant.milestones.len()
        {
            return Err(KoraError::Unauthorized);
        }

        let proof_key = DataKey::GrantMilestoneProof(grant_id, grant.next_milestone);
        if env.storage().persistent().has(&proof_key) {
            return Err(KoraError::Unauthorized);
        }
        env.storage().persistent().set(&proof_key, &proof_hash);
        Self::bump_persistent(&env, &proof_key);
        grant.last_activity_at = env.ledger().timestamp();
        env.storage().persistent().set(&DataKey::Grant(grant_id), &grant);
        Self::bump_persistent(&env, &DataKey::Grant(grant_id));
        Ok(())
    }

    /// Read the evidence hash submitted for a milestone, if any.
    pub fn get_grant_milestone_proof(
        env: Env,
        grant_id: u64,
        milestone_index: u32,
    ) -> Option<BytesN<32>> {
        env.storage().persistent()
            .get(&DataKey::GrantMilestoneProof(grant_id, milestone_index))
    }

    fn review_grant(env: &Env, grant_id: u64) -> Result<(), KoraError> {
        let mut grant: GrantProposal = env.storage().persistent().get(&DataKey::Grant(grant_id))
            .ok_or(KoraError::ProposalNotFound)?;
        if grant.closed || grant.reviewed {
            return Err(KoraError::Unauthorized);
        }
        grant.reviewed = true;
        grant.last_activity_at = env.ledger().timestamp();
        env.storage().persistent().set(&DataKey::Grant(grant_id), &grant);
        Ok(())
    }

    fn release_grant_milestone(env: &Env, grant_id: u64) -> Result<(), KoraError> {
        let _guard = ReentrancyGuard::new(env)?;
        let mut grant: GrantProposal = env.storage().persistent().get(&DataKey::Grant(grant_id))
            .ok_or(KoraError::ProposalNotFound)?;
        if grant.closed {
            return Err(KoraError::Unauthorized);
        }
        if !grant.reviewed {
            return Err(KoraError::Unauthorized);
        }
        if !env.storage().persistent().has(&DataKey::GrantMilestoneProof(
            grant_id,
            grant.next_milestone,
        )) {
            return Err(KoraError::Unauthorized);
        }
        let amount = grant.milestones.get(grant.next_milestone).ok_or(KoraError::ProposalNotFound)?;
        let balance = token::Client::new(env, &grant.token).balance(&env.current_contract_address());
        let reserve: i128 = env.storage().persistent().get(&DataKey::ReserveBalance(grant.token.clone())).unwrap_or(0);
        // Reserve floor requires a wired parameter_registry; without one the
        // floor is zero (same graceful fallback used across the module).
        let min_reserve_ratio_bps: u32 = env
            .storage()
            .persistent()
            .get::<_, Address>(&DataKey::ParameterRegistry)
            .map(|registry| {
                ParameterRegistryContractClient::new(env, &registry)
                    .get_parameter(&RegistryParamKey::MinReserveRatioBps)
                    .map(|p| p.value)
                    .unwrap_or(0)
            })
            .unwrap_or(0)
            .clamp(0, 10_000) as u32;
        Self::check_grant_reserve(balance, reserve, amount, min_reserve_ratio_bps)?;

        grant.remaining = grant
            .remaining
            .checked_sub(amount)
            .ok_or(KoraError::ArithmeticUnderflow)?;
        grant.next_milestone = grant
            .next_milestone
            .checked_add(1)
            .ok_or(KoraError::ArithmeticOverflow)?;
        grant.last_activity_at = env.ledger().timestamp();
        if grant.next_milestone >= grant.milestones.len() {
            grant.closed = true;
        }
        env.storage().persistent().set(&DataKey::Grant(grant_id), &grant);
        Self::bump_persistent(env, &DataKey::Grant(grant_id));

        token::Client::new(env, &grant.token).transfer(
            &env.current_contract_address(),
            &grant.recipient,
            &amount,
        );
        Ok(())
    }

    fn check_grant_reserve(
        balance: i128,
        earmarked_reserve: i128,
        amount: i128,
        min_reserve_ratio_bps: u32,
    ) -> Result<(), KoraError> {
        if balance.saturating_sub(earmarked_reserve) < amount {
            return Err(KoraError::InsufficientPoolBalance);
        }
        let ratio_product = balance
            .checked_mul(min_reserve_ratio_bps as i128)
            .ok_or(KoraError::ArithmeticOverflow)?;
        let minimum_remaining = ratio_product
            .checked_add(9_999)
            .ok_or(KoraError::ArithmeticOverflow)?
            .checked_div(10_000)
            .ok_or(KoraError::ArithmeticOverflow)?;
        let remaining = balance
            .checked_sub(amount)
            .ok_or(KoraError::ArithmeticUnderflow)?;
        if remaining < minimum_remaining {
            return Err(KoraError::InsufficientReserveBalance);
        }
        Ok(())
    }

    fn reclaim_grant(env: &Env, grant_id: u64) -> Result<(), KoraError> {
        let mut grant: GrantProposal = env.storage().persistent().get(&DataKey::Grant(grant_id))
            .ok_or(KoraError::ProposalNotFound)?;
        if grant.closed || !grant.reviewed || grant.remaining <= 0
            || env.ledger().timestamp() < grant.last_activity_at.saturating_add(GRANT_RECLAIM_DELAY)
        {
            return Err(KoraError::Unauthorized);
        }
        grant.closed = true;
        grant.remaining = 0;
        env.storage().persistent().set(&DataKey::Grant(grant_id), &grant);
        Ok(())
    }

    // ── Revenue-Sharing Distribution (#667) ───────────────────────────────────

    /// Propose a new distribution configuration with an updated stakeholder list.
    /// Admin only. Requires full governance if multisig is configured.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `stakeholders` — Vec of addresses eligible for revenue sharing.
    /// - `distribution_bps` — Basis points of fees to distribute (0–10,000).
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::InvalidFeeRate` — `distribution_bps` > 10,000.
    /// - `TreasuryError::InvalidDistributionConfig` — Stakeholders list is empty.
    pub fn propose_distribution(
        env: Env,
        admin: Address,
        stakeholders: Vec<Address>,
        distribution_bps: u32,
    ) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if stakeholders.is_empty() {
            return Err(TreasuryError::InvalidDistributionConfig);
        }
        require_valid_fee_bps(distribution_bps)?;

        let epoch: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::CurrentDistributionEpoch)
            .unwrap_or(0);

        let now = env.ledger().timestamp();
        let expires_at = now + TREASURY_PROPOSAL_TTL;

        let proposal = DistributionProposal {
            epoch: epoch + 1,
            stakeholders: stakeholders.clone(),
            distribution_bps,
            proposed_at: now,
            expires_at,
        };

        env.storage()
            .persistent()
            .set(&DataKey::DistributionProposal, &proposal);
        Self::bump_persistent(&env, &DataKey::DistributionProposal);

        events::fee_collected(&env, &admin, epoch, distribution_bps as i128, &env.current_contract_address());
        Ok(())
    }

    /// Execute a pending distribution proposal. Admin only.
    /// Locks in the stakeholder set for this epoch.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::DistributionProposalNotFound` — No proposal pending.
    pub fn execute_distribution(env: Env, admin: Address) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let proposal: DistributionProposal = env
            .storage()
            .persistent()
            .get(&DataKey::DistributionProposal)
            .ok_or(TreasuryError::DistributionProposalNotFound)?;

        let now = env.ledger().timestamp();
        if now > proposal.expires_at {
            return Err(TreasuryError::DistributionProposalNotFound);
        }

        let config = DistributionConfig {
            epoch: proposal.epoch,
            stakeholders: proposal.stakeholders.clone(),
            created_at: now,
            distribution_bps: proposal.distribution_bps,
        };

        env.storage()
            .persistent()
            .set(&DataKey::DistributionConfig(proposal.epoch), &config);
        Self::bump_persistent(&env, &DataKey::DistributionConfig(proposal.epoch));

        env.storage()
            .persistent()
            .set(&DataKey::CurrentDistributionEpoch, &proposal.epoch);
        Self::bump_persistent(&env, &DataKey::CurrentDistributionEpoch);

        env.storage().persistent().remove(&DataKey::DistributionProposal);

        Ok(())
    }

    /// Claim accumulated revenue share for the current epoch.
    /// Eligible stakeholders can withdraw their pro-rata share of distributed fees.
    ///
    /// **Parameters:**
    /// - `stakeholder` — The address claiming their share.
    /// - `token` — The token to claim in.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotEligibleStakeholder` — Caller is not a registered stakeholder.
    /// - `TreasuryError::NoShareAvailable` — No claimable share available.
    /// - `TreasuryError::InsufficientPoolBalance` — Treasury balance insufficient.
    pub fn claim_share(
        env: Env,
        stakeholder: Address,
        token: Address,
    ) -> Result<i128, TreasuryError> {
        stakeholder.require_auth();

        let epoch: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::CurrentDistributionEpoch)
            .unwrap_or(0);

        // Load the distribution config for this epoch
        let config: DistributionConfig = env
            .storage()
            .persistent()
            .get(&DataKey::DistributionConfig(epoch))
            .ok_or(TreasuryError::NotEligibleStakeholder)?;

        // Verify stakeholder is in the list
        let is_eligible = config.stakeholders.iter().any(|s| s == stakeholder);
        if !is_eligible {
            return Err(TreasuryError::NotEligibleStakeholder);
        }

        // Check if already claimed this epoch
        let claim_key = DataKey::StakeholderClaim(stakeholder.clone(), epoch, token.clone());
        if env.storage().persistent().has(&claim_key) {
            return Err(TreasuryError::NoShareAvailable);
        }

        // Calculate available balance from distribution pool
        let collected_key = DataKey::Collected(token.clone());
        let total_collected: i128 = env
            .storage()
            .persistent()
            .get(&collected_key)
            .unwrap_or(0);

        let pool_key = DataKey::DistributionPool(token.clone());
        let current_pool: i128 = env
            .storage()
            .persistent()
            .get(&pool_key)
            .unwrap_or(0);

        // Calculate distribution pool if not yet allocated
        let available_pool = if current_pool == 0 {
            kora_shared::validation::bps_of(total_collected, config.distribution_bps)?
        } else {
            current_pool
        };

        if available_pool <= 0 {
            return Err(TreasuryError::NoShareAvailable);
        }

        // Calculate this stakeholder's pro-rata share
        let stakeholder_count = config.stakeholders.len() as i128;
        let individual_share = available_pool / stakeholder_count;

        if individual_share <= 0 {
            return Err(TreasuryError::NoShareAvailable);
        }

        // Record claim
        env.storage()
            .persistent()
            .set(&claim_key, &individual_share);
        Self::bump_persistent(&env, &claim_key);

        // Transfer funds
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&env.current_contract_address(), &stakeholder, &individual_share);

        events::fee_withdrawn(&env, &stakeholder, &token, individual_share);

        Ok(individual_share)
    }

    /// Get current distribution epoch number.
    pub fn get_distribution_epoch(env: Env) -> u64 {
        env.storage()
            .persistent()
            .get(&DataKey::CurrentDistributionEpoch)
            .unwrap_or(0)
    }

    /// Get the distribution configuration for a specific epoch.
    pub fn get_distribution_config(env: Env, epoch: u64) -> Option<DistributionConfig> {
        env.storage()
            .persistent()
            .get(&DataKey::DistributionConfig(epoch))
    }

    /// Get the pending distribution proposal.
    pub fn get_pending_distribution(env: Env) -> Option<DistributionProposal> {
        env.storage()
            .persistent()
            .get(&DataKey::DistributionProposal)
    }

    // ── Fee Sweep Automation (#742) ─────────────────────────────────────────

    /// Configure destinations and percentage splits for fee sweeps. Admin only.
    ///
    /// **Parameters:**
    /// - `splits` — A list of (destination, split_bps) tuples. The sum of split_bps
    ///   across all destinations must equal exactly 10,000 basis points (100%).
    /// - `primary` — The primary destination address that will receive any rounding dust.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::InvalidSweepSplits` — Splits is empty, contains an invalid address,
    ///   or the sum of split_bps does not equal 10,000.
    pub fn configure_sweep_splits(
        env: Env,
        admin: Address,
        splits: Vec<(Address, u32)>,
        primary: Address,
    ) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        if splits.is_empty() {
            return Err(TreasuryError::InvalidSweepSplits);
        }

        let mut total_bps: u32 = 0;
        let mut has_primary = false;
        for i in 0..splits.len() {
            let (dest, bps) = splits.get(i).unwrap();
            kora_shared::validation::require_not_self(&env, &dest)?;
            if dest == primary {
                has_primary = true;
            }
            total_bps = total_bps
                .checked_add(bps)
                .ok_or(TreasuryError::ArithmeticOverflow)?;
        }

        if total_bps != 10_000 || !has_primary {
            return Err(TreasuryError::InvalidSweepSplits);
        }

        env.storage().persistent().set(&DataKey::SweepSplit, &splits);
        Self::bump_persistent(&env, &DataKey::SweepSplit);
        env.storage().persistent().set(&DataKey::SweepPrimaryDestination, &primary);
        Self::bump_persistent(&env, &DataKey::SweepPrimaryDestination);

        events::sweep_splits_configured(&env, &admin, splits.len(), env.ledger().timestamp());
        Ok(())
    }

    /// Read the currently configured sweep split destinations and percentages.
    pub fn get_sweep_splits(env: Env) -> Option<Vec<(Address, u32)>> {
        env.storage().persistent().get(&DataKey::SweepSplit)
    }

    /// Read the primary destination that receives rounding dust during sweeps.
    pub fn get_sweep_primary_destination(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::SweepPrimaryDestination)
    }

    /// Permissionless sweep entrypoint: sweeps accumulated protocol fees for a given token
    /// to the configured split destinations according to their basis point allocations.
    ///
    /// **Parameters:**
    /// - `token` — The token contract address to sweep.
    ///
    /// **Behavior:**
    /// - Checks that the token is whitelisted.
    /// - Rejects if sweep splits have not been configured (`SweepSplitsNotConfigured`).
    /// - Distributable amount is `Collected(token) - ReserveBalance(token)`.
    /// - If distributable amount is <= 0, gracefully returns `Ok(())` (no-op).
    /// - Protects against reentrancy via `ReentrancyGuard`.
    /// - Transfers allocated shares to each destination, and routes rounding dust to the
    ///   configured primary destination.
    /// - Updates `Collected(token)` accounting by subtracting the swept amount.
    /// - Emits `sweep_executed` events for each destination.
    pub fn sweep(env: Env, token: Address) -> Result<(), TreasuryError> {
        Self::require_whitelisted_token(&env, &token)?;

        let splits: Vec<(Address, u32)> = env
            .storage()
            .persistent()
            .get(&DataKey::SweepSplit)
            .ok_or(TreasuryError::SweepSplitsNotConfigured)?;

        let primary: Address = env
            .storage()
            .persistent()
            .get(&DataKey::SweepPrimaryDestination)
            .ok_or(TreasuryError::SweepSplitsNotConfigured)?;

        let token_client = token::Client::new(&env, &token);
        let balance = token_client.balance(&env.current_contract_address());
        let reserved: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::ReserveBalance(token.clone()))
            .unwrap_or(0);
        let spendable = balance.saturating_sub(reserved);

        let collected_key = DataKey::Collected(token.clone());
        let collected: i128 = env
            .storage()
            .persistent()
            .get(&collected_key)
            .unwrap_or(0);

        let distributable = spendable.min(collected);
        if distributable <= 0 {
            return Ok(());
        }

        let _guard = ReentrancyGuard::new(&env)?;

        let mut total_distributed: i128 = 0;
        let now = env.ledger().timestamp();

        for i in 0..splits.len() {
            let (dest, bps) = splits.get(i).unwrap();
            let share = kora_shared::validation::bps_of(distributable, bps)?;
            if share > 0 {
                token_client.transfer(&env.current_contract_address(), &dest, &share);
                events::sweep_executed(&env, &token, &dest, share, now);
                total_distributed = total_distributed
                    .checked_add(share)
                    .ok_or(TreasuryError::ArithmeticOverflow)?;
            }
        }

        let dust = distributable.saturating_sub(total_distributed);
        if dust > 0 {
            token_client.transfer(&env.current_contract_address(), &primary, &dust);
            events::sweep_executed(&env, &token, &primary, dust, now);
            total_distributed = total_distributed
                .checked_add(dust)
                .ok_or(TreasuryError::ArithmeticOverflow)?;
        }

        let new_collected = collected.saturating_sub(total_distributed);
        env.storage().persistent().set(&collected_key, &new_collected);
        Self::bump_persistent(&env, &collected_key);

        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

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

    fn require_whitelisted_token(env: &Env, token: &Address) -> Result<(), KoraError> {
        let whitelisted: bool = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false);
        if !whitelisted {
            return Err(KoraError::TokenNotWhitelisted);
        }
        Ok(())
    }

    fn require_allowed_recipient(env: &Env, recipient: &Address) -> Result<(), KoraError> {
        let allowed: bool = env
            .storage()
            .persistent()
            .get(&DataKey::AllowedRecipient(recipient.clone()))
            .unwrap_or(false);
        if !allowed {
            return Err(KoraError::RecipientNotAllowed);
        }
        Ok(())
    }

    /// Loads the linked `access_control`'s multisig config, if both a link and a multisig
    /// are configured. Returns `None` for the degenerate/unconfigured migration case.
    fn load_signer_config(env: &Env) -> Result<MultisigConfig, KoraError> {
        let ac: Address = env
            .storage()
            .persistent()
            .get(&DataKey::AccessControl)
            .ok_or(KoraError::MultisigNotConfigured)?;
        let client = AccessControlContractClient::new(env, &ac);
        client
            .try_get_multisig_config()
            .map_err(|_| KoraError::MultisigNotConfigured)?
            .map_err(|_| KoraError::MultisigNotConfigured)
    }

    fn try_load_signer_config(env: &Env) -> Option<MultisigConfig> {
        Self::load_signer_config(env).ok()
    }

    fn require_signer(config: &MultisigConfig, caller: &Address) -> Result<(), KoraError> {
        for i in 0..config.signers.len() {
            if &config.signers.get(i).ok_or(KoraError::Unauthorized)? == caller {
                return Ok(());
            }
        }
        Err(KoraError::SignerNotFound)
    }

    /// Highest-risk functions may only be called directly (single-signature) when no
    /// `access_control` multisig with `threshold > 1` is configured — the degenerate,
    /// backward-compatible case for deployments that haven't opted into multisig.
    fn require_no_quorum_required(env: &Env) -> Result<(), KoraError> {
        if let Some(config) = Self::try_load_signer_config(env) {
            if config.threshold > 1 {
                return Err(KoraError::QuorumRequired);
            }
        }
        Ok(())
    }

    /// Advance the epoch if 24 h have elapsed, then check the cap.
    fn enforce_rate_limit(env: &Env, token: &Address, amount: i128) -> Result<(), KoraError> {
        let cap: i128 = env
            .storage()
            .instance()
            .get(&DataKey::WithdrawalCap(token.clone()))
            .unwrap_or(0);
        if cap == 0 {
            return Ok(());
        }

        let now = env.ledger().timestamp();
        let epoch_start_key = DataKey::EpochStart(token.clone());
        let epoch_withdrawn_key = DataKey::EpochWithdrawn(token.clone());
        let epoch_start: u64 = env
            .storage()
            .instance()
            .get(&epoch_start_key)
            .unwrap_or(now);

        let epoch_withdrawn: i128 = if now.saturating_sub(epoch_start) >= EPOCH_DURATION {
            // New epoch: reset counters.
            env.storage().instance().set(&epoch_start_key, &now);
            env.storage().instance().set(&epoch_withdrawn_key, &0i128);
            0
        } else {
            env.storage().instance().get(&epoch_withdrawn_key).unwrap_or(0)
        };

        let new_total = epoch_withdrawn
            .checked_add(amount)
            .ok_or(KoraError::ArithmeticOverflow)?;
        if new_total > cap {
            return Err(KoraError::WithdrawalRateLimitExceeded);
        }
        Ok(())
    }

    /// Record a successful withdrawal against `token`'s current epoch running total.
    fn record_withdrawal(env: &Env, token: &Address, amount: i128) {
        let epoch_start: u64 = env.storage().instance().get(&DataKey::EpochStart(token.clone())).unwrap_or(env.ledger().timestamp());
        let epoch_withdrawn: i128 = env.storage().instance().get(&DataKey::EpochWithdrawn(token.clone())).unwrap_or(0);
        env.storage().instance().set(&DataKey::EpochStart(token.clone()), &epoch_start);
        env.storage().instance().set(&DataKey::EpochWithdrawn(token.clone()), &(epoch_withdrawn + amount));
    }

    fn transfer_token(env: &Env, _admin: &Address, token: &Address, recipient: &Address, amount: i128) {
        let token_client = token::Client::new(env, token);
        token_client.transfer(&env.current_contract_address(), recipient, &amount);
    }

    fn require_not_paused(env: &Env) -> Result<(), TreasuryError> {
        if let Some(ac_addr) = env.storage().instance().get::<DataKey, Address>(&DataKey::AccessControl) {
            let ac = AccessControlContractClient::new(env, &ac_addr);
            if ac.is_paused() {
                return Err(TreasuryError::NotAdmin); // mapped from KoraError::ProtocolPaused
            }
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

    fn append_audit_entry(
        env: &Env,
        actor: &Address,
        action: AdminActionType,
        token: Option<Address>,
        amount: Option<i128>,
    ) {
        let action_str = match action {
            AdminActionType::SetFeeBps => "set_fee_bps",
            AdminActionType::Withdraw => "withdraw",
            AdminActionType::EmergencyWithdraw => "emergency_withdraw",
            AdminActionType::SetReserveAllocation => "set_reserve_allocation_bps",
            AdminActionType::WhitelistToken => "whitelist_token",
            AdminActionType::ProposeWithdrawalCap => "propose_withdrawal_cap",
            AdminActionType::ExecuteWithdrawalCap => "execute_withdrawal_cap",
            AdminActionType::ProposeUpgrade => "propose_upgrade",
            AdminActionType::ExecuteUpgrade => "execute_upgrade",
            AdminActionType::ProposeRecipient => "propose_recipient",
            AdminActionType::ExecuteRecipient => "execute_recipient",
            AdminActionType::SetAccessControl => "set_access_control",
            AdminActionType::TreasuryProposeUpgrade => "treasury_propose_upgrade",
            AdminActionType::TreasuryExecuteUpgrade => "treasury_execute_upgrade",
            _ => "unknown_admin_action",
        };
        Self::append_audit_entry_internal(env, actor, String::from_str(&env, action_str));
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
        token, Address, Env,
    };

    /// Governance timelock the treasury-action tests must advance past.
    /// Mirrors the shared timelock default (no test overrides it).
    const TREASURY_GOVERNANCE_TIMELOCK_DELAY: u64 =
        kora_shared::timelock::DEFAULT_UPGRADE_DELAY;

    fn setup() -> (Env, Address, TreasuryContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &50u32);
        (env, admin, client)
    }

    #[test]
    fn test_grant_reserve_floor_preserves_earmarked_and_ratio_reserves() {
        assert!(TreasuryContract::check_grant_reserve(1_000, 100, 800, 1_000).is_ok());
        assert_eq!(
            TreasuryContract::check_grant_reserve(1_000, 100, 901, 1_000),
            Err(KoraError::InsufficientPoolBalance)
        );
        assert_eq!(
            TreasuryContract::check_grant_reserve(1_000, 0, 901, 1_000),
            Err(KoraError::InsufficientReserveBalance)
        );
        assert_eq!(
            TreasuryContract::check_grant_reserve(1_001, 0, 1_000, 10),
            Err(KoraError::InsufficientReserveBalance)
        );
    }

    #[test]
    fn test_grant_reserve_floor_rejects_arithmetic_overflow() {
        assert_eq!(
            TreasuryContract::check_grant_reserve(i128::MAX, 0, 1, 10_000),
            Err(KoraError::ArithmeticOverflow)
        );
    }

    // ── Basic contract tests ──────────────────────────────────────────────────
    /// Deploy a minimal Soroban token contract and return its address +
    /// a client minted with `amount` to `recipient`.
    fn deploy_token(env: &Env, admin: &Address, recipient: &Address, amount: i128) -> Address {
        let token_id = env.register_stellar_asset_contract_v2(admin.clone()).address();
        let token_client = token::StellarAssetClient::new(env, &token_id);
        token_client.mint(recipient, &amount);
        token_id
    }

    fn configure_grant_multisig(
        env: &Env,
        admin: &Address,
        client: &TreasuryContractClient<'_>,
    ) -> (Address, Address) {
        let access_control_id = env.register_contract(
            None,
            kora_access_control::AccessControlContract,
        );
        let access_control =
            kora_access_control::AccessControlContractClient::new(env, &access_control_id);
        access_control.initialize(admin);
        let signer_one = Address::generate(env);
        let signer_two = Address::generate(env);
        let mut signers = Vec::new(env);
        signers.push_back(signer_one.clone());
        signers.push_back(signer_two.clone());
        access_control.configure_multisig(admin, &signers, &2u32);
        client.set_access_control(admin, &access_control_id);
        (signer_one, signer_two)
    }

    fn execute_grant_action(
        env: &Env,
        client: &TreasuryContractClient<'_>,
        signer_one: &Address,
        signer_two: &Address,
        action: TreasuryAction,
    ) {
        let proposal_id = client.propose_treasury_action(signer_one, &action);
        client.approve_treasury_action(signer_two, &proposal_id);
        env.ledger()
            .with_mut(|ledger| ledger.timestamp += TREASURY_GOVERNANCE_TIMELOCK_DELAY);
        client.execute_treasury_action(signer_one, &proposal_id);
    }

    #[test]
    fn test_grant_proof_partial_release_reclaim_and_report_lifecycle() {
        let (env, admin, client) = setup();
        let treasury_id = client.address.clone();
        let recipient = Address::generate(&env);
        let token_id = deploy_token(&env, &admin, &treasury_id, 100);
        client.whitelist_token(&admin, &token_id);
        // Register decimals before the multisig quorum is configured below:
        // set_token_decimals is a direct admin call gated by require_no_quorum_required.
        client.set_token_decimals(&admin, &token_id, &7u32);
        let (signer_one, signer_two) = configure_grant_multisig(&env, &admin, &client);
        client.collect_fee(&token_id, &100i128);

        let mut milestones = Vec::new(&env);
        milestones.push_back(20i128);
        milestones.push_back(30i128);
        let grant_id = client.submit_grant_proposal(
            &signer_one,
            &recipient,
            &token_id,
            &String::from_str(&env, "Community support program"),
            &milestones,
        );

        execute_grant_action(
            &env,
            &client,
            &signer_one,
            &signer_two,
            TreasuryAction::ReviewGrant(grant_id),
        );
        assert!(client.get_grant_proposal(&grant_id).reviewed);

        let release_proposal = client.propose_treasury_action(
            &signer_one,
            &TreasuryAction::ReleaseGrantMilestone(grant_id),
        );
        client.approve_treasury_action(&signer_two, &release_proposal);
        env.ledger()
            .with_mut(|ledger| ledger.timestamp += TREASURY_GOVERNANCE_TIMELOCK_DELAY);
        assert_eq!(
            client
                .try_execute_treasury_action(&signer_one, &release_proposal)
                .unwrap_err()
                .unwrap(),
            KoraError::Unauthorized
        );

        let proof_one = BytesN::<32>::from_array(&env, &[1u8; 32]);
        let stranger = Address::generate(&env);
        assert_eq!(
            client
                .try_submit_grant_milestone_proof(&stranger, &grant_id, &proof_one)
                .unwrap_err()
                .unwrap(),
            KoraError::Unauthorized
        );
        client.submit_grant_milestone_proof(&recipient, &grant_id, &proof_one);
        assert_eq!(client.get_grant_milestone_proof(&grant_id, &0), Some(proof_one));

        execute_grant_action(
            &env,
            &client,
            &signer_one,
            &signer_two,
            TreasuryAction::ReleaseGrantMilestone(grant_id),
        );
        let after_partial_release = client.get_grant_proposal(&grant_id);
        assert_eq!(after_partial_release.next_milestone, 1);
        assert_eq!(after_partial_release.remaining, 30);
        assert!(!after_partial_release.closed);
        assert_eq!(token::Client::new(&env, &token_id).balance(&recipient), 20);

        let proof_two = BytesN::<32>::from_array(&env, &[2u8; 32]);
        client.submit_grant_milestone_proof(&recipient, &grant_id, &proof_two);
        env.ledger()
            .with_mut(|ledger| ledger.timestamp += GRANT_RECLAIM_DELAY);
        execute_grant_action(
            &env,
            &client,
            &signer_one,
            &signer_two,
            TreasuryAction::ReclaimGrant(grant_id),
        );

        let reclaimed = client.get_grant_proposal(&grant_id);
        assert!(reclaimed.closed);
        assert_eq!(reclaimed.remaining, 0);
        assert_eq!(reclaimed.next_milestone, 1);
        assert_eq!(token::Client::new(&env, &token_id).balance(&recipient), 20);

        let mut report_tokens = Vec::new(&env);
        report_tokens.push_back(token_id.clone());
        let report = client.generate_transparency_report(&admin, &7u64, &report_tokens);
        let token_report = report.balances.get(0).unwrap();
        assert_eq!(token_report.inflows, 100);
        assert_eq!(token_report.outflows, 20);
        assert_eq!(token_report.ending_balance, 80);
        assert_eq!(client.get_last_reported_epoch(), 7);
        assert_eq!(
            client
                .get_transparency_report(&7u64)
                .unwrap()
                .balances
                .get(0)
                .unwrap()
                .outflows,
            20,
        );
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_creates_contract() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        assert!(client.try_initialize(&admin, &50u32).is_ok());
        assert_eq!(client.get_fee_bps(), 50);
    }

    #[test]
    fn test_initialize_already_initialized() {
        let (env, admin, client) = setup();
        let result = client.try_initialize(&admin, &0u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_self_as_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        let result = client.try_initialize(&contract_id, &0u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_fee_bps_after_init() {
        let (_env, _admin, client) = setup();
        assert_eq!(client.get_fee_bps(), 50);
    }

    #[test]
    fn test_token_decimal_registry_requires_admin_and_bounds_precision() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);

        client.set_token_decimals(&admin, &token, &0);
        assert_eq!(client.get_token_decimals(&token), Some(0));
        client.set_token_decimals(&admin, &token, &18);
        assert_eq!(client.get_token_decimals(&token), Some(18));
        assert!(client.try_set_token_decimals(&admin, &token, &19).is_err());

        let non_admin = Address::generate(&env);
        assert!(client.try_set_token_decimals(&non_admin, &token, &6).is_err());
        assert_eq!(client.get_token_decimals(&token), Some(18));
    }

    #[test]
    fn test_transparency_report_requires_registered_token_decimals() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let tokens = soroban_sdk::vec![&env, token];

        assert!(client
            .try_generate_transparency_report(&admin, &1, &tokens)
            .is_err());
        assert_eq!(client.get_last_reported_epoch(), 0);
    }

    #[test]
    fn test_transparency_report_includes_registered_token_decimals() {
        let (env, admin, client) = setup();
        let treasury = client.address.clone();
        let token = deploy_token(&env, &admin, &treasury, 123_456);
        client.set_token_decimals(&admin, &token, &6);
        let tokens = soroban_sdk::vec![&env, token.clone()];

        let report = client.generate_transparency_report(&admin, &1, &tokens);
        let balance = report.balances.get(0).unwrap();

        assert_eq!(balance.token, token);
        assert_eq!(balance.decimals, 6);
        assert_eq!(balance.ending_balance, 123_456);
    }

    // ── set_fee_bps ───────────────────────────────────────────────────────────

    #[test]
    fn test_set_fee_bps_success() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &100u32);
        assert_eq!(client.get_fee_bps(), 100);
    }

    #[test]
    fn test_set_fee_bps_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        assert!(client.try_set_fee_bps(&non_admin, &100u32).is_err());
    }

    #[test]
    fn test_set_fee_bps_over_max_fails() {
        let (_env, admin, client) = setup();
        assert!(client.try_set_fee_bps(&admin, &10_001u32).is_err());
    }

    #[test]
    fn test_set_fee_bps_zero_allowed() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &0u32);
        assert_eq!(client.get_fee_bps(), 0);
    }

    #[test]
    fn test_set_fee_bps_max_allowed() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &10_000u32);
        assert_eq!(client.get_fee_bps(), 10_000);
    }

    #[test]
    fn test_set_fee_bps_multiple_updates() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &100u32);
        assert_eq!(client.get_fee_bps(), 100);
        client.set_fee_bps(&admin, &200u32);
        assert_eq!(client.get_fee_bps(), 200);
        client.set_fee_bps(&admin, &50u32);
        assert_eq!(client.get_fee_bps(), 50);
    }

    #[test]
    fn test_fee_rate_updated_event_reports_correct_old_fee() {
        let (env, admin, client) = setup();
        // With default 50, set to 75
        client.set_fee_bps(&admin, &75u32);
        assert_eq!(client.get_fee_bps(), 75);
        // Update to 100 — old must be 75
        client.set_fee_bps(&admin, &100u32);
        assert_eq!(client.get_fee_bps(), 100);
    }

    // ── withdraw ──────────────────────────────────────────────────────────────

    #[test]
    fn test_withdraw_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client.try_withdraw(&non_admin, &token, &recipient, &1_000_000i128).is_err());
    }

    #[test]
    fn test_withdraw_zero_amount_fails() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client.try_withdraw(&admin, &token, &recipient, &0i128).is_err());
    }
}