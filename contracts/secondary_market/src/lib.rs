#![no_std]

use kora_shared::{
    errors::CommonError,
    events,
    types::{Position, PositionSaleOffer, Pool},
    validation::{bps_of, safe_sub, require_valid_bps_range},
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype,
    vec, BytesN, Env, Address, Map, Symbol, Vec,
};

/// Default protocol fee: 250 bps (2.5%) on secondary-market trades.
const DEFAULT_PROTOCOL_FEE_BPS: u32 = 250;

/// Maximum allowed protocol fee (1000 bps = 10%).
const MAX_PROTOCOL_FEE_BPS: u32 = 1000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum SecondaryMarketError {
    AlreadyInitialized = 1,
    InvalidAmount = 2,
    InvalidAddress = 3,
    InvalidFeeRate = 4,
    ListingNotFound = 5,
    NotAdmin = 6,
    NotInitialized = 7,
    PoolAlreadyClosed = 8,
    PositionNotFound = 9,
    ProtocolPaused = 10,
    SameAddress = 11,
}

impl From<CommonError> for SecondaryMarketError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => SecondaryMarketError::InvalidAmount,
            CommonError::InvalidAddress => SecondaryMarketError::InvalidAddress,
            CommonError::InvalidFeeRate => SecondaryMarketError::InvalidFeeRate,
            _ => SecondaryMarketError::InvalidAmount,
        }
    }
}

#[contracttype]
pub enum DataKey {
    Admin,
    FinancingPool,
    Treasury,
    ProtocolFeeBps,
    Paused,
    Listing(u64, Address),
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), SecondaryMarketError> {
    caller.require_auth();
    let admin: Address = env
        .storage()
        .persistent()
        .get(&DataKey::Admin)
        .ok_or(SecondaryMarketError::NotInitialized)?;
    if caller != &admin {
        return Err(SecondaryMarketError::NotAdmin);
    }
    Ok(())
}

fn require_not_paused(env: &Env) -> Result<(), SecondaryMarketError> {
    if let Some(paused) = env.storage().persistent().get(&DataKey::Paused) {
        if paused {
            return Err(SecondaryMarketError::ProtocolPaused);
        }
    }
    Ok(())
}

#[contract]
pub struct SecondaryMarket;

