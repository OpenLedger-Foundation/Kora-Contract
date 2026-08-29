use crate::audit::AdminAuditEntry;
use crate::types::RiskTier;
use soroban_sdk::{symbol_short, Address, Bytes, Env, Symbol, Vec};

// ── Event Schema Version (#583) ───────────────────────────────────────────────
//
// EVENT_SCHEMA_VERSION is the current schema generation for all Kora events.
// Off-chain indexers MUST check the "SCHEMA_V" topic on every event to detect
// contract upgrades that change field count, order, or types.
//
// Versioning policy:
//   • Additive change (new optional field appended to an existing event):
//     increment the MINOR digit only (no topic bump required for pure appends
//     at the end of an existing tuple, but SCHEMA_V must be bumped so indexers
//     know to re-check the schema catalogue).
//   • Breaking change (field removed, type changed, or ordering altered):
//     increment the MAJOR digit and update docs/EVENTS.md with migration notes.
//
// Current version: 1 (initial versioned release).
// See docs/EVENTS.md §"Schema Versioning" for the full changelog.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

// ── Canonical Event Schema ────────────────────────────────────────────────────
//
// Every event published by the Kora protocol follows this payload convention:
//
//   (actor: Address, subject: ..., amount: i128, ledger_timestamp: u64)
//
// - actor    — the address initiating the action (SME, investor, admin, etc.)
// - subject  — what is being acted on (invoice_id, token, etc.)
// - amount   — the monetary value involved (0 when not applicable)
// - ledger_timestamp — env.ledger().timestamp() — always included for
//               deterministic off-chain indexing and reconciliation
//
// Events that carry multiple data fields extend this tuple while preserving
// the actor-first, timestamp-last ordering.
//
// Each event is published with two topics: ("SCHEMA_V", <event_topic>).
// The first topic carries EVENT_SCHEMA_VERSION so off-chain indexers can gate
// on the version without decoding the payload.

fn emit(env: &Env, topic: Symbol, data: impl soroban_sdk::IntoVal<Env, soroban_sdk::Val>) {
    env.events()
        .publish((symbol_short!("SCHEMA_V"), topic, EVENT_SCHEMA_VERSION), data);
}

/// Returns the current event schema version constant.
/// Indexers and tests can call this to assert the version they were built against.
pub fn schema_version() -> u32 {
    EVENT_SCHEMA_VERSION
}

// ── Invoice Events ────────────────────────────────────────────────────────────

/// Schema: (actor=sme, invoice_id, amount, currency, timestamp)
pub fn invoice_created(env: &Env, invoice_id: u64, sme: &Address, amount: i128, currency: Symbol) {
    emit(
        env,
        symbol_short!("INV_CRT"),
        (sme.clone(), invoice_id, amount, currency, env.ledger().timestamp()),
    );
}

/// Standardized marketplace event: invoice listed for financing.
/// Schema: (actor=seller, invoice_id, asking_price, currency, timestamp)
pub fn invoice_listed(env: &Env, invoice_id: u64, seller: &Address, asking_price: i128, currency: Symbol) {
    emit(
        env,
        symbol_short!("INV_LIST"),
        (
            seller.clone(),
            invoice_id,
            asking_price,
            currency,
            env.ledger().timestamp(),
        ),
    );
}

