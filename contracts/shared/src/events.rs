use soroban_sdk::{symbol_short, Address, Bytes, Env, Symbol};
use crate::types::InvoiceStatus;

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

fn emit(env: &Env, topic: Symbol, data: impl soroban_sdk::IntoVal<Env, soroban_sdk::Val>) {
    env.events().publish((topic,), data);
}

// ── Invoice Events ────────────────────────────────────────────────────────────

/// Schema: (actor=sme, invoice_id, amount, timestamp)
pub fn invoice_created(env: &Env, invoice_id: u64, sme: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("INV_CRT"),
        (sme.clone(), invoice_id, amount, env.ledger().timestamp()),
    );
}

/// Standardized marketplace event: invoice listed for financing.
/// Schema: (actor=seller, invoice_id, asking_price, timestamp)
pub fn invoice_listed(env: &Env, invoice_id: u64, seller: &Address, asking_price: i128) {
    emit(
        env,
        symbol_short!("INV_LISTED"),
        (
            seller.clone(),
            invoice_id,
            asking_price,
            env.ledger().timestamp(),
        ),
    );
}

/// Standardized marketplace event: investor funded a listing.
/// Schema: (actor=investor, invoice_id, funded_amount, timestamp)
pub fn invoice_funded(env: &Env, invoice_id: u64, investor: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("INV_FUNDED"),
        (
            investor.clone(),
            invoice_id,
            amount,
            env.ledger().timestamp(),
        ),
    );
}

/// Schema: (actor=sme, invoice_id, amount, timestamp)
pub fn invoice_repaid(env: &Env, invoice_id: u64, sme: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("INV_RPD"),
        (sme.clone(), invoice_id, amount, env.ledger().timestamp()),
    );
}

/// Schema: (actor, invoice_id, timestamp)
/// actor is the admin marking the default (or the SME address in invoice_nft context)
pub fn invoice_defaulted(env: &Env, invoice_id: u64, actor: &Address) {
    emit(
        env,
        symbol_short!("INV_DFT"),
        (actor.clone(), invoice_id, env.ledger().timestamp()),
    );
}

pub fn invoice_amended(env: &Env, invoice_id: u64, sme: &Address) {
    emit(
        env,
        symbol_short!("INV_AMD"),
        (invoice_id, sme.clone(), env.ledger().timestamp()),
    );
}

pub fn invoice_withdrawn(env: &Env, invoice_id: u64, sme: &Address) {
    emit(
        env,
        symbol_short!("INV_WTH"),
        (invoice_id, sme.clone(), env.ledger().timestamp()),
    );
}

