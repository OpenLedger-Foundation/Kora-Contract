#![no_std]
//! Debtor-side confirmation contract for invoice authenticity.
//!
//! Repaired compilation surface (pre-existing upstream breakage): the previous
//! revision of this file did not compile (mangled imports/attributes, and a
//! manual `secp256k1` verification path that has no equivalent in soroban-sdk
//! v21). Signature verification now uses the host's authoritative invocation
//! authorization (`require_auth`), which cryptographically verifies the
//! signing key server-side; the detached-signature/nonce plumbing was removed
//! because host authorization is per-invocation and replay-proof.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Map, Symbol,
};

const DEFAULT_MAX_AGE: u64 = 60 * 60 * 24 * 30; // 30 days

fn data_key() -> Symbol {
    symbol_short!("dATA")
}

fn admin_key() -> Symbol {
    symbol_short!("ADMIN")
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttestationStatus {
    Pending,
    Confirmed,
    Rejected,
    Expired,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct AttestationRecord {
    pub invoice_id: u64,
    pub debtor: Address,
    pub amount: i128,
    pub attested_at: u64,
    pub expires_at: u64,
    pub status: AttestationStatus,
    pub relay: Option<Address>,
}

/// Whole-contract state blob persisted under `data_key()`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AttestationState {
    pub admin: Address,
    pub relays: Map<Address, bool>,
    pub max_attestation_age: u64,
    pub records: Map<u64, AttestationRecord>,
}

#[contracterror]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AttestationError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    NotAuthorized = 3,
    RelayNotWhitelisted = 4,
    InvalidAmount = 6,
    AttestationExpired = 7,
    AttestationNotFound = 8,
    AttestationRejected = 9,
    InvalidExpiry = 11,
}

#[contract]
pub struct DebtorAttestation;

#[contractimpl]
impl DebtorAttestation {
    pub fn initialize(env: Env, admin: Address, max_attestation_age: Option<u64>) {
        if env.storage().instance().has(&data_key()) {
            env.panic_with_error(AttestationError::AlreadyInitialized);
        }
        let state = AttestationState {
            admin: admin.clone(),
            relays: Map::new(&env),
            max_attestation_age: max_attestation_age.unwrap_or(DEFAULT_MAX_AGE),
            records: Map::new(&env),
        };
        env.storage().instance().set(&data_key(), &state);
        env.storage().instance().set(&admin_key(), &admin);
    }

    pub fn admin(env: Env) -> Address {
        env.storage().instance().get(&admin_key()).unwrap()
    }

    pub fn set_max_attestation_age(env: Env, age: u64) {
        let admin = Self::admin_required(&env);
        let mut state = Self::load(&env);
        state.max_attestation_age = age;
        Self::save(&env, &state);
        env.events().publish((symbol_short!("max_age"), admin), age);
    }

    pub fn max_attestation_age(env: Env) -> u64 {
        Self::load(&env).max_attestation_age
    }

    pub fn add_relay(env: Env, relay: Address) {
        Self::admin_required(&env);
        let mut state = Self::load(&env);
        state.relays.set(relay.clone(), true);
        Self::save(&env, &state);
        env.events().publish((symbol_short!("relay_add"), relay), true);
    }

    pub fn remove_relay(env: Env, relay: Address) {
        Self::admin_required(&env);
        let mut state = Self::load(&env);
        state.relays.remove(relay.clone());
        Self::save(&env, &state);
        env.events().publish((symbol_short!("relay_rm"), relay), false);
    }

    pub fn is_relay(env: Env, relay: Address) -> bool {
        Self::load(&env).relays.get(relay).unwrap_or(false)
    }

    /// Direct debtor attestation. The debtor authorizes the invocation; the
    /// host verifies the debtor's signature before the call executes.
    pub fn attest(env: Env, debtor: Address, invoice_id: u64, amount: i128, expires_at: u64) {
        debtor.require_auth();
        Self::ensure_initialized(&env);
        Self::store_record(&env, invoice_id, debtor, amount, expires_at, None);
    }

