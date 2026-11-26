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
    /// A fill would take more than the listing's remaining size.
    Overfill = 12,
    /// The configured fee recipient is not on the allowlist (fails closed).
    FeeRecipientNotAllowed = 13,
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
    /// Whether an address is permitted to receive protocol fees (allowlist).
    FeeRecipientAllowed(Address),
}

/// Settlement breakdown for one fill of a listing (fee disclosure).
///
/// Produced by [`quote_fill`]. For every quote:
/// `fee + seller_proceeds == gross` and `filled_after + remaining == size`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FillQuote {
    /// Amount the buyer pays for this fill.
    pub gross: i128,
    /// Protocol fee routed to the treasury.
    pub fee: i128,
    /// Amount routed to the seller (`gross - fee`).
    pub seller_proceeds: i128,
    /// Cumulative size filled after this fill.
    pub filled_after: i128,
    /// Size still open after this fill; `0` means the listing is closed.
    pub remaining: i128,
}

/// Cumulative price owed for the first `filled` units of a `size`-unit
/// listing priced at `price` in total: `floor(price * filled / size)`.
fn cumulative_price(price: i128, size: i128, filled: i128) -> Result<i128, SecondaryMarketError> {
    price
        .checked_mul(filled)
        .and_then(|v| v.checked_div(size))
        .ok_or(SecondaryMarketError::InvalidAmount)
}

/// Quote one (partial or exact) fill of a listing.
///
/// A listing has a total `price` for `size` units, of which `filled` units are
/// already sold. Buying `fill_amount` more units is priced and charged on a
/// **cumulative** basis — the gross and fee for this fill are the difference
/// between the cumulative totals after and before it — so that across any
/// sequence of fills that fully closes the listing:
///
/// - `sum(gross) == price`
/// - `sum(fee) == bps_of(price, fee_bps)` (no per-fill rounding drift)
/// - `sum(seller_proceeds) == price - bps_of(price, fee_bps)`
///
/// See `docs/secondary-market.md` for the full invariant list.
///
/// **Errors:**
/// - `SecondaryMarketError::InvalidAmount` — non-positive `price`, `size` or
///   `fill_amount`; `filled` outside `0..=size`; or a fill too small to carry
///   a non-zero price.
/// - `SecondaryMarketError::Overfill` — `fill_amount > size - filled`
///   (includes any fill on an already-closed listing).
/// - `SecondaryMarketError::InvalidFeeRate` — `fee_bps` exceeds 1000.
pub fn quote_fill(
    price: i128,
    size: i128,
    filled: i128,
    fill_amount: i128,
    fee_bps: u32,
) -> Result<FillQuote, SecondaryMarketError> {
    require_valid_bps_range(fee_bps, 0, MAX_PROTOCOL_FEE_BPS)?;
    if price <= 0 || size <= 0 || fill_amount <= 0 || filled < 0 || filled > size {
        return Err(SecondaryMarketError::InvalidAmount);
    }
    let remaining_before = safe_sub(size, filled)?;
    if fill_amount > remaining_before {
        return Err(SecondaryMarketError::Overfill);
    }
    let filled_after = filled
        .checked_add(fill_amount)
        .ok_or(SecondaryMarketError::InvalidAmount)?;

    let cum_before = cumulative_price(price, size, filled)?;
    let cum_after = cumulative_price(price, size, filled_after)?;
    let gross = safe_sub(cum_after, cum_before)?;
    if gross <= 0 {
        return Err(SecondaryMarketError::InvalidAmount);
    }

    let fee = safe_sub(bps_of(cum_after, fee_bps)?, bps_of(cum_before, fee_bps)?)?;
    let seller_proceeds = safe_sub(gross, fee)?;
    let remaining = safe_sub(size, filled_after)?;

    Ok(FillQuote {
        gross,
        fee,
        seller_proceeds,
        filled_after,
        remaining,
    })
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

/// Marks `recipient` as a permitted fee recipient (idempotent).
fn set_fee_recipient_allowed(env: &Env, recipient: &Address, allowed: bool) {
    env.storage()
        .persistent()
        .set(&DataKey::FeeRecipientAllowed(recipient.clone()), &allowed);
}

/// True when `recipient` is on the fee-recipient allowlist. Defaults to false
/// (fail closed) when no entry exists.
fn is_fee_recipient_allowed_storage(env: &Env, recipient: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::FeeRecipientAllowed(recipient.clone()))
        .unwrap_or(false)
}

