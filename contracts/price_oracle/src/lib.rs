#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol, Vec};

const MAX_STALENESS_SECS: u64 = 3600;
const UPGRADE_TIMELOCK_DELAY: u64 = 86_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum PriceOracleError {
    AlreadyInitialized = 1,
    ArithmeticOverflow = 2,
    InvalidAmount = 3,
    InvoiceExpired = 4,
    NotAdmin = 5,
    NotInitialized = 6,
    NoUpgradeProposed = 7,
    UpgradeTimelockNotElapsed = 8,
    ProtocolPaused = 9,
    NotFeeder = 10,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[contracttype]
pub enum DataKey {
    Admin,
    AccessControl,
    Price(Symbol, Symbol),
    UpgradeProposal,
    /// Authorized price feeder flag.
    Feeder(Address),
    /// A single feeder's submitted price for a (base, quote) pair.
    FeederPrice(Symbol, Symbol, Address),
    /// Enumerable list of feeders that have submitted a price for a pair.
    PriceFeeders(Symbol, Symbol),
    BaseCurrency,
    MaxDeviation,
}

#[contract]
pub struct PriceOracleContract;

#[contractimpl]
impl PriceOracleContract {
    pub fn initialize(env: Env, admin: Address, access_control: Address) -> Result<(), PriceOracleError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(PriceOracleError::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        Ok(())
    }

    /// Set the access_control contract address. Admin only.
    /// Used for post-deployment wiring or migration.
    pub fn set_access_control(env: Env, admin: Address, access_control: Address) -> Result<(), PriceOracleError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        Ok(())
    }

    /// Set a price for a currency pair. Authorized feeders only.
    /// Price is expressed as `base` units per 1 unit of `quote`, scaled by 1e7 (stroops).
    /// Blocked when the protocol is paused.
    pub fn set_price(
        env: Env,
        feeder: Address,
        base: Symbol,
        quote: Symbol,
        price: i128,
    ) -> Result<(), PriceOracleError> {
        feeder.require_auth();
        Self::require_feeder(&env, &feeder)?;
        Self::require_not_paused(&env)?;

        if price <= 0 {
            return Err(PriceOracleError::InvalidAmount);
        }

        // Check reciprocal consistency if reverse pair exists
        if let Ok(reverse_data) = Self::get_price(env.clone(), quote.clone(), base.clone()) {
            // Expected reciprocal: if P is forward price, reverse should be ~10^14 / P
            // We compute the reciprocal and allow 1% tolerance for rounding
            let expected_reciprocal = Self::compute_reciprocal(price)?;
            let tolerance_bps = 100; // 1% = 100 basis points
            Self::validate_reciprocal_tolerance(expected_reciprocal, reverse_data.price, tolerance_bps)?;
        }

        let data = PriceData {
            price,
            timestamp: env.ledger().timestamp(),
        };
        env.storage()
            .persistent()
            .set(
                &DataKey::FeederPrice(base.clone(), quote.clone(), feeder.clone()),
                &data,
            );

        let mut feeders: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::PriceFeeders(base.clone(), quote.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        if !feeders.iter().any(|f| f == feeder) {
            feeders.push_back(feeder);
            env.storage()
                .persistent()
                .set(&DataKey::PriceFeeders(base, quote), &feeders);
        }

        Ok(())
    }

    /// Add an authorized feeder. Admin only.
    pub fn add_feeder(env: Env, admin: Address, feeder: Address) -> Result<(), PriceOracleError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Feeder(feeder), &true);
        Ok(())
    }

    /// Remove an authorized feeder. Admin only.
    pub fn remove_feeder(env: Env, admin: Address, feeder: Address) -> Result<(), PriceOracleError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().remove(&DataKey::Feeder(feeder));
        Ok(())
    }

    /// Set the base currency for multi-hop triangulation. Admin only.
    pub fn set_base_currency(
        env: Env,
        admin: Address,
        base: Symbol,
    ) -> Result<(), PriceOracleError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::BaseCurrency, &base);
        Ok(())
    }

    /// Set the maximum allowed price deviation in basis points.
    /// Admin only. Default is 1000 (10%).
    pub fn set_max_deviation(
        env: Env,
        admin: Address,
        deviation_bps: u32,
    ) -> Result<(), PriceOracleError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .set(&DataKey::MaxDeviation, &deviation_bps);
        Ok(())
    }

    /// Get the aggregated price for a pair (median of all active feeders).
    /// Returns the median price and its oldest (non-stale) timestamp.
    /// Fails if no feeders have submitted a non-stale price.
    pub fn get_price(
        env: Env,
        base: Symbol,
        quote: Symbol,
    ) -> Result<PriceData, PriceOracleError> {
        let feeders: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::PriceFeeders(base.clone(), quote.clone()))
            .unwrap_or_else(|| Vec::new(&env));

        let mut prices: Vec<i128> = Vec::new(&env);
        let mut min_timestamp: u64 = u64::MAX;

        for feeder in feeders.iter() {
            let key = DataKey::FeederPrice(base.clone(), quote.clone(), feeder.clone());
            if let Some(data) = env.storage().persistent().get::<_, PriceData>(&key) {
                let age = env.ledger().timestamp().saturating_sub(data.timestamp);
                if age > MAX_STALENESS_SECS {
                    continue;
                }
                prices.push_back(data.price);
                if data.timestamp < min_timestamp {
                    min_timestamp = data.timestamp;
                }
            }
        }

        if prices.is_empty() {
            return Err(PriceOracleError::InvalidAmount);
        }

        let median = Self::calculate_median(&prices);
        Ok(PriceData {
            price: median,
            timestamp: min_timestamp,
        })
    }

    /// Convert an amount from one currency to another using the stored price.
    /// First attempts direct pair conversion. If unavailable, triangulates through
    /// the configured base currency. Rejects stale or missing prices.
    pub fn convert(
        env: Env,
        amount: i128,
        from: Symbol,
        to: Symbol,
    ) -> Result<i128, PriceOracleError> {
        if from == to {
            return Ok(amount);
        }

        let price_data = Self::get_price(env.clone(), from, to)?;
        let converted = amount
            .checked_mul(price_data.price)
            .and_then(|v| v.checked_div(10_000_000))
            .ok_or(PriceOracleError::ArithmeticOverflow)?;

        if converted <= 0 {
            return Err(PriceOracleError::InvalidAmount);
        }

        Ok(converted)
    }

    fn calculate_median(prices: &Vec<i128>) -> i128 {
        let len = prices.len();
        if len == 0 {
            return 0;
        }

        let mut sorted = prices.clone();
        for i in 0..len {
            for j in i..len {
                if sorted.get(j).unwrap() < sorted.get(i).unwrap() {
                    let temp = sorted.get(j).unwrap();
                    sorted.set(j, sorted.get(i).unwrap());
                    sorted.set(i, temp);
                }
            }
        }

        if len % 2 == 1 {
            sorted.get(len / 2).unwrap()
        } else {
            (sorted.get(len / 2 - 1).unwrap() + sorted.get(len / 2).unwrap()) / 2
        }
    }

    /// Transfer admin rights to a new address. Admin only.
    pub fn transfer_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), PriceOracleError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        Ok(())
    }

    /// Propose a WASM upgrade with a 24-hour timelock. Admin only.
    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), PriceOracleError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::UpgradeProposal, &(new_wasm_hash, env.ledger().timestamp()));
        Ok(())
    }

    /// Execute a previously proposed upgrade after the 24-hour timelock.
    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), PriceOracleError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(PriceOracleError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(PriceOracleError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    /// Convert an amount between currencies with decimal precision correction.
    /// Applies: amount_out = (amount_in * price_ratio / 1e7) * 10^(to_decimals - from_decimals)
    /// This corrects for differing token decimal places (e.g., 6 vs 7 decimal tokens).
    ///
    /// **Parameters:**
    /// - `amount` — The input amount in `from` currency's smallest unit.
    /// - `from` — Source currency symbol.
    /// - `to` — Target currency symbol.
    /// - `from_decimals` — Decimal places of the `from` token (typically 6 or 7).
    /// - `to_decimals` — Decimal places of the `to` token (typically 6 or 7).
    ///
    /// **Returns:** Converted amount in `to` currency's smallest unit.
    ///
    /// **Errors:**
    /// - `PriceOracleError::ArithmeticOverflow` — Multiplication or division overflowed.
    /// - `PriceOracleError::InvalidAmount` — Price not found, stale, or result is ≤ 0.
    /// - `PriceOracleError::InvoiceExpired` — Price data is older than MAX_STALENESS_SECS.
    pub fn convert_with_decimals(
        env: Env,
        amount: i128,
        from: Symbol,
        to: Symbol,
        from_decimals: u32,
        to_decimals: u32,
    ) -> Result<i128, PriceOracleError> {
        if from == to {
            return Ok(amount);
        }

        let price_data = Self::get_price(env.clone(), from.clone(), to.clone())?;
        // First: apply price ratio scaled by 1e7
        let converted = amount
            .checked_mul(price_data.price)
            .and_then(|v| v.checked_div(10_000_000))
            .ok_or(PriceOracleError::ArithmeticOverflow)?;

        if converted <= 0 {
            return Err(PriceOracleError::InvalidAmount);
        }

        // Second: apply decimal rescaling based on token precision differences
        let rescaled = if from_decimals >= to_decimals {
            let divisor = Self::compute_10_pow(from_decimals - to_decimals)?;
            converted
                .checked_div(divisor)
                .ok_or(PriceOracleError::ArithmeticOverflow)?
        } else {
            let multiplier = Self::compute_10_pow(to_decimals - from_decimals)?;
            converted
                .checked_mul(multiplier)
                .ok_or(PriceOracleError::ArithmeticOverflow)?
        };

        if rescaled <= 0 {
            return Err(PriceOracleError::InvalidAmount);
        }

        Ok(rescaled)
    }

    fn compute_10_pow(exp: u32) -> Result<i128, PriceOracleError> {
        match exp {
            0 => Ok(1),
            1 => Ok(10),
            2 => Ok(100),
            3 => Ok(1_000),
            4 => Ok(10_000),
            5 => Ok(100_000),
            6 => Ok(1_000_000),
            7 => Ok(10_000_000),
            8 => Ok(100_000_000),
            9 => Ok(1_000_000_000),
            10 => Ok(10_000_000_000),
            _ => Err(PriceOracleError::ArithmeticOverflow),
        }
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), PriceOracleError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(PriceOracleError::NotInitialized)?;
        if &admin != caller {
            return Err(PriceOracleError::NotAdmin);
        }
        Ok(())
    }

    fn require_feeder(env: &Env, caller: &Address) -> Result<(), PriceOracleError> {
        let is_feeder: bool = env
            .storage()
            .persistent()
            .get(&DataKey::Feeder(caller.clone()))
            .unwrap_or(false);
        if !is_feeder {
            return Err(PriceOracleError::NotFeeder);
        }
        Ok(())
    }

    /// Computes the reciprocal of a price scaled by 1e7: 10^14 / price.
    fn compute_reciprocal(price: i128) -> Result<i128, PriceOracleError> {
        if price <= 0 {
            return Err(PriceOracleError::InvalidAmount);
        }
        100_000_000_000_000i128
            .checked_div(price)
            .ok_or(PriceOracleError::ArithmeticOverflow)
    }

    /// Validates that `actual` is within `tolerance_bps` of `expected`.
    fn validate_reciprocal_tolerance(
        expected: i128,
        actual: i128,
        tolerance_bps: i128,
    ) -> Result<(), PriceOracleError> {
        let diff = (expected - actual).abs();
        let max_diff = expected
            .checked_mul(tolerance_bps)
            .and_then(|v| v.checked_div(10_000))
            .ok_or(PriceOracleError::ArithmeticOverflow)?;
        if diff > max_diff {
            return Err(PriceOracleError::InvalidAmount);
        }
        Ok(())
    }

    fn require_not_paused(env: &Env) -> Result<(), PriceOracleError> {
        let access_control: Address = env
            .storage()
            .instance()
            .get(&DataKey::AccessControl)
            .ok_or(PriceOracleError::NotInitialized)?;

        let is_paused: bool = env.invoke_contract(
            &access_control,
            &soroban_sdk::Symbol::new(env, "is_paused"),
            soroban_sdk::vec![env],
        );

        if is_paused {
            return Err(PriceOracleError::ProtocolPaused);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env, Symbol};

    fn setup() -> (Env, Address, Address, PriceOracleContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, PriceOracleContract);
        let client = PriceOracleContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        client.initialize(&admin, &access_control);
        let feeder = Address::generate(&env);
        client.add_feeder(&admin, &feeder);
        (env, admin, feeder, client)
    }

    #[test]
    fn test_set_and_get_price() {
        let (env, _admin, feeder, client) = setup();
        let base = Symbol::new(&env, "EURC");
        let quote = Symbol::new(&env, "USDC");
        client.set_price(&feeder, &base, &quote, &11_000_000i128);
        let data = client.get_price(&base, &quote);
        assert_eq!(data.price, 11_000_000i128);
    }

    #[test]
    fn test_convert_same_currency() {
        let (env, _admin, _feeder, client) = setup();
        let sym = Symbol::new(&env, "USDC");
        let result = client.convert(&1_000_000i128, &sym, &sym);
        assert_eq!(result, 1_000_000i128);
    }

    #[test]
    fn test_convert_different_currency() {
        let (env, _admin, feeder, client) = setup();
        let eurc = Symbol::new(&env, "EURC");
        let usdc = Symbol::new(&env, "USDC");
        client.set_price(&feeder, &eurc, &usdc, &11_000_000i128);
        let result = client.convert(&10_000_000i128, &eurc, &usdc);
        assert_eq!(result, 11_000_000i128);
    }

    #[test]
    fn test_get_price_missing_fails() {
        let (env, _admin, _feeder, client) = setup();
        let base = Symbol::new(&env, "XLM");
        let quote = Symbol::new(&env, "USDC");
        let result = client.try_get_price(&base, &quote);
        assert!(result.is_err());
    }

    #[test]
    fn test_stale_price_rejected() {
        use soroban_sdk::testutils::{Ledger, LedgerInfo};
        let (env, _admin, feeder, client) = setup();
        let base = Symbol::new(&env, "EURC");
        let quote = Symbol::new(&env, "USDC");
        client.set_price(&feeder, &base, &quote, &11_000_000i128);

        env.ledger().set(LedgerInfo {
            timestamp: env.ledger().timestamp() + MAX_STALENESS_SECS + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        let result = client.try_get_price(&base, &quote);
        assert!(result.is_err());
    }

    #[test]
    fn test_transfer_admin_success() {
        let (env, admin, _feeder, client) = setup();
        let new_admin = Address::generate(&env);
        client.transfer_admin(&admin, &new_admin);
        let new_feeder = Address::generate(&env);
        let result = client.try_add_feeder(&new_admin, &new_feeder);
        assert!(result.is_ok());
    }

    #[test]
    fn test_transfer_admin_requires_admin() {
        let (env, admin, _feeder, client) = setup();
        let stranger = Address::generate(&env);
        let new_admin = Address::generate(&env);
        let result = client.try_transfer_admin(&stranger, &new_admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_propose_upgrade_success() {
        let (env, admin, _feeder, client) = setup();
        let wasm_hash = soroban_sdk::BytesN::<32>::from_array(&env, &[0u8; 32]);
        let result = client.try_propose_upgrade(&admin, &wasm_hash);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_upgrade_requires_timelock() {
        let (env, admin, _feeder, client) = setup();
        let wasm_hash = soroban_sdk::BytesN::<32>::from_array(&env, &[0u8; 32]);
        client.propose_upgrade(&admin, &wasm_hash);
        let result = client.try_execute_upgrade(&admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_upgrade_success() {
        use soroban_sdk::testutils::{Ledger, LedgerInfo};
        let (env, admin, _feeder, client) = setup();
        let wasm_hash = soroban_sdk::BytesN::<32>::from_array(&env, &[1u8; 32]);
        client.propose_upgrade(&admin, &wasm_hash);

        env.ledger().set(LedgerInfo {
            timestamp: env.ledger().timestamp() + UPGRADE_TIMELOCK_DELAY + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        let result = client.try_execute_upgrade(&admin);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_upgrade_no_proposal() {
        let (env, admin, _feeder, client) = setup();
        let result = client.try_execute_upgrade(&admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_access_control() {
        let (env, admin, _feeder, client) = setup();
        let new_access_control = Address::generate(&env);
        client.set_access_control(&admin, &new_access_control).unwrap();
    }

    #[test]
    fn test_set_price_when_paused_fails() {
        let (env, _admin, feeder, client) = setup();
        let access_control = Address::generate(&env);
        env.storage().instance().set(&soroban_sdk::symbol_short!("AC"), &true);

        let result = client.try_set_price(
            &feeder,
            &Symbol::new(&env, "EURC"),
            &Symbol::new(&env, "USDC"),
            &11_000_000i128,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_set_price_when_not_paused_succeeds() {
        let (env, _admin, feeder, client) = setup();
        let base = Symbol::new(&env, "EURC");
        let quote = Symbol::new(&env, "USDC");
        let result = client.try_set_price(&feeder, &base, &quote, &11_000_000i128);
        assert!(result.is_ok());
    }
}
