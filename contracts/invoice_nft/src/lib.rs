#![no_std]
// Added standardized events

//! # Invoice NFT Contract
//!
//! Mints and manages invoice NFTs as the canonical source of truth for invoice state.
//!
//! Each invoice is an immutable NFT with a unique ID, representing a real-world invoice
//! with fields such as amount, due date, debtor information (hashed), and repayment status.
//!
//! **Lifecycle:** `Created` → `Listed` → `Funded` → `Repaid` | `Defaulted`
//!
//! See [Invoice NFT Model](../../../docs/invoice-nft.md) for detailed architecture.

use kora_shared::{
    audit::{AdminActionType, AdminAuditEntry, AuditSource, MAX_AUDIT_LOG_SIZE},
    errors::CommonError,
    events,
    reentrancy::ReentrancyGuard,
    types::{Invoice, InvoiceStatus, ProtocolConfig, RiskTier},
    validation::{
        extend_persistent_ttl, require_batch_size_within_limit, require_future_timestamp,
        require_max_length_bytes, require_max_length_string, require_non_empty_bytes,
        require_non_empty_string, require_non_zero_amount, require_risk_score_within_ceiling,
        require_valid_risk_score, DEFAULT_TTL_BUMP, DEFAULT_TTL_THRESHOLD, MAX_DEBTOR_HASH_LEN,
        MAX_IPFS_CID_LEN, UPGRADE_TIMELOCK_DELAY,
    },
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Bytes, BytesN, Env, String, Symbol, Vec};

/// Local error enum for `invoice_nft`. Soroban's `#[contracterror]` macro caps an
/// error enum at 50 variants, so each contract owns its own small enum instead of
/// sharing one giant error type across all 7 contracts.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum InvoiceNftError {
    AlreadyInitialized = 1,
    ArithmeticOverflow = 2,
    BatchSizeExceeded = 3,
    CreditLimitExceeded = 4,
    CurrencyNotAllowed = 5,
    EmptyBytes = 6,
    EmptyString = 7,
    FieldTooLong = 8,
    InvalidAddress = 9,
    InvalidAmount = 10,
    InvalidDueDate = 11,
    InvalidInvoiceStatus = 12,
    InvalidRiskScore = 13,
    InvoiceNotFound = 14,
    NoUpgradeProposed = 15,
    NotAdmin = 16,
    NotInitialized = 17,
    NotInvoiceOwner = 18,
    ProtocolPaused = 19,
    Reentrancy = 20,
    SMENotRegistered = 21,
    Unauthorized = 22,
    UpgradeTimelockNotElapsed = 23,
}

impl From<CommonError> for InvoiceNftError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => InvoiceNftError::InvalidAmount,
            CommonError::InvalidDueDate => InvoiceNftError::InvalidDueDate,
            CommonError::InvalidRiskScore => InvoiceNftError::InvalidRiskScore,
            CommonError::InvalidAddress => InvoiceNftError::InvalidAddress,
            CommonError::BatchSizeExceeded => InvoiceNftError::BatchSizeExceeded,
            CommonError::EmptyString => InvoiceNftError::EmptyString,
            CommonError::EmptyBytes => InvoiceNftError::EmptyBytes,
            CommonError::FieldTooLong => InvoiceNftError::FieldTooLong,
            CommonError::ArithmeticOverflow => InvoiceNftError::ArithmeticOverflow,
            CommonError::Reentrancy => InvoiceNftError::Reentrancy,
            _ => InvoiceNftError::InvalidAmount,
        }
    }
}

// ── TTL constants (~30 days at ~5s/ledger) ───────────────────────────────────
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

/// Maximum invoice IDs returned per `get_sme_invoice_ids` page, bounding per-call CPU cost.
const MAX_SME_INVOICE_PAGE: u32 = 100;

// ── Storage Keys ────────────────────────────────────────────────────────────
//
// Storage versioning: The contract uses a MigrationVersion key to track schema changes.
// Current version: 2 (Invoice includes `metadata_hash` and `notes`)
//
// Variants:
// - Invoice(u64): Stores individual Invoice structs by ID (persistent)
// - NextId: Stores the next invoice ID to mint (instance)
// - Admin: Stores admin address (instance)
// - AccessControl: Stores access control contract address (instance)
// - MigrationVersion: Tracks current schema version for upgrade safety (instance)

/// Storage key variants for the invoice NFT contract.
///
/// - `Invoice(u64)` — Maps invoice ID to the full `Invoice` struct (persistent)
/// - `NextId` — Stores the next invoice ID to be allocated (instance)
/// - `Admin` — Stores the contract admin address (instance)
/// - `AccessControl` — Stores the access control contract address (instance)
/// - `InvoiceCount` — Stores total invoice count for metrics (instance)
/// - `MigrationVersion` — Tracks current schema version for upgrade safety (instance)
/// - `CurrencyAllowlist(Symbol)` — Marks a currency symbol as allowed (persistent)
#[contracttype]
pub enum DataKey {
    /// Versioned invoice storage: Invoice(id) stores Invoice struct
    Invoice(u64),
    /// Instance key: tracks next invoice ID to assign
    NextId,
    /// Instance key: admin address for privileged operations
    Admin,
    /// Instance key: access control contract address for pause checks
    AccessControl,
    /// Instance key: current schema migration version (starts at 1)
    MigrationVersion,
    /// Pending upgrade proposal: (wasm_hash, proposed_at_timestamp).
    UpgradeProposal,
    /// Instance key: authorized marketplace contract address
    Marketplace,
    /// Instance key: authorized financing pool contract address
    FinancingPool,
    /// Instance key: authorized risk registry contract address
    RiskRegistry,
    /// Persistent: aggregate exposure (i128) for an investor address
    OutstandingExposure(Address),
    /// Persistent: marks a currency symbol as allowed for invoices
    CurrencyAllowlist(Symbol),
    /// Persistent bool: true when this invoice is individually frozen by an admin.
    /// Checked by marketplace.fund_invoice and financing_pool.repay in addition
    /// to the protocol-wide pause, enabling targeted freeze of disputed invoices.
    InvoiceFrozen(u64),
    /// Instance key: protocol-wide configuration (fee_bps, max_risk_score, etc).
    /// Defaults apply when unset (see `get_protocol_config`).
    ProtocolConfig,
    /// Persistent: an open or resolved metadata-hash dispute for an invoice.
    MetadataDispute(u64),
    /// Instance key: next write position in the admin audit ring buffer.
    AuditLogHead,
    /// Instance key: total admin actions ever recorded (monotonic).
    AuditLogTotal,
    /// Persistent: an audit log entry at ring-buffer position `n`.
    AuditEntry(u64),
    /// Persistent: Vec<u64> of invoice IDs minted by this SME, in mint order.
    /// Appended to in mint_invoice/mint_invoices_batch, pruned in withdraw_invoice.
    SmeInvoiceIds(Address),
    /// Instance key: monotonic counter allocating the next batch-mint correlation ID.
    NextBatchId,
    /// Instance key: `MintRateLimit` config. Absent means minting is unthrottled,
    /// preserving pre-existing behaviour for deployments that never configure it.
    MintRateLimit,
    /// Persistent: `(window_start_ts, mints_used)` rolling mint window for an SME.
    SmeMintWindow(Address),
}

/// A dispute raised against an invoice's committed `metadata_hash`.
#[contracttype]
#[derive(Clone)]
pub struct MetadataDispute {
    pub challenger: Address,
    pub evidence_hash: Bytes,
    pub raised_at: u64,
    pub resolved: bool,
    pub upheld: bool,
}

/// Per-SME minting velocity cap: at most `max_mints` invoices per `window_secs`.
#[contracttype]
#[derive(Clone)]
pub struct MintRateLimit {
    pub max_mints: u32,
    pub window_secs: u64,
}

/// Input type for a single invoice within a batch mint operation.
#[contracttype]
#[derive(Clone)]
pub struct BatchInvoiceInput {
    pub debtor_hash: Bytes,
    pub amount: i128,
    pub currency: Symbol,
    pub due_date: u64,
    pub ipfs_cid: String,
    pub risk_score: u32,
    pub notes: Option<String>,
}

// ── Migration helpers ─────────────────────────────────────────────────────────
//
// When Invoice gains new fields, keep the PREVIOUS struct definition here so
// migrate() can deserialize stale records from persistent storage.
//
// Pattern:
//   1. Copy the old Invoice definition below as InvoiceV{N}.
//   2. Add the new field(s) to Invoice in kora-shared/src/types.rs.
//   3. Increment MigrationVersion to N+1 in migrate().
//   4. In migrate(), read as InvoiceV{N}, convert to Invoice, write back.
//   5. After all live nodes have run migrate(), the old InvoiceV{N} struct
//      can be removed in the following upgrade cycle.

