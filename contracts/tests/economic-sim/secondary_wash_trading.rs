//! Secondary-market wash trading detection invariants (issue #1182)
//!
//! ## Attack Vector
//!
//! A colluding group trades the same invoice position back and forth on
//! `secondary_market` (`list_position` → `buy_position`) to manufacture
//! volume. Inflated secondary volume distorts the APR / liquidity and risk
//! signals that indexers and the risk pipeline derive from settled trades.
//!
//! ## Expected Defenses (modelled here)
//!
//! - `buy_position` rejects `buyer == seller` with `SecondaryMarketError::SameAddress`,
//!   so a single address cannot self-trade on-chain.
//! - Every settled trade pays `protocol_fee_bps` to the treasury, so each wash
//!   leg is a strict loss for the colluding group (no fee rebate exists).
//! - Off-chain, the detector below flags round-trips (A→B→A) and short cycles
//!   (A→B→C→A) on the same invoice inside a ledger window; flagged trades are
//!   excluded from "organic" volume so APR signals are not inflated.
//! - While `secondary_market` is paused no trade settles (`ProtocolPaused`), so
//!   a trade stamped inside a pause window is treated as invalid input.
//!
//! The detector is a pure model over settled-trade records (as the indexer
//! emits them); it does not introduce a new on-chain subsystem.

#![cfg(test)]

use std::collections::BTreeSet;

use crate::economic_sim::{estimate_gas_cost, AttackMetrics};

/// A settled secondary-market trade as emitted by the indexer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Trade {
    pub tx_id: u64,
    pub invoice_id: u64,
    pub seller: u32,
    pub buyer: u32,
    pub price: i128,
    pub ledger: u32,
}

