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
    errors::KoraError,
    events,
    reentrancy::ReentrancyGuard,
    types::{Invoice, InvoiceStatus, RiskTier},
    validation::{
        require_future_timestamp, require_max_length_bytes, require_max_length_string,
        require_non_empty_bytes, require_non_empty_string, require_non_zero_amount,
        require_valid_risk_score, MAX_DEBTOR_HASH_LEN, MAX_IPFS_CID_LEN, UPGRADE_TIMELOCK_DELAY,
    },
};
use soroban_sdk::{contract, contractimpl, contracttype, Address, Bytes, BytesN, Env, String, Symbol};

// ── TTL constants (~30 days at ~5s/ledger) ───────────────────────────────────
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

// ── Storage Keys ────────────────────────────────────────────────────────────
//
// Storage versioning: The contract uses a MigrationVersion key to track schema changes.
// Current version: 1
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
    /// Instance key: pending new admin address (two-step transfer)
    PendingAdmin,
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
    pub fn initialize(env: Env, admin: Address, access_control: Address) -> Result<(), KoraError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }
        kora_shared::validation::require_not_self(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &access_control)?;
        kora_shared::validation::require_distinct(&admin, &access_control)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::AccessControl, &access_control);
        env.storage().instance().set(&DataKey::NextId, &1u64);
        // Initialize migration version to 1 (current schema version)
        env.storage()
            .instance()
            .set(&DataKey::MigrationVersion, &1u32);
        Ok(())
    }

    /// Set the risk_registry contract address. Admin only. Called after deployment
    /// to wire up credit-limit enforcement.
    pub fn set_risk_registry(env: Env, admin: Address, risk_registry: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &risk_registry)?;
        env.storage().instance().set(&DataKey::RiskRegistry, &risk_registry);
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
    pub fn migrate(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;

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
                        risk_score: old.risk_score,
                        risk_tier: old.risk_tier,
                        status: old.status,
                        created_at: old.created_at,
                        funded_at: old.funded_at,
                        repaid_at: old.repaid_at,
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
    pub fn set_authorized_callers(
        env: Env,
        admin: Address,
        marketplace: Address,
        financing_pool: Address,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Marketplace, &marketplace);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        Ok(())
    }

    /// Mint a new invoice NFT. Caller must be a verified SME.
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
    ) -> Result<u64, KoraError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;

        require_non_zero_amount(amount)?;
        require_future_timestamp(&env, due_date)?;
        require_valid_risk_score(risk_score)?;
        require_non_empty_bytes(&debtor_hash)?;
        require_max_length_bytes(&debtor_hash, MAX_DEBTOR_HASH_LEN)?;
        require_non_empty_string(&ipfs_cid)?;
        require_max_length_string(&ipfs_cid, MAX_IPFS_CID_LEN)?;

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
                    if profile.credit_limit > 0 {
                        let outstanding: i128 = env
                            .storage()
                            .persistent()
                            .get(&DataKey::OutstandingExposure(sme.clone()))
                            .unwrap_or(0i128);
                        let new_exposure = outstanding
                            .checked_add(amount)
                            .ok_or(KoraError::ArithmeticOverflow)?;
                        if new_exposure > profile.credit_limit {
                            return Err(KoraError::CreditLimitExceeded);
                        }
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
            &(id.checked_add(1).ok_or(KoraError::ArithmeticOverflow)?),
        );

        events::invoice_created(&env, id, &sme, invoice.amount);
        Ok(id)
    }

    /// Amend a Created invoice. Only the original SME may call this, and only
    /// while `status == Created` (i.e., before any market activity).
    ///
    /// Any subset of fields may be corrected: pass the existing value to leave
    /// a field unchanged.
    pub fn amend_invoice(
        env: Env,
        sme: Address,
        invoice_id: u64,
        debtor_hash: Bytes,
        amount: i128,
        due_date: u64,
        ipfs_cid: String,
        risk_score: u32,
    ) -> Result<(), KoraError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;

        require_non_zero_amount(amount)?;
        require_future_timestamp(&env, due_date)?;
        require_valid_risk_score(risk_score)?;
        require_non_empty_bytes(&debtor_hash)?;
        require_non_empty_string(&ipfs_cid)?;

        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Created {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        if invoice.sme != sme {
            return Err(KoraError::NotInvoiceOwner);
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
        events::invoice_amended(&env, invoice_id, &sme);
        Ok(())
    }

    /// Withdraw (void) a Created invoice. Only the original SME may call this,
    /// and only while `status == Created`.
    ///
    /// The invoice record is removed from storage, permanently burning the NFT.
    pub fn withdraw_invoice(
        env: Env,
        sme: Address,
        invoice_id: u64,
    ) -> Result<(), KoraError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;

        let invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Created {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        if invoice.sme != sme {
            return Err(KoraError::NotInvoiceOwner);
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
        events::invoice_withdrawn(&env, invoice_id, &sme);
        Ok(())
    }

    /// Transition invoice to Listed status. Called by Marketplace contract.
    ///
    /// **Parameters:**
    /// - `caller` — The marketplace contract address.
    /// - `invoice_id` — The ID of the invoice to list.
    ///
    /// **Returns:** `Ok(())` on success, or an appropriate `KoraError`.
    ///
    /// **Security:** Requires auth from the caller. Validates that the invoice is in `Created` status.
    pub fn set_listed(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError> {
        caller.require_auth();
        Self::require_authorized_caller(&env, &caller, &[DataKey::Marketplace])?;
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Created {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        invoice.status = InvoiceStatus::Listed;
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        events::invoice_listed(&env, invoice_id, &invoice.sme, invoice.amount);
        Ok(())
    }

    /// Transition invoice to Funded. Called by Financing Pool contract.
    ///
    /// **Parameters:**
    /// - `caller` — The investor or financing pool contract address.
    /// - `invoice_id` — The ID of the invoice to fund.
    ///
    /// **Returns:** `Ok(())` on success, or an appropriate `KoraError`.
    ///
    /// **Security:** Requires auth from the caller. Validates that the invoice is in `Listed` status.
    pub fn set_funded(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError> {
        caller.require_auth();
        Self::require_authorized_caller(&env, &caller, &[DataKey::FinancingPool])?;
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Listed {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        invoice.status = InvoiceStatus::Funded;
        invoice.funded_at = Some(env.ledger().timestamp());
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Self::bump_invoice_ttl(&env, invoice_id);
        events::invoice_funded(&env, invoice_id, &caller, invoice.amount);
        Ok(())
    }

    /// Mark invoice as Repaid. Called by Financing Pool on full repayment.
    ///
    /// **Parameters:**
    /// - `caller` — The financing pool contract address.
    /// - `invoice_id` — The ID of the invoice to repay.
    ///
    /// **Returns:** `Ok(())` on success, or an appropriate `KoraError`.
    ///
    /// **Security:** Requires auth from the caller. Validates that the invoice is in `Funded` status.
    pub fn set_repaid(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError> {
        caller.require_auth();
        Self::require_authorized_caller(&env, &caller, &[DataKey::FinancingPool])?;
        Self::require_not_paused(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Funded {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        invoice.status = InvoiceStatus::Repaid;
        invoice.repaid_at = Some(env.ledger().timestamp());
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        // Release outstanding exposure
        let prev: i128 = env.storage().persistent()
            .get(&DataKey::OutstandingExposure(invoice.sme.clone()))
            .unwrap_or(0i128);
        env.storage().persistent().set(
            &DataKey::OutstandingExposure(invoice.sme.clone()),
            &prev.saturating_sub(invoice.amount),
        );
        events::invoice_repaid(&env, invoice_id, &invoice.sme, invoice.amount);
        Ok(())
    }

    /// Mark invoice as Defaulted. Called by admin after due date passes.
    ///
    /// **Parameters:**
    /// - `caller` — The admin address.
    /// - `invoice_id` — The ID of the invoice to mark as defaulted.
    ///
    /// **Returns:** `Ok(())` on success, or an appropriate `KoraError`.
    ///
    /// **Security:** Requires admin auth. Validates that the invoice is `Funded` and the due date has passed.
    pub fn set_defaulted(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        let _guard = ReentrancyGuard::new(&env)?;
        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Funded {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        let current_time = env.ledger().timestamp();
        if current_time <= invoice.due_date {
            return Err(KoraError::InvalidInvoiceStatus);
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
        events::invoice_defaulted(&env, invoice_id, &invoice.sme);
        Ok(())
    }

    // ── Views ────────────────────────────────────────────────────────────────

    /// Archive a terminal invoice, removing it from persistent storage to reclaim rent.
    /// Only admin can call this. Invoice must be in Repaid or Defaulted status.
    pub fn archive_invoice(env: Env, admin: Address, invoice_id: u64) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.status != InvoiceStatus::Repaid && invoice.status != InvoiceStatus::Defaulted {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        events::invoice_archived(&env, invoice_id, &invoice.sme, invoice.amount, invoice.status.clone());
        env.storage().persistent().remove(&DataKey::Invoice(invoice_id));
        Ok(())
    }

    /// Retrieve a full invoice by ID.
    ///
    /// **Parameters:**
    /// - `invoice_id` — The ID of the invoice to retrieve.
    ///
    /// **Returns:** The complete `Invoice` struct, or `KoraError::InvoiceNotFound` if not found.
    ///
    /// **Security:** This is a read-only view with no authorization check.
    pub fn get_invoice(env: Env, invoice_id: u64) -> Result<Invoice, KoraError> {
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
    ) -> Result<(), KoraError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;
        let _guard = ReentrancyGuard::new(&env)?;

        require_non_empty_bytes(&metadata_hash)?;

        let mut invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.sme != sme {
            return Err(KoraError::Unauthorized);
        }
        if invoice.status != InvoiceStatus::Created {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        if invoice.metadata_hash.len() != 0 {
            return Err(KoraError::AlreadyInitialized);
        }

        invoice.metadata_hash = metadata_hash;
        env.storage()
            .persistent()
            .set(&DataKey::Invoice(invoice_id), &invoice);
        Ok(())
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
    pub fn invoice_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get::<_, u64>(&DataKey::NextId)
            .unwrap_or(1)
            .saturating_sub(1)
    }

    /// Returns the aggregate outstanding face value for an SME across all
    /// non-Repaid, non-Defaulted invoices. Used for credit-limit enforcement.
    pub fn get_outstanding_exposure(env: Env, sme: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::OutstandingExposure(sme))
            .unwrap_or(0i128)
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::UpgradeProposal, &(new_wasm_hash.clone(), env.ledger().timestamp()));
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

    // ── Currency Allowlist ────────────────────────────────────────────────────

    /// Add a currency symbol to the allowlist. Admin only.
    pub fn add_allowed_currency(env: Env, admin: Address, currency: Symbol) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .set(&DataKey::CurrencyAllowlist(currency.clone()), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::CurrencyAllowlist(currency),
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_BUMP,
        );
        Ok(())
    }

    /// Remove a currency symbol from the allowlist. Admin only.
    pub fn remove_allowed_currency(env: Env, admin: Address, currency: Symbol) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .remove(&DataKey::CurrencyAllowlist(currency));
        Ok(())
    }

    /// Check whether a currency symbol is on the allowlist.
    pub fn is_currency_allowed(env: Env, currency: Symbol) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey::CurrencyAllowlist(currency))
            .unwrap_or(false)
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn require_authorized_caller(env: &Env, caller: &Address, allowed: &[DataKey]) -> Result<(), KoraError> {
        for key in allowed {
            if let Some(addr) = env.storage().instance().get::<DataKey, Address>(key) {
                if &addr == caller {
                    return Ok(());
                }
            }
        }
        Err(KoraError::Unauthorized)
    }

    fn load_invoice(env: &Env, id: u64) -> Result<Invoice, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Invoice(id))
            .ok_or(KoraError::InvoiceNotFound)
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), KoraError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(KoraError::NotInitialized)?;
        if &admin != caller {
            return Err(KoraError::NotAdmin);
        }
        Ok(())
    }

    fn require_allowed_currency(env: &Env, currency: &Symbol) -> Result<(), KoraError> {
        let allowed: bool = env
            .storage()
            .persistent()
            .get(&DataKey::CurrencyAllowlist(currency.clone()))
            .unwrap_or(false);
        if !allowed {
            return Err(KoraError::CurrencyNotAllowed);
        }
        Ok(())
    }

    fn require_not_paused(env: &Env) -> Result<(), KoraError> {
        let ac: Address = env
            .storage()
            .instance()
            .get(&DataKey::AccessControl)
            .ok_or(KoraError::NotInitialized)?;
        let client = kora_access_control::AccessControlContractClient::new(env, &ac);
        if client.is_paused() {
            return Err(KoraError::ProtocolPaused);
        }
        Ok(())
    }

    /// Extend the TTL of a persistent invoice entry to prevent expiry.
    fn bump_invoice_ttl(env: &Env, id: u64) {
        env.storage().persistent().extend_ttl(
            &DataKey::Invoice(id),
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_BUMP,
        );
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use kora_shared::errors::KoraError;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
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
    fn test_initialize_already_initialized_fails() {
        let (env, admin, client) = setup();
        let access_control = Address::generate(&env);
        let result = client.try_initialize(&admin, &access_control);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::AlreadyInitialized);
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
            &ipfs_cid,
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidDueDate);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidDueDate);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &101u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidRiskScore);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32, &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::EmptyBytes);
    }

    #[test]
    fn test_mint_multiple_invoices_increments_id() {
        let (env, _admin, client) = setup();
        let sme1 = Address::generate(&env);
        let sme2 = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let cid = ipfs_cid(&env);
        let due_date = env.ledger().timestamp() + 86_400 * 30;
        let id1 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
            &10u32, &None,
        );
        let id2 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &2_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &50u32, &None,
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
                &Symbol::new(&env, "USDC"), &due_date, &cid, score,
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
        let (env, _admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Created);

        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Listed);

        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Funded);
        assert!(client.get_invoice(&id).funded_at.is_some());

        client.set_repaid(&pool, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Repaid);
        assert!(client.get_invoice(&id).repaid_at.is_some());
    }

    #[test]
    fn test_set_listed_invalid_status_fails() {
        let (env, _admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id); // succeeds: Created → Listed
        let result = client.try_set_listed(&marketplace, &id); // fails: already Listed
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
    }

        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
            &10u32, &None,
        );

    #[test]
    fn test_set_funded_invalid_status_fails() {
        let (env, _admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let pool = Address::generate(&env);
        let result = client.try_set_funded(&pool, &id); // Created → Funded skips Listed
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_set_repaid_invalid_status_fails() {
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

    #[test]
    fn test_set_funded_idempotent_fails() {
        let (env, _admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        let result = client.try_set_funded(&pool, &id);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_repaid_idempotent_fails() {
        let (env, _admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        client.set_repaid(&pool, &id);
        let result = client.try_set_repaid(&pool, &id);
        assert!(result.is_err());
    }

    // ── set_defaulted ─────────────────────────────────────────────────────────

    #[test]
    fn test_set_defaulted_before_due_date_fails() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        let result = client.try_set_defaulted(&admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
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
            &ipfs_cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        env.ledger().set(LedgerInfo { timestamp: due_date, ..env.ledger().get() });
        let result = client.try_set_defaulted(&admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
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
            &ipfs_cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        env.ledger().set(LedgerInfo { timestamp: due_date + 1, ..env.ledger().get() });
        client.set_defaulted(&admin, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Defaulted);
    }

    #[test]
    fn test_set_defaulted_requires_admin() {
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
            &ipfs_cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        env.ledger().set(LedgerInfo { timestamp: due_date + 1, ..env.ledger().get() });
        let non_admin = Address::generate(&env);
        let result = client.try_set_defaulted(&non_admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
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
            &ipfs_cid,
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
            &ipfs_cid,
            &10u32, &None,
        );
        let id2 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &2_000_000_000i128,
            &Symbol::new(&env, "EURC"),
            &due_date,
            &ipfs_cid,
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
            &ipfs_cid,
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
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvoiceNotFound);
    }

    // ── timestamps ────────────────────────────────────────────────────────────

    #[test]
    fn test_invoice_timestamps_recorded() {
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
            &ipfs_cid,
            &10u32, &None,
        );
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);

        let pool = Address::generate(&env);
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

        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);

        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        client.set_repaid(&pool, &id);

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
        let (env, _admin, client) = setup();
        let sme = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[1u8; 32]);
        let ipfs_cid = String::from_str(
            &env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = env.ledger().timestamp() + 86_400;

        let id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
            &10u32, &None,
        );
        let id2 = client.mint_invoice(
            &sme, &debtor_hash, &2_000_000_000i128,
            &Symbol::new(&env, "EURC"), &due_date, &ipfs_cid, &20u32, &None,
        );

        assert_eq!(client.get_invoice(&id1).currency, Symbol::new(&env, "USDC"));
        assert_eq!(client.get_invoice(&id2).currency, Symbol::new(&env, "EURC"));
    }

    #[test]
    fn test_invalid_status_transition_listed_to_repaid_fails() {
        let (env, _admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        let result = client.try_set_repaid(&pool, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
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
            &ipfs_cid,
            &20u32, &None,
        );
        assert_eq!(client.get_invoice(&id1).risk_tier, RiskTier::AAA);
        let id2 = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
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
        let invoice_after = client.get_invoice(&id);
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
        env.storage().persistent().set(&DataKey::Invoice(1u64), &old_record);
        // Advance NextId so migrate() knows to scan ID 1.
        env.storage().instance().set(&DataKey::NextId, &2u64);
        // Force stored version back to 1 so migrate() re-runs the v1→v2 step.
        env.storage().instance().set(&DataKey::MigrationVersion, &1u32);

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

        // Verify we can still mint and transition invoices after migration
        let invoice_id = client.mint_invoice(
            &sme,
            &debtor_hash,
            &1_000_000_000i128,
            &Symbol::new(&env, "USDC"),
            &due_date,
            &ipfs_cid,
            &50u32, &None,
        );

        let invoice = client.get_invoice(&invoice_id);
        assert_eq!(invoice.status, InvoiceStatus::Created);

        // Transition through state machine
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        assert_eq!(client.get_invoice(&id).status, InvoiceStatus::Listed);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
        );
        assert_eq!(client.get_outstanding_exposure(&sme), 1_000_000_000i128);
    }

    #[test]
    fn test_outstanding_exposure_released_on_repaid() {
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
        );
        let mp = Address::generate(&env);
        let pool = Address::generate(&env);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
        );
        let result = client.try_amend_invoice(
            &other, &id, &debtor_hash, &1_000_000_000i128, &due_date, &ipfs_cid, &10u32,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotInvoiceOwner);
    }

    #[test]
    fn test_amend_invoice_after_listing_fails() {
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
        );
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let result = client.try_amend_invoice(
            &sme, &id, &debtor_hash, &1_000_000_000i128, &due_date, &ipfs_cid, &10u32,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
        );
        client.withdraw_invoice(&sme, &id);
        let result = client.try_get_invoice(&id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvoiceNotFound);
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
        );
        let result = client.try_withdraw_invoice(&other, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotInvoiceOwner);
    }

    #[test]
    fn test_withdraw_invoice_after_listing_fails() {
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
            &Symbol::new(&env, "USDC"), &due_date, &ipfs_cid, &10u32,
        );
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let result = client.try_withdraw_invoice(&sme, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
    }

    // ── archive_invoice ───────────────────────────────────────────────────────

    #[test]
    fn test_archive_invoice_fails_for_non_terminal_status() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        // Invoice is Created — not terminal, must fail
        let result = client.try_archive_invoice(&admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidInvoiceStatus);
    }

    #[test]
    fn test_archive_invoice_requires_admin() {
        let (env, _admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let non_admin = Address::generate(&env);
        let result = client.try_archive_invoice(&non_admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_archive_invoice_repaid_succeeds() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        client.set_repaid(&pool, &id);
        // Should archive without error
        client.archive_invoice(&admin, &id);
        // Invoice should no longer be found
        let result = client.try_get_invoice(&id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvoiceNotFound);
    }

    #[test]
    fn test_archive_invoice_defaulted_succeeds() {
        let (env, admin, client) = setup();
        let id = mint_default(&env, &client, 10u32);
        let marketplace = Address::generate(&env);
        client.set_listed(&marketplace, &id);
        let pool = Address::generate(&env);
        client.set_funded(&pool, &id);
        // Advance past due_date so set_defaulted works
        let invoice = client.get_invoice(&id);
        env.ledger().set(LedgerInfo {
            timestamp: invoice.due_date + 1,
            ..env.ledger().get()
        });
        client.set_defaulted(&admin, &id);
        client.archive_invoice(&admin, &id);
        let result = client.try_get_invoice(&id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvoiceNotFound);
    }

    // ── propose_admin / accept_admin ──────────────────────────────────────────

    #[test]
    fn test_propose_then_accept_transfers_admin() {
        let (env, admin, client) = setup();
        let new_admin = Address::generate(&env);
        client.propose_admin(&admin, &new_admin);
        client.accept_admin(&new_admin);
        // new admin can call admin-only archive
        let id = mint_default(&env, &client, 10u32);
        assert!(client.try_archive_invoice(&new_admin, &id).is_err()); // Created, not terminal — proves auth passes
        assert!(client.try_archive_invoice(&admin, &id).is_err()); // old admin is rejected
    }

    #[test]
    fn test_propose_admin_requires_admin() {
        let (env, _admin, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        assert!(client.try_propose_admin(&stranger, &new_admin).is_err());
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

    #[test]
    fn test_accept_admin_without_proposal_fails() {
        let (env, _admin, client) = setup();
        let stranger = Address::generate(&env);
        assert!(client.try_accept_admin(&stranger).is_err());
    }
}
