//! Issue #1078 — authorized caller registry for risk_registry write paths.
//!
//! System score mutators (`increment_invoice_count`, `record_default`,
//! `record_successful_repayment`) accept their built-in caller (invoice_nft /
//! admin) or an address on the admin-managed authorized caller registry.
//! Verifier score mutators are scoped by the verifier registry. Unknown callers
//! are rejected with a typed error on every mutator.

#![cfg(test)]

use kora_access_control::{AccessControlContract, AccessControlContractClient};
use kora_risk_registry::{RiskRegistryContract, RiskRegistryContractClient, RiskRegistryError};
use soroban_sdk::{testutils::Address as _, Address, Bytes, Env};

struct Setup {
    env: Env,
    admin: Address,
    nft: Address,
    verifier: Address,
    sme: Address,
    rr: RiskRegistryContractClient<'static>,
}

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let admin = Address::generate(&env);
    let nft = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
    let staking_token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let rr =
        RiskRegistryContractClient::new(&env, &env.register_contract(None, RiskRegistryContract));
    rr.initialize(&admin, &nft, &staking_token, &0i128, &5_000u32);
    let verifier = Address::generate(&env);
    rr.add_verifier(&admin, &verifier, &0i128);
    let sme = Address::generate(&env);
    rr.register_sme(&verifier, &sme, &40u32, &true);
    Setup {
        env,
        admin,
        nft,
        verifier,
        sme,
        rr,
    }
}

fn debtor_hash(env: &Env) -> Bytes {
    Bytes::from_array(env, &[7u8; 32])
}

#[test]
fn unknown_caller_is_rejected_on_every_mutator() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    let unauthorized = Err(Ok(RiskRegistryError::Unauthorized));
    let not_verifier = Err(Ok(RiskRegistryError::NotVerifier));

    // System mutators — authorized caller registry.
    assert_eq!(
        s.rr.try_increment_invoice_count(&stranger, &s.sme),
        unauthorized
    );
    assert_eq!(s.rr.try_record_default(&stranger, &s.sme), unauthorized);
    assert_eq!(
        s.rr.try_record_successful_repayment(&stranger, &s.sme),
        unauthorized
    );

    // Verifier mutators — scoped by the verifier registry.
    assert_eq!(
        s.rr.try_register_sme(&stranger, &Address::generate(&s.env), &50u32, &true),
        not_verifier
    );
    assert_eq!(
        s.rr.try_update_sme_score(&stranger, &s.sme, &10u32),
        not_verifier
    );
    assert_eq!(
        s.rr.try_set_credit_limit(&stranger, &s.sme, &1_000i128),
        not_verifier
    );
    assert_eq!(
        s.rr.try_set_debtor_score(&stranger, &debtor_hash(&s.env), &60u32),
        not_verifier
    );

    let profile = s.rr.get_sme_profile(&s.sme);
    assert_eq!(
        (profile.total_invoices, profile.defaults, profile.risk_score),
        (0, 0, 40)
    );
}

#[test]
fn allowlisted_caller_succeeds_on_system_mutators() {
    let s = setup();
    let pool = Address::generate(&s.env);
    assert!(!s.rr.is_authorized_caller(&pool));

    s.rr.add_authorized_caller(&s.admin, &pool);
    assert!(s.rr.is_authorized_caller(&pool));

    s.rr.increment_invoice_count(&pool, &s.sme);
    s.rr.record_default(&pool, &s.sme);
    s.rr.record_successful_repayment(&pool, &s.sme);

    let profile = s.rr.get_sme_profile(&s.sme);
    assert_eq!((profile.total_invoices, profile.defaults), (1, 1));
}

#[test]
fn built_in_callers_keep_working() {
    let s = setup();
    s.rr.increment_invoice_count(&s.nft, &s.sme);
    s.rr.record_default(&s.admin, &s.sme);
    s.rr.record_successful_repayment(&s.admin, &s.sme);
    assert_eq!(s.rr.get_sme_profile(&s.sme).total_invoices, 1);
}

#[test]
fn only_admin_manages_the_registry() {
    let s = setup();
    let stranger = Address::generate(&s.env);
    let pool = Address::generate(&s.env);
    let not_admin = Err(Ok(RiskRegistryError::NotAdmin));

    assert_eq!(s.rr.try_add_authorized_caller(&stranger, &pool), not_admin);
    s.rr.add_authorized_caller(&s.admin, &pool);
    assert_eq!(
        s.rr.try_remove_authorized_caller(&stranger, &pool),
        not_admin
    );
    assert!(s.rr.is_authorized_caller(&pool));
}

#[test]
fn revoked_caller_is_rejected_mid_flight() {
    let s = setup();
    let pool = Address::generate(&s.env);
    s.rr.add_authorized_caller(&s.admin, &pool);
    s.rr.increment_invoice_count(&pool, &s.sme);

    s.rr.remove_authorized_caller(&s.admin, &pool);

    assert!(!s.rr.is_authorized_caller(&pool));
    assert_eq!(
        s.rr.try_increment_invoice_count(&pool, &s.sme),
        Err(Ok(RiskRegistryError::Unauthorized))
    );
    assert_eq!(
        s.rr.try_record_default(&pool, &s.sme),
        Err(Ok(RiskRegistryError::Unauthorized))
    );
    assert_eq!(s.rr.get_sme_profile(&s.sme).total_invoices, 1);
}

#[test]
fn paused_protocol_blocks_grants_but_allows_revocation() {
    let s = setup();
    let pool = Address::generate(&s.env);
    s.rr.add_authorized_caller(&s.admin, &pool);

    let ac_id = s.env.register_contract(None, AccessControlContract);
    let ac = AccessControlContractClient::new(&s.env, &ac_id);
    ac.initialize(&s.admin);
    s.rr.set_access_control(&s.admin, &ac_id);
    ac.pause(&s.admin);

    let paused = Err(Ok(RiskRegistryError::ProtocolPaused));
    assert_eq!(
        s.rr.try_add_authorized_caller(&s.admin, &Address::generate(&s.env)),
        paused
    );
    // Allowlisted callers are still subject to the protocol pause.
    assert_eq!(s.rr.try_record_default(&pool, &s.sme), paused);
    // Revocation must stay available during an incident.
    s.rr.remove_authorized_caller(&s.admin, &pool);
    assert!(!s.rr.is_authorized_caller(&pool));
}