/// Standardized marketplace event: investor funded a listing.
/// Schema: (actor=investor, invoice_id, funded_amount, currency, timestamp)
pub fn invoice_funded(env: &Env, invoice_id: u64, investor: &Address, amount: i128, currency: Symbol) {
    emit(
        env,
        symbol_short!("INV_FUND"),
        (
            investor.clone(),
            invoice_id,
            amount,
            currency,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (actor=sme, invoice_id, amount, currency, timestamp)
pub fn invoice_repaid(env: &Env, invoice_id: u64, sme: &Address, amount: i128, currency: Symbol) {
    emit(
        env,
        symbol_short!("INV_RPD"),
        (sme.clone(), invoice_id, amount, currency, env.ledger().timestamp()),
    );
}

/// Schema: (actor, invoice_id, amount, currency, timestamp)
/// actor is the admin marking the default (or the SME address in invoice_nft context)
pub fn invoice_defaulted(env: &Env, invoice_id: u64, actor: &Address, amount: i128, currency: Symbol) {
    emit(
        env,
        symbol_short!("INV_DFT"),
        (actor.clone(), invoice_id, amount, currency, env.ledger().timestamp()),
    );
}

/// Schema: (invoice_id, sme, currency, timestamp)
pub fn invoice_amended(env: &Env, invoice_id: u64, sme: &Address, currency: Symbol) {
    emit(
        env,
        symbol_short!("INV_AMD"),
        (invoice_id, sme.clone(), currency, env.ledger().timestamp()),
    );
}

/// Schema: (invoice_id, sme, currency, timestamp)
pub fn invoice_withdrawn(env: &Env, invoice_id: u64, sme: &Address, currency: Symbol) {
    emit(
        env,
        symbol_short!("INV_WTH"),
        (invoice_id, sme.clone(), currency, env.ledger().timestamp()),
    );
}

/// Batch-level correlation event for `mint_invoices_batch`, emitted once per
/// call (in addition to the per-invoice `invoice_created` events) so an
/// off-chain indexer can group invoices minted together in one batch.
/// Schema: (actor=sme, batch_id, invoice_ids, timestamp)
pub fn invoice_batch_minted(env: &Env, batch_id: u64, sme: &Address, invoice_ids: &Vec<u64>) {
    emit(
        env,
        symbol_short!("INV_BATCH"),
        (
            sme.clone(),
            batch_id,
            invoice_ids.clone(),
            env.ledger().timestamp(),
        ),
    );
}

/// Records a risk-score refresh on an already-`Funded` invoice, capturing both
/// the prior and updated score/tier for audit purposes.
/// Schema: (actor=caller, invoice_id, old_score, new_score, old_tier, new_tier, timestamp)
pub fn risk_score_refreshed(
    env: &Env,
    invoice_id: u64,
    caller: &Address,
    old_score: u32,
    new_score: u32,
    old_tier: &RiskTier,
    new_tier: &RiskTier,
) {
    emit(
        env,
        symbol_short!("RISK_RFSH"),
        (
            caller.clone(),
            invoice_id,
            old_score,
            new_score,
            old_tier.clone(),
            new_tier.clone(),
            env.ledger().timestamp(),
        ),
    );
}

// ── Repayment Events ──────────────────────────────────────────────────────────

/// Schema: (actor=payer, invoice_id, amount, timestamp)
pub fn repayment_made(env: &Env, invoice_id: u64, payer: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("REPAY"),
        (payer.clone(), invoice_id, amount, env.ledger().timestamp()),
    );
}

/// Schema: (actor=payer, invoice_id, installment_index, amount, timestamp)
pub fn installment_paid(env: &Env, invoice_id: u64, payer: &Address, index: u32, amount: i128) {
    emit(
        env,
        symbol_short!("INSTMT_PD"),
        (payer.clone(), invoice_id, index, amount, env.ledger().timestamp()),
    );
}

/// Schema: (actor=investor, invoice_id, yield_amount, timestamp)
pub fn yield_distributed(env: &Env, invoice_id: u64, investor: &Address, yield_amount: i128) {
    emit(
        env,
        symbol_short!("YIELD"),
        (investor.clone(), invoice_id, yield_amount, env.ledger().timestamp()),
    );
}

/// System event — no single actor; penalty is applied automatically on late repayment.
/// Schema: (invoice_id, penalty_amount, total_owed, timestamp)
pub fn late_penalty_applied(env: &Env, invoice_id: u64, penalty_amount: i128, total_owed: i128) {
    emit(
        env,
        symbol_short!("LATE_PEN"),
        (invoice_id, penalty_amount, total_owed, env.ledger().timestamp()),
    );
}

// ── Marketplace Events ──────────────────────────────────────────────────────

/// Schema: (actor=seller, invoice_id, timestamp)
pub fn listing_cancelled(env: &Env, invoice_id: u64, seller: &Address) {
    emit(
        env,
        symbol_short!("LST_CXL"),
        (seller.clone(), invoice_id, env.ledger().timestamp()),
    );
}

/// Schema: (actor=seller, invoice_id, timestamp)
pub fn listing_expired(env: &Env, invoice_id: u64, seller: &Address) {
    emit(
        env,
        symbol_short!("LST_EXP"),
        (seller.clone(), invoice_id, env.ledger().timestamp()),
    );
}

// ── Fee Events ────────────────────────────────────────────────────────────────

/// Schema: (actor=investor, invoice_id, fee_amount, token, timestamp)
/// investor is the address that paid the fee; use contract address when the
/// fee is deposited programmatically (e.g., treasury.collect_fee).
pub fn fee_collected(
    env: &Env,
    investor: &Address,
    invoice_id: u64,
    fee_amount: i128,
    token: &Address,
) {
    emit(
        env,
        symbol_short!("FEE_COL"),
        (
            investor.clone(),
            invoice_id,
            fee_amount,
            token.clone(),
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (actor=admin, token, amount, timestamp)
pub fn fee_withdrawn(env: &Env, actor: &Address, token: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("FEE_WTH"),
        (actor.clone(), token.clone(), amount, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, token, amount, timestamp)
pub fn emergency_withdrawn(env: &Env, by: &Address, token: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("EMRG_WTH"),
        (by.clone(), token.clone(), amount, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, old_bps, new_bps, timestamp)
pub fn fee_rate_updated(env: &Env, by: &Address, old_bps: u32, new_bps: u32) {
    emit(
        env,
        symbol_short!("FEE_UPD"),
        (by.clone(), old_bps, new_bps, env.ledger().timestamp()),
    );
}

/// Emitted when part of a funding fee is routed to the invoice's referrer.
/// Schema: (invoice_id, referrer, referral_fee, timestamp)
pub fn referral_fee_paid(env: &Env, invoice_id: u64, referrer: &Address, referral_fee: i128) {
    emit(
        env,
        symbol_short!("REF_FEE"),
        (
            invoice_id,
            referrer.clone(),
            referral_fee,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (actor=admin, fee_bps, timestamp)
pub fn treasury_initialized(env: &Env, admin: &Address, fee_bps: u32) {
    emit(
        env,
        symbol_short!("TRES_INI"),
        (admin.clone(), fee_bps, env.ledger().timestamp()),
    );
}

// ── Protocol / Admin Events ───────────────────────────────────────────────────

/// Schema: (actor=admin, timestamp)
pub fn protocol_paused(env: &Env, by: &Address) {
    emit(
        env,
        symbol_short!("AC_PAUSED"),
        (by.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, timestamp)
pub fn protocol_unpaused(env: &Env, by: &Address) {
    emit(
        env,
        symbol_short!("UNPAUSED"),
        (by.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, token, timestamp)
pub fn token_whitelisted(env: &Env, actor: &Address, token: &Address) {
    emit(
        env,
        symbol_short!("TOK_WL"),
        (actor.clone(), token.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, token, timestamp)
pub fn token_whitelist_removed(env: &Env, actor: &Address, token: &Address) {
    emit(
        env,
        symbol_short!("TOK_UNWL"),
        (actor.clone(), token.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=current_admin, new_admin, timestamp)
pub fn admin_transferred(env: &Env, actor: &Address, new_admin: &Address) {
    emit(
        env,
        symbol_short!("ADM_TRF"),
        (actor.clone(), new_admin.clone(), env.ledger().timestamp()),
    );
}

/// Emitted during deliberate key-rotation recovery (e.g. suspected compromise).
/// Distinct from `admin_transferred` so off-chain monitors can alert specifically
/// on rotation events. Schema: (actor=executor, old_admin, new_admin, timestamp)
pub fn admin_rotated(env: &Env, executor: &Address, old_admin: &Address, new_admin: &Address) {
    emit(
        env,
        symbol_short!("ADM_ROT"),
        (
            executor.clone(),
            old_admin.clone(),
            new_admin.clone(),
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (actor=admin, target, timestamp)
pub fn role_granted(env: &Env, admin: &Address, target: &Address) {
    emit(
        env,
        symbol_short!("ROL_GRT"),
        (admin.clone(), target.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, target, timestamp)
pub fn role_revoked(env: &Env, admin: &Address, target: &Address) {
    emit(
        env,
        symbol_short!("ROL_RVK"),
        (admin.clone(), target.clone(), env.ledger().timestamp()),
    );
}

// ── Financing Pool Events ─────────────────────────────────────────────────

/// Schema: (actor=marketplace, invoice_id, token, face_value, timestamp)
pub fn pool_opened(env: &Env, marketplace: &Address, invoice_id: u64, token: &Address, face_value: i128) {
    emit(
        env,
        symbol_short!("POOL_OPN"),
        (
            marketplace.clone(),
            invoice_id,
            token.clone(),
            face_value,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (actor=admin, invoice_id, investor, contributed, share_bps, timestamp)
pub fn position_recorded(
    env: &Env,
    admin: &Address,
    invoice_id: u64,
    investor: &Address,
    contributed: i128,
    share_bps: u32,
) {
    emit(
        env,
        symbol_short!("POS_RECRD"),
        (
            admin.clone(),
            invoice_id,
            investor.clone(),
            contributed,
            share_bps,
            env.ledger().timestamp(),
        ),
    );
}

/// Cross-invoice net settlement event emitted once per `net_settle` call (#588).
/// Schema: (actor=payer, invoice_ids, total_amount, timestamp)
pub fn net_settled(env: &Env, payer: &Address, invoice_ids: &Vec<u64>, total_amount: i128) {
    emit(
        env,
        symbol_short!("NET_SETTL"),
        (
            payer.clone(),
            invoice_ids.clone(),
            total_amount,
            env.ledger().timestamp(),
        ),
    );
}

// ── Risk Registry Events ──────────────────────────────────────────────────────

/// Schema: (actor=admin, verifier, timestamp)
pub fn verifier_added(env: &Env, admin: &Address, verifier: &Address) {
    emit(
        env,
        symbol_short!("VRF_ADD"),
        (admin.clone(), verifier.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, verifier, timestamp)
pub fn verifier_removed(env: &Env, admin: &Address, verifier: &Address) {
    emit(
        env,
        symbol_short!("VRF_REM"),
        (admin.clone(), verifier.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=verifier, sme, risk_score, timestamp)
pub fn sme_registered(env: &Env, verifier: &Address, sme: &Address, risk_score: u32) {
    emit(
        env,
        symbol_short!("SME_REG"),
        (verifier.clone(), sme.clone(), risk_score, env.ledger().timestamp()),
    );
}

/// Schema: (actor=verifier, sme, new_score, timestamp)
pub fn sme_score_updated(env: &Env, verifier: &Address, sme: &Address, new_score: u32) {
    emit(
        env,
        symbol_short!("SME_UPD"),
        (verifier.clone(), sme.clone(), new_score, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, sme, total_defaults, timestamp)
pub fn sme_default_recorded(env: &Env, admin: &Address, sme: &Address, total_defaults: u32) {
    emit(
        env,
        symbol_short!("SME_DFT"),
        (admin.clone(), sme.clone(), total_defaults, env.ledger().timestamp()),
    );
}

/// Schema: (actor=sme, new_total_invoices, timestamp)
pub fn sme_invoice_count_incremented(env: &Env, sme: &Address, new_total: u32) {
    emit(
        env,
        symbol_short!("SME_INV"),
        (sme.clone(), new_total, env.ledger().timestamp()),
    );
}

/// Schema: (actor=verifier, debtor_hash, score, timestamp)
pub fn debtor_score_set(env: &Env, verifier: &Address, debtor_hash: &Bytes, score: u32) {
    emit(
        env,
        symbol_short!("DBT_SCORE"),
        (
            verifier.clone(),
            debtor_hash.clone(),
            score,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (actor=admin, invoice_nft, timestamp)
pub fn registry_initialized(env: &Env, admin: &Address, invoice_nft: &Address) {
    emit(
        env,
        symbol_short!("REG_INI"),
        (admin.clone(), invoice_nft.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (primary_verifier, sub_account, timestamp)
pub fn sub_account_added(env: &Env, primary: &Address, sub_account: &Address) {
    emit(
        env,
        symbol_short!("SUB_ADD"),
        (primary.clone(), sub_account.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (primary_verifier, sub_account, timestamp)
pub fn sub_account_removed(env: &Env, primary: &Address, sub_account: &Address) {
    emit(
        env,
        symbol_short!("SUB_RMV"),
        (primary.clone(), sub_account.clone(), env.ledger().timestamp()),
    );
}

// ── Upgrade Events ───────────────────────────────────────────────────────────

/// Schema: (actor=admin, wasm_hash, timestamp)
pub fn upgrade_proposed(env: &Env, admin: &Address, wasm_hash: &soroban_sdk::BytesN<32>) {
    emit(
        env,
        symbol_short!("UPG_PROP"),
        (admin.clone(), wasm_hash.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, wasm_hash, timestamp)
pub fn upgrade_executed(env: &Env, admin: &Address, wasm_hash: &soroban_sdk::BytesN<32>) {
    emit(
        env,
        symbol_short!("UPG_EXEC"),
        (admin.clone(), wasm_hash.clone(), env.ledger().timestamp()),
    );
}

// ── Multisig Events ──────────────────────────────────────────────────────────

/// System event — records the multisig configuration (no single actor).
/// Schema: (threshold, signer_count, timestamp)
pub fn multisig_configured(env: &Env, threshold: u32, signer_count: u32) {
    emit(
        env,
        symbol_short!("MS_CFG"),
        (threshold, signer_count, env.ledger().timestamp()),
    );
}

/// Schema: (proposal_id, actor=proposer, timestamp)
pub fn action_proposed(env: &Env, proposal_id: u64, proposer: &Address) {
    emit(
        env,
        symbol_short!("MS_PROP"),
        (proposal_id, proposer.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (proposal_id, actor=approver, approval_count, timestamp)
pub fn action_approved(env: &Env, proposal_id: u64, approver: &Address, approval_count: u32) {
    emit(
        env,
        symbol_short!("MS_APPR"),
        (
            proposal_id,
            approver.clone(),
            approval_count,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (proposal_id, actor=executor, timestamp)
pub fn action_executed(env: &Env, proposal_id: u64, executor: &Address) {
    emit(
        env,
        symbol_short!("MS_EXEC"),
        (proposal_id, executor.clone(), env.ledger().timestamp()),
    );
}

// ── Refund Events ────────────────────────────────────────────────────────────

/// Schema: (actor=investor, invoice_id, amount, timestamp)
pub fn refund_claimed(env: &Env, invoice_id: u64, investor: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("REFUND"),
        (
            investor.clone(),
            invoice_id,
            amount,
            env.ledger().timestamp(),
        ),
    );
}

// ── Secondary Market Events ───────────────────────────────────────────────────

pub fn position_listed_for_sale(env: &Env, invoice_id: u64, seller: &Address, price: i128) {
    emit(
        env,
        symbol_short!("POS_SALE"),
        (invoice_id, seller.clone(), price, env.ledger().timestamp()),
    );
}

pub fn position_sold(env: &Env, invoice_id: u64, seller: &Address, buyer: &Address, price: i128) {
    emit(
        env,
        symbol_short!("POS_SOLD"),
        (invoice_id, seller.clone(), buyer.clone(), price, env.ledger().timestamp()),
    );
}

// ── Treasury Cap Events ───────────────────────────────────────────────────────

/// Schema: (actor=admin, token, new_cap, timestamp)
pub fn withdrawal_cap_proposed(env: &Env, admin: &Address, token: &Address, new_cap: i128) {
    emit(
        env,
        symbol_short!("WTH_CAP_P"),
        (admin.clone(), token.clone(), new_cap, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, token, old_cap, new_cap, timestamp)
pub fn withdrawal_cap_updated(env: &Env, admin: &Address, token: &Address, old_cap: i128, new_cap: i128) {
    emit(
        env,
        symbol_short!("WTH_CAP_U"),
        (admin.clone(), token.clone(), old_cap, new_cap, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, timestamp)
pub fn emergency_declared(env: &Env, admin: &Address) {
    emit(
        env,
        symbol_short!("EMRG_DECL"),
        (admin.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, timestamp)
pub fn emergency_revoked(env: &Env, admin: &Address) {
    emit(
        env,
        symbol_short!("EMRG_REVK"),
        (admin.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, access_control, timestamp)
pub fn access_control_updated(env: &Env, admin: &Address, access_control: &Address) {
    emit(
        env,
        symbol_short!("AC_SET"),
        (admin.clone(), access_control.clone(), env.ledger().timestamp()),
    );
}

// ── Treasury Recipient Allowlist Events (#457) ───────────────────────────────

/// Schema: (actor=admin, recipient, timestamp)
pub fn recipient_proposed(env: &Env, admin: &Address, recipient: &Address) {
    emit(
        env,
        symbol_short!("RCP_PROP"),
        (admin.clone(), recipient.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, recipient, timestamp)
pub fn recipient_allowed(env: &Env, admin: &Address, recipient: &Address) {
    emit(
        env,
        symbol_short!("RCP_ALLOW"),
        (admin.clone(), recipient.clone(), env.ledger().timestamp()),
    );
}

// ── Treasury Insurance Reserve Events (#458) ─────────────────────────────────

/// Schema: (actor=caller, token, recipient, amount, timestamp)
pub fn reserve_disbursed(env: &Env, caller: &Address, token: &Address, recipient: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("RSRV_DISB"),
        (
            caller.clone(),
            token.clone(),
            recipient.clone(),
            amount,
            env.ledger().timestamp(),
        ),
    );
}

// ── Risk Registry — Credit Limit ─────────────────────────────────────────────

/// Schema: (actor=verifier, sme, credit_limit, timestamp)
pub fn sme_credit_limit_set(env: &Env, verifier: &Address, sme: &Address, credit_limit: i128) {
    emit(
        env,
        symbol_short!("SME_CL"),
        (verifier.clone(), sme.clone(), credit_limit, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, sme, new_verifier, timestamp)
/// Emitted when the admin reassigns an SME to a new verifier-of-record.
pub fn sme_verifier_reassigned(env: &Env, admin: &Address, sme: &Address, new_verifier: &Address) {
    emit(
        env,
        symbol_short!("SME_REAS"),
        (admin.clone(), sme.clone(), new_verifier.clone(), env.ledger().timestamp()),
    );
}

// ── Invoice Freeze Events ─────────────────────────────────────────────────────

/// Schema: (actor=admin, invoice_id, timestamp)
pub fn invoice_frozen(env: &Env, invoice_id: u64, admin: &Address) {
    emit(
        env,
        symbol_short!("INV_FRZ"),
        (admin.clone(), invoice_id, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, invoice_id, timestamp)
pub fn invoice_unfrozen(env: &Env, invoice_id: u64, admin: &Address) {
    emit(
        env,
        symbol_short!("INV_UFRZ"),
        (admin.clone(), invoice_id, env.ledger().timestamp()),
    );
}

// ── Marketplace Cancellation Events ──────────────────────────────────────────

/// Schema: (actor=caller, invoice_id, timestamp)
pub fn cancellation_requested(env: &Env, invoice_id: u64, caller: &Address) {
    emit(
        env,
        symbol_short!("CXL_REQ"),
        (caller.clone(), invoice_id, env.ledger().timestamp()),
    );
}

// ── Metadata Hash Dispute Events ──────────────────────────────────────────────

/// Schema: (actor=challenger, invoice_id, timestamp)
pub fn metadata_mismatch_flagged(env: &Env, invoice_id: u64, challenger: &Address) {
    emit(
        env,
        symbol_short!("MTD_DISP"),
        (challenger.clone(), invoice_id, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, invoice_id, upheld, timestamp)
pub fn metadata_dispute_resolved(env: &Env, invoice_id: u64, admin: &Address, upheld: bool) {
    emit(
        env,
        symbol_short!("MTD_RES"),
        (admin.clone(), invoice_id, upheld, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, invoice_id, old_hash, new_hash, timestamp)
pub fn metadata_hash_corrected(
    env: &Env,
    invoice_id: u64,
    admin: &Address,
    old_hash: &Bytes,
    new_hash: &Bytes,
) {
    emit(
        env,
        symbol_short!("MTD_CORR"),
        (
            admin.clone(),
            invoice_id,
            old_hash.clone(),
            new_hash.clone(),
            env.ledger().timestamp(),
        ),
    );
}

// ── Dutch Auction / Decay Schedule Events (#439) ──────────────────────────────

/// Schema: (actor=seller, invoice_id, floor_price, decay_end_ts, timestamp)
pub fn decay_schedule_set(
    env: &Env,
    invoice_id: u64,
    seller: &Address,
    floor_price: i128,
    decay_end_ts: u64,
) {
    emit(
        env,
        symbol_short!("DECAY_SET"),
        (
            seller.clone(),
            invoice_id,
            floor_price,
            decay_end_ts,
            env.ledger().timestamp(),
        ),
    );
}

// ── Reverse Auction / Bid Events (#440) ───────────────────────────────────────

/// Schema: (actor=investor, invoice_id, bid_price, amount, timestamp)
pub fn bid_submitted(
    env: &Env,
    invoice_id: u64,
    investor: &Address,
    bid_price: i128,
    amount: i128,
) {
    emit(
        env,
        symbol_short!("BID_SUBM"),
        (
            investor.clone(),
            invoice_id,
            bid_price,
            amount,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (actor=seller, invoice_id, investor, bid_price, amount, timestamp)
pub fn bid_accepted(
    env: &Env,
    invoice_id: u64,
    seller: &Address,
    investor: &Address,
    bid_price: i128,
    amount: i128,
) {
    emit(
        env,
        symbol_short!("BID_ACCP"),
        (
            seller.clone(),
            invoice_id,
            investor.clone(),
            bid_price,
            amount,
            env.ledger().timestamp(),
        ),
    );
}

// ── Admin Audit Trail ─────────────────────────────────────────────────────────

/// Canonical admin-action audit event emitted alongside every admin-gated call.
/// Subscribe to `ADM_AUDIT` across all protocol contracts to build a consolidated
/// off-chain compliance report via Horizon, Mercury, or a custom indexer.
/// Schema: (sequence, actor, action, source, timestamp)
pub fn admin_action_audited(env: &Env, entry: &AdminAuditEntry) {
    emit(
        env,
        symbol_short!("ADM_AUDIT"),
        (
            entry.sequence,
            entry.actor.clone(),
            entry.action.clone(),
            entry.source.clone(),
            entry.timestamp,
        ),
    );
}

// ── Audit Events ─────────────────────────────────────────────────────────────

/// Emitted on every admin action — canonical off-chain history source.
pub fn adm_audit(env: &Env, sequence: u64, action: soroban_sdk::String, actor: &Address, timestamp: u64) {
    emit(
        env,
        symbol_short!("ADM_AUDT"),
        (sequence, action, actor.clone(), timestamp),
    );
}

/// Emitted right before a ring-buffer wraparound begins overwriting old entries.
/// Carries the rolling checksum that commits the full history up to this point,
/// and the raw entry that is about to be discarded — giving off-chain systems an
/// unambiguous, permanent archival signal.
pub fn audit_checkpoint(
    env: &Env,
    total_entries: u64,
    checksum: soroban_sdk::BytesN<32>,
    discarded_action: soroban_sdk::String,
    discarded_actor: &Address,
    discarded_timestamp: u64,
    discarded_sequence: u64,
) {
    emit(
        env,
        symbol_short!("AUDT_CHK"),
        (
            total_entries,
            checksum,
            discarded_action,
            discarded_actor.clone(),
            discarded_timestamp,
            discarded_sequence,
        ),
    );
}

// ── PositionShare Events (#563) ────────────────────────────────────────────────

/// Schema: (invoice_id, original_investor, share_index, amount, new_owner, timestamp)
pub fn position_share_created(
    env: &Env,
    invoice_id: u64,
    original_investor: &Address,
    share_index: u32,
    amount: i128,
    new_owner: &Address,
) {
    emit(
        env,
        symbol_short!("POS_SHR_CR"),
        symbol_short!("SHARE_CRT"),
        (
            invoice_id,
            original_investor.clone(),
            share_index,
            amount,
            new_owner.clone(),
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (invoice_id, original_investor, share_index, from, to, timestamp)
pub fn position_share_transferred(
    env: &Env,
    invoice_id: u64,
    original_investor: &Address,
    share_index: u32,
    from: &Address,
    to: &Address,
) {
    emit(
        env,
        symbol_short!("POS_SHR_TR"),
        symbol_short!("SHARE_TRF"),
        (
            invoice_id,
            original_investor.clone(),
            share_index,
            from.clone(),
            to.clone(),
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (invoice_id, share_index, price, timestamp)
pub fn share_listed_for_sale(
    env: &Env,
    invoice_id: u64,
    share_index: u32,
    price: i128,
) {
    emit(
        env,
        symbol_short!("SHARE_SALE"),
        (
            invoice_id,
            share_index,
            price,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (invoice_id, share_index, buyer, price, timestamp)
pub fn share_sold(
    env: &Env,
    invoice_id: u64,
    share_index: u32,
    buyer: &Address,
    price: i128,
) {
    emit(
        env,
        symbol_short!("SHARE_SOLD"),
        (
            invoice_id,
            share_index,
            buyer.clone(),
            price,
            env.ledger().timestamp(),
        ),
    );
}

// ── Dispute Resolution Events (#565) ──────────────────────────────────────────

/// Schema: (challenger, invoice_id, timestamp)
pub fn dispute_opened(
    env: &Env,
    invoice_id: u64,
    challenger: &Address,
) {
    emit(
        env,
        symbol_short!("DISP_OPEN"),
        (
            challenger.clone(),
            invoice_id,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (invoice_id, evidence_cid, timestamp)
pub fn dispute_evidence_submitted(
    env: &Env,
    invoice_id: u64,
    evidence_cid: &String,
) {
    emit(
        env,
        symbol_short!("DISP_EVID"),
        (
            invoice_id,
            evidence_cid.clone(),
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (resolver, invoice_id, upheld, timestamp)
pub fn dispute_resolved(
    env: &Env,
    invoice_id: u64,
    resolver: &Address,
    upheld: bool,
) {
    emit(
        env,
        symbol_short!("DISP_RES"),
        (
            resolver.clone(),
            invoice_id,
            upheld,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (invoice_id, amount, timestamp)
pub fn dispute_funded(
    env: &Env,
    invoice_id: u64,
    amount: i128,
) {
    emit(
        env,
        symbol_short!("DISP_FUND"),
        (invoice_id, amount, env.ledger().timestamp()),
    );
}

/// Schema: (invoice_id, amount, timestamp)
pub fn dispute_payout(
    env: &Env,
    invoice_id: u64,
    amount: i128,
) {
    emit(
        env,
        symbol_short!("DISP_PAY"),
        (invoice_id, amount, env.ledger().timestamp()),
    );
}