/// Detector configuration. All fields are required; zero values fail closed.
#[derive(Debug, Clone, Copy)]
pub struct WashConfig {
    /// Ledger window inside which a return of the position is suspicious.
    pub window_ledgers: u32,
    /// Maximum number of hops for cycle detection (2 = round-trip only).
    pub max_hops: usize,
    /// Protocol fee in bps (mirrors `secondary_market` `ProtocolFeeBps`).
    pub fee_bps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WashConfigError {
    ZeroWindow,
    HopsTooSmall,
    FeeOutOfRange,
    /// Trade settled during a pause window — impossible on-chain.
    TradeDuringPause,
    /// Self-trade — `buy_position` rejects with `SameAddress`.
    SelfTrade,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum WashFlag {
    /// Position returned to its original seller after `hops` trades.
    Cycle { invoice_id: u64, origin: u32, hops: usize, tx_ids: Vec<u64> },
}

/// Mirror of `secondary_market` `MAX_PROTOCOL_FEE_BPS`.
const MAX_FEE_BPS: u32 = 1_000;

fn validate(cfg: &WashConfig) -> Result<(), WashConfigError> {
    if cfg.window_ledgers == 0 {
        return Err(WashConfigError::ZeroWindow);
    }
    if cfg.max_hops < 2 {
        return Err(WashConfigError::HopsTooSmall);
    }
    if cfg.fee_bps > MAX_FEE_BPS {
        return Err(WashConfigError::FeeOutOfRange);
    }
    Ok(())
}

/// Detect wash-looking cycles. Input order does not matter and duplicate
/// records (same `tx_id`) are collapsed, so re-running over a replayed
/// indexer page is idempotent.
pub fn detect_wash_patterns(
    trades: &[Trade],
    cfg: &WashConfig,
    paused_ledgers: &[(u32, u32)],
) -> Result<Vec<WashFlag>, WashConfigError> {
    validate(cfg)?;
    let mut seen = BTreeSet::new();
    let mut sorted: Vec<Trade> = Vec::new();
    for t in trades {
        if t.seller == t.buyer {
            return Err(WashConfigError::SelfTrade);
        }
        if paused_ledgers.iter().any(|(s, e)| t.ledger >= *s && t.ledger <= *e) {
            return Err(WashConfigError::TradeDuringPause);
        }
        if seen.insert(t.tx_id) {
            sorted.push(*t);
        }
    }
    sorted.sort_by_key(|t| (t.invoice_id, t.ledger, t.tx_id));

    let mut flags = BTreeSet::new();
    for (i, start) in sorted.iter().enumerate() {
        let mut chain = vec![start.tx_id];
        let mut holder = start.buyer;
        for next in sorted[i + 1..].iter() {
            if next.invoice_id != start.invoice_id
                || next.ledger - start.ledger > cfg.window_ledgers
                || chain.len() >= cfg.max_hops
            {
                break;
            }
            if next.seller != holder {
                continue;
            }
            chain.push(next.tx_id);
            holder = next.buyer;
            if holder == start.seller {
                flags.insert(WashFlag::Cycle {
                    invoice_id: start.invoice_id,
                    origin: start.seller,
                    hops: chain.len(),
                    tx_ids: chain.clone(),
                });
                break;
            }
        }
    }
    Ok(flags.into_iter().collect())
}

/// Volume excluding every trade that participates in a flagged cycle.
pub fn organic_volume(trades: &[Trade], flags: &[WashFlag]) -> i128 {
    let flagged: BTreeSet<u64> = flags
        .iter()
        .flat_map(|WashFlag::Cycle { tx_ids, .. }| tx_ids.iter().copied())
        .collect();
    let mut seen = BTreeSet::new();
    trades
        .iter()
        .filter(|t| seen.insert(t.tx_id) && !flagged.contains(&t.tx_id))
        .map(|t| t.price)
        .sum()
}

fn fee(price: i128, fee_bps: u32) -> i128 {
    price * fee_bps as i128 / 10_000
}

fn t(tx_id: u64, invoice_id: u64, seller: u32, buyer: u32, price: i128, ledger: u32) -> Trade {
    Trade { tx_id, invoice_id, seller, buyer, price, ledger }
}

fn cfg() -> WashConfig {
    WashConfig { window_ledgers: 100, max_hops: 4, fee_bps: 100 }
}

const PRICE: i128 = 100_000_000_000; // 100K

#[test]
fn test_round_trip_is_flagged_and_excluded_from_organic_volume() {
    let trades = [t(1, 7, 1, 2, PRICE, 10), t(2, 7, 2, 1, PRICE, 20)];
    let flags = detect_wash_patterns(&trades, &cfg(), &[]).unwrap();
    assert_eq!(
        flags,
        vec![WashFlag::Cycle { invoice_id: 7, origin: 1, hops: 2, tx_ids: vec![1, 2] }]
    );
    assert_eq!(organic_volume(&trades, &flags), 0);
}

#[test]
fn test_three_hop_ring_is_flagged() {
    let trades = [t(1, 7, 1, 2, PRICE, 10), t(2, 7, 2, 3, PRICE, 11), t(3, 7, 3, 1, PRICE, 12)];
    let flags = detect_wash_patterns(&trades, &cfg(), &[]).unwrap();
    assert!(flags.contains(&WashFlag::Cycle {
        invoice_id: 7,
        origin: 1,
        hops: 3,
        tx_ids: vec![1, 2, 3]
    }));
    assert_eq!(organic_volume(&trades, &flags), 0);
}

#[test]
fn test_organic_one_way_flow_is_not_flagged() {
    let trades = [t(1, 7, 1, 2, PRICE, 10), t(2, 7, 2, 3, PRICE, 20), t(3, 8, 4, 1, PRICE, 30)];
    let flags = detect_wash_patterns(&trades, &cfg(), &[]).unwrap();
    assert!(flags.is_empty());
    assert_eq!(organic_volume(&trades, &flags), 3 * PRICE);
}

#[test]
fn test_return_outside_window_is_not_flagged() {
    let trades = [t(1, 7, 1, 2, PRICE, 10), t(2, 7, 2, 1, PRICE, 10 + 101)];
    assert!(detect_wash_patterns(&trades, &cfg(), &[]).unwrap().is_empty());
    // Boundary: exactly at the window edge is still flagged.
    let edge = [t(1, 7, 1, 2, PRICE, 10), t(2, 7, 2, 1, PRICE, 110)];
    assert_eq!(detect_wash_patterns(&edge, &cfg(), &[]).unwrap().len(), 1);
}

#[test]
fn test_cycle_longer_than_max_hops_is_not_flagged() {
    let trades = [t(1, 7, 1, 2, PRICE, 10), t(2, 7, 2, 3, PRICE, 11), t(3, 7, 3, 1, PRICE, 12)];
    let c = WashConfig { max_hops: 2, ..cfg() };
    assert!(detect_wash_patterns(&trades, &c, &[]).unwrap().is_empty());
}

#[test]
fn test_cross_invoice_returns_are_not_conflated() {
    let trades = [t(1, 7, 1, 2, PRICE, 10), t(2, 8, 2, 1, PRICE, 11)];
    assert!(detect_wash_patterns(&trades, &cfg(), &[]).unwrap().is_empty());
}

#[test]
fn test_detection_is_idempotent_over_replayed_and_reordered_input() {
    let trades = [t(1, 7, 1, 2, PRICE, 10), t(2, 7, 2, 1, PRICE, 20)];
    let replayed = [trades[1], trades[0], trades[1], trades[0]];
    let a = detect_wash_patterns(&trades, &cfg(), &[]).unwrap();
    let b = detect_wash_patterns(&replayed, &cfg(), &[]).unwrap();
    assert_eq!(a, b);
    assert_eq!(organic_volume(&replayed, &b), 0);
}

#[test]
fn test_invalid_or_missing_config_fails_closed() {
    let trades = [t(1, 7, 1, 2, PRICE, 10)];
    let zero_window = WashConfig { window_ledgers: 0, ..cfg() };
    assert_eq!(detect_wash_patterns(&trades, &zero_window, &[]), Err(WashConfigError::ZeroWindow));
    let one_hop = WashConfig { max_hops: 1, ..cfg() };
    assert_eq!(detect_wash_patterns(&trades, &one_hop, &[]), Err(WashConfigError::HopsTooSmall));
    let fee_hi = WashConfig { fee_bps: MAX_FEE_BPS + 1, ..cfg() };
    assert_eq!(detect_wash_patterns(&trades, &fee_hi, &[]), Err(WashConfigError::FeeOutOfRange));
    // Empty input with valid config is fine (nothing to flag).
    assert_eq!(detect_wash_patterns(&[], &cfg(), &[]), Ok(vec![]));
}

#[test]
fn test_self_trade_is_rejected_like_same_address_guard() {
    let trades = [t(1, 7, 1, 1, PRICE, 10)];
    assert_eq!(detect_wash_patterns(&trades, &cfg(), &[]), Err(WashConfigError::SelfTrade));
}

#[test]
fn test_trade_during_pause_window_is_rejected() {
    let trades = [t(1, 7, 1, 2, PRICE, 10), t(2, 7, 2, 1, PRICE, 55)];
    assert_eq!(
        detect_wash_patterns(&trades, &cfg(), &[(50, 60)]),
        Err(WashConfigError::TradeDuringPause)
    );
    // Trades outside the pause window are accepted.
    assert!(detect_wash_patterns(&trades, &cfg(), &[(30, 40)]).is_ok());
}

#[test]
fn test_wash_trading_is_never_profitable() {
    // Invariant: every wash leg pays the protocol fee + gas, so a colluding
    // group always ends with less capital than it started with.
    let mut metrics = AttackMetrics::new();
    let c = cfg();
    let rounds = 50u32;
    metrics.capital_deployed = PRICE;
    for _ in 0..rounds {
        metrics.net_profit -= 2 * fee(PRICE, c.fee_bps);
        metrics.estimated_gas_cost += 2 * estimate_gas_cost(25);
        metrics.tx_count += 4; // list + buy, twice
    }
    metrics.net_profit -= metrics.estimated_gas_cost;
    metrics.outcome = format!("{rounds} wash round-trips: net {}", metrics.net_profit);
    assert!(metrics.net_profit < 0, "{}", metrics.outcome);
    assert!(!metrics.is_exploitable());

    // Even at 0 bps fee, gas keeps wash trading strictly unprofitable.
    assert!(-(2 * fee(PRICE, 0)) - 2 * estimate_gas_cost(25) < 0);
}
