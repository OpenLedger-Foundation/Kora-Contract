#![no_std]

use kora_shared::{
    errors::KoraError,
    events,
    reentrancy::ReentrancyGuard,
    types::SmeProfile,
    validation::{require_non_empty_bytes, require_valid_risk_score},
};
use soroban_sdk::{contract, contractimpl, contracttype, Address, Bytes, Env};

// ── TTL constants (in ledgers; ~5s per ledger on Stellar) ────────────────────
/// ~30 days worth of ledgers for persistent SME/verifier data
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

// ── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    InvoiceNft, // authorized caller for increment_invoice_count
    Verifier(Address),
    SmeProfile(Address),
    DebtorScore(Bytes), // keyed by debtor_hash (SHA-256 of PII)
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RiskRegistryContract;

#[contractimpl]
impl RiskRegistryContract {
    /// One-time initialization. Sets admin and the authorized invoice_nft address.
    pub fn initialize(env: Env, admin: Address, invoice_nft: Address) -> Result<(), KoraError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::InvoiceNft, &invoice_nft);
        events::registry_initialized(&env, &admin, &invoice_nft);
        Ok(())
    }

    /// Transfer admin role to a new address. Current admin only.
    pub fn transfer_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, &new_admin);
        Ok(())
    }

    // ── Verifier management ───────────────────────────────────────────────────

    /// Admin adds a trusted verifier.
    pub fn add_verifier(env: Env, admin: Address, verifier: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .set(&DataKey::Verifier(verifier.clone()), &true);
        Self::bump_persistent(&env, &DataKey::Verifier(verifier.clone()));
        events::verifier_added(&env, &admin, &verifier);
        Ok(())
    }

    /// Admin removes a verifier. Uses remove() to reclaim storage.
    pub fn remove_verifier(env: Env, admin: Address, verifier: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        // Only remove if it actually exists — avoids a no-op silently succeeding
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::Verifier(verifier.clone()))
            .unwrap_or(false)
        {
            return Err(KoraError::NotVerifier);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::Verifier(verifier.clone()));
        events::verifier_removed(&env, &admin, &verifier);
        Ok(())
    }

    // ── SME management ────────────────────────────────────────────────────────

    /// Verifier registers and scores an SME. Fails if SME is already registered.
    pub fn register_sme(
        env: Env,
        verifier: Address,
        sme: Address,
        risk_score: u32,
    ) -> Result<(), KoraError> {
        verifier.require_auth();
        Self::require_verifier(&env, &verifier)?;
        require_valid_risk_score(risk_score)?;

        // Guard against silent re-registration that would reset defaults/invoice counts
        if env
            .storage()
            .persistent()
            .has(&DataKey::SmeProfile(sme.clone()))
        {
            return Err(KoraError::AlreadyInitialized);
        }

        let profile = SmeProfile {
            address: sme.clone(),
            verified: true,
            verifier: verifier.clone(),
            risk_score,
            total_invoices: 0,
            defaults: 0,
            registered_at: env.ledger().timestamp(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_registered(&env, &verifier, &sme, risk_score);
        Ok(())
    }

    /// Update SME risk score. Verifier only.
    pub fn update_sme_score(
        env: Env,
        verifier: Address,
        sme: Address,
        new_score: u32,
    ) -> Result<(), KoraError> {
        verifier.require_auth();
        Self::require_verifier(&env, &verifier)?;
        require_valid_risk_score(new_score)?;

        let _guard = ReentrancyGuard::new(&env)?;

        let mut profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&DataKey::SmeProfile(sme.clone()))
            .ok_or(KoraError::SMENotRegistered)?;

        profile.risk_score = new_score;
        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_score_updated(&env, &verifier, &sme, new_score);
        Ok(())
    }

    /// Increment invoice count for an SME.
    /// Restricted to the invoice_nft contract address set at initialization.
    pub fn increment_invoice_count(
        env: Env,
        caller: Address,
        sme: Address,
    ) -> Result<(), KoraError> {
        caller.require_auth();
        Self::require_invoice_nft(&env, &caller)?;

        let mut profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&DataKey::SmeProfile(sme.clone()))
            .ok_or(KoraError::SMENotRegistered)?;

        profile.total_invoices = profile
            .total_invoices
            .checked_add(1)
            .ok_or(KoraError::ArithmeticOverflow)?;

        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_invoice_count_incremented(&env, &sme, profile.total_invoices);
        Ok(())
    }

    /// Record a default against an SME. Admin only.
    pub fn record_default(env: Env, admin: Address, sme: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let _guard = ReentrancyGuard::new(&env)?;

        let mut profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&DataKey::SmeProfile(sme.clone()))
            .ok_or(KoraError::SMENotRegistered)?;

        profile.defaults = profile
            .defaults
            .checked_add(1)
            .ok_or(KoraError::ArithmeticOverflow)?;

        env.storage()
            .persistent()
            .set(&DataKey::SmeProfile(sme.clone()), &profile);
        Self::bump_persistent(&env, &DataKey::SmeProfile(sme.clone()));
        events::sme_default_recorded(&env, &admin, &sme, profile.defaults);
        Ok(())
    }

    /// Store a debtor risk score keyed by debtor hash. Verifier only.
    pub fn set_debtor_score(
        env: Env,
        verifier: Address,
        debtor_hash: Bytes,
        score: u32,
    ) -> Result<(), KoraError> {
        verifier.require_auth();
        Self::require_verifier(&env, &verifier)?;
        // Validate bytes before score — returns EmptyBytes for empty hash
        require_non_empty_bytes(&debtor_hash)?;
        require_valid_risk_score(score)?;
        env.storage()
            .persistent()
            .set(&DataKey::DebtorScore(debtor_hash.clone()), &score);
        Self::bump_persistent(&env, &DataKey::DebtorScore(debtor_hash.clone()));
        events::debtor_score_set(&env, &verifier, &debtor_hash, score);
        Ok(())
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_sme_profile(env: Env, sme: Address) -> Result<SmeProfile, KoraError> {
        let key = DataKey::SmeProfile(sme);
        let profile: SmeProfile = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(KoraError::SMENotRegistered)?;
        Self::bump_persistent(&env, &key);
        Ok(profile)
    }

    pub fn is_verified_sme(env: Env, sme: Address) -> bool {
        env.storage()
            .persistent()
            .get::<DataKey, SmeProfile>(&DataKey::SmeProfile(sme))
            .map(|p| p.verified)
            .unwrap_or(false)
    }

    pub fn is_verifier(env: Env, verifier: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Verifier(verifier))
            .unwrap_or(false)
    }

    /// Returns the debtor score or `KoraError::DebtorNotRegistered` if not found.
    pub fn get_debtor_score(env: Env, debtor_hash: Bytes) -> Result<u32, KoraError> {
        let key = DataKey::DebtorScore(debtor_hash);
        let score: u32 = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(KoraError::DebtorNotRegistered)?;
        Self::bump_persistent(&env, &key);
        Ok(score)
    }

    pub fn get_admin(env: Env) -> Result<Address, KoraError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(KoraError::NotInitialized)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

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

    fn require_verifier(env: &Env, caller: &Address) -> Result<(), KoraError> {
        let ok: bool = env
            .storage()
            .persistent()
            .get(&DataKey::Verifier(caller.clone()))
            .unwrap_or(false);
        if !ok {
            return Err(KoraError::NotVerifier);
        }
        Ok(())
    }

    fn require_invoice_nft(env: &Env, caller: &Address) -> Result<(), KoraError> {
        let invoice_nft: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(KoraError::NotInitialized)?;
        if &invoice_nft != caller {
            return Err(KoraError::Unauthorized);
        }
        Ok(())
    }

    /// Extend TTL on a persistent entry if it's below the threshold.
    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_BUMP);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Bytes, Env};

    /// Returns (env, admin, invoice_nft, client)
    fn setup() -> (Env, Address, Address, RiskRegistryContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RiskRegistryContract);
        let client = RiskRegistryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let invoice_nft = Address::generate(&env);
        client.initialize(&admin, &invoice_nft).unwrap();
        (env, admin, invoice_nft, client)
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_success() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RiskRegistryContract);
        let client = RiskRegistryContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let invoice_nft = Address::generate(&env);
        assert!(client.try_initialize(&admin, &invoice_nft).is_ok());
    }

    #[test]
    fn test_initialize_already_initialized() {
        let (env, admin, invoice_nft, client) = setup();
        assert!(client.try_initialize(&admin, &invoice_nft).is_err());
    }

    // ── transfer_admin ────────────────────────────────────────────────────────

    #[test]
    fn test_transfer_admin_success() {
        let (env, admin, _, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin).unwrap();
        assert_eq!(client.get_admin().unwrap(), new_admin);
    }

    #[test]
    fn test_transfer_admin_requires_admin() {
        let (env, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        assert!(client.try_transfer_admin(&stranger, &new_admin).is_err());
    }

    // ── add_verifier / remove_verifier ────────────────────────────────────────

    #[test]
    fn test_add_verifier_success() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        assert!(client.try_add_verifier(&admin, &verifier).is_ok());
        assert!(client.is_verifier(&verifier));
    }

    #[test]
    fn test_add_verifier_not_admin() {
        let (env, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let verifier = Address::generate(&env);
        assert!(client.try_add_verifier(&stranger, &verifier).is_err());
    }

    #[test]
    fn test_remove_verifier_success() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        assert!(client.is_verifier(&verifier));
        assert!(client.try_remove_verifier(&admin, &verifier).is_ok());
        assert!(!client.is_verifier(&verifier));
    }

    #[test]
    fn test_remove_verifier_not_admin() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let stranger = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        assert!(client.try_remove_verifier(&stranger, &verifier).is_err());
    }

    #[test]
    fn test_remove_verifier_not_registered() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        assert!(client.try_remove_verifier(&admin, &verifier).is_err());
    }

    #[test]
    fn test_multiple_verifiers() {
        let (env, admin, _, client) = setup();
        let v1 = Address::generate(&env);
        let v2 = Address::generate(&env);
        let sme1 = Address::generate(&env);
        let sme2 = Address::generate(&env);
        client.add_verifier(&admin, &v1).unwrap();
        client.add_verifier(&admin, &v2).unwrap();
        client.register_sme(&v1, &sme1, &30u32).unwrap();
        client.register_sme(&v2, &sme2, &60u32).unwrap();
        assert_eq!(client.get_sme_profile(&sme1).unwrap().risk_score, 30);
        assert_eq!(client.get_sme_profile(&sme2).unwrap().risk_score, 60);
        assert_eq!(client.get_sme_profile(&sme1).unwrap().verifier, v1);
        assert_eq!(client.get_sme_profile(&sme2).unwrap().verifier, v2);
    }

    // ── register_sme ──────────────────────────────────────────────────────────

    #[test]
    fn test_register_sme_flow() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        assert!(client.is_verified_sme(&sme));
        let profile = client.get_sme_profile(&sme).unwrap();
        assert_eq!(profile.risk_score, 35);
        assert_eq!(profile.defaults, 0);
        assert_eq!(profile.total_invoices, 0);
        assert!(profile.verified);
        assert_eq!(profile.verifier, verifier);
    }

    #[test]
    fn test_register_sme_duplicate_rejected() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        assert!(client.try_register_sme(&verifier, &sme, &50u32).is_err());
    }

    #[test]
    fn test_register_sme_unverified_verifier() {
        let (env, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let sme = Address::generate(&env);
        assert!(client.try_register_sme(&stranger, &sme, &10u32).is_err());
    }

    #[test]
    fn test_register_sme_invalid_risk_score() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        assert!(client.try_register_sme(&verifier, &sme, &101u32).is_err());
    }

    #[test]
    fn test_register_sme_preserves_history_on_re_registration_attempt() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        let _ = client.try_register_sme(&verifier, &sme, &99u32);
        let profile = client.get_sme_profile(&sme).unwrap();
        assert_eq!(profile.risk_score, 35); // unchanged
    }

    // ── update_sme_score ──────────────────────────────────────────────────────

    #[test]
    fn test_update_sme_score_success() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        client.update_sme_score(&verifier, &sme, &50u32).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().risk_score, 50);
    }

    #[test]
    fn test_update_sme_score_not_registered() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        assert!(client.try_update_sme_score(&verifier, &sme, &50u32).is_err());
    }

    #[test]
    fn test_update_sme_score_invalid() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        assert!(client.try_update_sme_score(&verifier, &sme, &101u32).is_err());
    }

    #[test]
    fn test_update_sme_score_boundary_values() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &50u32).unwrap();
        client.update_sme_score(&verifier, &sme, &0u32).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().risk_score, 0);
        client.update_sme_score(&verifier, &sme, &100u32).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().risk_score, 100);
    }

    // ── increment_invoice_count ───────────────────────────────────────────────

    #[test]
    fn test_increment_invoice_count() {
        let (env, admin, invoice_nft, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().total_invoices, 0);
        client.increment_invoice_count(&invoice_nft, &sme).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().total_invoices, 1);
    }

    #[test]
    fn test_increment_invoice_count_multiple() {
        let (env, admin, invoice_nft, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        for i in 1u32..=5 {
            client.increment_invoice_count(&invoice_nft, &sme).unwrap();
            assert_eq!(client.get_sme_profile(&sme).unwrap().total_invoices, i);
        }
    }

    #[test]
    fn test_increment_invoice_count_unauthorized_caller() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        let stranger = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        assert!(client.try_increment_invoice_count(&stranger, &sme).is_err());
    }

    #[test]
    fn test_increment_invoice_count_sme_not_registered() {
        let (env, _, invoice_nft, client) = setup();
        let sme = Address::generate(&env);
        assert!(client.try_increment_invoice_count(&invoice_nft, &sme).is_err());
    }

    // ── record_default ────────────────────────────────────────────────────────

    #[test]
    fn test_record_default() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().defaults, 0);
        client.record_default(&admin, &sme).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().defaults, 1);
    }

    #[test]
    fn test_record_default_not_admin() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        let stranger = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        assert!(client.try_record_default(&stranger, &sme).is_err());
    }

    #[test]
    fn test_record_default_sme_not_registered() {
        let (env, admin, _, client) = setup();
        let sme = Address::generate(&env);
        assert!(client.try_record_default(&admin, &sme).is_err());
    }

    #[test]
    fn test_record_multiple_defaults() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        client.record_default(&admin, &sme).unwrap();
        client.record_default(&admin, &sme).unwrap();
        client.record_default(&admin, &sme).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().defaults, 3);
    }

    // ── set_debtor_score / get_debtor_score ───────────────────────────────────

    #[test]
    fn test_set_debtor_score() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xABu8; 32]);
        client.add_verifier(&admin, &verifier).unwrap();
        client.set_debtor_score(&verifier, &debtor_hash, &45u32).unwrap();
        assert_eq!(client.get_debtor_score(&debtor_hash).unwrap(), 45u32);
    }

    #[test]
    fn test_set_debtor_score_invalid_score() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xABu8; 32]);
        client.add_verifier(&admin, &verifier).unwrap();
        assert!(client.try_set_debtor_score(&verifier, &debtor_hash, &101u32).is_err());
    }

    #[test]
    fn test_set_debtor_score_empty_hash() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let empty_hash = Bytes::from_slice(&env, &[]);
        client.add_verifier(&admin, &verifier).unwrap();
        assert!(client.try_set_debtor_score(&verifier, &empty_hash, &50u32).is_err());
    }

    #[test]
    fn test_set_debtor_score_not_verifier() {
        let (env, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let debtor_hash = Bytes::from_slice(&env, &[0xABu8; 32]);
        assert!(client.try_set_debtor_score(&stranger, &debtor_hash, &50u32).is_err());
    }

    #[test]
    fn test_get_debtor_score_not_found() {
        let (env, _, _, client) = setup();
        let debtor_hash = Bytes::from_slice(&env, &[0xCDu8; 32]);
        assert!(client.try_get_debtor_score(&debtor_hash).is_err());
    }

    #[test]
    fn test_debtor_score_boundary_values() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        let hash0 = Bytes::from_slice(&env, &[0x01u8; 32]);
        client.set_debtor_score(&verifier, &hash0, &0u32).unwrap();
        assert_eq!(client.get_debtor_score(&hash0).unwrap(), 0u32);
        let hash100 = Bytes::from_slice(&env, &[0x02u8; 32]);
        client.set_debtor_score(&verifier, &hash100, &100u32).unwrap();
        assert_eq!(client.get_debtor_score(&hash100).unwrap(), 100u32);
        let hash_invalid = Bytes::from_slice(&env, &[0x03u8; 32]);
        assert!(client.try_set_debtor_score(&verifier, &hash_invalid, &101u32).is_err());
    }

    // ── views ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_get_sme_profile_not_found() {
        let (env, _, _, client) = setup();
        let sme = Address::generate(&env);
        assert!(client.try_get_sme_profile(&sme).is_err());
    }

    #[test]
    fn test_is_verified_sme_false_for_unregistered() {
        let (env, _, _, client) = setup();
        let sme = Address::generate(&env);
        assert!(!client.is_verified_sme(&sme));
    }

    // ── risk score boundary ───────────────────────────────────────────────────

    #[test]
    fn test_risk_score_boundary_values() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        let sme0 = Address::generate(&env);
        client.register_sme(&verifier, &sme0, &0u32).unwrap();
        assert_eq!(client.get_sme_profile(&sme0).unwrap().risk_score, 0);
        let sme100 = Address::generate(&env);
        client.register_sme(&verifier, &sme100, &100u32).unwrap();
        assert_eq!(client.get_sme_profile(&sme100).unwrap().risk_score, 100);
        let sme_invalid = Address::generate(&env);
        assert!(client.try_register_sme(&verifier, &sme_invalid, &101u32).is_err());
    }

    // ── event emission ────────────────────────────────────────────────────────

    #[test]
    fn test_add_verifier_emits_event() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        assert!(!env.events().all().is_empty());
    }

    #[test]
    fn test_remove_verifier_emits_event() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        let events_before = env.events().all().len();
        client.remove_verifier(&admin, &verifier).unwrap();
        assert!(env.events().all().len() > events_before);
    }

    #[test]
    fn test_register_sme_emits_event() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        let events_before = env.events().all().len();
        client.register_sme(&verifier, &sme, &42u32).unwrap();
        assert!(env.events().all().len() > events_before);
    }

    #[test]
    fn test_increment_invoice_count_emits_event() {
        let (env, admin, invoice_nft, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &35u32).unwrap();
        let events_before = env.events().all().len();
        client.increment_invoice_count(&invoice_nft, &sme).unwrap();
        assert!(env.events().all().len() > events_before);
        assert_eq!(client.get_sme_profile(&sme).unwrap().total_invoices, 1);
    }

    #[test]
    fn test_failed_operations_do_not_emit_events() {
        let (env, _, _, client) = setup();
        let stranger = Address::generate(&env);
        let verifier = Address::generate(&env);
        let events_before = env.events().all().len();
        let _ = client.try_add_verifier(&stranger, &verifier);
        let _ = client.try_register_sme(&stranger, &verifier, &50u32);
        let _ = client.try_record_default(&stranger, &verifier);
        assert_eq!(env.events().all().len(), events_before);
    }

    #[test]
    fn test_sme_default_event_carries_cumulative_count() {
        let (env, admin, _, client) = setup();
        let verifier = Address::generate(&env);
        let sme = Address::generate(&env);
        client.add_verifier(&admin, &verifier).unwrap();
        client.register_sme(&verifier, &sme, &80u32).unwrap();
        client.record_default(&admin, &sme).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().defaults, 1);
        client.record_default(&admin, &sme).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().defaults, 2);
        client.record_default(&admin, &sme).unwrap();
        assert_eq!(client.get_sme_profile(&sme).unwrap().defaults, 3);
    }
}