/// Ensures `recipient` is on the fee-recipient allowlist; fails closed with
/// `FeeRecipientNotAllowed` otherwise so fees can never flow to an unbounded
/// address (preventing fee skimming).
fn require_fee_recipient_allowed(
    env: &Env,
    recipient: &Address,
) -> Result<(), SecondaryMarketError> {
    if is_fee_recipient_allowed_storage(env, recipient) {
        Ok(())
    } else {
        Err(SecondaryMarketError::FeeRecipientNotAllowed)
    }
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

        // Auto-allowlist the configured treasury so existing deployments keep
        // routing fees to it; every fee recipient must be on the allowlist.
        set_fee_recipient_allowed(&env, &treasury, true);

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
    /// - `SecondaryMarketError::FeeRecipientNotAllowed` — Treasury is not on the fee-recipient allowlist (fails closed).
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

        // Fee recipient allowlist: fees may only flow to an allowlisted address.
        // Fails closed (FeeRecipientNotAllowed) if the treasury was removed.
        require_fee_recipient_allowed(&env, &treasury)?;

        // Checks-effects-interactions: clear the listing before external calls.
        env.storage()
            .persistent()
            .remove(&DataKey::Listing(invoice_id, seller.clone()));

        // Settle payment: fee to treasury, remainder to seller. A whole-position
        // purchase is the exact fill of a single-unit listing.
        let quote = quote_fill(listing.price, 1, 0, 1, fee_bps)?;

        let token_client = soroban_sdk::token::Client::new(&env, &listing.token);
        token_client.transfer(&buyer, &treasury, &quote.fee);
        token_client.transfer(&buyer, &seller, &quote.seller_proceeds);

        // Atomically reassign position ownership in the financing pool.
        pool_client.transfer_position(&invoice_id, &seller, &buyer);

        events::position_sold(&env, invoice_id, &seller, &buyer, listing.price);
        Ok(())
    }

    /// Discloses the settlement breakdown a buyer would pay for an active
    /// listing at the current protocol fee rate. Read-only.
    ///
    /// **Errors:**
    /// - `SecondaryMarketError::ListingNotFound` — No active listing exists.
    pub fn quote_purchase(
        env: Env,
        invoice_id: u64,
        seller: Address,
    ) -> Result<FillQuote, SecondaryMarketError> {
        let listing: PositionSaleOffer = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id, seller))
            .ok_or(SecondaryMarketError::ListingNotFound)?;
        let fee_bps: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(DEFAULT_PROTOCOL_FEE_BPS);
        quote_fill(listing.price, 1, 0, 1, fee_bps)
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

    /// Adds an address to the fee-recipient allowlist. Admin-only.
    ///
    /// The protocol fee at settlement is only routed to the configured treasury
    /// while it remains on this allowlist, bounding fee recipients and preventing
    /// fee skimming via an unbounded destination. Idempotent: adding an
    /// already-allowed recipient is a no-op.
    ///
    /// **Errors:**
    /// - `SecondaryMarketError::NotAdmin` — caller is not the admin.
    pub fn add_fee_recipient(
        env: Env,
        admin: Address,
        recipient: Address,
    ) -> Result<(), SecondaryMarketError> {
        require_admin(&env, &admin)?;
        set_fee_recipient_allowed(&env, &recipient, true);
        events::secondary_market_fee_recipient_added(&env, &admin, &recipient);
        Ok(())
    }

    /// Removes an address from the fee-recipient allowlist. Admin-only.
    ///
    /// Idempotent. Removing the active treasury causes subsequent
    /// `buy_position` calls to fail closed with `FeeRecipientNotAllowed` until
    /// a permitted recipient is restored, so fees are never routed to an
    /// unallowlisted address.
    ///
    /// **Errors:**
    /// - `SecondaryMarketError::NotAdmin` — caller is not the admin.
    pub fn remove_fee_recipient(
        env: Env,
        admin: Address,
        recipient: Address,
    ) -> Result<(), SecondaryMarketError> {
        require_admin(&env, &admin)?;
        env.storage()
            .persistent()
            .remove(&DataKey::FeeRecipientAllowed(recipient.clone()));
        events::secondary_market_fee_recipient_removed(&env, &admin, &recipient);
        Ok(())
    }

    /// Returns whether `recipient` is on the fee-recipient allowlist.
    pub fn is_fee_recipient_allowed(env: Env, recipient: Address) -> bool {
        is_fee_recipient_allowed_storage(&env, &recipient)
    }
}