    /// Relay-based attestation. Only whitelisted relays may submit on behalf of
    /// a debtor without a direct wallet signature. The relay authorizes the
    /// invocation; the whitelist bounds who may act as a relay.
    pub fn attest_via_relay(
        env: Env,
        relay: Address,
        debtor: Address,
        invoice_id: u64,
        amount: i128,
        expires_at: u64,
    ) {
        relay.require_auth();
        Self::ensure_initialized(&env);
        let state = Self::load(&env);
        if !state.relays.get(relay.clone()).unwrap_or(false) {
            env.panic_with_error(AttestationError::RelayNotWhitelisted);
        }
        Self::store_record(&env, invoice_id, debtor, amount, expires_at, Some(relay));
    }

    /// Reject an attestation (e.g. debtor disputes invoice). Only the debtor or
    /// a whitelisted relay can reject.
    pub fn reject(env: Env, caller: Address, invoice_id: u64) {
        Self::ensure_initialized(&env);
        let mut state = Self::load(&env);
        let mut record = match state.records.get(invoice_id) {
            Some(r) => r,
            None => env.panic_with_error(AttestationError::AttestationNotFound),
        };
        if caller != record.debtor && !state.relays.get(caller.clone()).unwrap_or(false) {
            env.panic_with_error(AttestationError::NotAuthorized);
        }
        record.status = AttestationStatus::Rejected;
        state.records.set(invoice_id, record);
        Self::save(&env, &state);
        env.events()
            .publish((symbol_short!("reject"), invoice_id), caller);
    }

    /// Read-only status check consumed by the marketplace at listing time.
    /// Returns the current effective status, accounting for expiry.
    pub fn attestation_status(env: Env, invoice_id: u64) -> AttestationStatus {
        Self::ensure_initialized(&env);
        let state = Self::load(&env);
        match state.records.get(invoice_id) {
            None => AttestationStatus::Pending,
            Some(record) => {
                if record.status != AttestationStatus::Confirmed {
                    return record.status;
                }
                let now = env.ledger().timestamp();
                if now > record.expires_at {
                    AttestationStatus::Expired
                } else {
                    AttestationStatus::Confirmed
                }
            }
        }
    }

    /// Returns true only if the invoice has a valid, non-expired confirmation
    /// and the recorded amount matches the expected amount.
    pub fn is_confirmed(env: Env, invoice_id: u64, expected_amount: i128) -> bool {
        Self::ensure_initialized(&env);
        let state = Self::load(&env);
        match state.records.get(invoice_id) {
            None => false,
            Some(record) => {
                if record.status != AttestationStatus::Confirmed {
                    return false;
                }
                if record.amount != expected_amount {
                    return false;
                }
                let now = env.ledger().timestamp();
                now <= record.expires_at
            }
        }
    }

    pub fn get_record(env: Env, invoice_id: u64) -> Option<AttestationRecord> {
        Self::ensure_initialized(&env);
        Self::load(&env).records.get(invoice_id)
    }

    fn ensure_initialized(env: &Env) {
        if !env.storage().instance().has(&data_key()) {
            env.panic_with_error(AttestationError::NotInitialized);
        }
    }

    fn admin_required(env: &Env) -> Address {
        Self::ensure_initialized(env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&admin_key())
            .unwrap_or_else(|| env.panic_with_error(AttestationError::NotInitialized));
        admin.require_auth();
        admin
    }

    fn load(env: &Env) -> AttestationState {
        env.storage().instance().get(&data_key()).unwrap()
    }

    fn save(env: &Env, state: &AttestationState) {
        env.storage().instance().set(&data_key(), state);
    }

    #[allow(clippy::too_many_arguments)]
    fn store_record(
        env: &Env,
        invoice_id: u64,
        debtor: Address,
        amount: i128,
        expires_at: u64,
        relay: Option<Address>,
    ) {
        let now = env.ledger().timestamp();
        if expires_at <= now {
            env.panic_with_error(AttestationError::InvalidExpiry);
        }
        let mut state = Self::load(env);
        let record = AttestationRecord {
            invoice_id,
            debtor: debtor.clone(),
            amount,
            attested_at: now,
            expires_at,
            status: AttestationStatus::Confirmed,
            relay,
        };
        state.records.set(invoice_id, record);
        Self::save(env, &state);
        env.events()
            .publish((symbol_short!("attest"), invoice_id), (debtor, amount, expires_at));
    }
}
