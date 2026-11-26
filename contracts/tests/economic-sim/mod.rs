//! Economic Attack Simulation Harness
//!
//! This module models economically-motivated attacks against marketplace/financing_pool
//! mechanics to verify no exploitable strategy yields attacker profit beyond documented
//! acceptable bounds.
//!
//! ## Assumed Attacker Capital Bound
//!
//! All scenarios assume the attacker has access to:
//! - Up to 10M USDC (or equivalent) in liquid capital
//! - Multiple coordinated addresses (up to 10 colluding accounts)
//! - Ability to execute transactions in rapid succession within single ledger
//!
//! This bound represents a well-funded but not nation-state-level adversary,
//! appropriate for the protocol's expected early-stage scale.
//!
//! ## Attack Profitability Threshold
//!
//! An attack is considered "exploitable" if:
//! - Net attacker profit > 1% of capital deployed, OR
//! - Attacker can disrupt protocol operation for >100 ledgers with <1M capital
//!
//! Attacks below this threshold are documented but not necessarily fixed, as
//! they represent economically irrational behavior given gas costs and opportunity cost.

pub mod fund_refund_gaming;
pub mod referral_self_dealing;
pub mod insurance_pool_timing;
pub mod concentration_cap_manipulation;
pub mod secondary_wash_trading;

use soroban_sdk::{Address, Env};

/// Common metrics tracked across all attack simulations
#[derive(Debug, Clone)]
pub struct AttackMetrics {
    /// Total capital deployed by attacker
    pub capital_deployed: i128,
    /// Net profit/loss (negative = loss)
    pub net_profit: i128,
    /// Number of transactions executed
    pub tx_count: u32,
    /// Estimated gas cost (in stroops)
    pub estimated_gas_cost: i128,
    /// Protocol disruption duration in ledgers (if applicable)
    pub disruption_ledgers: u32,
    /// Description of attack outcome
    pub outcome: String,
}

impl AttackMetrics {
    pub fn new() -> Self {
        Self {
            capital_deployed: 0,
            net_profit: 0,
            tx_count: 0,
            estimated_gas_cost: 0,
            disruption_ledgers: 0,
            outcome: String::new(),
        }
    }

    /// Calculate return on investment as basis points
    pub fn roi_bps(&self) -> i128 {
        if self.capital_deployed == 0 {
            return 0;
        }
        (self.net_profit * 10_000) / self.capital_deployed
    }

    /// Check if attack is profitable beyond acceptable threshold (100 bps = 1%)
    pub fn is_exploitable(&self) -> bool {
        self.roi_bps() > 100 || 
        (self.disruption_ledgers > 100 && self.capital_deployed < 1_000_000_000_000) // <1M with 6 decimals
    }
}

/// Estimate gas cost for a transaction based on operation count
/// Rough approximation: 100 stroops base + 10 stroops per operation
pub fn estimate_gas_cost(operation_count: u32) -> i128 {
    100 + (operation_count as i128 * 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attack_metrics_roi() {
        let mut metrics = AttackMetrics::new();
        metrics.capital_deployed = 1_000_000;
        metrics.net_profit = 50_000; // 5% profit
        
        assert_eq!(metrics.roi_bps(), 500); // 500 bps = 5%
        assert!(metrics.is_exploitable()); // > 1% threshold
    }

    #[test]
    fn test_attack_metrics_not_exploitable() {
        let mut metrics = AttackMetrics::new();
        metrics.capital_deployed = 1_000_000;
        metrics.net_profit = 5_000; // 0.5% profit
        
        assert_eq!(metrics.roi_bps(), 50); // 50 bps = 0.5%
        assert!(!metrics.is_exploitable()); // < 1% threshold
    }
}
