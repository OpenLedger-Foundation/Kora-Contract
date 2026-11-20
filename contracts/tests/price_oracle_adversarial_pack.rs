//! Issue #1081 — automated oracle adversarial scenario pack.
//!
//! Encodes scenarios from `docs/ORACLE_ADVERSARIAL_TESTING.md` that were not yet
//! automated (or only existed in the WASM-import suite, which is not part of the
//! `kora-tests` targets): rate-change spikes, heartbeat failure with and without
//! a fallback feed, per-pair staleness, conflicting sources losing quorum, and a
//! stale feeder rejoining with a spike.
//!
//! Uses the native `kora_price_oracle` contract so it runs with `cargo test`
//! (no prior WASM build needed).

#![cfg(test)]

use kora_access_control::{AccessControlContract, AccessControlContractClient};
use kora_price_oracle::{PriceOracleContract, PriceOracleContractClient, PriceOracleError};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events, Ledger},
    Address, Env, Symbol, TryFromVal, Val, Vec,
};

const T0: u64 = 1_700_000_000;
const PRICE: i128 = 1_000_000; // 0.1 with 7 decimals

struct Oracle {
    env: Env,
    admin: Address,
    id: Address,
    client: PriceOracleContractClient<'static>,
}

fn base() -> Symbol {
    symbol_short!("XLM")
}
fn quote() -> Symbol {
    symbol_short!("USDC")
}

fn set_time(env: &Env, t: u64) {
    env.ledger().with_mut(|l| l.timestamp = t);
}

fn new_oracle(env: &Env, admin: &Address) -> Oracle {
    let ac_id = env.register_contract(None, AccessControlContract);
    AccessControlContractClient::new(env, &ac_id).initialize(admin);
    let id = env.register_contract(None, PriceOracleContract);
    let client = PriceOracleContractClient::new(env, &id);
    client.initialize(admin, &ac_id);
    Oracle {
        env: env.clone(),
        admin: admin.clone(),
        id,
        client,
    }
}

fn setup() -> Oracle {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    set_time(&env, T0);
    let admin = Address::generate(&env);
    new_oracle(&env, &admin)
}

fn feeder(o: &Oracle) -> Address {
    let f = Address::generate(&o.env);
    o.client.add_feeder(&o.admin, &f);
    f
}

/// Number of events from `contract` whose first topic is `topic` (cumulative).
fn count_events(o: &Oracle, contract: &Address, topic: Symbol) -> usize {
    o.env
        .events()
        .all()
        .iter()
        .filter(|(c, topics, _): &(Address, Vec<Val>, Val)| {
            c == contract
                && topics
                    .get(0)
                    .and_then(|t| Symbol::try_from_val(&o.env, &t).ok())
                    .map_or(false, |s| s == topic)
        })
        .count()
}

/// The typed error `get_price` returns for the test pair (panics if it succeeds).
fn price_err(o: &Oracle) -> PriceOracleError {
    match o.client.try_get_price(&base(), &quote()) {
        Err(Ok(e)) => e,
        Ok(Ok(p)) => panic!("expected an error, got price {}", p.price),
        other => panic!("unexpected result: {:?}", other.err()),
    }
}

// ── Spike path ────────────────────────────────────────────────────────────────

/// Fresh-but-wrong extreme jump: a compromised feeder tries +200% in one update.
/// The rate-change bound rejects it, emits RATE_EX, and leaves the aggregate intact.
/// (Automates the scenario behind the still-`#[ignore]`d
/// `test_recommend_max_rate_change_per_update`.)
#[test]
fn pack_spike_rejected_by_rate_change_bound() {
    let o = setup();
    o.client.set_min_fresh_feeders(&o.admin, &1u32);
    o.client
        .set_max_rate_change_per_update(&o.admin, &base(), &quote(), &2_000u32); // 20%
    let attacker = feeder(&o);
    o.client.set_price(&attacker, &base(), &quote(), &PRICE);

    let before = count_events(&o, &o.id, symbol_short!("RATE_EX"));
    let res = o
        .client
        .try_set_price(&attacker, &base(), &quote(), &(PRICE * 3));

    assert_eq!(res, Err(Ok(PriceOracleError::RateChangeExceeded)));
    assert_eq!(
        count_events(&o, &o.id, symbol_short!("RATE_EX")),
        before + 1
    );
    assert_eq!(o.client.get_price(&base(), &quote()).price, PRICE);
}