#[contractimpl]
impl SecondaryMarket {
    /// Initializes the secondary market contract.
    ///
    /// **Parameters:**
    /// - `admin` — The admin address with governance privileges.
    /// - `financing_pool` — The address of the financing pool contract to
    ///   delegate position transfers to.
    /// - `treasury` — The treasury address that receives protocol fees.
    /// - `protocol_fee_bps` — Protocol fee in basis points (0–1000).
    ///
    /// **Errors:**
    /// - `SecondaryMarketError::AlreadyInitialized` — Contract already initialized.
    /// - `SecondaryMarketError::InvalidFeeRate` — `protocol_fee_bps` exceeds 1000.
    pub fn initialize(
        env: Env,
        admin: Address,
        financing_pool: Address,
        treasury: Address,
        protocol_fee_bps: u32,
    ) -> Result<(), SecondaryMarketError> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(SecondaryMarketError::AlreadyInitialized);
        }

        require_valid_bps_range(protocol_fee_bps, 0, MAX_PROTOCOL_FEE_BPS)?;

        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().persistent().set(&DataKey::Treasury, &treasury);
        env.storage().persistent().set(&DataKey::ProtocolFeeBps, &protocol_fee_bps);
        env.storage().persistent().set(&DataKey::Paused, &false);

        events::secondary_market_initialized(&env, &admin, &financing_pool, protocol_fee_bps);
        Ok(())
    }

    /// Pauses all secondary-market operations. Admin-only.
    pub fn pause(env: Env, admin: Address) -> Result<(), SecondaryMarketError> {
        require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Paused, &true);
        events::secondary_market_paused(&env);
        Ok(())
    }

    /// Unpauses secondary-market operations. Admin-only.
    pub fn unpause(env: Env, admin: Address) -> Result<(), SecondaryMarketError> {
        require_admin(&env, &admin)?;
        env.storage().persistent().set(&DataKey::Paused, &false);
        events::secondary_market_unpaused(&env);
        Ok(())
    }

    /// Updates the protocol fee rate. Admin-only.
    ///
    /// **Parameters:**
    /// - `fee_bps` — New fee in basis points (0–1000 inclusive).
    ///
    /// **Errors:**
    /// - `SecondaryMarketError::InvalidFeeRate` — Fee exceeds 1000 bps.
    pub fn update_protocol_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), SecondaryMarketError> {
        require_admin(&env, &admin)?;
        require_valid_bps_range(fee_bps, 0, MAX_PROTOCOL_FEE_BPS)?;
        env.storage().persistent().set(&DataKey::ProtocolFeeBps, &fee_bps);
        events::secondary_market_fee_updated(&env, fee_bps);
        Ok(())
    }

    /// Lists an investor position for sale on the secondary market.
    ///
    /// The seller must hold a position in the financing pool for `invoice_id`.
    /// A position can only have one active listing at a time.
    ///
    /// **Parameters:**
    /// - `seller` — The address listing the position (must authenticate).
    /// - `invoice_id` — The invoice ID of the pool.
    /// - `token` — The token address used for pricing (same as pool token).
    /// - `price` — The asking price (must be positive).
    ///
    /// **Errors:**
    /// - `SecondaryMarketError::ProtocolPaused` — Contract is paused.
    /// - `SecondaryMarketError::InvalidAmount` — Price is not positive.
    /// - `SecondaryMarketError::PoolAlreadyClosed` — Pool is closed.
    /// - `SecondaryMarketError::PositionNotFound` — Seller holds no position.
    /// - `SecondaryMarketError::AlreadyInitialized` — A listing already exists.
    pub fn list_position(
        env: Env,
        seller: Address,
        invoice_id: u64,
        token: Address,
        price: i128,
    ) -> Result<(), SecondaryMarketError> {
        seller.require_auth();
        require_not_paused(&env)?;

        if price <= 0 {
            return Err(SecondaryMarketError::InvalidAmount);
        }

        let financing_pool: Address = env
            .storage()
            .persistent()
            .get(&DataKey::FinancingPool)
            .ok_or(SecondaryMarketError::NotInitialized)?;

        // Verify the pool exists and is not closed.
        let pool_client = kora_financing_pool::FinancingPoolContractClient::new(&env, &financing_pool);
        let pool: Pool = pool_client
            .try_get_pool(&invoice_id)
            .map_err(|_| SecondaryMarketError::PositionNotFound)?
            .map_err(|_| SecondaryMarketError::PositionNotFound)?;
        if pool.is_closed {
            return Err(SecondaryMarketError::PoolAlreadyClosed);
        }

        // Verify the seller holds a position.
        let position: Position = pool_client.get_position(&invoice_id, &seller);
        if position.investor != seller {
            return Err(SecondaryMarketError::PositionNotFound);
        }

        // Ensure no existing listing.
        if env
            .storage()
            .persistent()
            .has(&DataKey::Listing(invoice_id, seller.clone()))
        {
            return Err(SecondaryMarketError::AlreadyInitialized);
        }

        let listing = PositionSaleOffer {
            seller: seller.clone(),
            invoice_id,
            token,
            price,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id, seller.clone()), &listing);

        events::position_listed_for_sale(&env, invoice_id, &seller, price);
        Ok(())
    }

    /// Purchases a listed position from the secondary market.
    ///
    /// The buyer pays `price` tokens. A protocol fee (in bps) is deducted;
    /// the remainder goes to the seller. The position ownership is then
    /// transferred on the financing pool contract.
    ///
    /// **Parameters:**
    /// - `buyer` — The address purchasing the position (must authenticate).
    /// - `invoice_id` — The invoice ID of the pool.
    /// - `seller` — The address of the current position holder / lister.
    ///
    /// **Errors:**
    /// - `SecondaryMarketError::ProtocolPaused` — Contract is paused.
    /// - `SecondaryMarketError::ListingNotFound` — No active listing exists.
    /// - `SecondaryMarketError::SameAddress` — Buyer is the seller.
    /// - `SecondaryMarketError::PoolAlreadyClosed` — Pool is closed.
    pub fn buy_position(
        env: Env,
        buyer: Address,
        invoice_id: u64,
        seller: Address,
    ) -> Result<(), SecondaryMarketError> {
        buyer.require_auth();
        require_not_paused(&env)?;

        if buyer == seller {
            return Err(SecondaryMarketError::SameAddress);
        }

        let listing: PositionSaleOffer = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id, seller.clone()))
            .ok_or(SecondaryMarketError::ListingNotFound)?;

        let financing_pool: Address = env
            .storage()
            .persistent()
            .get(&DataKey::FinancingPool)
            .ok_or(SecondaryMarketError::NotInitialized)?;
        let treasury: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Treasury)
            .ok_or(SecondaryMarketError::NotInitialized)?;
        let fee_bps: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(DEFAULT_PROTOCOL_FEE_BPS);

        let pool_client = kora_financing_pool::FinancingPoolContractClient::new(&env, &financing_pool);
        let pool: Pool = pool_client
            .try_get_pool(&invoice_id)
            .map_err(|_| SecondaryMarketError::PositionNotFound)?
            .map_err(|_| SecondaryMarketError::PositionNotFound)?;
        if pool.is_closed {
            return Err(SecondaryMarketError::PoolAlreadyClosed);
        }

        // Checks-effects-interactions: clear the listing before external calls.
        env.storage()
            .persistent()
            .remove(&DataKey::Listing(invoice_id, seller.clone()));

        // Settle payment: fee to treasury, remainder to seller.
        let fee = bps_of(listing.price, fee_bps)?;
        let seller_proceeds = safe_sub(listing.price, fee)?;

        let token_client = soroban_sdk::token::Client::new(&env, &listing.token);
        token_client.transfer(&buyer, &treasury, &fee);
        token_client.transfer(&buyer, &seller, &seller_proceeds);

        // Atomically reassign position ownership in the financing pool.
        pool_client.transfer_position(&invoice_id, &seller, &buyer);

        events::position_sold(&env, invoice_id, &seller, &buyer, listing.price);
        Ok(())
    }

    /// Cancels an active listing. Only the seller may cancel.
    ///
    /// **Parameters:**
    /// - `seller` — The address that created the listing (must authenticate).
    /// - `invoice_id` — The invoice ID of the pool.
    ///
    /// **Errors:**
    /// - `SecondaryMarketError::ListingNotFound` — No active listing exists.
    pub fn cancel_listing(
        env: Env,
        seller: Address,
        invoice_id: u64,
    ) -> Result<(), SecondaryMarketError> {
        seller.require_auth();

        let key = DataKey::Listing(invoice_id, seller.clone());
        if !env.storage().persistent().has(&key) {
            return Err(SecondaryMarketError::ListingNotFound);
        }
        env.storage().persistent().remove(&key);

        events::listing_cancelled(&env, invoice_id, &seller);
        Ok(())
    }
}
