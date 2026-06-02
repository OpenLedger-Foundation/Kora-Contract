use soroban_sdk::{symbol_short, Address, Bytes, Env, Symbol};

fn emit(env: &Env, topic: Symbol, data: impl soroban_sdk::IntoVal<Env, soroban_sdk::Val>) {
    env.events().publish((topic,), data);
}

// ── Invoice Events ────────────────────────────────────────────────────────────

pub fn invoice_created(env: &Env, invoice_id: u64, sme: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("INV_CRT"),
        (invoice_id, sme.clone(), amount, env.ledger().timestamp()),
    );
}

/// Standardized marketplace event: invoice listed for financing.
/// Schema: topic, actor (seller), listing (invoice_id), amount (asking_price), ledger_seq (timestamp)
pub fn invoice_listed(env: &Env, invoice_id: u64, seller: &Address, asking_price: i128) {
    emit(
        env,
        symbol_short!("INV_LST"),
        (
            seller.clone(),
            invoice_id,
            asking_price,
            env.ledger().timestamp(),
        ),
    );
}

/// Standardized marketplace event: investor funded a listing.
/// Schema: topic, actor (investor), listing (invoice_id), amount (funded_amount), ledger_seq (timestamp)
pub fn invoice_funded(env: &Env, invoice_id: u64, investor: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("INV_FND"),
        (
            investor.clone(),
            invoice_id,
            amount,
            env.ledger().timestamp(),
        ),
    );
}

pub fn invoice_repaid(env: &Env, invoice_id: u64, sme: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("INV_RPD"),
        (invoice_id, sme.clone(), amount),
    );
}

pub fn invoice_defaulted(env: &Env, invoice_id: u64, sme: &Address) {
    emit(
        env,
        symbol_short!("INV_DFT"),
        (invoice_id, sme.clone(), env.ledger().timestamp()),
    );
}

// ── Repayment Events ──────────────────────────────────────────────────────────

pub fn repayment_made(env: &Env, invoice_id: u64, payer: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("REPAY"),
        (invoice_id, payer.clone(), amount),
    );
}

pub fn yield_distributed(env: &Env, invoice_id: u64, investor: &Address, yield_amount: i128) {
    emit(
        env,
        symbol_short!("YIELD"),
        (invoice_id, investor.clone(), yield_amount),
    );
}

// ── Marketplace Events ──────────────────────────────────────────────────────

pub fn listing_cancelled(env: &Env, invoice_id: u64, seller: &Address) {
    emit(
        env,
        symbol_short!("LST_CXL"),
        (invoice_id, seller.clone(), env.ledger().timestamp()),
    );
}

/// Standardized marketplace event: listing expired (funding deadline passed).
/// Schema: topic, actor (seller), listing (invoice_id), amount (0), ledger_seq (timestamp)
pub fn listing_expired(env: &Env, invoice_id: u64, seller: &Address) {
    emit(
        env,
        symbol_short!("LST_EXP"),
        (invoice_id, seller.clone(), env.ledger().timestamp()),
    );
}

// ── Fee Events ────────────────────────────────────────────────────────────────

/// Standardized marketplace event: fee collected from funding.
/// Schema: topic, listing (invoice_id), amount (fee_amount), token, ledger_seq (timestamp)
pub fn fee_collected(env: &Env, invoice_id: u64, fee_amount: i128, token: &Address) {
    emit(
        env,
        symbol_short!("FEE_COL"),
        (
            invoice_id,
            fee_amount,
            token.clone(),
            env.ledger().timestamp(),
        ),
    );
}

pub fn fee_withdrawn(env: &Env, token: &Address, amount: i128) {
    emit(env, symbol_short!("FEE_WTH"), (token.clone(), amount));
}

/// Emitted when the full token balance is drained via emergency_withdraw.
pub fn emergency_withdrawn(env: &Env, by: &Address, token: &Address, amount: i128) {
    emit(
        env,
        symbol_short!("EMRG_WTH"),
        (by.clone(), token.clone(), amount),
    );
}

/// Emitted when the protocol fee rate is updated.
pub fn fee_rate_updated(env: &Env, by: &Address, old_bps: u32, new_bps: u32) {
    emit(
        env,
        symbol_short!("FEE_UPD"),
        (by.clone(), old_bps, new_bps),
    );
}

