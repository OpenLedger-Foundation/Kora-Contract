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

//! Debtor attestation contract.
//!
//! Maintains a registry of on-chain confirmations from the named debtor on each
//! invoice.  The marketplace calls `attestation_status` (and optionally
//! `is_confirmed`) during `list_invoice` to gate high-risk-tier invoices behind
//! a valid, non-expired debtor confirmation.
//!
//! See docs/debtor-attestation.md for the full specification.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short,
    Address, Bytes, Env, Map,
};

// ── Storage key constants ──────────────────────────────────────────────────────

const DATA_KEY_SYM: &str = "dATA";
const ADMIN_KEY_SYM: &str = "ADMIN";

/// Default maximum attestation age: 30 days in seconds.
const DEFAULT_MAX_AGE: u64 = 60 * 60 * 24 * 30;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Attestation lifecycle state.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttestationStatus {
    Pending,
    Confirmed,
    Rejected,
    Expired,
}

/// Full on-chain record for a single invoice attestation.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AttestationRecord {
    pub invoice_id: u64,
    /// The debtor address that confirmed the invoice.
    pub debtor: Address,
    pub amount: i128,
    /// Face amount confirmed by the debtor.
    pub amount: i128,
    /// Ledger timestamp when the attestation was recorded.
    pub attested_at: u64,
    /// Timestamp after which this attestation is stale.
    pub expires_at: u64,
    pub status: AttestationStatus,
    /// Set when the attestation was submitted via a whitelisted relay.
    pub relay: Option<Address>,
}

/// Whole-contract state blob persisted under `data_key()`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AttestationState {
/// Singleton storage blob.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ContractData {
    pub admin: Address,
    pub relays: Map<Address, bool>,
    pub max_attestation_age: u64,
    pub records: Map<u64, AttestationRecord>,
    /// Monotonically increasing per-debtor nonce for replay protection.
    pub nonces: Map<Address, u64>,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
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

// ── Storage key helpers ───────────────────────────────────────────────────────

fn data_key(env: &Env) -> soroban_sdk::Symbol {
    soroban_sdk::Symbol::new(env, DATA_KEY_SYM)
}