pub fn invoice_archived(env: &Env, invoice_id: u64, sme: &Address, amount: i128, _status: InvoiceStatus) {
    emit(
        env,
        symbol_short!("INV_ARC"),
        (invoice_id, sme.clone(), amount, env.ledger().timestamp()),
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
        symbol_short!("PROTOCOL_LATE_PEN"),
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
        symbol_short!("TREAS_FEE_COLLECTED"),
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
        symbol_short!("AC_UNPAUSED"),
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

/// Schema: (actor=current_admin, new_admin, timestamp)
pub fn admin_transferred(env: &Env, actor: &Address, new_admin: &Address) {
    emit(
        env,
        symbol_short!("ADM_TRF"),
        (actor.clone(), new_admin.clone(), env.ledger().timestamp()),
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
        symbol_short!("POOL_OPENED"),
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
        symbol_short!("POS_RECORDED"),
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

// ── Risk Registry Events ──────────────────────────────────────────────────────

/// Schema: (actor=admin, verifier, timestamp)
pub fn verifier_added(env: &Env, admin: &Address, verifier: &Address) {
    emit(
        env,
        symbol_short!("RISK_VERIFIER_ADDED"),
        (admin.clone(), verifier.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, verifier, timestamp)
pub fn verifier_removed(env: &Env, admin: &Address, verifier: &Address) {
    emit(
        env,
        symbol_short!("RISK_VERIFIER_REMOVED"),
        (admin.clone(), verifier.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=verifier, sme, risk_score, timestamp)
pub fn sme_registered(env: &Env, verifier: &Address, sme: &Address, risk_score: u32) {
    emit(
        env,
        symbol_short!("RISK_SME_REGISTERED"),
        (verifier.clone(), sme.clone(), risk_score, env.ledger().timestamp()),
    );
}

/// Schema: (actor=verifier, sme, new_score, timestamp)
pub fn sme_score_updated(env: &Env, verifier: &Address, sme: &Address, new_score: u32) {
    emit(
        env,
        symbol_short!("RISK_SME_SCORE_UPDATED"),
        (verifier.clone(), sme.clone(), new_score, env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, sme, total_defaults, timestamp)
pub fn sme_default_recorded(env: &Env, admin: &Address, sme: &Address, total_defaults: u32) {
    emit(
        env,
        symbol_short!("RISK_SME_DEFAULT_REC"),
        (admin.clone(), sme.clone(), total_defaults, env.ledger().timestamp()),
    );
}

/// Schema: (actor=sme, new_total_invoices, timestamp)
pub fn sme_invoice_count_incremented(env: &Env, sme: &Address, new_total: u32) {
    emit(
        env,
        symbol_short!("RISK_SME_INV_COUNT"),
        (sme.clone(), new_total, env.ledger().timestamp()),
    );
}

/// Schema: (actor=verifier, debtor_hash, score, timestamp)
pub fn debtor_score_set(env: &Env, verifier: &Address, debtor_hash: &Bytes, score: u32) {
    emit(
        env,
        symbol_short!("RISK_DEBTOR_SCORE_SET"),
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

// ── Upgrade Events ───────────────────────────────────────────────────────────

/// Schema: (actor=admin, wasm_hash, timestamp)
pub fn upgrade_proposed(env: &Env, admin: &Address, wasm_hash: &soroban_sdk::BytesN<32>) {
    emit(
        env,
        symbol_short!("AC_UPGRADE_PROPOSED"),
        (admin.clone(), wasm_hash.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (actor=admin, wasm_hash, timestamp)
pub fn upgrade_executed(env: &Env, admin: &Address, wasm_hash: &soroban_sdk::BytesN<32>) {
    emit(
        env,
        symbol_short!("AC_UPGRADE_EXECUTED"),
        (admin.clone(), wasm_hash.clone(), env.ledger().timestamp()),
    );
}

// ── Multisig Events ──────────────────────────────────────────────────────────

/// System event — records the multisig configuration (no single actor).
/// Schema: (threshold, signer_count, timestamp)
pub fn multisig_configured(env: &Env, threshold: u32, signer_count: u32) {
    emit(
        env,
        symbol_short!("AC_MULTISIG_CFG"),
        (threshold, signer_count, env.ledger().timestamp()),
    );
}

/// Schema: (proposal_id, actor=proposer, timestamp)
pub fn action_proposed(env: &Env, proposal_id: u64, proposer: &Address) {
    emit(
        env,
        symbol_short!("AC_ACTION_PROPOSED"),
        (proposal_id, proposer.clone(), env.ledger().timestamp()),
    );
}

/// Schema: (proposal_id, actor=approver, approval_count, timestamp)
pub fn action_approved(env: &Env, proposal_id: u64, approver: &Address, approval_count: u32) {
    emit(
        env,
        symbol_short!("AC_ACTION_APPROVED"),
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
        symbol_short!("AC_ACTION_EXECUTED"),
        (proposal_id, executor.clone(), env.ledger().timestamp()),
    );
}

// ── Refund Events ────────────────────────────────────────────────────────────

/// Schema: (actor=investor, invoice_id, amount, timestamp)
pub fn refund_claimed(env: &Env, invoice_id: u64, investor: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("PROTOCOL_REFUND_CLAIMED"),
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