/// Emitted when the treasury contract is initialized.
pub fn treasury_initialized(env: &Env, admin: &Address, fee_bps: u32) {
    emit(
        env,
        symbol_short!("TRES_INI"),
        (admin.clone(), fee_bps),
    );
}

// ── Protocol / Admin Events ───────────────────────────────────────────────────

pub fn protocol_paused(env: &Env, by: &Address) {
    emit(
        env,
        symbol_short!("PAUSED"),
        (by.clone(), env.ledger().timestamp()),
    );
}

pub fn protocol_unpaused(env: &Env, by: &Address) {
    emit(
        env,
        symbol_short!("UNPAUSED"),
        (by.clone(), env.ledger().timestamp()),
    );
}

pub fn token_whitelisted(env: &Env, token: &Address) {
    emit(env, symbol_short!("TOK_WL"), token.clone());
}

pub fn admin_transferred(env: &Env, new_admin: &Address) {
    emit(env, symbol_short!("ADM_TRF"), new_admin.clone());
}

pub fn role_granted(env: &Env, admin: &Address, target: &Address) {
    emit(
        env,
        symbol_short!("ROL_GRT"),
        (admin.clone(), target.clone()),
    );
}

pub fn role_revoked(env: &Env, admin: &Address, target: &Address) {
    emit(
        env,
        symbol_short!("ROL_RVK"),
        (admin.clone(), target.clone()),
    );
}

// ── Risk Registry Events ──────────────────────────────────────────────────────

/// Emitted when the admin whitelists a new verifier.
/// Payload: (admin, verifier, timestamp)
pub fn verifier_added(env: &Env, admin: &Address, verifier: &Address) {
    emit(
        env,
        symbol_short!("VRF_ADD"),
        (admin.clone(), verifier.clone()),
    );
}

/// Emitted when the admin removes a verifier.
/// Payload: (admin, verifier, timestamp)
pub fn verifier_removed(env: &Env, admin: &Address, verifier: &Address) {
    emit(
        env,
        symbol_short!("VRF_REM"),
        (admin.clone(), verifier.clone()),
    );
}

/// Emitted when a verifier registers a new SME profile.
/// Payload: (verifier, sme, risk_score, timestamp)
pub fn sme_registered(env: &Env, verifier: &Address, sme: &Address, risk_score: u32) {
    emit(
        env,
        symbol_short!("SME_REG"),
        (verifier.clone(), sme.clone(), risk_score),
    );
}

/// Emitted when a verifier updates an SME's risk score.
/// Payload: (verifier, sme, new_score, timestamp)
pub fn sme_score_updated(env: &Env, verifier: &Address, sme: &Address, new_score: u32) {
    emit(
        env,
        symbol_short!("SME_UPD"),
        (verifier.clone(), sme.clone(), new_score),
    );
}

/// Emitted when the admin records a default against an SME.
/// Payload: (admin, sme, total_defaults, timestamp)
pub fn sme_default_recorded(env: &Env, admin: &Address, sme: &Address, total_defaults: u32) {
    emit(
        env,
        symbol_short!("SME_DFT"),
        (admin.clone(), sme.clone(), total_defaults),
    );
}

/// Emitted when the invoice_nft contract increments an SME's invoice count.
/// Payload: (sme, new_total_invoices, timestamp)
pub fn sme_invoice_count_incremented(env: &Env, sme: &Address, new_total: u32) {
    emit(
        env,
        symbol_short!("SME_INV"),
        (sme.clone(), new_total, env.ledger().timestamp()),
    );
}

/// Emitted when a verifier sets or updates a debtor risk score.
/// Includes the debtor_hash so indexers can correlate the score to the debtor.
/// Payload: (verifier, debtor_hash, score, timestamp)
pub fn debtor_score_set(env: &Env, verifier: &Address, debtor_hash: &Bytes, score: u32) {
    emit(
        env,
        symbol_short!("DBT_SCR"),
        (verifier.clone(), debtor_hash.clone(), score, env.ledger().timestamp()),
    );
}

/// Emitted when the risk registry contract is initialized.
/// Payload: (admin, invoice_nft)
pub fn registry_initialized(env: &Env, admin: &Address, invoice_nft: &Address) {
    emit(
        env,
        symbol_short!("REG_INI"),
        (admin.clone(), invoice_nft.clone()),
    );
}

// AUDIT FIX: Removed duplicate sme_invoice_counted — use sme_invoice_count_incremented instead.