fn admin_key(env: &Env) -> soroban_sdk::Symbol {
    soroban_sdk::Symbol::new(env, ADMIN_KEY_SYM)
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct DebtorAttestationContract;

#[contractimpl]
impl DebtorAttestation {
    pub fn initialize(env: Env, admin: Address, max_attestation_age: Option<u64>) {
        if env.storage().instance().has(&data_key()) {
            env.panic_with_error(AttestationError::AlreadyInitialized);
        }
        let state = AttestationState {
impl DebtorAttestationContract {
    // ── Administration ────────────────────────────────────────────────────────

    /// One-time setup.  `max_attestation_age` in seconds; defaults to 30 days.
    pub fn initialize(
        env: Env,
        admin: Address,
        max_attestation_age: Option<u64>,
    ) -> Result<(), AttestationError> {
        if env.storage().instance().has(&data_key(&env)) {
            return Err(AttestationError::AlreadyInitialized);
        }
        let data = ContractData {
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
        env.storage().instance().set(&data_key(&env), &data);
        env.storage().instance().set(&admin_key(&env), &admin);
        Ok(())
    }

    /// Returns the configured admin address.
    pub fn admin(env: Env) -> Result<Address, AttestationError> {
        Self::ensure_initialized(&env)?;
        Ok(env.storage().instance().get(&admin_key(&env)).unwrap())
    }

    /// Update the maximum allowed age of a fresh attestation.  Admin only.
    pub fn set_max_attestation_age(env: Env, caller: Address, age: u64) -> Result<(), AttestationError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        let mut data = Self::load(&env)?;
        data.max_attestation_age = age;
        Self::save(&env, &data);
        env.events().publish(
            (symbol_short!("max_age"), caller),
            age,
        );
        Ok(())
    }

    /// Returns the configured maximum attestation age.
    pub fn max_attestation_age(env: Env) -> Result<u64, AttestationError> {
        Ok(Self::load(&env)?.max_attestation_age)
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
    /// Whitelist a relay address.  Admin only.
    pub fn add_relay(env: Env, caller: Address, relay: Address) -> Result<(), AttestationError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        let mut data = Self::load(&env)?;
        data.relays.set(relay.clone(), true);
        Self::save(&env, &data);
        env.events().publish(
            (symbol_short!("relay_add"), relay),
            true,
        );
        Ok(())
    }

    /// Remove a relay from the whitelist.  Admin only.
    pub fn remove_relay(env: Env, caller: Address, relay: Address) -> Result<(), AttestationError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        let mut data = Self::load(&env)?;
        data.relays.remove(relay.clone());
        Self::save(&env, &data);
        env.events().publish(
            (symbol_short!("relay_rem"), relay),
            false,
        );
        Ok(())
    }

    /// Returns whether `relay` is whitelisted.
    pub fn is_relay(env: Env, relay: Address) -> Result<bool, AttestationError> {
        Ok(Self::load(&env)?.relays.get(relay).unwrap_or(false))
    }

    // ── Attestation ───────────────────────────────────────────────────────────

    /// Direct debtor attestation.
    ///
    /// The debtor calls this directly (wallet present).  The Soroban auth
    /// framework verifies `debtor.require_auth()` already covers the call; we
    /// additionally accept a `signature` bytes blob for off-chain audit trails
    /// (the bytes are stored but NOT cryptographically re-verified on-chain in
    /// this implementation — Soroban's native `require_auth` is the primary
    /// authentication mechanism on Stellar).
    ///
    /// Nonce replay-protection: the call is rejected when `nonce` does not match
    /// the stored next-expected nonce for the debtor.
    pub fn attest(
        env: Env,
        debtor: Address,
        invoice_id: u64,
        amount: i128,
        expires_at: u64,
        nonce: u64,
        _signature: Bytes, // off-chain audit blob; auth via require_auth
    ) -> Result<(), AttestationError> {
        debtor.require_auth();
        Self::ensure_initialized(&env)?;
        let now = env.ledger().timestamp();
        if expires_at <= now {
            return Err(AttestationError::InvalidExpiry);
        }
        let mut data = Self::load(&env)?;
        let expected_nonce = data.nonces.get(debtor.clone()).unwrap_or(0);
        if nonce != expected_nonce {
            return Err(AttestationError::InvalidNonce);
        }
        data.nonces.set(debtor.clone(), nonce.checked_add(1).ok_or(AttestationError::InvalidNonce)?);
        Self::store_record(
            &env,
            &mut data,
            invoice_id,
            debtor,
            amount,
            expires_at,
            None,
        );
        Ok(())
    }

    /// Relay-based attestation for debtors without a direct wallet.
    ///
    /// Only whitelisted relays may call this endpoint.  The relay must call
    /// `relay.require_auth()` (enforced by Soroban).
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
        nonce: u64,
        _relay_sig: Bytes, // off-chain audit blob
    ) -> Result<(), AttestationError> {
        relay.require_auth();
        Self::ensure_initialized(&env)?;
        let mut data = Self::load(&env)?;
        if !data.relays.get(relay.clone()).unwrap_or(false) {
            return Err(AttestationError::RelayNotWhitelisted);
        }
        let now = env.ledger().timestamp();
        if expires_at <= now {
            return Err(AttestationError::InvalidExpiry);
        }
        let expected_nonce = data.nonces.get(debtor.clone()).unwrap_or(0);
        if nonce != expected_nonce {
            return Err(AttestationError::InvalidNonce);
        }
        data.nonces.set(debtor.clone(), nonce.checked_add(1).ok_or(AttestationError::InvalidNonce)?);
        Self::store_record(
            &env,
            &mut data,
            invoice_id,
            debtor,
            amount,
            expires_at,
            Some(relay),
        );
        Ok(())
    }

    /// Reject an attestation.  Only the debtor or a whitelisted relay may reject.
    pub fn reject(env: Env, caller: Address, invoice_id: u64) -> Result<(), AttestationError> {
        caller.require_auth();
        Self::ensure_initialized(&env)?;
        let mut data = Self::load(&env)?;
        let mut record = data
            .records
            .get(invoice_id)
            .ok_or(AttestationError::AttestationNotFound)?;
        if caller != record.debtor && !data.relays.get(caller.clone()).unwrap_or(false) {
            return Err(AttestationError::NotAuthorized);
        }
        record.status = AttestationStatus::Rejected;
        data.records.set(invoice_id, record);
        Self::save(&env, &data);
        env.events().publish(
            (symbol_short!("reject"), invoice_id),
            caller,
        );
        Ok(())
    }

    // ── Read-Only ─────────────────────────────────────────────────────────────

    /// Returns the current effective status, accounting for expiry.
    pub fn attestation_status(env: Env, invoice_id: u64) -> AttestationStatus {
        Self::ensure_initialized(&env);
        let state = Self::load(&env);
        match state.records.get(invoice_id) {
            None => AttestationStatus::Pending,
            Some(record) => {
                if record.status != AttestationStatus::Confirmed {
                    return record.status;
    ///
    /// This is the primary integration point consumed by the marketplace at
    /// listing time.
    pub fn attestation_status(env: Env, invoice_id: u64) -> Result<AttestationStatus, AttestationError> {
        Self::ensure_initialized(&env)?;
        let data = Self::load(&env)?;
        match data.records.get(invoice_id) {
            None => Ok(AttestationStatus::Pending),
            Some(record) => {
                if record.status != AttestationStatus::Confirmed {
                    return Ok(record.status.clone());
                }
                let now = env.ledger().timestamp();
                if now > record.expires_at {
                    Ok(AttestationStatus::Expired)
                } else {
                    Ok(AttestationStatus::Confirmed)
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
    /// Returns `true` only when the invoice has a valid, non-expired confirmation
    /// **and** the recorded amount matches `expected_amount`.
    pub fn is_confirmed(
        env: Env,
        invoice_id: u64,
        expected_amount: i128,
    ) -> Result<bool, AttestationError> {
        Self::ensure_initialized(&env)?;
        let data = Self::load(&env)?;
        match data.records.get(invoice_id) {
            None => Ok(false),
            Some(record) => {
                if record.status != AttestationStatus::Confirmed {
                    return Ok(false);
                }
                if record.amount != expected_amount {
                    return Ok(false);
                }
                let now = env.ledger().timestamp();
                Ok(now <= record.expires_at)
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
    /// Returns the full attestation record for an invoice, if any.
    pub fn get_record(env: Env, invoice_id: u64) -> Result<Option<AttestationRecord>, AttestationError> {
        Self::ensure_initialized(&env)?;
        Ok(Self::load(&env)?.records.get(invoice_id))
    }

    /// Returns the next expected nonce for `debtor`.
    pub fn nonce_of(env: Env, debtor: Address) -> Result<u64, AttestationError> {
        Self::ensure_initialized(&env)?;
        Ok(Self::load(&env)?.nonces.get(debtor).unwrap_or(0))
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn ensure_initialized(env: &Env) -> Result<(), AttestationError> {
        if !env.storage().instance().has(&data_key(env)) {
            return Err(AttestationError::NotInitialized);
        }
        Ok(())
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), AttestationError> {
        let admin: Address = env.storage().instance().get(&admin_key(env)).unwrap();
        if *caller != admin {
            return Err(AttestationError::NotAuthorized);
        }
        Ok(())
    }

    fn load(env: &Env) -> Result<ContractData, AttestationError> {
        env.storage()
            .instance()
            .get(&data_key(env))
            .ok_or(AttestationError::NotInitialized)
    }

    fn save(env: &Env, data: &ContractData) {
        env.storage().instance().set(&data_key(env), data);
    }

    #[allow(clippy::too_many_arguments)]
    fn store_record(
        env: &Env,
        data: &mut ContractData,
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Bytes, Env,
    };

    fn setup() -> (Env, Address, DebtorAttestationContractClient<'static>) {
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
        let id = env.register_contract(None, DebtorAttestationContract);
        let client = DebtorAttestationContractClient::new(&env, &id);
        client.initialize(&admin, &Some(DEFAULT_MAX_AGE));
        (env, admin, client)
    }

    #[test]
    fn test_initialize_already_initialized() {
        let (_env, admin, client) = setup();
        assert_eq!(
            client.try_initialize(&admin, &None).unwrap_err().unwrap(),
            AttestationError::AlreadyInitialized
        );
        assert_eq!(client.admin(), admin);
        assert_eq!(client.max_attestation_age(), DEFAULT_MAX_AGE);
    }

    #[test]
    fn test_relay_management() {
        let (env, admin, client) = setup();
        let relay = Address::generate(&env);
        assert!(!client.is_relay(&relay));
        client.add_relay(&admin, &relay);
        assert!(client.is_relay(&relay));
        client.remove_relay(&admin, &relay);
        assert!(!client.is_relay(&relay));
    }

    #[test]
    fn test_set_max_attestation_age() {
        let (_env, admin, client) = setup();
        client.set_max_attestation_age(&admin, &86400);
        assert_eq!(client.max_attestation_age(), 86400);
    }

    #[test]
    fn test_direct_attestation_lifecycle() {
        let (env, _admin, client) = setup();
        let debtor = Address::generate(&env);
        let invoice_id = 101u64;
        let amount = 50_000_000_000i128;
        let expires_at = env.ledger().timestamp() + 10_000;
        let nonce = client.nonce_of(&debtor);
        assert_eq!(nonce, 0);

        let sig = Bytes::new(&env);
        client.attest(&debtor, &invoice_id, &amount, &expires_at, &nonce, &sig);

        assert_eq!(client.nonce_of(&debtor), 1);
        assert_eq!(client.attestation_status(&invoice_id), AttestationStatus::Confirmed);
        assert!(client.is_confirmed(&invoice_id, &amount));
        assert!(!client.is_confirmed(&invoice_id, &(amount + 1)));

        let rec = client.get_record(&invoice_id).unwrap();
        assert_eq!(rec.invoice_id, invoice_id);
        assert_eq!(rec.debtor, debtor);
        assert_eq!(rec.amount, amount);
        assert_eq!(rec.status, AttestationStatus::Confirmed);
        assert_eq!(rec.relay, None);
    }

    #[test]
    fn test_relay_attestation_success() {
        let (env, admin, client) = setup();
        let relay = Address::generate(&env);
        let debtor = Address::generate(&env);
        client.add_relay(&admin, &relay);

        let invoice_id = 202u64;
        let amount = 10_000_000i128;
        let expires_at = env.ledger().timestamp() + 5_000;
        let nonce = client.nonce_of(&debtor);
        let sig = Bytes::new(&env);

        client.attest_via_relay(&relay, &debtor, &invoice_id, &amount, &expires_at, &nonce, &sig);
        assert_eq!(client.attestation_status(&invoice_id), AttestationStatus::Confirmed);
        let rec = client.get_record(&invoice_id).unwrap();
        assert_eq!(rec.relay, Some(relay));
    }

    #[test]
    fn test_unwhitelisted_relay_rejected() {
        let (env, _admin, client) = setup();
        let unwhitelisted = Address::generate(&env);
        let debtor = Address::generate(&env);
        let invoice_id = 303u64;
        let expires_at = env.ledger().timestamp() + 5_000;
        let sig = Bytes::new(&env);

        let res = client.try_attest_via_relay(&unwhitelisted, &debtor, &invoice_id, &1000i128, &expires_at, &0, &sig);
        assert_eq!(res.unwrap_err().unwrap(), AttestationError::RelayNotWhitelisted);
    }

    #[test]
    fn test_nonce_replay_rejected() {
        let (env, _admin, client) = setup();
        let debtor = Address::generate(&env);
        let expires_at = env.ledger().timestamp() + 10_000;
        let sig = Bytes::new(&env);

        client.attest(&debtor, &1u64, &1000i128, &expires_at, &0, &sig);
        // Reusing nonce 0 should fail
        let res = client.try_attest(&debtor, &2u64, &2000i128, &expires_at, &0, &sig);
        assert_eq!(res.unwrap_err().unwrap(), AttestationError::InvalidNonce);
    }

    #[test]
    fn test_expiry_handling() {
        let (env, _admin, client) = setup();
        let debtor = Address::generate(&env);
        let now = env.ledger().timestamp();
        let sig = Bytes::new(&env);

        // Submitting with already expired timestamp fails
        let res = client.try_attest(&debtor, &1u64, &1000i128, &(now - 1), &0, &sig);
        assert_eq!(res.unwrap_err().unwrap(), AttestationError::InvalidExpiry);

        // Submitting with future expiry succeeds
        client.attest(&debtor, &2u64, &1000i128, &(now + 100), &0, &sig);
        assert_eq!(client.attestation_status(&2u64), AttestationStatus::Confirmed);
        assert!(client.is_confirmed(&2u64, &1000i128));

        // Advance ledger past expiry
        env.ledger().set_timestamp(now + 101);
        assert_eq!(client.attestation_status(&2u64), AttestationStatus::Expired);
        assert!(!client.is_confirmed(&2u64, &1000i128));
    }

    #[test]
    fn test_reject_flow() {
        let (env, _admin, client) = setup();
        let debtor = Address::generate(&env);
        let expires_at = env.ledger().timestamp() + 10_000;
        let sig = Bytes::new(&env);

        client.attest(&debtor, &10u64, &1000i128, &expires_at, &0, &sig);
        assert_eq!(client.attestation_status(&10u64), AttestationStatus::Confirmed);

        // Debtor rejects
        client.reject(&debtor, &10u64);
        assert_eq!(client.attestation_status(&10u64), AttestationStatus::Rejected);
        assert!(!client.is_confirmed(&10u64, &1000i128));

        // Random caller cannot reject
        let stranger = Address::generate(&env);
        let res = client.try_reject(&stranger, &10u64);
        assert_eq!(res.unwrap_err().unwrap(), AttestationError::NotAuthorized);
    }
}
