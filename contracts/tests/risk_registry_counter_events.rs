//! Issue #1079 — every risk_registry counter increment emits exactly one event.
//!
//! Counter increment sites (AUDIT_LOG RR-01 follow-up):
//!
//! | Counter                         | Entrypoint                | Event (topic)                    |
//! |---------------------------------|---------------------------|----------------------------------|
//! | `SmeProfile.total_invoices`     | `increment_invoice_count` | `SME_INV` (sme, new_total, ts)   |
//! | `SmeProfile.defaults`           | `record_default`          | `SME_DFT` (admin, sme, new_total, ts) |
//! | `DefaultCountTier(tier)`        | `record_default`          | `SME_DFT` (same call)            |
//! | `TotalScoredCountTier(tier)`    | `register_sme`            | `SME_REG` (verifier, sme, score, ts) |
//! | `DisputeCount(verifier)`        | `raise_dispute`           | `DSP_RAIS` (raw topic)           |
//!
//! Each test fails if the corresponding event emission is removed.

#![cfg(test)]

use kora_risk_registry::{
    DataKey, RiskRegistryContract, RiskRegistryContractClient, RiskRegistryError,
};
use kora_shared::types::SmeProfile;
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events},
    Address, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};

struct Setup {
    env: Env,
    admin: Address,
    nft: Address,
    verifier: Address,
    rr_id: Address,
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
    let rr_id = env.register_contract(None, RiskRegistryContract);
    let rr = RiskRegistryContractClient::new(&env, &rr_id);
    // minimum_stake = 0 avoids needing real token balances.
    rr.initialize(&admin, &nft, &staking_token, &0i128, &5_000u32);
    let verifier = Address::generate(&env);
    rr.add_verifier(&admin, &verifier, &0i128);
    Setup {
        env,
        admin,
        nft,
        verifier,
        rr_id,
        rr,
    }
}

/// Topic symbol of an event: shared events publish (SCHEMA_V, topic, version);
/// raw events publish their topic first.
fn topic_of(env: &Env, topics: &Vec<Val>) -> Option<Symbol> {
    let first = Symbol::try_from_val(env, &topics.get(0)?).ok()?;
    if first == symbol_short!("SCHEMA_V") {
        Symbol::try_from_val(env, &topics.get(1)?).ok()
    } else {
        Some(first)
    }
}

/// Payloads of all registry events with `topic` recorded so far (cumulative).
fn events_with(s: &Setup, topic: Symbol) -> std::vec::Vec<Val> {
    s.env
        .events()
        .all()
        .iter()
        .filter(|(contract, topics, _)| {
            *contract == s.rr_id && topic_of(&s.env, topics) == Some(topic.clone())
        })
        .map(|(_, _, data)| data)
        .collect()
}

fn register(s: &Setup) -> Address {
    let sme = Address::generate(&s.env);
    s.rr.register_sme(&s.verifier, &sme, &40u32, &true);
    sme
}

#[test]
fn increment_invoice_count_emits_one_event_with_new_total() {
    let s = setup();
    let sme = register(&s);

    for expected in 1u32..=3 {
        let before = events_with(&s, symbol_short!("SME_INV")).len();
        s.rr.increment_invoice_count(&s.nft, &sme);
        let evs = events_with(&s, symbol_short!("SME_INV"));
        assert_eq!(evs.len(), before + 1, "exactly one SME_INV per increment");
        let (who, total, _ts): (Address, u32, u64) = evs[evs.len() - 1].into_val(&s.env);
        assert_eq!((who, total), (sme.clone(), expected));
    }
    assert_eq!(s.rr.get_sme_profile(&sme).total_invoices, 3);
}

#[test]
fn rejected_increment_emits_no_event() {
    let s = setup();
    let sme = register(&s);

    // Unauthorized caller (not the invoice_nft contract).
    assert!(s
        .rr
        .try_increment_invoice_count(&Address::generate(&s.env), &sme)
        .is_err());
    assert!(events_with(&s, symbol_short!("SME_INV")).is_empty());

    // Unregistered SME.
    let res =
        s.rr.try_increment_invoice_count(&s.nft, &Address::generate(&s.env));
    assert_eq!(res, Err(Ok(RiskRegistryError::SMENotRegistered)));
    assert!(events_with(&s, symbol_short!("SME_INV")).is_empty());
}

#[test]
fn invoice_count_overflow_errors_without_dangling_event() {
    let s = setup();
    let sme = register(&s);

    s.env.as_contract(&s.rr_id, || {
        let key = DataKey::SmeProfile(sme.clone());
        let mut profile: SmeProfile = s.env.storage().persistent().get(&key).unwrap();
        profile.total_invoices = u32::MAX;
        s.env.storage().persistent().set(&key, &profile);
    });

    let res = s.rr.try_increment_invoice_count(&s.nft, &sme);
    assert_eq!(res, Err(Ok(RiskRegistryError::ArithmeticOverflow)));
    assert!(events_with(&s, symbol_short!("SME_INV")).is_empty());
    assert_eq!(s.rr.get_sme_profile(&sme).total_invoices, u32::MAX);
}

#[test]
fn record_default_emits_one_event_with_new_total() {
    let s = setup();
    let sme = register(&s);

    for expected in 1u32..=2 {
        let before = events_with(&s, symbol_short!("SME_DFT")).len();
        s.rr.record_default(&s.admin, &sme);
        let evs = events_with(&s, symbol_short!("SME_DFT"));
        assert_eq!(evs.len(), before + 1, "exactly one SME_DFT per default");
        let (_admin, who, total, _ts): (Address, Address, u32, u64) =
            evs[evs.len() - 1].into_val(&s.env);
        assert_eq!((who, total), (sme.clone(), expected));
    }
}

#[test]
fn register_sme_emits_one_event_for_tier_count_increment() {
    let s = setup();
    let sme = register(&s);

    let evs = events_with(&s, symbol_short!("SME_REG"));
    assert_eq!(evs.len(), 1, "exactly one SME_REG per registration");
    let (_verifier, who, score, _ts): (Address, Address, u32, u64) = evs[0].into_val(&s.env);
    assert_eq!((who, score), (sme, 40));
}

#[test]
fn raise_dispute_emits_one_event_per_increment() {
    let s = setup();
    s.rr.configure_slashing(&s.admin, &86_400u64, &Address::generate(&s.env));

    for expected in 1u32..=2 {
        let before = events_with(&s, symbol_short!("DSP_RAIS")).len();
        s.rr.raise_dispute(
            &Address::generate(&s.env),
            &s.verifier,
            &Address::generate(&s.env),
        );
        assert_eq!(
            events_with(&s, symbol_short!("DSP_RAIS")).len(),
            before + 1,
            "exactly one DSP_RAIS per dispute"
        );
        assert_eq!(s.rr.get_dispute_count(&s.verifier), expected);
    }
}