/// The bound forces an attacker to walk the price: each in-bound step is accepted,
/// so reaching +100% at a 20% bound takes several visible updates, not one.
#[test]
fn pack_spike_must_be_walked_gradually_under_rate_bound() {
    let o = setup();
    o.client.set_min_fresh_feeders(&o.admin, &1u32);
    o.client
        .set_max_rate_change_per_update(&o.admin, &base(), &quote(), &2_000u32);
    let attacker = feeder(&o);
    o.client.set_price(&attacker, &base(), &quote(), &PRICE);

    let target = PRICE * 2;
    let mut price = PRICE;
    let mut steps = 0;
    while price < target {
        let next = core::cmp::min(price + price / 5, target); // +20% max
        o.client.set_price(&attacker, &base(), &quote(), &next);
        price = next;
        steps += 1;
    }
    assert!(
        steps >= 4,
        "doubling at a 20% bound needs >= 4 updates, took {steps}"
    );
    assert_eq!(o.client.get_price(&base(), &quote()).price, target);
}

/// A feeder that went quiet keeps its last price as the rate-bound reference:
/// staleness does not reset the bound, so it cannot rejoin with a spike.
#[test]
fn pack_stale_feeder_cannot_rejoin_with_spike() {
    let o = setup();
    o.client.set_min_fresh_feeders(&o.admin, &1u32);
    o.client
        .set_max_rate_change_per_update(&o.admin, &base(), &quote(), &2_000u32);
    let attacker = feeder(&o);
    o.client.set_price(&attacker, &base(), &quote(), &PRICE);

    set_time(&o.env, T0 + 3 * 3_600); // long past MAX_STALENESS_SECS
    let res = o
        .client
        .try_set_price(&attacker, &base(), &quote(), &(PRICE * 5));
    assert_eq!(res, Err(Ok(PriceOracleError::RateChangeExceeded)));
}

// ── Stale path / heartbeat failure ────────────────────────────────────────────

/// Heartbeat failure: every feeder stops reporting. Consumers get AllFeedsStale
/// instead of an outdated price.
#[test]
fn pack_heartbeat_failure_returns_all_feeds_stale() {
    let o = setup();
    let (a, b) = (feeder(&o), feeder(&o));
    o.client.set_price(&a, &base(), &quote(), &PRICE);
    o.client.set_price(&b, &base(), &quote(), &(PRICE + 1_000));
    assert!(o.client.try_get_price(&base(), &quote()).is_ok());

    set_time(&o.env, T0 + 3_601);
    assert_eq!(price_err(&o), PriceOracleError::AllFeedsStale);
}

/// Heartbeat failure with a configured fallback feed: the fallback serves the
/// price and a FALLBACK event is emitted for monitoring.
#[test]
fn pack_heartbeat_failure_served_by_fallback_feed() {
    let o = setup();
    let fallback = new_oracle(&o.env, &o.admin);
    let fb_feeder = feeder(&fallback);
    fallback.client.set_min_fresh_feeders(&o.admin, &1u32);

    let a = feeder(&o);
    o.client.set_min_fresh_feeders(&o.admin, &1u32);
    o.client.set_price(&a, &base(), &quote(), &PRICE);
    o.client
        .set_fallback_feed(&o.admin, &base(), &quote(), &fallback.id);

    set_time(&o.env, T0 + 3_601);
    fallback
        .client
        .set_price(&fb_feeder, &base(), &quote(), &(PRICE + 5_000));

    let before = count_events(&o, &o.id, symbol_short!("FALLBACK"));
    assert_eq!(o.client.get_price(&base(), &quote()).price, PRICE + 5_000);
    assert_eq!(
        count_events(&o, &o.id, symbol_short!("FALLBACK")),
        before + 1
    );
}

