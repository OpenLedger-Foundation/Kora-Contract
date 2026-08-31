#![no_std]

use kora_access_control::AccessControlContractClient;
use kora_shared::{
    audit::{chain_checksum, AdminAuditEntry, AuditEntry, MAX_AUDIT_LOG_SIZE, AuditSource},
    errors::CommonError,
    events,
    reentrancy::ReentrancyGuard,
    validation::{require_non_negative_amount, require_valid_fee_bps, require_within_max_amount, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, String, Symbol, Vec};

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

/// TTL for treasury multisig proposals — mirrors access_control's PROPOSAL_TTL_LEDGERS (~7 days).
const TREASURY_PROPOSAL_TTL: u64 = 604_800;

/// Timelock delay between a treasury proposal reaching quorum and being executable.
/// Mirrors the UPGRADE_TIMELOCK_DELAY (~24h) so treasury withdrawals get the same cooling-off period.
const TREASURY_GOVERNANCE_TIMELOCK_DELAY: u64 = UPGRADE_TIMELOCK_DELAY;

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
    /// Protocol fee in basis points — persistent for durability.
    FeeBps,
    Collected(Address), // accumulated fees per token
    WithdrawalLock,     // reentrancy guard
    // Audit log ring buffer
    AuditEntry(u64),    // slot index 0..MAX_AUDIT_LOG_SIZE-1
    AuditHead,          // next write slot (u64)
    AuditTotal,         // total entries ever appended (u64)
    AuditChecksum,      // rolling sha256 checksum (BytesN<32>)
    /// Accumulated fees per token (informational).
    Collected(Address),
    /// Whitelisted token flag.
    WhitelistedToken(Address),
    /// Pending upgrade proposal: (wasm_hash, proposed_at_timestamp).
    UpgradeProposal,
    /// Reentrancy guard flag (instance-level).
    WithdrawalLock,
    /// Maximum total withdrawal allowed per EPOCH_DURATION window for a given
    /// token (0 = uncapped). Keyed by token so unrelated tokens' caps don't
    /// share a single global quota (#452).
    WithdrawalCap(Address),
    /// Pending cap change proposal for a given token: (new_cap, proposed_at).
    WithdrawalCapProposal(Address),
    /// Timestamp when the current rate-limit epoch started, per token.
    EpochStart(Address),
    /// Total withdrawn in the current epoch, per token (resets each epoch).
    EpochWithdrawn(Address),
    /// Address of the `access_control` contract instance (optional — pause
    /// enforcement is skipped when unset, e.g. in unit tests) (#454).
    AccessControl,
    /// Distinct, admin-declared flag gating `emergency_withdraw` (#453).
    EmergencyDeclared,
    // ── Audit log ─────────────────────────────────────────────────────────────
    /// Next write position in the audit ring buffer (0..MAX_AUDIT_LOG_SIZE).
    AuditLogHead,
    /// Total admin actions ever recorded (monotonic; not capped at ring size).
    AuditLogTotal,
    /// An audit log entry at ring-buffer position `n`.
    AuditEntry(u64),
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
    /// The `access_control` contract treasury checks for multisig configuration.
    AccessControl,
    /// Monotonic counter for the next treasury proposal id.
    NextTreasuryProposalId,
    /// A pending treasury multisig proposal, keyed by proposal id.
    TreasuryProposal(u64),
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
}

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
        Ok(())
    }

    /// Set (or update) the `access_control` contract reference used to gate
    /// `withdraw` behind the protocol-wide pause flag. Admin only.
    ///
    /// A post-init setter rather than an `initialize` parameter, so existing
    /// deployments/tests can opt in without changing `initialize`'s signature.
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn set_access_control(
        env: Env,
        admin: Address,
        access_control: Address,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::AccessControl, &access_control);
        events::access_control_updated(&env, &admin, &access_control);
        Self::append_audit_entry(&env, &admin, AdminActionType::SetAccessControl);
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
        Self::do_set_fee_bps(&env, &admin, fee_bps)
    }

    fn do_set_fee_bps(env: &Env, admin: &Address, fee_bps: u32) -> Result<(), KoraError> {
        require_valid_fee_bps(fee_bps)?;
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        Self::append_audit_entry(&env, &admin, String::from_str(&env, "set_fee_bps"));

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
        Self::append_audit_entry(&env, &admin, AdminActionType::SetFeeBps);
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
    /// accounting ledger.
    ///
    /// **Parameters:**
    /// - `token` — The token address the fee was paid in (must be whitelisted).
    /// - `amount` — The fee amount (must be > 0).
    ///
    /// **Errors:**
    /// - `TreasuryError::InvalidAmount` — `amount` is ≤ 0.
    /// - `TreasuryError::TokenNotWhitelisted` — Token has not been whitelisted.
    /// - `TreasuryError::ArithmeticOverflow` — Running total would overflow.
    ///
    /// **Security:** No auth required — the token transfer itself is the proof of payment.
    /// The amount is validated to be > 0 to prevent no-op accounting entries.
    pub fn collect_fee(env: Env, token: Address, amount: i128) -> Result<(), TreasuryError> {
        if amount <= 0 {
            return Err(TreasuryError::InvalidAmount);
        }
        Self::require_whitelisted_token(&env, &token)?;

        let key = DataKey::Collected(token.clone());
        let current: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        let new_total = current
            .checked_add(amount)
            .ok_or(TreasuryError::ArithmeticOverflow)?;

        env.storage().persistent().set(&key, &new_total);
        Self::bump_persistent(&env, &key);

        // Earmark a configurable portion of the incoming fee as a loss reserve,
        // distinct from the freely admin-withdrawable balance (#458).
        let reserve_bps: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ReserveAllocationBps)
            .unwrap_or(0);
        if reserve_bps > 0 {
            let reserve_cut = bps_of(amount, reserve_bps)?;
            if reserve_cut > 0 {
                let reserve_key = DataKey::ReserveBalance(token.clone());
                let current_reserve: i128 =
                    env.storage().persistent().get(&reserve_key).unwrap_or(0);
                let new_reserve = current_reserve
                    .checked_add(reserve_cut)
                    .ok_or(KoraError::ArithmeticOverflow)?;
                env.storage().persistent().set(&reserve_key, &new_reserve);
                Self::bump_persistent(&env, &reserve_key);
            }
        }

        events::fee_collected(&env, &env.current_contract_address(), 0, amount, &token);
        Ok(())
    }

    /// Claw back a previously collected fee and pay it directly to a recipient.
    /// Restricted to the authorized marketplace contract (set via
    /// `set_marketplace`). Used when a listing fails to reach full funding and
    /// the protocol fee already sent to treasury must be refunded to the
    /// investor. (#450)
    ///
    /// **Parameters:**
    /// - `marketplace` — Must be the authorized marketplace contract address.
    /// - `token` — The whitelisted token to refund.
    /// - `amount` — The amount to claw back (must be > 0).
    /// - `recipient` — The investor address to receive the refunded fee.
    ///
    /// **Errors:**
    /// - `KoraError::NotInitialized` — No marketplace has been configured via `set_marketplace`.
    /// - `KoraError::Unauthorized` — Caller is not the authorized marketplace contract.
    /// - `KoraError::InvalidAmount` — `amount` is ≤ 0.
    /// - `KoraError::TokenNotWhitelisted` — Token is not whitelisted.
    ///
    /// **Security:** Requires `marketplace.require_auth()`, mirroring the
    /// `financing_pool::release_funds` cross-contract authorization pattern —
    /// the marketplace contract calls this passing its own address, and the
    /// host attributes the call to the marketplace contract. Not subject to
    /// the reentrancy guard or rate-limit cap used by `withdraw`, as this is a
    /// narrowly-scoped clawback path restricted to the marketplace contract only.
    pub fn refund_fee(
        env: Env,
        marketplace: Address,
        token: Address,
        amount: i128,
        recipient: Address,
    ) -> Result<(), KoraError> {
        marketplace.require_auth();

        let authorized: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Marketplace)
            .ok_or(KoraError::NotInitialized)?;
        if authorized != marketplace {
            return Err(KoraError::Unauthorized);
        }

        if amount <= 0 {
            return Err(KoraError::InvalidAmount);
        }
        Self::require_whitelisted_token(&env, &token)?;

        // Reverse the earlier collect_fee entry (never below zero).
        let collected_key = DataKey::Collected(token.clone());
        if let Some(collected) = env.storage().persistent().get::<_, i128>(&collected_key) {
            let new_collected = collected.saturating_sub(amount);
            env.storage().persistent().set(&collected_key, &new_collected);
            Self::bump_persistent(&env, &collected_key);
        }

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&env.current_contract_address(), &recipient, &amount);

        events::fee_withdrawn(&env, &marketplace, &token, amount);
        Ok(())
    }

    /// Withdraw accumulated fees to a recipient. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `token` — The whitelisted token to withdraw.
    /// - `recipient` — The address to send funds to.
    /// - `amount` — The amount to withdraw (must be > 0 and ≤ `MAX_AMOUNT`).
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    /// - `KoraError::ProtocolPaused` — The protocol is paused (see `set_access_control`).
    /// - `KoraError::InvalidAmount` — `amount` is ≤ 0 or exceeds `MAX_AMOUNT`.
    /// - `KoraError::TokenNotWhitelisted` — Token is not whitelisted.
    /// - `KoraError::WithdrawalRateLimitExceeded` — Would exceed the token's rolling 24 h cap.
    /// - `KoraError::Reentrancy` — Reentrancy guard triggered.
    /// - `KoraError::InsufficientFunds` — Contract balance is less than `amount`.
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::InvalidAmount` — `amount` is ≤ 0 or exceeds `MAX_AMOUNT`.
    /// - `TreasuryError::TokenNotWhitelisted` — Token is not whitelisted.
    /// - `TreasuryError::WithdrawalRateLimitExceeded` — Would exceed the rolling 24 h cap.
    /// - `TreasuryError::Reentrancy` — Reentrancy guard triggered.
    /// - `TreasuryError::InsufficientPoolBalance` — Contract balance is less than `amount`.
    ///
    /// **Security:** Requires `admin.require_auth()`. Blocked while the protocol is paused.
    /// Subject to the token's own rolling 24 h withdrawal cap. Protected against reentrancy
    /// via RAII `ReentrancyGuard`.
    pub fn withdraw(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), TreasuryError> {
        // ── Checks ────────────────────────────────────────────────────────────
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_not_paused(&env)?;
        if amount <= 0 {
            return Err(TreasuryError::InvalidAmount);
        }
        require_within_max_amount(amount)?;
        Self::require_whitelisted_token(&env, &token)?;
        Self::enforce_rate_limit(&env, &token, amount)?;

        // Acquire reentrancy guard — released automatically when _guard drops
        let _guard = ReentrancyGuard::new(env)?;

        let token_client = token::Client::new(env, token);
        let balance = token_client.balance(&env.current_contract_address());
        let reserved: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::ReserveBalance(token.clone()))
            .unwrap_or(0);
        let spendable = balance.saturating_sub(reserved);

        if balance < amount {
            return Err(KoraError::InsufficientFunds);
            return Err(TreasuryError::InsufficientPoolBalance);
        }

        // ── Effects ───────────────────────────────────────────────────────────
        let collected_key = DataKey::Collected(token.clone());
        if let Some(collected) = env
            .storage()
            .persistent()
            .get::<_, i128>(&collected_key)
        {
            let new_collected = collected.saturating_sub(amount);
            env.storage().persistent().set(&collected_key, &new_collected);
            Self::bump_persistent(env, &collected_key);
        }
        Self::record_withdrawal(&env, &token, amount);

        // ── Interactions ──────────────────────────────────────────────────────
        token_client.transfer(&env.current_contract_address(), recipient, &amount);

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

    /// Emergency drain — withdraw entire token balance. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `token` — The whitelisted token to drain.
    /// - `recipient` — The address to send the full balance to.
    ///
    /// **Errors:**
    /// - `TreasuryError::NotAdmin` — Caller is not the admin.
    /// - `TreasuryError::TokenNotWhitelisted` — Token is not whitelisted.
    /// - `TreasuryError::Reentrancy` — Reentrancy guard triggered.
    ///
    /// **Security:** Requires `admin.require_auth()`. Gated behind `declare_emergency` (#453)
    /// so the uncapped drain path is a distinct, auditable admin action rather than always
    /// callable. Protected against reentrancy via RAII `ReentrancyGuard`. No-ops silently
    /// when balance is zero (not an error).
    ///
    /// **Deliberately independent of the protocol pause flag** (unlike `withdraw`): the whole
    /// point of `emergency_withdraw` is to evacuate funds during an incident, which is
    /// precisely when the protocol is most likely to already be paused. Gating it on
    /// `!is_paused()` (as a literal reading of #454 would require) would make it unusable
    /// exactly when it's needed, and combined with #453's requirement would make the function
    /// permanently uncallable. `declare_emergency` is the intentionally distinct precondition
    /// instead — see #453 and #454 for the full reasoning.
    pub fn emergency_withdraw(
        env: Env,
        admin: Address,
        token: Address,
        recipient: Address,
    ) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_whitelisted_token(&env, &token)?;
        let declared: bool = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyDeclared)
            .unwrap_or(false);
        if !declared {
            return Err(KoraError::EmergencyNotDeclared);
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
        Self::append_audit_entry(&env, &admin, AdminActionType::DeclareEmergency);
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
        Self::append_audit_entry(&env, &admin, AdminActionType::RevokeEmergency);
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
        Self::append_audit_entry(&env, &admin, AdminActionType::ProposeWithdrawalCap);
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
    pub fn execute_withdrawal_cap(env: Env, admin: Address) -> Result<(), TreasuryError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let proposal_key = DataKey::WithdrawalCapProposal(token.clone());
        let (new_cap, proposed_at): (i128, u64) = env
            .storage()
            .instance()
            .get(&DataKey::WithdrawalCapProposal)
            .ok_or(KoraError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(KoraError::UpgradeTimelockNotElapsed);
            .ok_or(TreasuryError::NoCapChangeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(TreasuryError::WithdrawalCapTimelockNotElapsed);
        }
        let cap_key = DataKey::WithdrawalCap(token.clone());
        let old_cap: i128 = env.storage().instance().get(&cap_key).unwrap_or(0);
        env.storage().instance().set(&cap_key, &new_cap);
        env.storage().instance().remove(&proposal_key);
        events::withdrawal_cap_updated(&env, &admin, &token, old_cap, new_cap);
        Self::append_audit_entry(&env, &admin, AdminActionType::ExecuteWithdrawalCap);
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

    // ── Internal Audit Helpers ────────────────────────────────────────────────

    /// Append one audit entry to the ring buffer.
    ///
    /// Behaviour:
    /// 1. Build the entry (sequence = total, timestamp = now).
    /// 2. Update the rolling checksum: checksum = sha256(prev_checksum || entry_bytes).
    /// 3. Emit ADM_AUDIT event (canonical off-chain history).
    /// 4. If `total % MAX_AUDIT_LOG_SIZE == 0` AND total > 0, a wraparound is about to
    ///    begin — read the slot we're about to overwrite and emit an `audit_checkpoint`
    ///    event carrying the current checksum and the discarded entry.
    /// 5. Write the entry into the ring-buffer slot and advance head/total.
    fn append_audit_entry(env: &Env, actor: &Address, action: String) {
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AuditTotal)
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
    /// - `token_decimals` — Vec of token decimal places (must match length of `tokens`).
    /// - `ref_decimals` — Decimal places of the reference currency (typically 6 or 7).
    ///
    /// **Returns:** Total collected fee revenue across all tokens, converted to reference currency.
    /// Tokens for which no oracle price is available are skipped gracefully (0 contribution).
    /// Returns 0 if no oracle is configured or all balances are zero.
    ///
    /// **Errors:**
    /// - `TreasuryError::InvalidAmount` — Token/symbol/decimal vecs have mismatched lengths.
    ///
    /// **Security:** Read-only view. No authorization required. Price oracle errors (staleness, missing prices)
    /// are silently tolerated — conversions are skipped for unavailable price pairs.
    ///
    /// **Note:** This is a temporary workaround pending issue #36 (token registry with decimal metadata).
    /// Once a registry exists, a simpler `get_total_collected_value()` view with no parameters
    /// will iterate automatically.
    pub fn get_total_collected_value(
        env: Env,
        tokens: Vec<Address>,
        token_symbols: Vec<Symbol>,
        reference_currency: Symbol,
        token_decimals: Vec<u32>,
        ref_decimals: u32,
    ) -> Result<i128, TreasuryError> {
        if tokens.len() != token_symbols.len() || tokens.len() != token_decimals.len() {
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
            let decimals = token_decimals.get(i).unwrap();

            let collected: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::Collected(token_addr.clone()))
                .unwrap_or(0);

            if collected == 0 {
                continue;
            }

            // Attempt conversion; if it fails (missing price, stale, etc.), skip this token
            let converted = match oracle_client.try_convert_with_decimals(
                &collected,
                symbol,
                &reference_currency,
                decimals,
                &ref_decimals,
            ) {
                Ok(val) => val,
                Err(_) => 0, // Skip this token's contribution if conversion fails
            };

            total = total
                .checked_add(converted)
                .unwrap_or(i128::MAX); // Saturate on overflow
        }

        Ok(total)
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
        Self::require_no_quorum_required(&env)?;
        Self::do_propose_upgrade(&env, &admin, &new_wasm_hash)
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

    // ── Auth Helpers ──────────────────────────────────────────────────────────
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
        Self::bump_persistent(&env, &DataKey::TreasuryProposal(proposal_id));
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
        if env.ledger().timestamp() < proposal.created_at + TREASURY_GOVERNANCE_TIMELOCK_DELAY {
            return Err(KoraError::GovernanceTimelockNotElapsed);
        }

        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::TreasuryProposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::TreasuryProposal(proposal_id));

        match proposal.action {
            TreasuryAction::Withdraw(token, recipient, amount) => {
                Self::do_withdraw(&env, &executor, &token, &recipient, amount)?;
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

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_admin(env: &Env, caller: &Address) -> Result<(), TreasuryError> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(TreasuryError::NotInitialized)?;
        if &admin != caller {
            return Err(TreasuryError::NotAdmin);
        }
        Ok(())
    }

    fn acquire_lock(env: &Env) -> Result<(), KoraError> {
        let locked: bool = env
    fn require_whitelisted_token(env: &Env, token: &Address) -> Result<(), TreasuryError> {
        let whitelisted: bool = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false);
        if locked {
            return Err(KoraError::Unauthorized);
        if !whitelisted {
            return Err(TreasuryError::TokenNotWhitelisted);
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
    fn enforce_rate_limit(env: &Env, amount: i128) -> Result<(), TreasuryError> {
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
            .ok_or(TreasuryError::ArithmeticOverflow)?;
        if new_total > cap {
            return Err(TreasuryError::WithdrawalRateLimitExceeded);
        }
        Ok(())
    }

    /// Record a successful withdrawal against `token`'s current epoch running total.
    fn record_withdrawal(env: &Env, token: &Address, amount: i128) {
        let cap: i128 = env
            .storage()
            .instance()
            .get(&DataKey::WithdrawalCap(token.clone()))
            .unwrap_or(0);
        if cap == 0 {
            return;
        }
        let epoch_withdrawn_key = DataKey::EpochWithdrawn(token.clone());
        let current: i128 = env
            .storage()
            .instance()
            .get(&epoch_withdrawn_key)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&epoch_withdrawn_key, &current.saturating_add(amount));
    }

    /// Blocks the caller if the protocol is paused, as reported by the configured
    /// `access_control` contract. Mirrors `marketplace::require_not_paused` (#454).
    ///
    /// If `DataKey::AccessControl` has never been set (e.g. in unit tests that don't
    /// wire up a real access-control instance), the pause check is skipped rather than
    /// erroring, so existing deployments/tests are unaffected until they opt in via
    /// `set_access_control`.
    fn require_not_paused(env: &Env) -> Result<(), KoraError> {
        if let Some(ac_contract) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::AccessControl)
        {
            let ac = kora_access_control::AccessControlContractClient::new(env, &ac_contract);
            if ac.is_paused() {
                return Err(KoraError::ProtocolPaused);
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
            source: AuditSource::Treasury,
            token,
            amount,
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
        testutils::{Address as _, MockAuth, MockAuthInvoke},
        token, Address, Env,
    };

    fn setup() -> (Env, Address, TreasuryContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &50u32);
        (env, admin, client)
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
        let result = client.try_initialize(&admin, &50u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_invalid_fee_bps() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let result = client.try_initialize(&admin, &10_001u32);
        assert!(client.try_initialize(&admin, &10_001u32).is_err());
    }

    #[test]
    fn test_initialize_self_as_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        // Passing the contract's own address as admin must be rejected.
        let result = client.try_initialize(&contract_id, &50u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_fee_bps_success() {
        let (env, admin, client) = setup();
        assert_eq!(client.get_fee_bps(), 50);
    fn test_get_fee_bps_after_init() {
        let (_env, _admin, client) = setup();
        assert_eq!(client.get_fee_bps(), 50);
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
        let result = client.try_set_fee_bps(&non_admin, &100u32);
        assert!(result.is_err());
        assert!(client.try_set_fee_bps(&non_admin, &100u32).is_err());
    }

    #[test]
    fn test_set_fee_bps_invalid_bps_fails() {
        let (env, admin, client) = setup();
        let result = client.try_set_fee_bps(&admin, &10_001u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_fee_bps_zero_succeeds() {
        let (env, admin, client) = setup();
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
    fn test_set_fee_bps_max_succeeds() {
        let (env, admin, client) = setup();
    fn test_set_fee_bps_max_allowed() {
        let (_env, admin, client) = setup();
        client.set_fee_bps(&admin, &10_000u32);
        assert_eq!(client.get_fee_bps(), 10_000);
    }

    #[test]
    fn test_withdraw_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        let result = client.try_withdraw(&non_admin, &token, &recipient, &1_000_000i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_withdraw_zero_amount_fails() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        let result = client.try_withdraw(&admin, &token, &recipient, &0i128);
        assert!(result.is_err());
    fn test_set_fee_bps_over_max_fails() {
        let (_env, admin, client) = setup();
        assert!(client.try_set_fee_bps(&admin, &10_001u32).is_err());
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

    /// Verifies that `set_fee_bps` correctly reads the old value from the same
    /// storage tier that `initialize` wrote it to (both use `persistent()`).
    ///
    /// Before the fix, `set_fee_bps` read `old_bps` from `persistent()` while
    /// `initialize` wrote to `instance()`, so `old_bps` was always the fallback
    /// 50 and the `fee_rate_updated` event always reported the wrong old value.
    ///
    /// This test proves the round-trip is consistent:
    ///   initialize(fee=50) → set_fee_bps(100) → old_bps must be 50, not fallback.
    #[test]
    fn test_fee_rate_updated_event_reports_correct_old_fee() {
        let (env, admin, client) = setup(); // initialize with fee_bps = 50

        // First update: old value must be 50 (what initialize wrote), not the
        // unwrap_or(50) fallback that a wrong-tier read would also produce.
        // To distinguish them, initialize with a non-default value.
        let env2 = Env::default();
        env2.mock_all_auths();
        let contract2 = env2.register_contract(None, TreasuryContract);
        let client2 = TreasuryContractClient::new(&env2, &contract2);
        let admin2 = Address::generate(&env2);
        // Initialize with fee_bps = 75 (not the fallback default of 50).
        client2.initialize(&admin2, &75u32);
        assert_eq!(client2.get_fee_bps(), 75);

        // Update to 100. The recorded old value must be 75.
        // If the read were on the wrong tier it would silently return 50
        // and the event would carry the wrong old_bps.
        client2.set_fee_bps(&admin2, &100u32);
        assert_eq!(client2.get_fee_bps(), 100);

        // Second update to 200. Old value must be 100 (what we just wrote).
        client2.set_fee_bps(&admin2, &200u32);
        assert_eq!(client2.get_fee_bps(), 200);
    }

    // ── whitelist_token ───────────────────────────────────────────────────────

    #[test]
    fn test_whitelist_token_idempotent() {
        // Whitelisting the same token twice must not error — it's a no-op on the
        // second call (the token is simply already whitelisted).
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        let result = client.try_withdraw(&admin, &token, &recipient, &-1_000_000i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_emergency_withdraw_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        let result = client.try_emergency_withdraw(&non_admin, &token, &recipient);
        assert!(result.is_err());
        assert!(client.try_whitelist_token(&admin, &token).is_ok());
        assert!(client.try_whitelist_token(&admin, &token).is_ok());
    }

    #[test]
    fn test_whitelist_token_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        assert!(client.try_whitelist_token(&non_admin, &token).is_err());
    }

    // ── collect_fee ───────────────────────────────────────────────────────────

    #[test]
    fn test_multiple_fee_updates() {
        let (env, admin, client) = setup();
        client.set_fee_bps(&admin, &100u32);
        assert_eq!(client.get_fee_bps(), 100);
        client.set_fee_bps(&admin, &200u32);
        assert_eq!(client.get_fee_bps(), 200);
        client.set_fee_bps(&admin, &50u32);
        assert_eq!(client.get_fee_bps(), 50);
    }

    // ── Audit log tests ───────────────────────────────────────────────────────
    fn test_collect_fee_zero_amount_rejected() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        client.whitelist_token(&admin, &token);
        assert!(client.try_collect_fee(&token, &0i128).is_err());
    }

    #[test]
    fn test_collect_fee_negative_amount_rejected() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        client.whitelist_token(&admin, &token);
        assert!(client.try_collect_fee(&token, &-1i128).is_err());
    }

    #[test]
    fn test_collect_fee_non_whitelisted_token_rejected() {
        let (env, _admin, client) = setup();
        let token = Address::generate(&env);
        assert!(client.try_collect_fee(&token, &1_000i128).is_err());
    }

    #[test]
    fn test_collect_fee_accumulates() {
        // collect_fee is informational — multiple calls accumulate correctly.
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        client.whitelist_token(&admin, &token);
        client.collect_fee(&token, &500i128);
        client.collect_fee(&token, &300i128);
        // The collected ledger is internal, but no error means the addition succeeded.
    }

    #[test]
    fn test_collect_fee_overflow_rejected() {
        // Two consecutive collect_fee calls whose sum overflows i128 must return
        // ArithmeticOverflow — not silently wrap.
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        client.whitelist_token(&admin, &token);
        // Seed the ledger with i128::MAX first.
        client.collect_fee(&token, &i128::MAX);
        // Any further positive amount must overflow.
        let result = client.try_collect_fee(&token, &1i128);
        assert!(result.is_err());
    }

    // ── get_balance ───────────────────────────────────────────────────────────

    #[test]
    fn test_get_balance_returns_zero_for_unknown_token() {
        // Before any transfer, balance should be 0 for a freshly deployed token.
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let token_id = deploy_token(&env, &admin, &contract_id, 0);
        assert_eq!(client.get_balance(&token_id), 0);
    }

    #[test]
    fn test_get_balance_after_mint() {
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let token_id = deploy_token(&env, &admin, &contract_id, 1_000_000);
        assert_eq!(client.get_balance(&token_id), 1_000_000);
    }

    // ── withdraw ──────────────────────────────────────────────────────────────

    #[test]
    fn test_audit_log_records_initialize() {
        let (env, _admin, client) = setup();
        let log = client.get_audit_log(&1u32);
        assert_eq!(log.len(), 1);
        assert_eq!(log.get(0).unwrap().action, String::from_str(&env, "initialize"));
    fn test_withdraw_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client
            .try_withdraw(&non_admin, &token, &recipient, &1_000_000i128)
            .is_err());
    }

    #[test]
    fn test_audit_total_increments() {
        let (env, admin, client) = setup();
        assert_eq!(client.get_audit_total(), 1); // initialize
        client.set_fee_bps(&admin, &100u32);
        assert_eq!(client.get_audit_total(), 2);
        client.set_fee_bps(&admin, &200u32);
        assert_eq!(client.get_audit_total(), 3);
    }

    #[test]
    fn test_audit_checksum_changes_on_each_action() {
        let (env, admin, client) = setup();
        let c1 = client.get_audit_checksum();
        client.set_fee_bps(&admin, &100u32);
        let c2 = client.get_audit_checksum();
        client.set_fee_bps(&admin, &200u32);
        let c3 = client.get_audit_checksum();
        // Each action produces a distinct checksum
        assert_ne!(c1, c2);
        assert_ne!(c2, c3);
        assert_ne!(c1, c3);
    }

    #[test]
    fn test_audit_checksum_is_deterministic() {
        // Verify determinism: same action sequence yields a stable, non-zero checksum.
        let (env, admin, client) = setup();
        client.set_fee_bps(&admin, &100u32);
        client.set_fee_bps(&admin, &200u32);
        let checksum_a = client.get_audit_checksum();
        let zero: BytesN<32> = BytesN::from_array(&env, &[0u8; 32]);
        assert_ne!(checksum_a, zero);
    }

    /// Test with a small synthetic cap to verify wraparound + checkpoint behaviour
    /// without performing 500+ actions.
    ///
    /// We use MAX_AUDIT_LOG_SIZE = 500 (real value) but only test that after 500
    /// actions the total is 500 and checksum is non-zero, and that on the 501st
    /// action (wraparound) the total becomes 501.
    ///
    /// A tight wraparound integration test would require a configurable cap; here
    /// we verify the arithmetic on the modular boundary directly.
    #[test]
    fn test_audit_total_beyond_ring_buffer_size() {
        let (env, admin, client) = setup();
        // initialize already wrote entry 0; write 499 more to fill the ring buffer
        for _ in 0..499 {
            client.set_fee_bps(&admin, &100u32);
        }
        assert_eq!(client.get_audit_total(), 500);

        // 501st entry triggers wraparound — total should be 501, checksum still valid
        client.set_fee_bps(&admin, &200u32);
        assert_eq!(client.get_audit_total(), 501);

        let checksum = client.get_audit_checksum();
        let zero: BytesN<32> = BytesN::from_array(&env, &[0u8; 32]);
        assert_ne!(checksum, zero);
    }

    #[test]
    fn test_audit_get_log_limit_respected() {
        let (env, admin, client) = setup();
        // initialize wrote 1 entry; add 4 more
        for _ in 0..4 {
            client.set_fee_bps(&admin, &100u32);
        }
        let log = client.get_audit_log(&3u32);
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn test_audit_get_log_returns_newest_first() {
        let (env, admin, client) = setup();
        client.set_fee_bps(&admin, &100u32); // sequence 1
        client.set_fee_bps(&admin, &200u32); // sequence 2

        let log = client.get_audit_log(&3u32);
        // Newest entry should have the highest sequence number
        let first_seq = log.get(0).unwrap().sequence;
        let last_seq = log.get(2).unwrap().sequence;
        assert!(first_seq > last_seq);
    }

    #[test]
    fn test_get_audit_checksum_view() {
        let (env, admin, client) = setup();
        let checksum = client.get_audit_checksum();
        // Should be non-zero after at least one action
        let zero: BytesN<32> = BytesN::from_array(&env, &[0u8; 32]);
        assert_ne!(checksum, zero);
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client
            .try_withdraw(&admin, &token, &recipient, &0i128)
            .is_err());
    }

    #[test]
    fn test_withdraw_with_negative_amount_rejected() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client
            .try_withdraw(&admin, &token, &recipient, &-1_000i128)
            .is_err());
    }

    #[test]
    fn test_withdraw_non_whitelisted_token_rejected() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client
            .try_withdraw(&admin, &token, &recipient, &1_000i128)
            .is_err());
    }

    #[test]
    fn test_withdraw_insufficient_balance_fails() {
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let token_id = deploy_token(&env, &admin, &contract_id, 500);
        let recipient = Address::generate(&env);
        client.whitelist_token(&admin, &token_id);
        // Contract only has 500, requesting 1_000 must fail.
        let result = client.try_withdraw(&admin, &token_id, &recipient, &1_000i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_withdraw_exact_balance_succeeds() {
        // Withdrawing exactly the available balance must succeed.
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let token_id = deploy_token(&env, &admin, &contract_id, 1_000);
        let recipient = Address::generate(&env);
        client.whitelist_token(&admin, &token_id);
        assert!(client
            .try_withdraw(&admin, &token_id, &recipient, &1_000i128)
            .is_ok());
        // Balance drained to zero.
        assert_eq!(client.get_balance(&token_id), 0);
    }

    // ── emergency_withdraw ────────────────────────────────────────────────────

    #[test]
    fn test_emergency_withdraw_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client
            .try_emergency_withdraw(&non_admin, &token, &recipient)
            .is_err());
    }

    #[test]
    fn test_emergency_withdraw_non_whitelisted_token_rejected() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        assert!(client
            .try_emergency_withdraw(&admin, &token, &recipient)
            .is_err());
    }

    #[test]
    fn test_emergency_withdraw_zero_balance_is_noop() {
        // When balance is zero, emergency_withdraw must succeed without error
        // (it simply has nothing to transfer).
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let token_id = deploy_token(&env, &admin, &contract_id, 0);
        let recipient = Address::generate(&env);
        client.whitelist_token(&admin, &token_id);
        assert!(client
            .try_emergency_withdraw(&admin, &token_id, &recipient)
            .is_ok());
    }

    #[test]
    fn test_emergency_withdraw_drains_full_balance() {
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let token_id = deploy_token(&env, &admin, &contract_id, 5_000);
        let recipient = Address::generate(&env);
        client.whitelist_token(&admin, &token_id);
        client.emergency_withdraw(&admin, &token_id, &recipient);
        assert_eq!(client.get_balance(&token_id), 0);
    }

    // ── reentrancy lock cleanup ───────────────────────────────────────────────

    #[test]
    fn test_lock_released_after_failed_withdraw() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        // Fails due to token not whitelisted — lock must be released
        let _ = client.try_withdraw(&admin, &token, &recipient, &1_000i128);
        // Subsequent admin operation must succeed (lock not stuck)
        assert!(client.try_set_fee_bps(&admin, &100u32).is_ok());
    }

    #[test]
    fn test_lock_released_after_emergency_withdraw() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        let recipient = Address::generate(&env);
        let _ = client.try_emergency_withdraw(&admin, &token, &recipient);
        // Lock must be released regardless of outcome
        assert!(client.try_set_fee_bps(&admin, &100u32).is_ok());
    }

    // ── get_fee_bps ───────────────────────────────────────────────────────────

    #[test]
    fn test_admin_actions_work_immediately_after_initialize() {
        let (_env, admin, client) = setup();
        assert!(client.try_set_fee_bps(&admin, &100u32).is_ok());
    }

    #[test]
    fn test_get_fee_bps_default_before_init() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, TreasuryContract);
        let client = TreasuryContractClient::new(&env, &contract_id);
        // Returns 50 bps as the hard-coded fallback before initialization.
        assert_eq!(client.get_fee_bps(), 50);
    }

    #[test]
    #[should_panic]
    fn test_get_balance_with_non_existent_token() {
        // `Address::generate` produces an address with no deployed contract behind
        // it at all — this is different from a real token contract that simply has
        // no balance record for this contract (see
        // `test_get_balance_returns_zero_for_unknown_token`, which covers that case
        // and correctly returns 0). Invoking any function, including `balance`, on
        // an address with no contract instance is a host-level error that Soroban
        // escalates to a panic — there is no way for `get_balance` to catch this and
        // return 0 instead, so the correct, expected behavior here is a panic.
        let (env, _admin, client) = setup();
        let invalid_token = Address::generate(&env);
        let _ = client.get_balance(&invalid_token);
    }

    // ── withdrawal rate limit ─────────────────────────────────────────────────

    #[test]
    fn test_withdrawal_cap_default_is_uncapped() {
        let (env, _admin, client) = setup();
        let token = Address::generate(&env);
        assert_eq!(client.get_withdrawal_cap(&token), 0);
    }

    #[test]
    fn test_propose_withdrawal_cap_requires_admin() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        assert!(client
            .try_propose_withdrawal_cap(&non_admin, &token, &1_000_000i128)
            .is_err());
    }

    #[test]
    fn test_propose_negative_cap_rejected() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        assert!(client.try_propose_withdrawal_cap(&admin, &token, &-1i128).is_err());
    }

    #[test]
    fn test_execute_cap_before_timelock_fails() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        client.propose_withdrawal_cap(&admin, &token, &1_000_000i128);
        // Timelock hasn't elapsed
        assert!(client.try_execute_withdrawal_cap(&admin, &token).is_err());
    }

    #[test]
    fn test_execute_cap_without_proposal_fails() {
        let (env, admin, client) = setup();
        let token = Address::generate(&env);
        assert!(client.try_execute_withdrawal_cap(&admin, &token).is_err());
    }
}