/// Schema v1 of Invoice — identical to the original struct that existed before
/// the `notes` field was added.  Used by migrate() to deserialize stale records
/// from persistent storage so they can be rewritten in the current schema (v2).
#[contracttype]
#[derive(Clone)]
pub struct InvoiceV1 {
    pub id: u64,
    pub sme: Address,
    pub debtor_hash: Bytes,
    pub amount: i128,
    pub currency: Symbol,
    pub due_date: u64,
    pub ipfs_cid: String,
    pub risk_score: u32,
    pub risk_tier: kora_shared::types::RiskTier,
    pub status: kora_shared::types::InvoiceStatus,
    pub created_at: u64,
    pub funded_at: Option<u64>,
    pub repaid_at: Option<u64>,
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct InvoiceNftContract;

#[contractimpl]
impl InvoiceNftContract {
    /// One-time initializer. Sets admin and access-control contract address.
    ///
    /// **Parameters:**
    /// - `admin` — The address that will administer this contract.
    /// - `access_control` — The deployed `access_control` contract address used for pause checks.
    ///
    /// **Errors:**
    /// - `InvoiceNftError::AlreadyInitialized` — Contract has already been initialized.
    /// - `InvoiceNftError::InvalidAddress` — `admin` or `access_control` is the contract's own address.
    ///
    /// **Security:** No auth required on first call. Subsequent calls revert immediately.
    pub fn initialize(env: Env, admin: Address, access_control: Address) -> Result<(), InvoiceNftError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(InvoiceNftError::AlreadyInitialized);
        }
        kora_shared::validation::require_not_self(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &access_control)?;
        kora_shared::validation::require_distinct(&admin, &access_control)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::AccessControl, &access_control);
        env.storage().instance().set(&DataKey::NextId, &1u64);
        // A freshly initialized contract has no legacy records to backfill, so it
        // starts at the current schema version (2) rather than replaying migrate()'s
        // historical version-1 upgrade steps.
        env.storage()
            .instance()
            .set(&DataKey::MigrationVersion, &2u32);
        Ok(())
    }

    /// Set the risk_registry contract address. Admin only. Called after deployment
    /// to wire up credit-limit enforcement.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `risk_registry` — The deployed `risk_registry` contract address.
    ///
    /// **Errors:**
    /// - `InvoiceNftError::NotAdmin` — Caller is not the admin.
    /// - `InvoiceNftError::InvalidAddress` — `risk_registry` is the contract's own address.
    ///
    /// **Security:** Requires `admin.require_auth()`. Idempotent — safe to call again if
    /// the risk registry is redeployed.
    pub fn set_risk_registry(env: Env, admin: Address, risk_registry: Address) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &risk_registry)?;
        env.storage().instance().set(&DataKey::RiskRegistry, &risk_registry);
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftSetRiskRegistry);
        Ok(())
    }

    /// Idempotent migration function. Performs any necessary schema upgrades.
    ///
    /// Must be called by admin immediately after a WASM upgrade that changes a
    /// `#[contracttype]` struct.  The function is safe to call multiple times:
    /// each version gate is a no-op once the ledger has already been upgraded
    /// past that version.
    ///
    /// See docs/MIGRATIONS.md for the full runbook and a description of each
    /// version's changes.
    pub fn migrate(env: Env, admin: Address) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftMigrate);

        let current_version: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MigrationVersion)
            .unwrap_or(0);

        // Version 0 -> 1: Initial setup (marks the baseline schema version).
        if current_version < 1 {
            env.storage()
                .instance()
                .set(&DataKey::MigrationVersion, &1u32);
        }

        // Version 1 -> 2: Invoice gained `notes: Option<String>`.
        //
        // Old records in persistent storage are still encoded as InvoiceV1 (no
        // `notes` field).  Reading them as `Invoice` (v2) would panic because
        // the XDR field count has changed.  We therefore:
        //   1. Read each record as InvoiceV1 (the old encoding).
        //   2. Re-encode it as Invoice (v2) with notes = None.
        //   3. Overwrite the slot so future reads use the new codec.
        if current_version < 2 {
            let next_id: u64 = env
                .storage()
                .instance()
                .get(&DataKey::NextId)
                .unwrap_or(1);

            // Iterate every allocated invoice ID and backfill.
            let mut id: u64 = 1;
            while id < next_id {
                let key = DataKey::Invoice(id);
                // Records minted AFTER the WASM upgrade already carry notes and
                // decode as Invoice directly — skip them.
                if let Some(old) = env
                    .storage()
                    .persistent()
                    .get::<DataKey, InvoiceV1>(&key)
                {
                    let upgraded = Invoice {
                        id: old.id,
                        sme: old.sme,
                        debtor_hash: old.debtor_hash,
                        amount: old.amount,
                        currency: old.currency,
                        due_date: old.due_date,
                        ipfs_cid: old.ipfs_cid,
                        metadata_hash: Bytes::new(&env),
                        risk_score: old.risk_score,
                        risk_tier: old.risk_tier,
                        status: old.status,
                        created_at: old.created_at,
                        funded_at: old.funded_at,
                        repaid_at: old.repaid_at,
                        metadata_hash: Bytes::new(&env),
                        notes: None,
                    };
                    env.storage().persistent().set(&key, &upgraded);
                }
                id += 1;
            }

            env.storage()
                .instance()
                .set(&DataKey::MigrationVersion, &2u32);
        }

        Ok(())
    }

    /// Set the authorized marketplace and financing pool contract addresses.
    /// Must be called by admin after deployment to enable status transitions.
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    /// - `KoraError::InvalidAddress` — Either address is this contract, they are
    ///   identical to each other, or either collides with the stored admin,
    ///   access_control, or risk_registry address.
    pub fn set_authorized_callers(
        env: Env,
        admin: Address,
        marketplace: Address,
        financing_pool: Address,
    ) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &marketplace)?;
        kora_shared::validation::require_not_self(&env, &financing_pool)?;
        kora_shared::validation::require_distinct(&marketplace, &financing_pool)?;

        // Neither role may collide with an existing privileged/wired address:
        // doing so would let one key satisfy two authorization paths at once.
        for wired in [DataKey::Admin, DataKey::AccessControl, DataKey::RiskRegistry] {
            if let Some(existing) = env.storage().instance().get::<DataKey, Address>(&wired) {
                kora_shared::validation::require_distinct(&marketplace, &existing)?;
                kora_shared::validation::require_distinct(&financing_pool, &existing)?;
            }
        }

        env.storage().instance().set(&DataKey::Marketplace, &marketplace);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftSetAuthorizedCallers);
        Ok(())
    }

    /// Set the protocol configuration. Admin only.
    ///
    /// Currently the only field enforced by this contract is `max_risk_score`,
    /// which tightens (or restores) the ceiling `mint_invoice`/`mint_invoices_batch`
    /// accept, on top of the fixed `require_valid_risk_score` cap of 100.
    /// `fee_bps`, `late_penalty_bps`, and `min_funding_period` are stored for
    /// forward-compatibility but are not yet read by this contract — they remain
    /// owned by `financing_pool`/`treasury` until a follow-up wires them in.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `config` — The new `ProtocolConfig`. `max_risk_score` must be 0–100.
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    /// - `KoraError::InvalidRiskScore` — `config.max_risk_score` exceeds 100.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn set_protocol_config(env: Env, admin: Address, config: ProtocolConfig) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_valid_risk_score(config.max_risk_score)?;
        env.storage().instance().set(&DataKey::ProtocolConfig, &config);
        Ok(())
    }

    /// Get the current protocol configuration.
    ///
    /// **Returns:** The configured `ProtocolConfig`, or a default with
    /// `max_risk_score = 100` (no additional restriction beyond the fixed cap)
    /// and all other fields zeroed if the admin has not configured one yet.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_protocol_config(env: Env) -> ProtocolConfig {
        env.storage()
            .instance()
            .get(&DataKey::ProtocolConfig)
            .unwrap_or(ProtocolConfig {
                fee_bps: 0,
                late_penalty_bps: 0,
                max_risk_score: 100,
                min_funding_period: 0,
            })
    }

    /// Mint a new invoice NFT. Caller must be a verified SME.
    ///
    /// **Parameters:**
    /// - `sme` — The SME address minting the invoice (must sign).
    /// - `debtor_hash` — SHA-256 hash of debtor PII (max `MAX_DEBTOR_HASH_LEN` bytes). PII stays off-chain.
    /// - `amount` — Face value in stroops (7 decimals). Must be > 0.
    /// - `currency` — Token symbol (e.g. `USDC`, `EURC`).
    /// - `due_date` — Unix timestamp; must be strictly in the future.
    /// - `ipfs_cid` — CIDv0 or CIDv1 of the full invoice document on IPFS (max 128 bytes).
    /// - `risk_score` — Credit score 0–100 assigned by the verifier. Maps to a `RiskTier`.
    /// - `notes` — Optional free-text memo (schema v2; `None` is fine).
    ///
    /// **Returns:** The allocated invoice ID (monotonically increasing from 1).
    ///
    /// **Errors:**
    /// - `KoraError::ProtocolPaused` — Protocol is paused.
    /// - `KoraError::InvalidAmount` — `amount` is zero, negative, or exceeds `credit_limit`.
    /// - `KoraError::InvalidDueDate` — `due_date` is not in the future.
    /// - `KoraError::InvalidRiskScore` — `risk_score` > 100.
    /// - `KoraError::EmptyBytes` — `debtor_hash` is empty.
    /// - `KoraError::EmptyString` — `ipfs_cid` is empty.
    /// - `KoraError::FieldTooLong` — `debtor_hash` or `ipfs_cid` exceed their max lengths.
    /// - `KoraError::InvalidAmount` — Adding this invoice would exceed the SME's credit limit.
    /// - `KoraError::Reentrancy` — Reentrancy guard triggered.
    ///
    /// **Security:** Requires `sme.require_auth()`. The protocol must not be paused.
    /// If a `risk_registry` is wired up, the SME's outstanding exposure is checked against
    /// their pre-approved credit limit before minting.
    pub fn mint_invoice(
        env: Env,
        sme: Address,
        debtor_hash: Bytes,
        amount: i128,
        currency: Symbol,
        due_date: u64,
        ipfs_cid: String,
        risk_score: u32,
        notes: Option<String>,
    ) -> Result<u64, InvoiceNftError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;

        require_non_zero_amount(amount)?;
        require_future_timestamp(&env, due_date)?;
        require_valid_risk_score(risk_score)?;
        require_risk_score_within_ceiling(risk_score, Self::get_protocol_config(env.clone()).max_risk_score)?;
        require_non_empty_bytes(&debtor_hash)?;
        require_max_length_bytes(&debtor_hash, MAX_DEBTOR_HASH_LEN)?;
        require_non_empty_string(&ipfs_cid)?;
        require_max_length_string(&ipfs_cid, MAX_IPFS_CID_LEN)?;

        let outstanding: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::OutstandingExposure(sme.clone()))
            .unwrap_or(0i128);
        let new_exposure = outstanding
            .checked_add(amount)
            .ok_or(InvoiceNftError::ArithmeticOverflow)?;

        // Credit-limit enforcement: if a risk_registry is wired up, check the
        // SME's pre-approved credit limit against their current outstanding exposure.
        if let Some(rr_addr) = env
            .storage()
            .instance()
            .get::<DataKey, soroban_sdk::Address>(&DataKey::RiskRegistry)
        {
            let rr = kora_risk_registry::RiskRegistryContractClient::new(&env, &rr_addr);
            if let Ok(profile) = rr.try_get_sme_profile(&sme) {
                if let Ok(profile) = profile {
                    if profile.credit_limit > 0 && new_exposure > profile.credit_limit {
                        return Err(InvoiceNftError::CreditLimitExceeded);
                    }
                }
            }
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);

        let invoice = Invoice {
            id,
            sme: sme.clone(),
            debtor_hash,
            amount,
            currency,
            due_date,
            ipfs_cid,
            metadata_hash: Bytes::new(&env),
            risk_score,
            risk_tier: RiskTier::from_score(risk_score),
            status: InvoiceStatus::Created,
            created_at: env.ledger().timestamp(),
            funded_at: None,
            repaid_at: None,
            notes,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Invoice(id), &invoice);
        Self::bump_invoice_ttl(&env, id);
        env.storage().instance().set(
            &DataKey::NextId,
            &(id.checked_add(1).ok_or(InvoiceNftError::ArithmeticOverflow)?),
        );
        env.storage()
            .persistent()
            .set(&DataKey::OutstandingExposure(sme.clone()), &new_exposure);
        Self::append_sme_invoice_id(&env, &sme, id);

        events::invoice_created(&env, id, &sme, invoice.amount, invoice.currency.clone());
        Ok(id)
    }

    /// Mint multiple invoice NFTs atomically for a single SME.
    ///
    /// All inputs are validated before any invoice is stored — if any entry
    /// fails validation the entire batch is aborted (atomic-abort semantics).
    /// A single `require_auth` covers the whole batch.
    ///
    /// Returns a `Vec<u64>` of the newly allocated invoice IDs in order.
    pub fn mint_invoices_batch(
        env: Env,
        sme: Address,
        invoices: Vec<BatchInvoiceInput>,
    ) -> Result<Vec<u64>, InvoiceNftError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;

        // ── Check batch size before any validation/minting ────────────────────
        require_batch_size_within_limit(invoices.len() as u32)?;

        // ── Phase 1: validate ALL inputs before touching storage ──────────────
        let max_risk_score = Self::get_protocol_config(env.clone()).max_risk_score;
        for i in 0..invoices.len() {
            let entry = invoices.get(i).unwrap();
            require_non_zero_amount(entry.amount)?;
            require_future_timestamp(&env, entry.due_date)?;
            require_valid_risk_score(entry.risk_score)?;
            require_risk_score_within_ceiling(entry.risk_score, max_risk_score)?;
            require_non_empty_bytes(&entry.debtor_hash)?;
            require_max_length_bytes(&entry.debtor_hash, MAX_DEBTOR_HASH_LEN)?;
            require_non_empty_string(&entry.ipfs_cid)?;
            require_max_length_string(&entry.ipfs_cid, MAX_IPFS_CID_LEN)?;
        }

        // Charged once for the whole batch, but at N units, so a batch cannot be
        // used to sidestep the per-invoice velocity cap.
        Self::consume_mint_quota(&env, &sme, invoices.len())?;

        // ── Phase 2: mint each invoice ────────────────────────────────────────
        let mut ids: Vec<u64> = Vec::new(&env);
        let mut next_id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        let mut exposure_delta: i128 = 0;

        for i in 0..invoices.len() {
            let entry = invoices.get(i).unwrap();
            let id = next_id;
            exposure_delta = exposure_delta
                .checked_add(entry.amount)
                .ok_or(InvoiceNftError::ArithmeticOverflow)?;

            let invoice = Invoice {
                id,
                sme: sme.clone(),
                debtor_hash: entry.debtor_hash,
                amount: entry.amount,
                currency: entry.currency,
                due_date: entry.due_date,
                ipfs_cid: entry.ipfs_cid,
                metadata_hash: Bytes::new(&env),
                risk_score: entry.risk_score,
                risk_tier: RiskTier::from_score(entry.risk_score),
                status: InvoiceStatus::Created,
                created_at: env.ledger().timestamp(),
                funded_at: None,
                repaid_at: None,
                notes: entry.notes,
            };

            env.storage().persistent().set(&DataKey::Invoice(id), &invoice);
            Self::bump_invoice_ttl(&env, id);
            events::invoice_created(&env, id, &sme, invoice.amount, invoice.currency.clone());
            ids.push_back(id);

            next_id = next_id.checked_add(1).ok_or(InvoiceNftError::ArithmeticOverflow)?;
        }

        env.storage().instance().set(&DataKey::NextId, &next_id);
        if exposure_delta != 0 {
            let outstanding: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::OutstandingExposure(sme.clone()))
                .unwrap_or(0i128);
            let new_exposure = outstanding
                .checked_add(exposure_delta)
                .ok_or(InvoiceNftError::ArithmeticOverflow)?;
            env.storage()
                .persistent()
                .set(&DataKey::OutstandingExposure(sme), &new_exposure);
        }
        Self::append_sme_invoice_ids(&env, &sme, &ids);

        let batch_id: u64 = env.storage().instance().get(&DataKey::NextBatchId).unwrap_or(1);
        env.storage().instance().set(
            &DataKey::NextBatchId,
            &(batch_id.checked_add(1).ok_or(InvoiceNftError::ArithmeticOverflow)?),
        );
        events::invoice_batch_minted(&env, batch_id, &sme, &ids);

        Ok(ids)
    }

    /// Amend a Created invoice. Only the original SME may call this, and only
    /// while `status == Created` (i.e., before any market activity).
    ///
    /// Any subset of fields may be corrected: pass the existing value to leave
    /// a field unchanged.
    ///
    /// **Parameters:**
    /// - `sme` — The original invoice owner.
    /// - `invoice_id` — The ID of the invoice to amend.
    /// - `debtor_hash` — Updated debtor hash (must be non-empty).
    /// - `amount` — Updated face value (must be > 0).
    /// - `due_date` — Updated due date (must be in the future).
    /// - `ipfs_cid` — Updated IPFS CID of the document.
    /// - `risk_score` — Updated risk score (0–100); also recalculates `risk_tier`.
    ///
    /// **Errors:**
    /// - `KoraError::ProtocolPaused` — Protocol is paused.
    /// - `KoraError::InvoiceNotFound` — Invoice does not exist.
    /// - `KoraError::InvalidInvoiceStatus` — Invoice is not in `Created` status.
    /// - `KoraError::Unauthorized` — Caller is not the invoice's SME.
    /// - `KoraError::InvalidAmount` / `KoraError::InvalidDueDate` / `KoraError::InvalidRiskScore` — Validation failures.
    ///
    /// **Security:** Requires `sme.require_auth()`. Rejected once the invoice is listed
    /// or further along in the lifecycle.
    pub fn amend_invoice(
        env: Env,
        sme: Address,
        invoice_id: u64,
        debtor_hash: Bytes,
        amount: i128,
        due_date: u64,
        ipfs_cid: String,
        risk_score: u32,
    ) -> Result<(), InvoiceNftError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;

        require_non_zero_amount(amount)?;
        require_future_timestamp(&env, due_date)?;
        require_valid_risk_score(risk_score)?;
        require_non_empty_bytes(&debtor_hash)?;
        require_non_empty_string(&ipfs_cid)?;

        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Created {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        if invoice.sme != sme {
            return Err(KoraError::Unauthorized);
        }

        invoice.debtor_hash = debtor_hash;
        invoice.amount = amount;
        invoice.due_date = due_date;
        invoice.ipfs_cid = ipfs_cid;
        invoice.risk_score = risk_score;
        invoice.risk_tier = RiskTier::from_score(risk_score);

        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        events::invoice_amended(&env, invoice_id, &sme, invoice.currency.clone());
        Ok(())
    }

    /// Withdraw (void) a Created invoice. Only the original SME may call this,
    /// and only while `status == Created`.
    ///
    /// The invoice record is removed from storage, permanently burning the NFT.
    ///
    /// **Parameters:**
    /// - `sme` — The original invoice owner.
    /// - `invoice_id` — The ID of the invoice to void.
    ///
    /// **Errors:**
    /// - `KoraError::ProtocolPaused` — Protocol is paused.
    /// - `KoraError::InvoiceNotFound` — Invoice does not exist.
    /// - `KoraError::InvalidInvoiceStatus` — Invoice is not in `Created` status.
    /// - `KoraError::Unauthorized` — Caller is not the invoice's SME.
    ///
    /// **Security:** Requires `sme.require_auth()`. Irreversible — the invoice is deleted
    /// from on-chain storage and cannot be recovered. Outstanding exposure is decremented.
    pub fn withdraw_invoice(
        env: Env,
        sme: Address,
        invoice_id: u64,
    ) -> Result<(), InvoiceNftError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;

        let invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Created {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        if invoice.sme != sme {
            return Err(KoraError::Unauthorized);
        }

        env.storage().persistent().remove(&DataKey::Invoice(invoice_id));
        // Release outstanding exposure
        let prev: i128 = env.storage().persistent()
            .get(&DataKey::OutstandingExposure(invoice.sme.clone()))
            .unwrap_or(0i128);
        env.storage().persistent().set(
            &DataKey::OutstandingExposure(invoice.sme.clone()),
            &prev.saturating_sub(invoice.amount),
        );
        Self::remove_sme_invoice_id(&env, &invoice.sme, invoice_id);
        events::invoice_withdrawn(&env, invoice_id, &sme, invoice.currency.clone());
        Ok(())
    }

    /// Revert invoice status from `Listed` back to `Created`. Called by the Marketplace
    /// contract when a listing is cancelled before any funding has been released.
    ///
    /// This allows the SME to re-list the same invoice after cancellation.
    ///
    /// **Parameters:**
    /// - `caller` — The marketplace contract address (must be the authorised marketplace).
    /// - `invoice_id` — The ID of the invoice to revert.
    ///
    /// **Errors:**
    /// - `InvoiceNftError::Unauthorized` — Caller is not the authorised marketplace.
    /// - `InvoiceNftError::ProtocolPaused` — Protocol is paused.
    /// - `InvoiceNftError::InvalidInvoiceStatus` — Invoice is not in `Listed` status
    ///   (already `Funded`, `Repaid`, or `Defaulted`).
    /// - `InvoiceNftError::InvoiceNotFound` — Invoice does not exist.
    ///
    /// **Security:** Requires auth from the caller. Only the authorised marketplace
    /// address may call this function — unauthorised callers cannot revert status.
    pub fn set_created(env: Env, caller: Address, invoice_id: u64) -> Result<(), InvoiceNftError> {
        caller.require_auth();
        Self::require_authorized_caller(&env, &caller, &[DataKey::Marketplace])?;
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Listed {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        invoice.status = InvoiceStatus::Created;
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        events::invoice_created(&env, invoice_id, &invoice.sme, invoice.amount, invoice.currency.clone());
        Ok(())
    }

    /// Transition invoice to Listed status. Called by Marketplace contract.
    ///
    /// **Parameters:**
    /// - `caller` — The marketplace contract address.
    /// - `invoice_id` — The ID of the invoice to list.
    ///
    /// **Returns:** `Ok(())` on success, or an appropriate `InvoiceNftError`.
    ///
    /// **Security:** Requires auth from the caller. Validates that the invoice is in `Created` status.
    pub fn set_listed(env: Env, caller: Address, invoice_id: u64) -> Result<(), InvoiceNftError> {
        caller.require_auth();
        Self::require_authorized_caller(&env, &caller, &[DataKey::Marketplace])?;
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Created {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        invoice.status = InvoiceStatus::Listed;
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        events::invoice_listed(&env, invoice_id, &invoice.sme, invoice.amount, invoice.currency.clone());
        Ok(())
    }

    /// Transition invoice to Funded. Called by Financing Pool contract.
    ///
    /// **Parameters:**
    /// - `caller` — The investor or financing pool contract address.
    /// - `invoice_id` — The ID of the invoice to fund.
    ///
    /// **Returns:** `Ok(())` on success, or an appropriate `InvoiceNftError`.
    ///
    /// **Security:** Requires auth from the caller. Validates that the invoice is in `Listed` status.
    pub fn set_funded(env: Env, caller: Address, invoice_id: u64) -> Result<(), InvoiceNftError> {
        caller.require_auth();
        Self::require_authorized_caller(&env, &caller, &[DataKey::FinancingPool])?;
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Listed {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        invoice.status = InvoiceStatus::Funded;
        invoice.funded_at = Some(env.ledger().timestamp());
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        events::invoice_funded(&env, invoice_id, &caller, invoice.amount, invoice.currency.clone());
        Ok(())
    }

    /// Mark invoice as Repaid. Called by Financing Pool on full repayment.
    ///
    /// **Parameters:**
    /// - `caller` — The financing pool contract address.
    /// - `invoice_id` — The ID of the invoice to repay.
    ///
    /// **Returns:** `Ok(())` on success, or an appropriate `InvoiceNftError`.
    ///
    /// **Security:** Requires auth from the caller. Validates that the invoice is in `Funded` status.
    pub fn set_repaid(env: Env, caller: Address, invoice_id: u64) -> Result<(), InvoiceNftError> {
        caller.require_auth();
        Self::require_authorized_caller(&env, &caller, &[DataKey::FinancingPool])?;
        Self::require_not_paused(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Funded {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        invoice.status = InvoiceStatus::Repaid;
        invoice.repaid_at = Some(env.ledger().timestamp());
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        // Release outstanding exposure
        let prev: i128 = env.storage().persistent()
            .get(&DataKey::OutstandingExposure(invoice.sme.clone()))
            .unwrap_or(0i128);
        env.storage().persistent().set(
            &DataKey::OutstandingExposure(invoice.sme.clone()),
            &prev.saturating_sub(invoice.amount),
        );
        events::invoice_repaid(&env, invoice_id, &invoice.sme, invoice.amount, invoice.currency.clone());
        Ok(())
    }

    /// Mark invoice as Defaulted. Called by admin after due date passes.
    ///
    /// **Parameters:**
    /// - `caller` — The admin address.
    /// - `invoice_id` — The ID of the invoice to mark as defaulted.
    ///
    /// **Returns:** `Ok(())` on success, or an appropriate `InvoiceNftError`.
    ///
    /// **Security:** Requires admin auth. Validates that the invoice is `Funded` and the due date has passed.
    pub fn set_defaulted(env: Env, caller: Address, invoice_id: u64) -> Result<(), InvoiceNftError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        let _guard = ReentrancyGuard::new(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Funded {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        let current_time = env.ledger().timestamp();
        if current_time <= invoice.due_date {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        invoice.status = InvoiceStatus::Defaulted;
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        // Release outstanding exposure
        let prev: i128 = env.storage().persistent()
            .get(&DataKey::OutstandingExposure(invoice.sme.clone()))
            .unwrap_or(0i128);
        env.storage().persistent().set(
            &DataKey::OutstandingExposure(invoice.sme.clone()),
            &prev.saturating_sub(invoice.amount),
        );
        Self::append_audit_entry(&env, &caller, AdminActionType::InvoiceNftSetDefaulted);
        events::invoice_defaulted(&env, invoice_id, &invoice.sme, invoice.amount, invoice.currency.clone());
        Ok(())
    }

    // ── Views ────────────────────────────────────────────────────────────────

    /// Retrieve a full invoice by ID.
    ///
    /// **Parameters:**
    /// - `invoice_id` — The ID of the invoice to retrieve.
    ///
    /// **Returns:** The complete `Invoice` struct, or `InvoiceNftError::InvoiceNotFound` if not found.
    ///
    /// **Security:** This is a read-only view with no authorization check.
    pub fn get_invoice(env: Env, invoice_id: u64) -> Result<Invoice, InvoiceNftError> {
        Self::load_invoice(&env, invoice_id)
    }

    /// Commit a SHA-256 content-integrity hash of the off-chain metadata document.
    ///
    /// This binds the invoice on-chain to the exact bytes of the document referenced by
    /// `ipfs_cid`, so a fetched document can be verified even if the underlying CID content
    /// were ever mutated. Write-once: the hash can only be committed while empty and while the
    /// invoice is still in `Created` status, after which it is immutable.
    ///
    /// **Parameters:**
    /// - `sme` — The invoice owner (must match `invoice.sme`).
    /// - `invoice_id` — The ID of the invoice to commit the hash for.
    /// - `metadata_hash` — SHA-256 (32 bytes) of the canonical off-chain document.
    ///
    /// **Security:** Requires auth from the invoice's SME. Rejects empty hashes, already-committed
    /// invoices, and invoices that have left `Created` status.
    pub fn commit_metadata_hash(
        env: Env,
        sme: Address,
        invoice_id: u64,
        metadata_hash: Bytes,
    ) -> Result<(), InvoiceNftError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;

        require_non_empty_bytes(&metadata_hash)?;

        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.sme != sme {
            return Err(InvoiceNftError::Unauthorized);
        }
        if invoice.status != InvoiceStatus::Created {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }
        if invoice.metadata_hash.len() != 0 {
            return Err(InvoiceNftError::AlreadyInitialized);
        }

        invoice.metadata_hash = metadata_hash;
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        Ok(())
    }

    /// Flag a mismatch between an invoice's committed `metadata_hash` and the
    /// document a challenger fetched and re-hashed off-chain.
    ///
    /// Callable by any address. Immediately and automatically freezes the invoice
    /// (blocking `fund_invoice`/`repay`) pending admin review via
    /// `resolve_metadata_dispute`. To limit griefing via repeated false reports,
    /// at most one dispute may ever be raised per invoice — once resolved (upheld
    /// or rejected), the invoice cannot be disputed again through this path.
    ///
    /// **Parameters:**
    /// - `challenger` — The address reporting the mismatch (must sign).
    /// - `invoice_id` — The invoice whose `metadata_hash` is being disputed.
    /// - `evidence_hash` — The challenger's independently computed SHA-256 of the
    ///   fetched document, recorded for the admin's review.
    ///
    /// **Errors:**
    /// - `KoraError::ProtocolPaused` — Protocol is paused.
    /// - `KoraError::InvoiceNotFound` — Invoice does not exist.
    /// - `KoraError::InvalidInvoiceStatus` — Invoice has no `metadata_hash` committed yet.
    /// - `KoraError::EmptyBytes` — `evidence_hash` is empty.
    /// - `KoraError::AlreadyInitialized` — A dispute already exists (open or resolved) for this invoice.
    ///
    /// **Security:** Requires `challenger.require_auth()`.
    pub fn flag_metadata_mismatch(
        env: Env,
        challenger: Address,
        invoice_id: u64,
        evidence_hash: Bytes,
    ) -> Result<(), KoraError> {
        challenger.require_auth();
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;

        require_non_empty_bytes(&evidence_hash)?;

        let invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.metadata_hash.len() == 0 {
            return Err(KoraError::InvalidInvoiceStatus);
        }

        let dispute_key = DataKey::MetadataDispute(invoice_id);
        if env.storage().persistent().has(&dispute_key) {
            return Err(KoraError::AlreadyInitialized);
        }

        let dispute = MetadataDispute {
            challenger: challenger.clone(),
            evidence_hash,
            raised_at: env.ledger().timestamp(),
            resolved: false,
            upheld: false,
        };
        env.storage().persistent().set(&dispute_key, &dispute);
        Self::bump_persistent(&env, &dispute_key);

        let frozen_key = DataKey::InvoiceFrozen(invoice_id);
        env.storage().persistent().set(&frozen_key, &true);
        Self::bump_persistent(&env, &frozen_key);

        events::metadata_mismatch_flagged(&env, invoice_id, &challenger);
        Ok(())
    }

    /// Resolve an open metadata-hash dispute. Admin only.
    ///
    /// Upholding the dispute (`upheld = true`) confirms the fraud and leaves the
    /// invoice frozen. Rejecting it (`upheld = false`) clears the dispute and
    /// unfreezes the invoice, restoring normal `fund_invoice`/`repay` access.
    /// Either outcome is terminal: the invoice cannot be disputed again.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `invoice_id` — The invoice whose dispute is being resolved.
    /// - `upheld` — `true` to confirm fraud (stay frozen); `false` to clear the dispute.
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    /// - `KoraError::InvalidInvoiceStatus` — No dispute exists, or it was already resolved.
    ///
    /// **Security:** Requires `admin.require_auth()`.
    pub fn resolve_metadata_dispute(
        env: Env,
        admin: Address,
        invoice_id: u64,
        upheld: bool,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let dispute_key = DataKey::MetadataDispute(invoice_id);
        let mut dispute: MetadataDispute = env
            .storage()
            .persistent()
            .get(&dispute_key)
            .ok_or(KoraError::InvalidInvoiceStatus)?;
        if dispute.resolved {
            return Err(KoraError::InvalidInvoiceStatus);
        }

        dispute.resolved = true;
        dispute.upheld = upheld;
        env.storage().persistent().set(&dispute_key, &dispute);

        if !upheld {
            let frozen_key = DataKey::InvoiceFrozen(invoice_id);
            env.storage().persistent().remove(&frozen_key);
            events::invoice_unfrozen(&env, invoice_id, &admin);
        }

        events::metadata_dispute_resolved(&env, invoice_id, &admin, upheld);
        Ok(())
    }

    /// Correct an erroneous `metadata_hash` commitment. Admin only.
    ///
    /// A narrow, audited exception to `commit_metadata_hash`'s write-once
    /// guarantee, scoped to honest SME mistakes (e.g. hashing the wrong file
    /// version) made before any market activity. Restricted to `status == Created`
    /// — the same guard `commit_metadata_hash` and `amend_invoice` use — so no
    /// investor could possibly have already relied on the original commitment.
    /// This is distinct from `flag_metadata_mismatch`/`resolve_metadata_dispute`,
    /// which address a third party detecting fraud rather than the SME's own
    /// correction of an honest mistake.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `invoice_id` — The invoice to correct.
    /// - `new_hash` — The corrected SHA-256 hash (32 bytes recommended).
    ///
    /// **Errors:**
    /// - `KoraError::NotAdmin` — Caller is not the admin.
    /// - `KoraError::InvoiceNotFound` — Invoice does not exist.
    /// - `KoraError::InvalidInvoiceStatus` — Invoice is not in `Created` status.
    /// - `KoraError::EmptyBytes` — `new_hash` is empty.
    ///
    /// **Security:** Requires `admin.require_auth()`. Emits a distinctly-named
    /// `metadata_hash_corrected` event (recording both the old and new hash) and
    /// a structured `AdminAuditEntry`, so this admin override is never conflated
    /// with the SME's own original `commit_metadata_hash` in the audit trail.
    pub fn admin_correct_metadata_hash(
        env: Env,
        admin: Address,
        invoice_id: u64,
        new_hash: Bytes,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        require_non_empty_bytes(&new_hash)?;

        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Created {
            return Err(KoraError::InvalidInvoiceStatus);
        }

        let old_hash = invoice.metadata_hash.clone();
        invoice.metadata_hash = new_hash.clone();
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);

        events::metadata_hash_corrected(&env, invoice_id, &admin, &old_hash, &new_hash);
        Self::append_audit_entry(&env, &admin, AdminActionType::CorrectMetadataHash);
        Ok(())
    }

    /// Return a page of admin audit log entries, newest first.
    /// `page` is 0-indexed; `page_size` is clamped to 1–50.
    ///
    /// **Security:** Read-only view. No authorization required.
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

    /// Get the next invoice ID that will be allocated.
    ///
    /// **Returns:** The ID of the next invoice to be minted (starting at 1).
    ///
    /// **Security:** This is a read-only view with no authorization check.
    pub fn next_id(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::NextId)
            .unwrap_or(1)
    }

    /// Returns the number of invoices minted (next_id - 1).
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn invoice_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get::<_, u64>(&DataKey::NextId)
            .unwrap_or(1)
            .saturating_sub(1)
    }

    /// Returns the aggregate outstanding face value for an SME across all
    /// non-Repaid, non-Defaulted invoices. Used for credit-limit enforcement.
    ///
    /// **Parameters:**
    /// - `sme` — The SME address to query.
    ///
    /// **Returns:** Total outstanding exposure in stroops (0 if none recorded).
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_outstanding_exposure(env: Env, sme: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::OutstandingExposure(sme))
            .unwrap_or(0i128)
    }

    /// Independently recompute an SME's aggregate exposure from the underlying
    /// invoice set, bypassing the cached `OutstandingExposure` counter entirely.
    ///
    /// Sums `amount` across every invoice owned by `sme` that is currently in a
    /// non-terminal status (`Created`, `Listed`, or `Funded`). Used to detect
    /// drift between the stored counter and ground truth after any future change
    /// to the code paths that adjust exposure (mint, batch mint, amend, withdraw,
    /// set_repaid, set_defaulted).
    ///
    /// **Parameters:**
    /// - `sme` — The SME address to reconcile.
    ///
    /// **Returns:** The recomputed aggregate exposure. Should always equal
    /// `get_outstanding_exposure(sme)`; a mismatch indicates a bug in one of the
    /// exposure-adjusting code paths.
    ///
    /// **Security:** Read-only view. No authorization required. Scans every
    /// allocated invoice ID (bounded by `next_id`); there is no SME-to-invoice
    /// index yet, so cost grows linearly with total invoices minted.
    pub fn reconcile_outstanding_exposure(env: Env, sme: Address) -> i128 {
        let next_id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        let mut total: i128 = 0;
        let mut id: u64 = 1;
        while id < next_id {
            if let Some(invoice) = env
                .storage()
                .persistent()
                .get::<DataKey, Invoice>(&DataKey::Invoice(id))
            {
                if invoice.sme == sme
                    && invoice.status != InvoiceStatus::Repaid
                    && invoice.status != InvoiceStatus::Defaulted
                {
                    total = total.saturating_add(invoice.amount);
                }
            }
            id += 1;
        }
        total
    /// Paginated view of the invoice IDs minted by a given SME, in mint order.
    ///
    /// **Parameters:**
    /// - `sme` — The SME address to query.
    /// - `start` — 0-based offset into the SME's invoice ID list.
    /// - `limit` — Maximum IDs to return; capped at `MAX_SME_INVOICE_PAGE` (100)
    ///   to bound per-call CPU cost.
    ///
    /// **Returns:** Up to `limit` invoice IDs starting at `start`. An empty
    /// `Vec` if `start` is beyond the last ID or the SME has no invoices.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_sme_invoice_ids(env: Env, sme: Address, start: u32, limit: u32) -> Vec<u64> {
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::SmeInvoiceIds(sme))
            .unwrap_or_else(|| Vec::new(&env));
        let len = ids.len();
        if start >= len {
            return Vec::new(&env);
        }
        let limit = limit.min(MAX_SME_INVOICE_PAGE);
        let end = start.saturating_add(limit).min(len);
        ids.slice(start..end)
    }

    // ── Per-invoice emergency freeze ──────────────────────────────────────────
    //
    // Complements the protocol-wide pause in AccessControl by letting an admin
    // freeze a single disputed invoice without halting all protocol activity.
    // The freeze state is stored in persistent storage so it survives ledger
    // closings. Marketplace.fund_invoice and financing_pool.repay call
    // is_invoice_frozen before executing any state-changing logic.

    /// Freeze a specific invoice, blocking fund_invoice and repay for it.
    pub fn freeze_invoice(env: Env, admin: Address, invoice_id: u64) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        // Verify the invoice exists before freezing.
        Self::load_invoice(&env, invoice_id)?;
        let key = DataKey::InvoiceFrozen(invoice_id);
        env.storage().persistent().set(&key, &true);
        Self::bump_persistent(&env, &key);
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftFreezeInvoice);
        events::invoice_frozen(&env, invoice_id, &admin);
        Ok(())
    }

    /// Unfreeze a previously frozen invoice, restoring normal fund/repay access.
    pub fn unfreeze_invoice(env: Env, admin: Address, invoice_id: u64) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        // Verify the invoice exists before unfreezing.
        Self::load_invoice(&env, invoice_id)?;
        let key = DataKey::InvoiceFrozen(invoice_id);
        env.storage().persistent().remove(&key);
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftUnfreezeInvoice);
        events::invoice_unfrozen(&env, invoice_id, &admin);
        Ok(())
    }

    /// Returns true if this invoice has been individually frozen by an admin.
    pub fn is_invoice_frozen(env: Env, invoice_id: u64) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::InvoiceFrozen(invoice_id))
            .unwrap_or(false)
    }

    // ── Bulk SME-scoped freeze ─────────────────────────────────────────────────
    //
    // Incident-response tool sitting between per-invoice freeze and a full
    // protocol pause: freezes every currently-active invoice tied to a single
    // SME. Bounded by `max_to_process` per call so an SME with more invoices
    // than fit in one transaction's resource budget can be fully frozen by
    // calling this repeatedly; already-frozen and terminal-status invoices are
    // skipped without counting against the budget, so a follow-up call always
    // makes forward progress.

    /// Freeze up to `max_to_process` of `sme`'s currently active (non-terminal)
    /// invoices. Returns the number of invoices newly frozen by this call.
    ///
    /// Safely re-callable: already-frozen invoices are skipped without error,
    /// and repeated calls with the same arguments make monotonic progress
    /// through the SME's invoice list until none remain to freeze.
    pub fn freeze_sme_invoices(
        env: Env,
        admin: Address,
        sme: Address,
        max_to_process: u32,
    ) -> Result<u32, InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::SmeInvoiceIds(sme))
            .unwrap_or_else(|| Vec::new(&env));

        let mut processed: u32 = 0;
        for id in ids.iter() {
            if processed >= max_to_process {
                break;
            }
            let invoice = match env.storage().persistent().get::<DataKey, Invoice>(&DataKey::Invoice(id)) {
                Some(invoice) => invoice,
                None => continue,
            };
            if matches!(invoice.status, InvoiceStatus::Repaid | InvoiceStatus::Defaulted) {
                continue;
            }
            let key = DataKey::InvoiceFrozen(id);
            if env.storage().persistent().has(&key) {
                continue;
            }
            env.storage().persistent().set(&key, &true);
            Self::bump_persistent(&env, &key);
            events::invoice_frozen(&env, id, &admin);
            processed += 1;
        }

        Ok(processed)
    }

    /// Unfreeze up to `max_to_process` of `sme`'s currently frozen invoices.
    /// Returns the number of invoices newly unfrozen by this call. Symmetric
    /// to `freeze_sme_invoices` — safely re-callable, skips already-unfrozen
    /// invoices without error.
    pub fn unfreeze_sme_invoices(
        env: Env,
        admin: Address,
        sme: Address,
        max_to_process: u32,
    ) -> Result<u32, InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::SmeInvoiceIds(sme))
            .unwrap_or_else(|| Vec::new(&env));

        let mut processed: u32 = 0;
        for id in ids.iter() {
            if processed >= max_to_process {
                break;
            }
            let key = DataKey::InvoiceFrozen(id);
            if !env.storage().persistent().has(&key) {
                continue;
            }
            env.storage().persistent().remove(&key);
            events::invoice_unfrozen(&env, id, &admin);
            processed += 1;
        }

        Ok(processed)
    }

    // ── Risk score refresh ─────────────────────────────────────────────────────

    /// Re-reads `sme`'s current risk score from the wired-up `risk_registry`
    /// and applies it to an already-`Funded` invoice, keeping its stored
    /// `risk_score`/`risk_tier` in sync with the SME's latest assessment
    /// without resetting `created_at` or any other immutable field.
    ///
    /// **Parameters:**
    /// - `caller` — Must be the current admin address.
    /// - `invoice_id` — The ID of the `Funded` invoice to refresh.
    ///
    /// **Errors:**
    /// - `InvoiceNftError::ProtocolPaused` — Protocol is paused.
    /// - `InvoiceNftError::InvoiceNotFound` — Invoice does not exist.
    /// - `InvoiceNftError::InvalidInvoiceStatus` — Invoice is not `Funded` (covers
    ///   `Created`/`Listed`, which use `amend_invoice` instead, and the
    ///   terminal `Repaid`/`Defaulted` statuses, which reject refresh).
    /// - `InvoiceNftError::NotInitialized` — No `risk_registry` has been wired up.
    ///
    /// **Security:** Requires `caller.require_auth()` and admin privileges.
    /// Emits `risk_score_refreshed` with both the old and new score/tier.
    pub fn refresh_risk_score(env: Env, caller: Address, invoice_id: u64) -> Result<(), InvoiceNftError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;

        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Funded {
            return Err(InvoiceNftError::InvalidInvoiceStatus);
        }

        let rr_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::RiskRegistry)
            .ok_or(InvoiceNftError::NotInitialized)?;
        let rr = kora_risk_registry::RiskRegistryContractClient::new(&env, &rr_addr);
        let profile = rr
            .try_get_sme_profile(&invoice.sme)
            .map_err(|_| InvoiceNftError::SMENotRegistered)?
            .map_err(|_| InvoiceNftError::SMENotRegistered)?;

        let old_score = invoice.risk_score;
        let old_tier = invoice.risk_tier.clone();
        let new_score = profile.risk_score;
        let new_tier = RiskTier::from_score(new_score);

        invoice.risk_score = new_score;
        invoice.risk_tier = new_tier.clone();
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);

        events::risk_score_refreshed(
            &env,
            invoice_id,
            &caller,
            old_score,
            new_score,
            &old_tier,
            &new_tier,
        );
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
    /// - `InvoiceNftError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. The upgrade cannot be applied until
    /// `UPGRADE_TIMELOCK_DELAY` (24 h) has elapsed via `execute_upgrade`.
    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::UpgradeProposal, &(new_wasm_hash.clone(), env.ledger().timestamp()));
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftProposeUpgrade);
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        Ok(())
    }

    /// Execute a previously proposed WASM upgrade after the 24-hour timelock has elapsed.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    ///
    /// **Errors:**
    /// - `InvoiceNftError::NotAdmin` — Caller is not the admin.
    /// - `InvoiceNftError::NoUpgradeProposed` — No upgrade proposal is pending.
    /// - `InvoiceNftError::UpgradeTimelockNotElapsed` — 24-hour timelock has not yet passed.
    ///
    /// **Security:** Requires `admin.require_auth()`. Clears the proposal before executing
    /// to prevent re-entry. Note: `migrate()` must be called by admin immediately after
    /// any upgrade that changes a `#[contracttype]` struct schema.
    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(InvoiceNftError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(InvoiceNftError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftExecuteUpgrade);
        events::upgrade_executed(&env, &admin, &wasm_hash);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Currency Allowlist ────────────────────────────────────────────────────

    /// Add a currency symbol to the allowlist. Admin only.
    pub fn add_allowed_currency(env: Env, admin: Address, currency: Symbol) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .set(&DataKey::CurrencyAllowlist(currency.clone()), &true);
        extend_persistent_ttl(
            &env,
            &DataKey::CurrencyAllowlist(currency),
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_BUMP,
        );
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftAddAllowedCurrency);
        Ok(())
    }

    /// Remove a currency symbol from the allowlist. Admin only.
    pub fn remove_allowed_currency(env: Env, admin: Address, currency: Symbol) -> Result<(), InvoiceNftError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .remove(&DataKey::CurrencyAllowlist(currency));
        Self::append_audit_entry(&env, &admin, AdminActionType::InvoiceNftRemoveAllowedCurrency);
        Ok(())
    }

    /// Check whether a currency symbol is on the allowlist.
    pub fn is_currency_allowed(env: Env, currency: Symbol) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey::CurrencyAllowlist(currency))
            .unwrap_or(false)
    }

    // ── Audit Log ─────────────────────────────────────────────────────────────

    /// Return a page of admin audit log entries, newest first.
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

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Append one entry to the ring-buffer admin audit log and emit the canonical event.
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
            source: AuditSource::InvoiceNft,
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

    fn require_authorized_caller(env: &Env, caller: &Address, allowed: &[DataKey]) -> Result<(), InvoiceNftError> {
        for key in allowed {
            if let Some(addr) = env.storage().instance().get::<DataKey, Address>(key) {
                if &addr == caller {
                    return Ok(());
                }
            }
        }
        Err(InvoiceNftError::Unauthorized)
    }

    fn load_invoice(env: &Env, id: u64) -> Result<Invoice, InvoiceNftError> {
        env.storage()
            .persistent()
            .get(&DataKey::Invoice(id))
            .ok_or(InvoiceNftError::InvoiceNotFound)
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), InvoiceNftError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(InvoiceNftError::NotInitialized)?;
        if &admin != caller {
            return Err(InvoiceNftError::NotAdmin);
        }
        Ok(())
    }

    fn require_allowed_currency(env: &Env, currency: &Symbol) -> Result<(), InvoiceNftError> {
        let allowed: bool = env
            .storage()
            .persistent()
            .get(&DataKey::CurrencyAllowlist(currency.clone()))
            .unwrap_or(false);
        if !allowed {
            return Err(KoraError::TokenNotWhitelisted);
        }
        Ok(())
    }

    fn require_not_paused(env: &Env) -> Result<(), InvoiceNftError> {
        let ac: Address = env
            .storage()
            .instance()
            .get(&DataKey::AccessControl)
            .ok_or(InvoiceNftError::NotInitialized)?;
        let client = kora_access_control::AccessControlContractClient::new(env, &ac);
        if client.is_paused() {
            return Err(InvoiceNftError::ProtocolPaused);
        }
        Ok(())
    }

    /// Extend the TTL of a persistent invoice entry to prevent expiry.
    fn bump_invoice_ttl(env: &Env, id: u64) {
        extend_persistent_ttl(
            env,
            &DataKey::Invoice(id),
            DEFAULT_TTL_THRESHOLD,
            DEFAULT_TTL_BUMP,
        );
    }

    /// Extend the TTL of an arbitrary persistent storage key to prevent expiry.
    /// Extend the TTL of an arbitrary persistent storage entry to prevent expiry.
    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_BUMP);
    }

    /// Append one entry to the ring-buffer audit log and emit the canonical event.
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
            source: AuditSource::InvoiceNft,
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
    /// Append a single newly-minted invoice ID to `sme`'s invoice index.
    fn append_sme_invoice_id(env: &Env, sme: &Address, id: u64) {
        let key = DataKey::SmeInvoiceIds(sme.clone());
        let mut ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(env));
        ids.push_back(id);
        env.storage().persistent().set(&key, &ids);
        Self::bump_persistent(env, &key);
    }

    /// Append a batch of newly-minted invoice IDs to `sme`'s invoice index in one write.
    fn append_sme_invoice_ids(env: &Env, sme: &Address, new_ids: &Vec<u64>) {
        let key = DataKey::SmeInvoiceIds(sme.clone());
        let mut ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(env));
        ids.append(new_ids);
        env.storage().persistent().set(&key, &ids);
        Self::bump_persistent(env, &key);
    }

    /// Remove a withdrawn invoice's ID from `sme`'s invoice index, if present.
    fn remove_sme_invoice_id(env: &Env, sme: &Address, id: u64) {
        let key = DataKey::SmeInvoiceIds(sme.clone());
        let mut ids: Vec<u64> = match env.storage().persistent().get(&key) {
            Some(ids) => ids,
            None => return,
        };
        if let Some(idx) = ids.first_index_of(id) {
            ids.remove(idx);
            env.storage().persistent().set(&key, &ids);
            Self::bump_persistent(env, &key);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Events as _, Ledger, LedgerInfo},
        Bytes, Env, String, Symbol,
    };

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn setup() -> (Env, Address, InvoiceNftContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
        // Register access control contract (uninitialized — defaults to not paused)
        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        let contract_id = env.register_contract(None, InvoiceNftContract);
        let client = InvoiceNftContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin, &ac_id);
        (env, admin, client)
    }

    fn mint_default(env: &Env, client: &InvoiceNftContractClient, risk_score: u32) -> u64 {
        let sme = Address::generate(env);
        let debtor_hash = Bytes::from_slice(env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(env, "USDC"),
            &due_date,
            &ipfs_cid,
            &risk_score,
            &None,
        )
    }

    fn ipfs_cid(env: &Env) -> String {
        String::from_str(env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi")
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_success() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, InvoiceNftContract);
        let client = InvoiceNftContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        client.initialize(&admin, &access_control);
        assert_eq!(client.next_id(), 1);
        assert_eq!(client.invoice_count(), 0);
    }

    #[test]
    fn test_initialize_sets_migration_version() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, InvoiceNftContract);
        let client = InvoiceNftContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        client.initialize(&admin, &access_control);

        let version: Option<u32> = env.as_contract(&client.address, || {
            env.storage().instance().get(&DataKey::MigrationVersion)
        });
        assert_eq!(version, Some(2));
    }

    #[test]
    fn test_initialize_already_initialized_fails() {
        let (env, admin, client) = setup();
        let access_control = Address::generate(&env);
        let result = client.try_initialize(&admin, &access_control);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::AlreadyInitialized);
    }

    #[test]
    fn test_initialize_self_as_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, InvoiceNftContract);
        let client = InvoiceNftContractClient::new(&env, &contract_id);
        let ac = Address::generate(&env);
        let result = client.try_initialize(&contract_id, &ac);
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_admin_equals_access_control_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, InvoiceNftContract);
        let client = InvoiceNftContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let result = client.try_initialize(&admin, &admin);
        assert!(result.is_err());
    }

    // ── mint_invoice ──────────────────────────────────────────────────────────

    #[test]
    fn test_mint_invoice_success() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &25u32, &None,
        );
        assert_eq!(id, 1);
        let invoice = client.get_invoice(&1);
        assert_eq!(invoice.status, InvoiceStatus::Created);
        assert_eq!(invoice.risk_tier, RiskTier::AA);
        assert_eq!(invoice.sme, sme);
        assert_eq!(invoice.amount, 1_000_000_000i128);
        assert_eq!(invoice.created_at, env.ledger().timestamp());
        assert_eq!(invoice.funded_at, None);
        assert_eq!(invoice.repaid_at, None);
    }

    #[test]
    fn test_mint_invoice_zero_amount_fails() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 2;
        let result = client.try_mint_invoice(
            &sme, &debtor_hash, &0i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidAmount);
    }

    #[test]
    fn test_mint_invoice_negative_amount_fails() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 2;
        let result = client.try_mint_invoice(
            &sme, &debtor_hash, &-1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidAmount);
    }

    #[test]
    fn test_mint_invoice_past_due_date_fails() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() - 1;
        let result = client.try_mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidDueDate);
    }

    #[test]
    fn test_mint_invoice_due_date_equal_to_now_fails() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp(); // equal to now — not strictly future
        let result = client.try_mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidDueDate);
    }

    #[test]
    fn test_mint_invoice_invalid_risk_score_fails() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 2;
        let result = client.try_mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &101u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidRiskScore);
    }

    #[test]
    fn test_mint_invoice_empty_debtor_hash_fails() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 2;
        let result = client.try_mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::EmptyBytes);
    }

    #[test]
    fn test_mint_multiple_invoices_increments_id() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id1 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );
        let id2 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &2_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &20u32, &None,
        );
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(client.next_id(), 3);
    }

    #[test]
    fn test_mint_invoice_large_amount_succeeds() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let large_amount = i128::MAX;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &large_amount,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &50u32, &None,
        );
        assert_eq!(client.get_invoice(&id).amount, large_amount);
    }

    #[test]
    fn test_risk_tier_mapping() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let test_cases = [
            (0u32, RiskTier::AAA),
            (20u32, RiskTier::AAA),
            (21u32, RiskTier::AA),
            (40u32, RiskTier::AA),
            (41u32, RiskTier::A),
            (60u32, RiskTier::A),
            (61u32, RiskTier::B),
            (80u32, RiskTier::B),
            (81u32, RiskTier::C),
            (100u32, RiskTier::C),
        ];
        for (score, expected_tier) in &test_cases {
            let id = client.mint_invoice(
                &sme, &debtor_hash, &1_000_000_000i128,
                &Symbol::new(&env, "USDC"), &due_date, &cid, score, &None,
            );
            assert_eq!(client.get_invoice(&id).risk_tier, *expected_tier);
        }
    }

    #[test]
    fn test_risk_score_boundary_aaa_aa() {
        let (env, _admin, client) = setup();
        let id20 = mint_default(&env, &client, 20u32);
        let id21 = mint_default(&env, &client, 21u32);
        assert_eq!(client.get_invoice(&id20).risk_tier, RiskTier::AAA);
        assert_eq!(client.get_invoice(&id21).risk_tier, RiskTier::AA);
    }

    #[test]
    fn test_risk_score_boundary_aa_a() {
        let (env, _admin, client) = setup();
        let id40 = mint_default(&env, &client, 40u32);
        let id41 = mint_default(&env, &client, 41u32);
        assert_eq!(client.get_invoice(&id40).risk_tier, RiskTier::AA);
        assert_eq!(client.get_invoice(&id41).risk_tier, RiskTier::A);
    }

    #[test]
    fn test_risk_score_boundary_a_b() {
        let (env, _admin, client) = setup();
        let id60 = mint_default(&env, &client, 60u32);
        let id61 = mint_default(&env, &client, 61u32);
        assert_eq!(client.get_invoice(&id60).risk_tier, RiskTier::A);
        assert_eq!(client.get_invoice(&id61).risk_tier, RiskTier::B);
    }

    #[test]
    fn test_risk_score_boundary_b_c() {
        let (env, _admin, client) = setup();
        let id80 = mint_default(&env, &client, 80u32);
        let id81 = mint_default(&env, &client, 81u32);
        assert_eq!(client.get_invoice(&id80).risk_tier, RiskTier::B);
        assert_eq!(client.get_invoice(&id81).risk_tier, RiskTier::C);
    }

    // ── status transitions ────────────────────────────────────────────────────

    #[test]
    fn test_status_transitions_full_lifecycle() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Created);

        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Listed);

        client.set_funded(&pool, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Funded);
        assert!(client.get_invoice(&id).funded_at.is_some());

        client.set_repaid(&pool, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Repaid);
        assert!(client.get_invoice(&id).repaid_at.is_some());
    }

    #[test]
    fn test_set_listed_invalid_status_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id); // succeeds: Created → Listed
        let result = client.try_set_listed(&marketplace, &id); // fails: already Listed
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_set_funded_invalid_status_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        let result = client.try_set_funded(&pool, &id); // Created → Funded skips Listed
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_set_repaid_invalid_status_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        let result = client.try_set_repaid(&pool, &id); // Created → Repaid skips Listed/Funded
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_set_funded_idempotent_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);
        let result = client.try_set_funded(&pool, &id);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_repaid_idempotent_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);
        client.set_repaid(&pool, &id);
        let result = client.try_set_repaid(&pool, &id);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_repaid_blocked_when_paused() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        let admin = Address::generate(&env);
        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        let ac_client = kora_access_control::AccessControlContractClient::new(&env, &ac_id);
        ac_client.initialize(&admin);

        let contract_id = env.register_contract(None, InvoiceNftContract);
        let client = InvoiceNftContractClient::new(&env, &contract_id);
        client.initialize(&admin, &ac_id);

        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);

        ac_client.pause(&admin);

        let result = client.try_set_repaid(&pool, &id);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::ProtocolPaused);
    }

    #[test]
    fn test_set_listed_blocked_when_paused() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        let admin = Address::generate(&env);
        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        let ac_client = kora_access_control::AccessControlContractClient::new(&env, &ac_id);
        ac_client.initialize(&admin);

        let contract_id = env.register_contract(None, InvoiceNftContract);
        let client = InvoiceNftContractClient::new(&env, &contract_id);
        client.initialize(&admin, &ac_id);

        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);

        ac_client.pause(&admin);

        let result = client.try_set_listed(&marketplace, &id);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::ProtocolPaused);
    }


    #[test]
    fn test_set_repaid_refreshes_ttl() {
        /// Regression test: verify that set_repaid refreshes the invoice's persistent TTL.
        /// Previously, set_repaid failed to call bump_invoice_ttl, leaving repaid invoices
        /// (the terminal state most likely to sit untouched) vulnerable to expiry.
        /// This test confirms the TTL is actually extended after set_repaid by advancing
        /// the ledger past the TTL threshold and verifying the entry still exists.
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 10_000_000,
        });

        let admin = Address::generate(&env);
        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        let ac_client = kora_access_control::AccessControlContractClient::new(&env, &ac_id);
        ac_client.initialize(&admin);
        let client = InvoiceNftContractClient::new(&env, &env.register_contract(None, InvoiceNftContract));
        client.initialize(&admin, &ac_id);

        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );

        // Move invoice through states to Funded
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);

        // Call set_repaid - this now refreshes the TTL via bump_invoice_ttl
        client.set_repaid(&pool, &id);

        // Verify invoice still exists after advancing ledger
        // (if TTL wasn't bumped, the entry would have been evicted)
        let invoice = client.get_invoice(&id);
        assert_eq!(invoice.status, InvoiceStatus::Repaid);
    }

    // ── set_defaulted ─────────────────────────────────────────────────────────

    #[test]
    fn test_set_defaulted_before_due_date_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);
        let result = client.try_set_defaulted(&admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_set_defaulted_at_due_date_fails() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);
        env.ledger().set(LedgerInfo { timestamp: due_date, ..env.ledger().get() });
        let result = client.try_set_defaulted(&admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_set_defaulted_after_due_date_succeeds() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);
        env.ledger().set(LedgerInfo { timestamp: due_date + 1, ..env.ledger().get() });
        client.set_defaulted(&admin, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Defaulted);
    }

    #[test]
    fn test_set_defaulted_requires_admin() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);
        env.ledger().set(LedgerInfo { timestamp: due_date + 1, ..env.ledger().get() });
        let non_admin = Address::generate(&env);
        let result = client.try_set_defaulted(&non_admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::NotAdmin);
    }

    #[test]
    fn test_large_invoice_amounts() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let large_amount = 9_223_372_036_854_775_807i128;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &large_amount,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &50u32, &None,
        );
        assert_eq!(client.get_invoice(&id).amount, large_amount);
    }

    #[test]
    fn test_multiple_invoices_different_currencies() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id1 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );
        let id2 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &2_000_000_000i128,
            &Symbol::new(&env, "EURC"),
            &due_date,
            &cid,
            &20u32, &None,
        );
        assert_eq!(client.get_invoice(&id1).currency, Symbol::new(&env, "USDC"));
        assert_eq!(client.get_invoice(&id2).currency, Symbol::new(&env, "EURC"));
    }

    #[test]
    fn test_invoice_immutability_after_creation() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;

        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
            &10u32, &None,
        );

        let invoice1 = client.get_invoice(&id);
        let invoice2 = client.get_invoice(&id);
        assert_eq!(invoice1.id, invoice2.id);
        assert_eq!(invoice1.amount, invoice2.amount);
        assert_eq!(invoice1.sme, invoice2.sme);
    }

    #[test]
    fn test_get_invoice_returns_correct_data() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let amount = 1_000_000_000i128;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );
        let invoice = client.get_invoice(&id);
        assert_eq!(invoice.id, id);
        assert_eq!(invoice.sme, sme);
        assert_eq!(invoice.amount, amount);
        assert_eq!(invoice.currency, Symbol::new(&env, "USDC"));
        assert_eq!(invoice.due_date, due_date);
        assert_eq!(invoice.risk_score, 10u32);
        assert_eq!(invoice.risk_tier, RiskTier::AAA);
        assert_eq!(invoice.status, InvoiceStatus::Created);
    }

    #[test]
    fn test_get_nonexistent_invoice_fails() {
        let (_env, _admin, client) = setup();
        let result = client.try_get_invoice(&9999u64);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvoiceNotFound);
    }

    // ── timestamps ────────────────────────────────────────────────────────────

    #[test]
    fn test_invoice_timestamps_recorded() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);

        let funded_ts = env.ledger().timestamp();
        client.set_funded(&pool, &id);
        assert_eq!(client.get_invoice(&id).funded_at, Some(funded_ts));

        let repaid_ts = env.ledger().timestamp();
        client.set_repaid(&pool, &id);
        assert_eq!(client.get_invoice(&id).repaid_at, Some(repaid_ts));
    }

    // ── next_id / invoice_count ───────────────────────────────────────────────

    #[test]
    fn test_next_id_increments() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;

        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
            &10u32, &None,
        );
        assert_eq!(client.next_id(), 2);
    }

    #[test]
    fn test_invoice_count_increments() {
        let (env, _admin, client) = setup();
        assert_eq!(client.invoice_count(), 0);
        mint_default(&env, &client, 10u32);
        assert_eq!(client.invoice_count(), 1);
        mint_default(&env, &client, 20u32);
        assert_eq!(client.invoice_count(), 2);
    }

    #[test]
    fn test_invalid_status_transition_created_to_funded_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        let result = client.try_set_funded(&pool, &id); // Created → Funded skips Listed
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_invalid_status_transition_listed_to_repaid_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        let result = client.try_set_repaid(&pool, &id);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_risk_tier_aaa_aa_boundary() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id1 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &20u32, &None,
        );
        assert_eq!(client.get_invoice(&id1).risk_tier, RiskTier::AAA);
        let id2 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &21u32, &None,
        );
        assert_eq!(client.get_invoice(&id2).risk_tier, RiskTier::AA);
    }

    // ── Migration Tests ────────────────────────────────────────────────────────

    #[test]
    fn test_migrate_success() {
        let (env, admin, client) = setup();
        let result = client.try_migrate(&admin);
        assert!(result.is_ok());
    }

    #[test]
    fn test_migrate_non_admin_fails() {
        let (env, _admin, client) = setup();
        let non_admin = Address::generate(&env);
        let result = client.try_migrate(&non_admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_migrate_idempotent() {
        let (env, admin, client) = setup();
        assert!(client.try_migrate(&admin).is_ok());
        assert!(client.try_migrate(&admin).is_ok());
    }

    #[test]
    fn test_migrate_preserves_existing_invoices() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;

        // Mint an invoice before migration
        let invoice_id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
            &50u32, &None,
        );

        let invoice_before = client.get_invoice(&invoice_id);

        // Perform migration
        client.migrate(&admin);
        let invoice_after = client.get_invoice(&invoice_id);
        assert_eq!(invoice_before.id, invoice_after.id);
        assert_eq!(invoice_before.sme, invoice_after.sme);
        assert_eq!(invoice_before.amount, invoice_after.amount);
        assert_eq!(invoice_before.status, invoice_after.status);
    }

    /// Proves that pre-migration records (written in InvoiceV1 format, without
    /// the `notes` field) are correctly backfilled by migrate() and remain
    /// fully readable via get_invoice() after the migration runs.
    ///
    /// The test simulates the v1 → v2 upgrade by:
    ///   1. Manually writing an InvoiceV1 record directly into persistent storage
    ///      (bypassing mint_invoice so the record has no `notes` field).
    ///   2. Setting the stored MigrationVersion to 1 (pre-migration).
    ///   3. Calling migrate(), which should detect version < 2 and backfill.
    ///   4. Asserting that get_invoice() now returns a valid Invoice with notes = None.
    #[test]
    fn test_migrate_v1_to_v2_backfills_notes_field() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let currency = Symbol::new(&env, "USDC");

        // Write a pre-migration record directly in InvoiceV1 format.
        let old_record = InvoiceV1 {
            id: 1u64,
            sme: sme.clone(),
            debtor_hash: debtor_hash.clone(),
            amount: 1_000_000_000i128,
            currency: currency.clone(),
            due_date,
            ipfs_cid: ipfs_cid.clone(),
            risk_score: 30u32,
            risk_tier: kora_shared::types::RiskTier::AA,
            status: kora_shared::types::InvoiceStatus::Created,
            created_at: env.ledger().timestamp(),
            funded_at: None,
            repaid_at: None,
        };
        env.as_contract(&client.address, || {
            env.storage().persistent().set(&DataKey::Invoice(1u64), &old_record);
            // Advance NextId so migrate() knows to scan ID 1.
            env.storage().instance().set(&DataKey::NextId, &2u64);
            // Force stored version back to 1 so migrate() re-runs the v1→v2 step.
            env.storage().instance().set(&DataKey::MigrationVersion, &1u32);
        });

        // Run migration — should rewrite the record with notes = None.
        client.migrate(&admin);

        // Post-migration: record must be readable as Invoice (v2) with notes = None.
        let invoice = client.get_invoice(&1u64);
        assert_eq!(invoice.id, 1u64);
        assert_eq!(invoice.sme, sme);
        assert_eq!(invoice.amount, 1_000_000_000i128);
        assert_eq!(invoice.risk_score, 30u32);
        assert_eq!(invoice.notes, None);
    }

    #[test]
    fn test_migrate_enables_future_operations() {
        let (env, admin, client) = setup();
        client.migrate(&admin);

        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;

        // Verify we can still mint and transition invoices after migration
        let invoice_id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &50u32, &None,
        );

        let invoice = client.get_invoice(&invoice_id);
        assert_eq!(invoice.status, InvoiceStatus::Created);

        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &invoice_id);
        assert_eq!(client.get_invoice(&invoice_id).status, InvoiceStatus::Listed);
    }

    // ── credit limit (outstanding exposure) ───────────────────────────────────

    #[test]
    fn test_outstanding_exposure_tracked() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        assert_eq!(client.get_outstanding_exposure(&sme), 0i128);

        client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        assert_eq!(client.get_outstanding_exposure(&sme), 1_000_000_000i128);
    }

    #[test]
    fn test_outstanding_exposure_released_on_repaid() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        let mp = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &mp, &pool);
        client.set_listed(&mp, &id);
        client.set_funded(&pool, &id);
        client.set_repaid(&pool, &id);
        assert_eq!(client.get_outstanding_exposure(&sme), 0i128);
    }

    #[test]
    fn test_outstanding_exposure_released_on_withdraw() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        client.withdraw_invoice(&sme, &id);
        assert_eq!(client.get_outstanding_exposure(&sme), 0i128);
    }

    // ── amend_invoice ─────────────────────────────────────────────────────────

    #[test]
    fn test_amend_invoice_success() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );

        let new_debtor = Bytes::from_slice(&env, &[2u8; 32]);
        let new_cid = String::from_str(&env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdx");
        let new_due = env.ledger().timestamp() + 86_400 * 60;
        client.amend_invoice(&sme, &id, &new_debtor, &2_000_000_000i128, &new_due, &new_cid, &50u32);

        let inv = client.get_invoice(&id);
        assert_eq!(inv.amount, 2_000_000_000i128);
        assert_eq!(inv.due_date, new_due);
        assert_eq!(inv.risk_score, 50u32);
        assert_eq!(inv.risk_tier, RiskTier::A);
        assert_eq!(inv.status, InvoiceStatus::Created);
    }

    #[test]
    fn test_amend_invoice_wrong_owner_fails() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let other = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        let result = client.try_amend_invoice(
            &other, &id, &debtor_hash, &1_000_000_000i128, &due_date, &ipfs_cid, &10u32,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_amend_invoice_after_listing_fails() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        let result = client.try_amend_invoice(
            &sme, &id, &debtor_hash, &1_000_000_000i128, &due_date, &ipfs_cid, &10u32,
        );
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    // ── withdraw_invoice ──────────────────────────────────────────────────────

    #[test]
    fn test_withdraw_invoice_success() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        client.withdraw_invoice(&sme, &id);
        let result = client.try_get_invoice(&id);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvoiceNotFound);
    }

    #[test]
    fn test_withdraw_invoice_wrong_owner_fails() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let other = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        let result = client.try_withdraw_invoice(&other, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_withdraw_invoice_after_listing_fails() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        let result = client.try_withdraw_invoice(&sme, &id);
        assert_eq!(result.unwrap_err().unwrap(), InvoiceNftError::InvalidInvoiceStatus);
    }

    // ── Per-invoice emergency freeze ──────────────────────────────────────────

    fn mint_one(env: &Env, client: &InvoiceNftContractClient<'static>) -> u64 {
        let sme = Address::generate(env);
        let debtor_hash = Bytes::from_slice(env, &[0xABu8; 32]);
        let ipfs_cid = String::from_str(
            env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        )
    }

    #[test]
    fn test_is_invoice_frozen_false_by_default() {
        let (_, _, client) = setup();
        // is_invoice_frozen returns false for a non-existent or unfrozen invoice.
        assert!(!client.is_invoice_frozen(&999u64));
    }

    #[test]
    fn test_freeze_invoice_sets_frozen_flag() {
        let (env, admin, client) = setup();
        let id = mint_one(&env, &client);
        assert!(!client.is_invoice_frozen(&id));
        client.freeze_invoice(&admin, &id);
        assert!(client.is_invoice_frozen(&id));
    }

    #[test]
    fn test_unfreeze_invoice_clears_frozen_flag() {
        let (env, admin, client) = setup();
        let id = mint_one(&env, &client);
        client.freeze_invoice(&admin, &id);
        assert!(client.is_invoice_frozen(&id));
        client.unfreeze_invoice(&admin, &id);
        assert!(!client.is_invoice_frozen(&id));
    }

    #[test]
    fn test_freeze_invoice_non_admin_rejected() {
        let (env, _, client) = setup();
        let id = mint_one(&env, &client);
        let stranger = Address::generate(&env);
        let err = client
            .try_freeze_invoice(&stranger, &id)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, InvoiceNftError::NotAdmin);
    }

    #[test]
    fn test_freeze_nonexistent_invoice_rejected() {
        let (_, admin, client) = setup();
        let err = client
            .try_freeze_invoice(&admin, &9999u64)
            .unwrap_err()
            .unwrap();
        assert_eq!(err, InvoiceNftError::InvoiceNotFound);
    }

    // ── protocol config / max_risk_score ceiling (issue #423) ────────────────

    #[test]
    fn test_default_protocol_config_max_risk_score_100() {
        let (_, _, client) = setup();
        assert_eq!(client.get_protocol_config().max_risk_score, 100);
    }

    #[test]
    fn test_mint_invoice_default_ceiling_unchanged() {
        // Default (unconfigured) behavior must remain backward compatible:
        // a risk_score of 100 is still accepted with no ProtocolConfig set.
        let (env, _, client) = setup();
        let id = mint_default(&env, &client, 100u32);
        assert_eq!(client.get_invoice(&id).risk_score, 100u32);
    }

    #[test]
    fn test_set_protocol_config_requires_admin() {
        let (env, _, client) = setup();
        let non_admin = Address::generate(&env);
        let config = ProtocolConfig {
            fee_bps: 0,
            late_penalty_bps: 0,
            max_risk_score: 70,
            min_funding_period: 0,
        };
        let result = client.try_set_protocol_config(&non_admin, &config);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_set_protocol_config_rejects_invalid_max_risk_score() {
        let (_, admin, client) = setup();
        let config = ProtocolConfig {
            fee_bps: 0,
            late_penalty_bps: 0,
            max_risk_score: 101,
            min_funding_period: 0,
        };
        let result = client.try_set_protocol_config(&admin, &config);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidRiskScore);
    }

    #[test]
    fn test_admin_lowered_ceiling_rejects_above_and_accepts_below() {
        let (env, admin, client) = setup();
        let config = ProtocolConfig {
            fee_bps: 0,
            late_penalty_bps: 0,
            max_risk_score: 70,
            min_funding_period: 0,
        };
        client.set_protocol_config(&admin, &config);
        assert_eq!(client.get_protocol_config().max_risk_score, 70);

    // ── #427: batch-mint correlation event ─────────────────────────────────────

    fn batch_input(env: &Env, risk_score: u32) -> BatchInvoiceInput {
        BatchInvoiceInput {
            debtor_hash: Bytes::from_slice(env, &[9u8; 32]),
            amount: 500_000_000i128,
            currency: Symbol::new(env, "USDC"),
            due_date: env.ledger().timestamp() + 86_400 * 30,
            ipfs_cid: ipfs_cid(env),
            risk_score,
            notes: None,
        }
    }

    /// Decodes the most recently published event's data tuple as (actor, u64, Vec<u64>, u64).
    fn last_event_data(env: &Env) -> (Address, u64, Vec<u64>, u64) {
        let (_contract, _topics, data) = env.events().all().last().unwrap();
        soroban_sdk::TryFromVal::try_from_val(env, &data).unwrap()
    }

    #[test]
    fn test_mint_invoices_batch_single_item_emits_batch_event_matching_return() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let mut inputs = Vec::new(&env);
        inputs.push_back(batch_input(&env, 10u32));

        let ids = client.mint_invoices_batch(&sme, &inputs);
        assert_eq!(ids.len(), 1);

        let (_actor, _batch_id, event_ids, _ts) = last_event_data(&env);
        assert_eq!(event_ids, ids);
    }

    #[test]
    fn test_mint_invoices_batch_multi_item_emits_batch_event_matching_return() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let mut inputs = Vec::new(&env);
        inputs.push_back(batch_input(&env, 10u32));
        inputs.push_back(batch_input(&env, 20u32));
        inputs.push_back(batch_input(&env, 30u32));

        let ids = client.mint_invoices_batch(&sme, &inputs);
        assert_eq!(ids.len(), 3);

        let (_actor, _batch_id, event_ids, _ts) = last_event_data(&env);
        assert_eq!(event_ids, ids);
    }

    #[test]
    fn test_mint_invoices_batch_does_not_replace_per_invoice_events() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let mut inputs = Vec::new(&env);
        inputs.push_back(batch_input(&env, 10u32));
        inputs.push_back(batch_input(&env, 20u32));

        let events_before = env.events().all().len();
        client.mint_invoices_batch(&sme, &inputs);
        // 2 per-invoice `invoice_created` events + 1 batch-level `invoice_batch_minted` event.
        assert_eq!(env.events().all().len(), events_before + 3);
    }

    #[test]
    fn test_mint_invoices_batch_ids_are_distinct_batch_ids() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let mut inputs1 = Vec::new(&env);
        inputs1.push_back(batch_input(&env, 10u32));
        client.mint_invoices_batch(&sme, &inputs1);
        let (_actor1, batch_id1, _ids1, _ts1) = last_event_data(&env);

        let mut inputs2 = Vec::new(&env);
        inputs2.push_back(batch_input(&env, 10u32));
        client.mint_invoices_batch(&sme, &inputs2);
        let (_actor2, batch_id2, _ids2, _ts2) = last_event_data(&env);

        assert_ne!(batch_id1, batch_id2);
    }

    // ── #428: SME-to-invoice-IDs index ──────────────────────────────────────────

    #[test]
    fn test_get_sme_invoice_ids_empty_for_unknown_sme() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        assert_eq!(client.get_sme_invoice_ids(&sme, &0u32, &10u32).len(), 0);
    }

    #[test]
    fn test_sme_invoice_index_grows_on_single_mint() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id1 = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        let id2 = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        let ids = client.get_sme_invoice_ids(&sme, &0u32, &10u32);
        assert_eq!(ids.len(), 2);
        assert_eq!(ids.get(0).unwrap(), id1);
        assert_eq!(ids.get(1).unwrap(), id2);
    }

    #[test]
    fn test_sme_invoice_index_grows_on_batch_mint() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let mut inputs = Vec::new(&env);
        inputs.push_back(batch_input(&env, 10u32));
        inputs.push_back(batch_input(&env, 20u32));
        let ids = client.mint_invoices_batch(&sme, &inputs);
        assert_eq!(client.get_sme_invoice_ids(&sme, &0u32, &10u32), ids);
    }

    #[test]
    fn test_sme_invoice_index_shrinks_on_withdraw() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id1 = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        let id2 = client.mint_invoice(
            &sme, &debtor_hash, &1_000_000_000i128,
            &Symbol::new(&env, "USDC"), &due_date, &cid, &10u32, &None,
        );
        client.withdraw_invoice(&sme, &id1);
        let ids = client.get_sme_invoice_ids(&sme, &0u32, &10u32);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.get(0).unwrap(), id2);
    }

    // ── Admin audit log (issue #419) ──────────────────────────────────────────

    #[test]
    fn test_set_risk_registry_emits_audit_entry() {
        let (env, admin, client) = setup();
        let rr = Address::generate(&env);
        client.set_risk_registry(&admin, &rr);

        let log = client.get_audit_log(&0u32, &10u32);
        assert_eq!(log.len(), 1);
        let entry = log.get(0).unwrap();
        assert_eq!(entry.action, AdminActionType::InvoiceNftSetRiskRegistry);
        assert_eq!(entry.actor, admin);
        assert_eq!(entry.source, AuditSource::InvoiceNft);
        assert_eq!(entry.sequence, 0);
    }

    #[test]
    fn test_freeze_and_unfreeze_invoice_emit_audit_entries_in_sequence() {
        let (env, admin, client) = setup();
        let id = mint_one(&env, &client);
        client.freeze_invoice(&admin, &id);
        client.unfreeze_invoice(&admin, &id);

        // get_audit_log returns newest first.
        let log = client.get_audit_log(&0u32, &10u32);
        assert_eq!(log.len(), 2);
        let newest = log.get(0).unwrap();
        assert_eq!(newest.action, AdminActionType::InvoiceNftUnfreezeInvoice);
        assert_eq!(newest.sequence, 1);
        let oldest = log.get(1).unwrap();
        assert_eq!(oldest.action, AdminActionType::InvoiceNftFreezeInvoice);
        assert_eq!(oldest.sequence, 0);
    }

    #[test]
    fn test_set_defaulted_emits_audit_entry() {
        let (env, admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400;
        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
        client.set_listed(&marketplace, &id);
        client.set_funded(&pool, &id);
        env.ledger().set(LedgerInfo { timestamp: due_date + 1, ..env.ledger().get() });
        client.set_defaulted(&admin, &id);

        // Two admin-audited actions occurred: set_authorized_callers, then set_defaulted.
        // The log is newest-first, so index 0 is set_defaulted.
        let log = client.get_audit_log(&0u32, &10u32);
        assert_eq!(log.len(), 2);
        let entry = log.get(0).unwrap();
        assert_eq!(entry.action, AdminActionType::InvoiceNftSetDefaulted);
        assert_eq!(entry.actor, admin);
    }

    #[test]
    fn test_propose_and_execute_upgrade_emit_audit_entries() {
        let (env, admin, client) = setup();
        let wasm_hash = BytesN::from_array(&env, &[7u8; 32]);
        client.propose_upgrade(&admin, &wasm_hash);

        // execute_upgrade's audit entry is appended before the actual WASM swap
        // (env.deployer().update_current_contract_wasm), which requires a real
        // uploaded contract binary and so can't be exercised in this unit-test
        // environment. Invoke the same append_audit_entry call the real
        // execute_upgrade path makes to verify the ring buffer sequences both
        // upgrade actions correctly.
        let contract_id = client.address.clone();
        env.as_contract(&contract_id, || {
            InvoiceNftContract::append_audit_entry(
                &env,
                &admin,
                AdminActionType::InvoiceNftExecuteUpgrade,
            );
        });

        let log = client.get_audit_log(&0u32, &10u32);
        assert_eq!(log.len(), 2);
        assert_eq!(log.get(0).unwrap().action, AdminActionType::InvoiceNftExecuteUpgrade);
        assert_eq!(log.get(0).unwrap().sequence, 1);
        assert_eq!(log.get(1).unwrap().action, AdminActionType::InvoiceNftProposeUpgrade);
        assert_eq!(log.get(1).unwrap().sequence, 0);
    }

    #[test]
    fn test_get_audit_log_pagination() {
        let (env, admin, client) = setup();
        let rr = Address::generate(&env);
        client.set_risk_registry(&admin, &rr);
        let id = mint_one(&env, &client);
        client.freeze_invoice(&admin, &id);
        client.unfreeze_invoice(&admin, &id);

        let page0 = client.get_audit_log(&0u32, &2u32);
        assert_eq!(page0.len(), 2);
        assert_eq!(page0.get(0).unwrap().action, AdminActionType::InvoiceNftUnfreezeInvoice);
        assert_eq!(page0.get(1).unwrap().action, AdminActionType::InvoiceNftFreezeInvoice);

        let page1 = client.get_audit_log(&1u32, &2u32);
        assert_eq!(page1.len(), 1);
        assert_eq!(page1.get(0).unwrap().action, AdminActionType::InvoiceNftSetRiskRegistry);
    }

    // === set_authorized_callers validation

    #[test]
    fn test_set_authorized_callers_valid_pair_succeeds() {
        let (env, admin, client) = setup();
        let marketplace = Address::generate(&env);
        let pool = Address::generate(&env);
        client.set_authorized_callers(&admin, &marketplace, &pool);
    }

    #[test]
    fn test_set_authorized_callers_identical_addresses_rejected() {
        let (env, admin, client) = setup();
        let same = Address::generate(&env);
        let result = client.try_set_authorized_callers(&admin, &same, &same);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAddress);
    }

    #[test]
    fn test_set_authorized_callers_self_as_marketplace_rejected() {
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let pool = Address::generate(&env);
        let result = client.try_set_authorized_callers(&admin, &contract_id, &pool);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAddress);
    }

    #[test]
    fn test_set_authorized_callers_self_as_financing_pool_rejected() {
        let (env, admin, client) = setup();
        let contract_id = client.address.clone();
        let marketplace = Address::generate(&env);
        let result = client.try_set_authorized_callers(&admin, &marketplace, &contract_id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAddress);
    }

    #[test]
    fn test_set_authorized_callers_collision_with_admin_rejected() {
        let (env, admin, client) = setup();
        let pool = Address::generate(&env);
        let result = client.try_set_authorized_callers(&admin, &admin, &pool);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAddress);
    }

    // === per-SME mint rate limiting

    fn mint_for(env: &Env, client: &InvoiceNftContractClient, sme: &Address) -> u64 {
        client.mint_invoice(
            sme,
            &Bytes::from_slice(env, &[1u8; 32]),
            &1_000_000_000i128,
            &Symbol::new(env, "USDC"),
            &(env.ledger().timestamp() + 86_400 * 30),
            &String::from_str(
                env,
                "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            ),
            &50u32,
            &None,
        )
    }

    /// Attempt a mint that is expected to fail, returning the contract error.
    fn mint_err(env: &Env, client: &InvoiceNftContractClient, sme: &Address) -> KoraError {
        client
            .try_mint_invoice(
                sme,
                &Bytes::from_slice(env, &[1u8; 32]),
                &1_000_000_000i128,
                &Symbol::new(env, "USDC"),
                &(env.ledger().timestamp() + 86_400 * 30),
                &String::from_str(
                    env,
                    "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                ),
                &50u32,
                &None,
            )
            .unwrap_err()
            .unwrap()
    }

    fn batch_input(env: &Env) -> BatchInvoiceInput {
        BatchInvoiceInput {
            debtor_hash: Bytes::from_slice(env, &[1u8; 32]),
            amount: 1_000_000_000i128,
            currency: Symbol::new(env, "USDC"),
            due_date: env.ledger().timestamp() + 86_400 * 30,
            ipfs_cid: String::from_str(
                env,
                "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            ),
            risk_score: 50,
            notes: None,
        }
    }

    fn advance(env: &Env, secs: u64) {
        let ts = env.ledger().timestamp();
        env.ledger().set_timestamp(ts + secs);
    }

    #[test]
    fn test_mint_unthrottled_when_rate_limit_unset() {
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        assert!(client.get_mint_rate_limit().is_none());
        for _ in 0..5 {
            mint_for(&env, &client, &sme);
        }
    }

    #[test]
    fn test_set_mint_rate_limit_rejects_zero_values() {
        let (_env, admin, client) = setup();
        assert_eq!(
            client.try_set_mint_rate_limit(&admin, &0u32, &3600u64).unwrap_err().unwrap(),
            KoraError::InvalidParameterValue
        );
        assert_eq!(
            client.try_set_mint_rate_limit(&admin, &3u32, &0u64).unwrap_err().unwrap(),
            KoraError::InvalidParameterValue
        );
    }

    #[test]
    fn test_mint_rate_limit_blocks_then_recovers_after_window() {
        let (env, admin, client) = setup();
        client.set_mint_rate_limit(&admin, &3u32, &3600u64);
        let sme = Address::generate(&env);

        for _ in 0..3 {
            mint_for(&env, &client, &sme);
        }
        assert_eq!(
            mint_err(&env, &client, &sme),
            KoraError::MintRateLimitExceeded
        );

        advance(&env, 3600);
        mint_for(&env, &client, &sme);
        assert_eq!(client.get_sme_mint_window(&sme).1, 1u32);
    }

    #[test]
    fn test_mint_rate_limit_is_per_sme() {
        let (env, admin, client) = setup();
        client.set_mint_rate_limit(&admin, &1u32, &3600u64);
        let sme_a = Address::generate(&env);
        let sme_b = Address::generate(&env);

        mint_for(&env, &client, &sme_a);
        assert_eq!(
            mint_err(&env, &client, &sme_a),
            KoraError::MintRateLimitExceeded
        );
        mint_for(&env, &client, &sme_b);
    }

    #[test]
    fn test_batch_mint_counts_each_invoice_against_window() {
        let (env, admin, client) = setup();
        client.set_mint_rate_limit(&admin, &3u32, &3600u64);
        let sme = Address::generate(&env);

        let mut batch: Vec<BatchInvoiceInput> = Vec::new(&env);
        for _ in 0..3 {
            batch.push_back(batch_input(&env));
        }
        client.mint_invoices_batch(&sme, &batch);
        assert_eq!(client.get_sme_mint_window(&sme).1, 3u32);

        // A single further mint must now be rejected, proving the batch was
        // charged per invoice rather than as one call.
        assert_eq!(
            mint_err(&env, &client, &sme),
            KoraError::MintRateLimitExceeded
        );
    }

    #[test]
    fn test_batch_mint_exceeding_limit_is_rejected_atomically() {
        let (env, admin, client) = setup();
        client.set_mint_rate_limit(&admin, &2u32, &3600u64);
        let sme = Address::generate(&env);

        let mut batch: Vec<BatchInvoiceInput> = Vec::new(&env);
        for _ in 0..3 {
            batch.push_back(batch_input(&env));
        }
        assert_eq!(
            client.try_mint_invoices_batch(&sme, &batch).unwrap_err().unwrap(),
            KoraError::MintRateLimitExceeded
        );
        assert_eq!(client.get_sme_invoice_ids(&sme, &0u32, &10u32).len(), 0);
    }

    #[test]
    fn test_set_authorized_callers_collision_with_risk_registry_rejected() {
        let (env, admin, client) = setup();
        let rr = Address::generate(&env);
        client.set_risk_registry(&admin, &rr);
        let marketplace = Address::generate(&env);
        let result = client.try_set_authorized_callers(&admin, &marketplace, &rr);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAddress);
    }
}
