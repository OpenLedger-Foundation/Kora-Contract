#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

const DEFAULT_MAX_AGE: u64 = 60 * 60 * 24 * 30; // 30 days

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttestationStatus {
    Pending,
    Confirmed,
    Rejected,
    Expired,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttestationRecord {
    pub invoice_id: u64,
    pub debtor: Address,
    pub amount: u128,
    pub attested_at: u64,
    pub expires_at: u64,
    pub status: AttestationStatus,
    pub relay: Option<Address>,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Relay(Address),
    MaxAttestationAge,
    Record(u64),
    Nonce(Address),
    Initialized,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AttestationError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    NotAuthorized = 3,
    RelayNotWhitelisted = 4,
    InvalidSignature = 5,
    InvalidAmount = 6,
    AttestationExpired = 7,
    AttestationNotFound = 8,
    AttestationRejected = 9,
    InvalidNonce = 10,
    InvalidExpiry = 11,
}

#[contract]
pub struct DebtorAttestation;

#[contractimpl]
impl DebtorAttestation {
    pub fn initialize(env: Env, admin: Address, max_attestation_age: Option<u64>) {
        if env.storage().instance().has(&DataKey::Initialized) {
            env.panic_with_error(AttestationError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Initialized, &true);
        env.storage().instance().set(&DataKey::Admin, &admin);
        let max_age = max_attestation_age.unwrap_or(DEFAULT_MAX_AGE);
        env.storage().instance().set(&DataKey::MaxAttestationAge, &max_age);
    }

    pub fn admin(env: Env) -> Address {
        Self::ensure_initialized(&env);
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn set_max_attestation_age(env: Env, age: u64) {
        let admin = Self::admin_required(&env);
        env.storage().instance().set(&DataKey::MaxAttestationAge, &age);
        env.events().publish(
            (symbol_short!("max_age"), admin),
            age,
        );
    }

    pub fn max_attestation_age(env: Env) -> u64 {
        Self::ensure_initialized(&env);
        env.storage().instance().get(&DataKey::MaxAttestationAge).unwrap_or(DEFAULT_MAX_AGE)
    }

    pub fn add_relay(env: Env, relay: Address) {
        Self::admin_required(&env);
        env.storage().persistent().set(&DataKey::Relay(relay.clone()), &true);
        env.events().publish(
            (symbol_short!("relay_add"), relay),
            true,
        );
    }

    pub fn remove_relay(env: Env, relay: Address) {
        Self::admin_required(&env);
        env.storage().persistent().set(&DataKey::Relay(relay.clone()), &false);
        env.events().publish(
            (symbol_short!("refay_set"), relay),
            false,
        );
    }

    pub fn is_relay(env: Env, relay: Address) -> bool {
        Self::ensure_initialized(&env);
        env.storage().persistent().get(&DataKey::Relay(relay)).unwrap_or(false)
    }

    pub fn attest(
        env: Env,
        debtor: Address,
        invoice_id: u64,
        amount: u128,
        expires_at: u64,
        nonce: u64,
    ) {
        Self::ensure_initialized(&env);
        debtor.require_auth();

        let now = env.ledger().timestamp();
        if expires_at <= now {
            env.panic_with_error(AttestationError::InvalidExpiry);
        }

        let expected_nonce: u64 = env.storage().persistent().get(&DataKey::Nonce(debtor.clone())).unwrap_or(0);
        if nonce != expected_nonce {
            env.panic_with_error(AttestationError::InvalidNonce);
        }

        env.storage().persistent().set(&DataKey::Nonce(debtor.clone()), &(nonce + 1));
        Self::store_record(
            &env,
            invoice_id,
            debtor,
            amount,
            expires_at,
            None,
        );
    }

    pub fn attest_via_relay(
        env: Env,
        relay: Address,
        debtor: Address,
        invoice_id: u64,
        amount: u128,
        expires_at: u64,
        nonce: u64,
    ) {
        Self::ensure_initialized(&env);
        relay.require_auth();

        let is_whitelisted: bool = env.storage().persistent().get(&DataKey::Relay(relay.clone())).unwrap_or(false);
        if !is_whitelisted {
            env.panic_with_error(AttestationError::RelayNotWhitelisted);
        }

        let now = env.ledger().timestamp();
        if expires_at <= now {
            env.panic_with_error(AttestationError::InvalidExpiry);
        }

        let expected_nonce: u64 = env.storage().persistent().get(&DataKey::Nonce(debtor.clone())).unwrap_or(0);
        if nonce != expected_nonce {
            env.panic_with_error(AttestationError::InvalidNonce);
        }

        env.storage().persistent().set(&DataKey::Nonce(debtor.clone()), &(nonce + 1));
        Self::store_record(
            &env,
            invoice_id,
            debtor,
            amount,
            expires_at,
            Some(relay),
        );
    }

    pub fn reject(env: Env, caller: Address, invoice_id: u64) {
        Self::ensure_initialized(&env);
        caller.require_auth();

        let mut record: AttestationRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Record(invoice_id))
            .unwrap_or_else(|| env.panic_with_error(AttestationError::AttestationNotFound));

        let is_relay = env.storage().persistent().get(&DataKey::Relay(caller.clone())).unwrap_or(false);
        if caller != record.debtor && !is_relay {
            env.panic_with_error(AttestationError::NotAuthorized);
        }

        record.status = AttestationStatus::Rejected;
        env.storage().persistent().set(&DataKey::Record(invoice_id), &record);
        env.events().publish(
            (symbol_short!("reject"), invoice_id),
            caller,
        );
    }

    pub fn attestation_status(env: Env, invoice_id: u64) -> AttestationStatus {
        Self::ensure_initialized(&env);
        match env.storage().persistent().get::<DataKey, AttestationRecord>(&DataKey::Record(invoice_id)) {
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

    pub fn is_confirmed(env: Env, invoice_id: u64, expected_amount: u128) -> bool {
        Self::ensure_initialized(&env);
        match env.storage().persistent().get::<DataKey, AttestationRecord>(&DataKey::Record(invoice_id)) {
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
        env.storage().persistent().get(&DataKey::Record(invoice_id))
    }

    pub fn nonce_of(env: Env, debtor: Address) -> u64 {
        Self::ensure_initialized(&env);
        env.storage().persistent().get(&DataKey::Nonce(debtor)).unwrap_or(0)
    }

    fn ensure_initialized(env: &Env) {
        if !env.storage().instance().has(&DataKey::Initialized) {
            env.panic_with_error(AttestationError::NotInitialized);
        }
    }

    fn admin_required(env: &Env) -> Address {
        Self::ensure_initialized(env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        admin
    }

    fn store_record(
        env: &Env,
        invoice_id: u64,
        debtor: Address,
        amount: u128,
        expires_at: u64,
        relay: Option<Address>,
    ) {
        let now = env.ledger().timestamp();
        let record = AttestationRecord {
            invoice_id,
            debtor: debtor.clone(),
            amount,
            attested_at: now,
            expires_at,
            status: AttestationStatus::Confirmed,
            relay: relay.clone(),
        };
        env.storage().persistent().set(&DataKey::Record(invoice_id), &record);
        env.events().publish(
            (symbol_short!("attest"), invoice_id),
            (debtor, amount, expires_at),
        );
    }
}