/// Coordinated failure: primary feeders and the fallback are all stale. The
/// oracle must still refuse to price rather than return stale fallback data.
#[test]
fn pack_heartbeat_failure_with_stale_fallback_still_refuses() {
    let o = setup();
    let fallback = new_oracle(&o.env, &o.admin);
    let fb_feeder = feeder(&fallback);
    fallback.client.set_min_fresh_feeders(&o.admin, &1u32);
    fallback
        .client
        .set_price(&fb_feeder, &base(), &quote(), &PRICE);

    let a = feeder(&o);
    o.client.set_min_fresh_feeders(&o.admin, &1u32);
    o.client.set_price(&a, &base(), &quote(), &PRICE);
    o.client
        .set_fallback_feed(&o.admin, &base(), &quote(), &fallback.id);

    set_time(&o.env, T0 + 3_601);
    assert_eq!(price_err(&o), PriceOracleError::AllFeedsStale);
}

/// A tighter per-pair staleness window (e.g. for a volatile pair) is enforced
/// even when the price would still be fresh under the global 1h window.
#[test]
fn pack_per_pair_staleness_tightens_window() {
    let o = setup();
    o.client.set_min_fresh_feeders(&o.admin, &1u32);
    o.client
        .set_max_staleness(&o.admin, &base(), &quote(), &60u64);
    let a = feeder(&o);
    o.client.set_price(&a, &base(), &quote(), &PRICE);

    set_time(&o.env, T0 + 60);
    assert!(o.client.try_get_price(&base(), &quote()).is_ok());
    set_time(&o.env, T0 + 120);
    assert_eq!(price_err(&o), PriceOracleError::AllFeedsStale);
}

// ── Conflicting sources ───────────────────────────────────────────────────────

/// Conflicting sources: the honest feeder goes stale and the only fresh source
/// is an outlier. With the default quorum (2) the oracle refuses to price from a
/// single source and emits INSUF_FD.
#[test]
fn pack_conflicting_sources_single_fresh_outlier_fails_quorum() {
    let o = setup();
    assert_eq!(o.client.get_min_fresh_feeders(), 2);
    let (honest, attacker) = (feeder(&o), feeder(&o));
    o.client.set_price(&honest, &base(), &quote(), &PRICE);

    set_time(&o.env, T0 + 3_000);
    o.client
        .set_price(&attacker, &base(), &quote(), &(PRICE * 10));
    assert_eq!(
        o.client.get_price(&base(), &quote()).price,
        (PRICE + PRICE * 10) / 2
    );

    set_time(&o.env, T0 + 3_601); // honest is now stale, attacker still fresh
    let before = count_events(&o, &o.id, symbol_short!("INSUF_FD"));
    assert_eq!(price_err(&o), PriceOracleError::InsufficientFreshFeeders);
    assert_eq!(
        count_events(&o, &o.id, symbol_short!("INSUF_FD")),
        before + 1
    );
}

/// Conflicting sources with an honest majority: one extreme outlier among three
/// fresh feeders does not move the median.
#[test]
fn pack_conflicting_sources_median_ignores_outlier() {
    let o = setup();
    let (a, b, attacker) = (feeder(&o), feeder(&o), feeder(&o));
    o.client.set_price(&a, &base(), &quote(), &PRICE);
    o.client.set_price(&b, &base(), &quote(), &(PRICE + 2_000));
    o.client
        .set_price(&attacker, &base(), &quote(), &(PRICE * 50));

    assert_eq!(o.client.get_price(&base(), &quote()).price, PRICE + 2_000);
}
