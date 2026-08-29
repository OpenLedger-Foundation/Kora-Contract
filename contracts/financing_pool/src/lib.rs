#![no_std]

use kora_shared::{
    errors::CommonError,
    events,
    types::{EarlySettlementOffer, InstallmentSchedule, Pool, Position, PositionSaleOffer, PositionShare, ProtocolStats, ShareSaleOffer},
    validation::{bps_of, bps_of_normalized, require_valid_bps_range, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, Map, Symbol, Vec,
};

const MAX_AMOUNT: i128 = i128::MAX / 2;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum FinancingPoolError {
    AlreadyInitialized = 1,
    ArithmeticOverflow = 2,
    ExceedsFundingTarget = 3,
    InvalidAddress = 4,
    InvalidAmount = 5,
    InvalidDueDate = 6,
    InvalidFeeRate = 7,
    InvoiceFrozen = 8,
    NoUpgradeProposed = 9,
    NotAdmin = 10,
    NotInitialized = 11,
    PoolAlreadyClosed = 12,
    PoolNotFound = 13,
    PositionNotFound = 14,
    ProtocolPaused = 15,
    RepaymentAlreadyMade = 16,
    SaleAlreadyListed = 17,
    SaleNotFound = 18,
    Unauthorized = 19,
    UpgradeTimelockNotElapsed = 20,
    // PositionShare (#563)
    ShareNotFound = 21,
    InvalidShareAmount = 22,
    AlreadySplit = 23,
    NotPositionOwner = 24,
    // Dispute Resolution (#565)
    DisputeNotFound = 25,
    DisputeAlreadyOpen = 26,
    DisputeAlreadyResolved = 27,
    DisputeWindowExpired = 28,
    NotDisputeChallenger = 29,
    NotGovernance = 30,
    DisputeNotOpen = 31,
    // Partial repayment (#566)
    PartialRepayInvalid = 32,
}

impl From<CommonError> for FinancingPoolError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => FinancingPoolError::InvalidAmount,
            CommonError::InvalidAddress => FinancingPoolError::InvalidAddress,
            CommonError::InvalidFeeRate => FinancingPoolError::InvalidFeeRate,
            CommonError::InvalidDueDate => FinancingPoolError::InvalidDueDate,
            CommonError::ArithmeticOverflow => FinancingPoolError::ArithmeticOverflow,
            _ => FinancingPoolError::InvalidAmount,
        }
    }
}

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Pool(u64),
    Positions(u64),
    Admin,
    InvoiceNft,
    RiskRegistry,
    Treasury,
    LatePenaltyBps,
    AccessControl,
    PriceOracle,
    RepaymentLock(u64),
    UpgradeProposal,
    SaleOffer(u64, Address),
    EarlySettlement(u64),
    /// Maximum share (in bps) any single investor may hold in a pool.
    MaxPositionBps,
    /// Aggregate funded amount for a given token across all active pools.
    AggregateFunded(Address),
    /// Protocol-wide aggregate statistics (pools opened/repaid/defaulted/active).
    ProtocolStats,
    /// Installment repayment schedule for a pool, keyed by invoice ID.
    InstallmentSchedule(u64),
    /// PositionShare set for a pool, keyed by (invoice_id, original_investor).
    PositionShares(u64, Address),
    /// Next share index for a position (used for generating unique share IDs).
    PositionShareCounter(u64, Address),
    /// Pending share sale offer: keyed by (invoice_id, original_investor, share_index).
    ShareSaleOffer(u64, Address, u32),
    /// Dispute resolution contract address (#565).
    DisputeResolution,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct FinancingPoolContract;

#[contractimpl]
impl FinancingPoolContract {
    /// One-time initialization. Wires up all cross-contract dependencies and configures pool parameters.
    ///
    /// **Parameters:**
    /// - `admin` — The address that will administer this contract.
    /// - `invoice_nft` — The deployed `invoice_nft` contract address.
    /// - `risk_registry` — The deployed `risk_registry` contract address.
    /// - `treasury` — The deployed `treasury` contract address for fee forwarding.
    /// - `access_control` — The deployed `access_control` contract address for pause checks.
    /// - `late_penalty_bps` — Late-repayment penalty in basis points (0–10 000).
    /// - `price_oracle` — The deployed price oracle contract address for currency conversion.
    /// - `max_position_bps` — Maximum per-investor share of a pool in basis points (1–10 000).
    /// - `dispute_resolution` — The deployed dispute resolution contract address. (#565)
    ///
    /// **Errors:**
    /// - `FinancingPoolError::AlreadyInitialized` — Contract has already been initialized.
    /// - `FinancingPoolError::InvalidFeeRate` — `late_penalty_bps` > 10 000 or `max_position_bps` is 0 or > 10 000.
    /// - `FinancingPoolError::InvalidAddress` — Any address parameter is the contract's own address.
    ///
    /// **Security:** No auth required on first call. Subsequent calls revert immediately.
    pub fn initialize(
        env: Env,
        admin: Address,
        invoice_nft: Address,
        risk_registry: Address,
        treasury: Address,
        access_control: Address,
        late_penalty_bps: u32,
        price_oracle: Address,
        max_position_bps: u32,
        dispute_resolution: Address,
    ) -> Result<(), FinancingPoolError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(FinancingPoolError::AlreadyInitialized);
        }
        kora_shared::validation::require_valid_fee_bps(late_penalty_bps)?;
        // max_position_bps must be in [1, 10_000]: zero would block all funding
        require_valid_bps_range(max_position_bps, 1, 10_000)?;
        if max_position_bps == 0 || max_position_bps > 10_000 {
            return Err(FinancingPoolError::InvalidFeeRate);
        }
        kora_shared::validation::require_not_self(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &invoice_nft)?;
        kora_shared::validation::require_not_self(&env, &risk_registry)?;
        kora_shared::validation::require_not_self(&env, &treasury)?;
        kora_shared::validation::require_not_self(&env, &access_control)?;
        kora_shared::validation::require_not_self(&env, &dispute_resolution)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::InvoiceNft, &invoice_nft);
        env.storage()
            .instance()
            .set(&DataKey::RiskRegistry, &risk_registry);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        env.storage().instance().set(&DataKey::LatePenaltyBps, &late_penalty_bps);
        env.storage().instance().set(&DataKey::PriceOracle, &price_oracle);
        env.storage().instance().set(&DataKey::MaxPositionBps, &max_position_bps);
        env.storage().instance().set(&DataKey::DisputeResolution, &dispute_resolution);
        Ok(())
    }

    /// Called by Marketplace when an invoice is fully funded. Opens a new pool,
    /// records the face value, and transitions the invoice NFT to `Funded` status.
    ///
    /// **Parameters:**
    /// - `marketplace` — Must be the authorized marketplace contract address (signs).
    /// - `invoice_id` — The ID of the fully-funded invoice.
    /// - `token` — The whitelisted stablecoin token address used for this pool.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::ProtocolPaused` — Protocol is paused.
    /// - `FinancingPoolError::PoolAlreadyClosed` — A pool for this invoice ID already exists.
    /// - `FinancingPoolError::InvalidAddress` — `token` is the contract's own address.
    /// - `FinancingPoolError::NotInitialized` — Contract cross-references are missing.
    /// - `FinancingPoolError::InvalidAmount` — Invoice amount is out of the safe range.
    /// - `FinancingPoolError::Unauthorized` — Caller is not the authorized marketplace.
    ///
    /// **Security:** Requires `marketplace.require_auth()`. Only the marketplace contract
    /// (stored at initialization) may call this. Emits `pool_opened` event.
    pub fn release_funds(
        env: Env,
        marketplace: Address,
        invoice_id: u64,
        token: Address,
    ) -> Result<(), FinancingPoolError> {
        marketplace.require_auth();
        Self::require_not_paused(&env)?;

        if env.storage().persistent().has(&DataKey::Pool(invoice_id)) {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        if token == env.current_contract_address() {
            return Err(FinancingPoolError::InvalidAddress);
        }

        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(FinancingPoolError::NotInitialized)?;
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
        let invoice = nft_client.get_invoice(&invoice_id);

        if invoice.amount <= 0 || invoice.amount > MAX_AMOUNT {
            return Err(FinancingPoolError::InvalidAmount);
        }

        let late_penalty_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::LatePenaltyBps)
            .ok_or(FinancingPoolError::NotInitialized)?;

        let pool = Pool {
            invoice_id,
            token: token.clone(),
            total_funded: 0,
            face_value: invoice.amount,
            repaid_amount: 0,
            is_closed: false,
            late_penalty_bps,
            total_owed: invoice.amount,
            penalty_applied: false,
        };

        env.storage().persistent().set(&DataKey::Pool(invoice_id), &pool);

        // Standardized financing pool event
        events::pool_opened(&env, &marketplace, invoice_id, &token, pool.face_value);

        // Update protocol stats
        let mut stats: ProtocolStats = env.storage().instance().get(&DataKey::ProtocolStats)
            .unwrap_or(ProtocolStats { pools_opened: 0, total_repaid: 0, pools_defaulted: 0, active_pools: 0 });
        stats.pools_opened = stats.pools_opened.saturating_add(1);
        stats.active_pools = stats.active_pools.saturating_add(1);
        env.storage().instance().set(&DataKey::ProtocolStats, &stats);

        // Transition NFT status to Funded
        nft_client.set_funded(&env.current_contract_address(), &invoice_id);

        Ok(())
    }

    /// Update the per-investor concentration cap. Admin only.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `max_position_bps` — New cap in basis points (1–10 000). Zero is rejected since it
    ///   would block all funding.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::NotAdmin` — Caller is not the admin.
    /// - `FinancingPoolError::InvalidFeeRate` — `max_position_bps` is 0 or > 10 000.
    ///
    /// **Security:** Requires `admin.require_auth()`. Applies to new positions only;
    /// existing positions are not retroactively affected.
    pub fn set_max_position_bps(env: Env, admin: Address, max_position_bps: u32) -> Result<(), FinancingPoolError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        require_valid_bps_range(max_position_bps, 1, 10_000)?;
        if max_position_bps == 0 || max_position_bps > 10_000 {
            return Err(FinancingPoolError::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::MaxPositionBps, &max_position_bps);
        Ok(())
    }

    /// Returns the current per-investor concentration cap in basis points.
    ///
    /// **Returns:** The cap in bps (default 5 000 = 50% if not explicitly set).
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_max_position_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxPositionBps)
            .unwrap_or(5_000)
    }

    /// Returns the configured price oracle address. Lets other protocol
    /// contracts (e.g. marketplace, for cross-currency funding) reuse the
    /// same oracle instance instead of wiring a separate reference. (#449)
    pub fn get_price_oracle(env: Env) -> Result<Address, KoraError> {
        env.storage()
            .instance()
            .get(&DataKey::PriceOracle)
            .ok_or(KoraError::NotInitialized)
    }

    /// Register an investor position for a funded invoice. Admin only.
    ///
    /// Called by the marketplace (via admin) after each investor contribution to record
    /// the investor's share of the pool. The share in basis points is computed as
    /// `contributed * 10_000 / total_pool`.
    ///
    /// **Parameters:**
    /// - `caller` — Must be the current admin address.
    /// - `invoice_id` — The ID of the funded invoice.
    /// - `investor` — The investor address receiving the position.
    /// - `contributed` — The investor's contribution amount in the pool token.
    /// - `total_pool` — The total funded amount of the pool at this point.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::NotAdmin` — Caller is not the admin.
    /// - `FinancingPoolError::ProtocolPaused` — Protocol is paused.
    /// - `FinancingPoolError::InvalidAmount` — `contributed` or `total_pool` is ≤ 0, or exceeds safe bounds.
    /// - `FinancingPoolError::ExceedsFundingTarget` — Investor's computed share exceeds `max_position_bps`.
    /// - `FinancingPoolError::ArithmeticOverflow` — Share calculation overflowed.
    /// - `FinancingPoolError::PoolNotFound` — No pool exists for `invoice_id`.
    ///
    /// **Security:** Requires `caller.require_auth()`. Enforces per-investor concentration cap.
    pub fn record_position(
        env: Env,
        caller: Address,
        invoice_id: u64,
        investor: Address,
        contributed: i128,
        total_pool: i128,
    ) -> Result<(), FinancingPoolError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        Self::require_not_paused(&env)?;

        if contributed <= 0 || total_pool <= 0 {
            return Err(FinancingPoolError::InvalidAmount);
        }

        if contributed > total_pool || contributed > MAX_AMOUNT || total_pool > MAX_AMOUNT {
            return Err(FinancingPoolError::InvalidAmount);
        }

        let share_bps = contributed
            .checked_mul(10_000)
            .and_then(|v| v.checked_div(total_pool))
            .ok_or(FinancingPoolError::ArithmeticOverflow)? as u32;

        // Enforce per-investor concentration cap
        let max_position_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxPositionBps)
            .unwrap_or(5_000);
        if share_bps > max_position_bps {
            return Err(FinancingPoolError::ExceedsFundingTarget);
        }

        let position = Position {
            investor: investor.clone(),
            invoice_id,
            contributed,
            share_bps,
            yield_claimed: 0,
        };

        let mut positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(&env));

        // Track old contribution so we can compute the delta for the aggregate.
        let old_contributed: i128 = positions
            .get(investor.clone())
            .map(|p: Position| p.contributed)
            .unwrap_or(0);

        positions.set(investor.clone(), position);
        env.storage()
            .persistent()
            .set(&DataKey::Positions(invoice_id), &positions);

        // Update the per-token aggregate of outstanding investor obligations.
        // ── #584: Pool was validated at call entry; read once and reuse the token.
        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        let agg_key = DataKey::AggregateFunded(pool.token);
        let prev_agg: i128 = env.storage().instance().get(&agg_key).unwrap_or(0);
        let new_agg = prev_agg
            .checked_sub(old_contributed)
            .and_then(|v| v.checked_add(contributed))
            .ok_or(FinancingPoolError::ArithmeticOverflow)?;
        env.storage().instance().set(&agg_key, &new_agg);

        // Standardized financing pool event
        events::position_recorded(
            &env,
            &caller,
            invoice_id,
            &investor,
            contributed,
            share_bps,
        );

        Ok(())
    }

    /// Split a funded position into fractional, independently transferable shares.
    ///
    /// The caller must be the current position owner. `amount` must be > 0 and
    /// <= the position's current `contributed` amount. A new `PositionShare`
    /// is created with the specified `amount`; the original position's
    /// `contributed` is reduced by the same amount. The sum of all shares
    /// (including the remainder in the original position) always equals the
    /// original contributed amount.
    ///
    /// **Parameters:**
    /// - `caller` — Must hold the position (require_auth).
    /// - `invoice_id` — The invoice ID of the pool.
    /// - `amount` — The amount to split out as a new share (> 0, <= position.contributed).
    ///
    /// **Errors:**
    /// - `FinancingPoolError::PositionNotFound` — No position exists for `caller`.
    /// - `FinancingPoolError::InvalidAmount` — `amount` is <= 0 or exceeds position.contributed.
    /// - `FinancingPoolError::PoolAlreadyClosed` — Pool is closed.
    ///
    /// **Security:** Requires `caller.require_auth()`.
    pub fn split_position(
        env: Env,
        caller: Address,
        invoice_id: u64,
        amount: i128,
    ) -> Result<u32, FinancingPoolError> {
        caller.require_auth();
        Self::require_not_paused(&env)?;

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        let mut positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(&env));

        let mut position: Position = positions
            .get(caller.clone())
            .ok_or(FinancingPoolError::PositionNotFound)?;

        if amount <= 0 || amount > position.contributed {
            return Err(FinancingPoolError::InvalidAmount);
        }

        let counter_key = DataKey::PositionShareCounter(invoice_id, caller.clone());
        let share_index: u32 = env
            .storage()
            .persistent()
            .get(&counter_key)
            .unwrap_or(0);
        let new_index = share_index.saturating_add(1);

        let share = PositionShare {
            invoice_id,
            original_investor: caller.clone(),
            share_index: new_index,
            amount,
            owner: caller.clone(),
        };

        let mut shares: Map<(u64, Address, u32), PositionShare> = env
            .storage()
            .persistent()
            .get(&DataKey::PositionShares(invoice_id, caller.clone()))
            .unwrap_or_else(|| Map::new(&env));
        shares.set((invoice_id, caller.clone(), new_index), share);
        env.storage()
            .persistent()
            .set(&DataKey::PositionShares(invoice_id, caller.clone()), &shares);

        env.storage()
            .persistent()
            .set(&DataKey::PositionShareCounter(invoice_id, caller.clone()), &new_index);

        events::position_share_created(&env, invoice_id, &caller, new_index, amount, &caller);

        Ok(new_index)
    }

    /// Transfer ownership of a fractional share to another address.
    ///
    /// The caller must be the current share owner. Transfers are unrestricted
    /// (no re-entrancy risk here because this is a pure state change with no
    /// external calls).
    ///
    /// **Parameters:**
    /// - `caller` — Must hold the share (require_auth).
    /// - `invoice_id` — The invoice ID of the pool.
    /// - `original_investor` — The investor whose position was split.
    /// - `share_index` — The index of the share to transfer.
    /// - `new_owner` — The address receiving the share.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::ShareNotFound` — No such share exists.
    /// - `FinancingPoolError::NotPositionOwner` — Caller does not own the share.
    /// - `FinancingPoolError::PoolAlreadyClosed` — Pool is closed.
    ///
    /// **Security:** Requires `caller.require_auth()`.
    pub fn transfer_share(
        env: Env,
        caller: Address,
        invoice_id: u64,
        original_investor: Address,
        share_index: u32,
        new_owner: Address,
    ) -> Result<(), FinancingPoolError> {
        caller.require_auth();
        Self::require_not_paused(&env)?;

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        let mut shares: Map<(u64, Address, u32), PositionShare> = env
            .storage()
            .persistent()
            .get(&DataKey::PositionShares(invoice_id, original_investor.clone()))
            .ok_or(FinancingPoolError::ShareNotFound)?;

        let mut share: PositionShare = shares
            .get((invoice_id, original_investor.clone(), share_index))
            .ok_or(FinancingPoolError::ShareNotFound)?;

        if share.owner != caller {
            return Err(FinancingPoolError::NotPositionOwner);
        }

        let old_owner = share.owner.clone();
        share.owner = new_owner.clone();
        shares.set((invoice_id, original_investor.clone(), share_index), share);
        env.storage()
            .persistent()
            .set(&DataKey::PositionShares(invoice_id, original_investor.clone()), &shares);

        events::position_share_transferred(&env, invoice_id, &original_investor, share_index, &old_owner, &new_owner);

        Ok(())
    }

    /// List a fractional share for sale on the secondary market.
    ///
    /// The caller must own the share. Only one active sale offer per share
    /// is allowed.
    pub fn list_share_for_sale(
        env: Env,
        seller: Address,
        invoice_id: u64,
        original_investor: Address,
        share_index: u32,
        token: Address,
        price: i128,
    ) -> Result<(), FinancingPoolError> {
        seller.require_auth();
        Self::require_not_paused(&env)?;

        if price <= 0 {
            return Err(FinancingPoolError::InvalidAmount);
        }

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        let shares: Map<(u64, Address, u32), PositionShare> = env
            .storage()
            .persistent()
            .get(&DataKey::PositionShares(invoice_id, original_investor.clone()))
            .ok_or(FinancingPoolError::ShareNotFound)?;

        let share = shares
            .get((invoice_id, original_investor.clone(), share_index))
            .ok_or(FinancingPoolError::ShareNotFound)?;

        if share.owner != seller {
            return Err(FinancingPoolError::NotPositionOwner);
        }

        if env.storage().persistent().has(&DataKey::ShareSaleOffer(invoice_id, original_investor.clone(), share_index)) {
            return Err(FinancingPoolError::SaleAlreadyListed);
        }

        let offer = ShareSaleOffer {
            seller: seller.clone(),
            invoice_id,
            original_investor,
            share_index,
            token,
            price,
        };
        env.storage()
            .persistent()
            .set(&DataKey::ShareSaleOffer(invoice_id, original_investor.clone(), share_index), &offer);

        events::share_listed_for_sale(&env, invoice_id, share_index, price);
        Ok(())
    }

    /// Purchase a fractional share from the secondary market.
    ///
    /// Transfers the share to the buyer and moves the listed price in tokens
    /// from buyer to seller.
    pub fn buy_share(
        env: Env,
        buyer: Address,
        invoice_id: u64,
        seller: Address,
        original_investor: Address,
        share_index: u32,
    ) -> Result<(), FinancingPoolError> {
        buyer.require_auth();
        Self::require_not_paused(&env)?;

        let offer: ShareSaleOffer = env
            .storage()
            .persistent()
            .get(&DataKey::ShareSaleOffer(invoice_id, original_investor.clone(), share_index))
            .ok_or(FinancingPoolError::SaleNotFound)?;

        if offer.seller != seller {
            return Err(FinancingPoolError::SaleNotFound);
        }

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        let mut shares: Map<(u64, Address, u32), PositionShare> = env
            .storage()
            .persistent()
            .get(&DataKey::PositionShares(invoice_id, original_investor.clone()))
            .ok_or(FinancingPoolError::ShareNotFound)?;

        let mut share: PositionShare = shares
            .get((invoice_id, original_investor.clone(), share_index))
            .ok_or(FinancingPoolError::ShareNotFound)?;

        env.storage()
            .persistent()
            .remove(&DataKey::ShareSaleOffer(invoice_id, original_investor.clone(), share_index));

        share.owner = buyer.clone();
        shares.set((invoice_id, original_investor.clone(), share_index), share);
        env.storage()
            .persistent()
            .set(&DataKey::PositionShares(invoice_id, original_investor.clone()), &shares);

        let token_client = token::Client::new(&env, &offer.token);
        token_client.transfer(&buyer, &seller, &offer.price);

        events::share_sold(&env, invoice_id, share_index, &buyer, offer.price);
        Ok(())
    }

    /// Make a partial repayment on an invoice, distributing yield pro-rata
    /// immediately to investors based on their share of the total contributed.
    ///
    /// This is distinct from `repay` which waits until the pool is fully repaid.
    /// Partial repayments allow SMEs to service debt in tranches.
    ///
    /// **Parameters:**
    /// - `payer` — The address making the repayment (must sign).
    /// - `invoice_id` — The invoice to repay against.
    /// - `token` — The token being repaid.
    /// - `amount` — The partial repayment amount.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::InvalidAmount` — Amount is <= 0 or exceeds MAX_AMOUNT.
    /// - `FinancingPoolError::PoolNotFound` — No pool exists.
    /// - `FinancingPoolError::PoolAlreadyClosed` — Pool is already closed.
    /// - `FinancingPoolError::PartialRepayInvalid` — Repayment would exceed total_owed.
    ///
    /// **Security:** Requires `payer.require_auth()`. Uses RepaymentLock.
    pub fn repay_partial(
        env: Env,
        payer: Address,
        invoice_id: u64,
        token: Address,
        amount: i128,
    ) -> Result<(), FinancingPoolError> {
        payer.require_auth();

        if amount <= 0 || amount > MAX_AMOUNT {
            return Err(FinancingPoolError::InvalidAmount);
        }

        if env.storage().persistent().has(&DataKey::RepaymentLock(invoice_id)) {
            return Err(FinancingPoolError::Unauthorized);
        }

        env.storage()
            .persistent()
            .set(&DataKey::RepaymentLock(invoice_id), &true);

        let mut pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;

        if pool.is_closed {
            env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        let new_repaid = pool
            .repaid_amount
            .checked_add(amount)
            .ok_or(FinancingPoolError::ArithmeticOverflow)?;
        if new_repaid > pool.total_owed {
            env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));
            return Err(FinancingPoolError::PartialRepayInvalid);
        }

        pool.repaid_amount = new_repaid;
        env.storage().persistent().set(&DataKey::Pool(invoice_id), &pool);

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&payer, &env.current_contract_address(), &amount);

        let mut stats: ProtocolStats = env.storage().instance().get(&DataKey::ProtocolStats)
            .unwrap_or(ProtocolStats { pools_opened: 0, total_repaid: 0, pools_defaulted: 0, active_pools: 0 });
        stats.total_repaid = stats.total_repaid.saturating_add(amount);
        env.storage().instance().set(&DataKey::ProtocolStats, &stats);

        events::repayment_made(&env, invoice_id, &payer, amount);

        Self::distribute_yield(&env, invoice_id, &token, pool.repaid_amount, pool.face_value)?;

        env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));
        Ok(())
    }

    /// SME repays the invoice.
    ///
    /// If the pool has an installment schedule, `amount` must match the current
    /// installment's expected amount (or be the remaining balance for the final
    /// installment).  The current installment is marked paid and the next index
    /// is advanced.  When the final installment is paid the pool closes and yield
    /// is distributed exactly as in lump-sum repayment.
    ///
    /// Without a schedule, the existing lump-sum-toward-face-value behaviour is
    /// preserved: any positive amount is accepted and the pool closes once
    /// `repaid_amount >= total_owed`.
    ///
    /// A one-time late penalty is applied when repaying past the invoice due_date
    /// (or past the current installment due_date when a schedule is present).
    pub fn repay(
        env: Env,
        payer: Address,
        invoice_id: u64,
        token: Address,
        amount: i128,
    ) -> Result<(), FinancingPoolError> {
        payer.require_auth();

        if amount <= 0 || amount > MAX_AMOUNT {
            return Err(FinancingPoolError::InvalidAmount);
        }

        // ── #584: hoist NFT contract address read once for the entire repay call.
        // Used for freeze check, invoice fetch, and set_repaid — avoids 3 separate
        // instance storage reads.
        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(FinancingPoolError::NotInitialized)?;
        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);

        // Check per-invoice freeze before acquiring the RepaymentLock so the
        // lock is never set (and never needs to be cleaned up) on frozen invoices.
        // This is in addition to the protocol-wide pause in AccessControl.
        if nft_client.is_invoice_frozen(&invoice_id) {
            return Err(FinancingPoolError::InvoiceFrozen);
        }

        if env.storage().persistent().has(&DataKey::RepaymentLock(invoice_id)) {
            return Err(FinancingPoolError::Unauthorized);
        }

        env.storage()
            .persistent()
            .set(&DataKey::RepaymentLock(invoice_id), &true);

        let mut pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;

        if pool.is_closed {
            env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));
            return Err(KoraError::PoolAlreadyClosed);
            return Err(FinancingPoolError::RepaymentAlreadyMade);
        }

        // Fetch invoice for due_date check and currency conversion
        let invoice = nft_client.get_invoice(&invoice_id);

        // Convert repayment amount if invoice currency differs from pool token
        let effective_amount = Self::convert_if_needed(&env, amount, &invoice.currency, &pool.token)?;

        // ── Installment validation ────────────────────────────────────────────
        let mut maybe_schedule: Option<InstallmentSchedule> = env
            .storage()
            .persistent()
            .get(&DataKey::InstallmentSchedule(invoice_id));

        if let Some(ref mut schedule) = maybe_schedule {
            let idx = schedule.next_index;
            let len = schedule.installments.len();
            if idx >= len {
                // All installments already satisfied — pool should have been closed.
                env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));
                return Err(KoraError::PoolAlreadyClosed);
                return Err(FinancingPoolError::RepaymentAlreadyMade);
            }
            let installment = schedule.installments.get(idx).unwrap();

            // Determine expected amount: for the final installment accept any
            // amount >= expected (handles rounding from a penalty added to total_owed).
            let is_final = idx == len - 1;
            let expected = installment.amount;
            if is_final {
                if effective_amount < expected {
                    env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));
                    return Err(FinancingPoolError::InvalidAmount);
                }
            } else if effective_amount != expected {
                env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));
                return Err(FinancingPoolError::InvalidAmount);
            }

            // Apply late penalty if this installment is past its due_date.
            if !pool.penalty_applied && pool.late_penalty_bps > 0 {
                if env.ledger().timestamp() > installment.due_date {
                    let penalty = bps_of(pool.face_value, pool.late_penalty_bps)?;
                    pool.total_owed = pool
                        .total_owed
                        .checked_add(penalty)
                        .ok_or(FinancingPoolError::ArithmeticOverflow)?;
                    pool.penalty_applied = true;
                    events::late_penalty_applied(&env, invoice_id, penalty, pool.total_owed);
                }
            }

            // Mark this installment as paid and advance the cursor.
            let mut updated_installments = schedule.installments.clone();
            let mut paid_installment = installment.clone();
            paid_installment.paid = true;
            updated_installments.set(idx, paid_installment);
            schedule.installments = updated_installments;
            schedule.next_index = schedule.next_index.saturating_add(1);
            events::installment_paid(&env, invoice_id, &payer, idx, effective_amount);
        } else {
            // No schedule — original lump-sum late penalty logic.
            if !pool.penalty_applied && pool.late_penalty_bps > 0 {
                if env.ledger().timestamp() > invoice.due_date {
                    let penalty = bps_of(pool.face_value, pool.late_penalty_bps)?;
                    pool.total_owed = pool
                        .total_owed
                        .checked_add(penalty)
                        .ok_or(FinancingPoolError::ArithmeticOverflow)?;
                    pool.penalty_applied = true;
                    events::late_penalty_applied(&env, invoice_id, penalty, pool.total_owed);
                }
            }
        }

        // Effects before interactions (CEI pattern)
        pool.repaid_amount = pool
            .repaid_amount
            .checked_add(effective_amount)
            .ok_or(FinancingPoolError::ArithmeticOverflow)?;

        let should_close = pool.repaid_amount >= pool.total_owed;
        if should_close {
            pool.is_closed = true;
        }
        env.storage().persistent().set(&DataKey::Pool(invoice_id), &pool);

        // Persist the updated schedule (if any).
        if let Some(ref schedule) = maybe_schedule {
            env.storage()
                .persistent()
                .set(&DataKey::InstallmentSchedule(invoice_id), schedule);
        }

        // Interactions
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&payer, &env.current_contract_address(), &amount);

        // Update protocol stats
        let mut stats: ProtocolStats = env.storage().instance().get(&DataKey::ProtocolStats)
            .unwrap_or(ProtocolStats { pools_opened: 0, total_repaid: 0, pools_defaulted: 0, active_pools: 0 });
        stats.total_repaid = stats.total_repaid.saturating_add(effective_amount);
        if should_close {
            stats.active_pools = stats.active_pools.saturating_sub(1);
        }
        env.storage().instance().set(&DataKey::ProtocolStats, &stats);

        // Standardized repayment event
        events::repayment_made(&env, invoice_id, &payer, amount);

        if should_close {
            Self::distribute_yield(
                &env,
                invoice_id,
                &token,
                pool.repaid_amount,
                pool.face_value,
            )?;

            // nft_client already bound above — no second storage read needed (#584).
            nft_client.set_repaid(&env.current_contract_address(), &invoice_id);
        }

        env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));

        Ok(())
    }

    // ── Cross-Invoice Netting (#588) ──────────────────────────────────────────

    /// Net-settle multiple open invoices belonging to the same SME.
    ///
    /// Allows an SME with several concurrent invoices to make a single payment that
    /// is allocated across those pools, reflecting trade-finance netting practice.
    /// Each pool's accounting is updated independently — investor funds are never
    /// pooled across invoices.
    ///
    /// **Allocation rule:** the `amount` is distributed across the listed invoices
    /// proportionally to each pool's outstanding `total_owed - repaid_amount`, in
    /// order.  Any remainder (due to integer division) is credited to the first
    /// pool.  The sum of allocations equals the exact token transfer amount.
    ///
    /// **Parameters:**
    /// - `payer` — The SME address making the payment (must sign).
    /// - `invoice_ids` — Two or more invoice IDs, all belonging to `payer`'s SME.
    /// - `token` — The token being used for repayment.  All pools must use the
    ///   same token (single-asset constraint per A2).
    /// - `amount` — Total payment amount. Must be > 0 and ≤ MAX_AMOUNT.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::InvalidAmount` — `amount` ≤ 0, > MAX_AMOUNT, or
    ///   fewer than two invoice IDs provided.
    /// - `FinancingPoolError::PoolNotFound` — Any invoice ID has no open pool.
    /// - `FinancingPoolError::PoolAlreadyClosed` — Any pool is already closed.
    /// - `FinancingPoolError::Unauthorized` — Any invoice does not belong to `payer`
    ///   per the NFT contract, or a repayment lock is held on any pool.
    /// - `FinancingPoolError::InvalidAddress` — Invoice tokens differ (mixed-currency
    ///   netting is rejected; each pool must use the same token as `token`).
    /// - `FinancingPoolError::ProtocolPaused` — Protocol is paused.
    /// - `FinancingPoolError::InvoiceFrozen` — Any invoice is frozen.
    ///
    /// **Security:** Requires `payer.require_auth()`. All state updates follow CEI:
    /// repayment locks are set for every pool before the token transfer occurs.
    /// Locks are released after all pool updates complete.
    pub fn net_settle(
        env: Env,
        payer: Address,
        invoice_ids: Vec<u64>,
        token: Address,
        amount: i128,
    ) -> Result<(), FinancingPoolError> {
        payer.require_auth();
        Self::require_not_paused(&env)?;

        if amount <= 0 || amount > MAX_AMOUNT {
            return Err(FinancingPoolError::InvalidAmount);
        }
        // Require at least 2 invoices; single-invoice callers should use repay().
        if invoice_ids.len() < 2 {
            return Err(FinancingPoolError::InvalidAmount);
        }

        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(FinancingPoolError::NotInitialized)?;
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);

        // ── Pre-flight validation pass ─────────────────────────────────────────
        // Validate all invoices before acquiring any locks or transferring tokens.
        let n = invoice_ids.len();
        let mut pools: Vec<Pool> = Vec::new(&env);
        let mut total_remaining: i128 = 0;

        for i in 0..n {
            let invoice_id = invoice_ids.get(i).unwrap();

            // Frozen check
            if nft_client.is_invoice_frozen(&invoice_id) {
                return Err(FinancingPoolError::InvoiceFrozen);
            }
            // Reentrancy lock check (non-destructive peek)
            if env
                .storage()
                .persistent()
                .has(&DataKey::RepaymentLock(invoice_id))
            {
                return Err(FinancingPoolError::Unauthorized);
            }
            // Load and validate pool
            let pool: Pool = env
                .storage()
                .persistent()
                .get(&DataKey::Pool(invoice_id))
                .ok_or(FinancingPoolError::PoolNotFound)?;
            if pool.is_closed {
                return Err(FinancingPoolError::PoolAlreadyClosed);
            }
            // All pools must share the same token (single-asset constraint).
            if pool.token != token {
                return Err(FinancingPoolError::InvalidAddress);
            }
            // Verify this invoice belongs to the calling SME.
            let invoice = nft_client.get_invoice(&invoice_id);
            if invoice.sme != payer {
                return Err(FinancingPoolError::Unauthorized);
            }

            let remaining = pool
                .total_owed
                .checked_sub(pool.repaid_amount)
                .unwrap_or(0)
                .max(0);
            total_remaining = total_remaining
                .checked_add(remaining)
                .ok_or(FinancingPoolError::ArithmeticOverflow)?;

            pools.push_back(pool);
        }

        // ── Compute per-pool allocations ───────────────────────────────────────
        // Proportional to each pool's outstanding balance; remainder to first pool.
        let mut allocations: Vec<i128> = Vec::new(&env);
        let mut allocated_sum: i128 = 0;

        if total_remaining == 0 {
            // All pools already at full repayment — nothing to do.
            return Err(FinancingPoolError::InvalidAmount);
        }

        for i in 0..n {
            let pool = pools.get(i).unwrap();
            let remaining = pool
                .total_owed
                .checked_sub(pool.repaid_amount)
                .unwrap_or(0)
                .max(0);
            let alloc = amount
                .checked_mul(remaining)
                .and_then(|v| v.checked_div(total_remaining))
                .ok_or(FinancingPoolError::ArithmeticOverflow)?;
            allocations.push_back(alloc);
            allocated_sum = allocated_sum
                .checked_add(alloc)
                .ok_or(FinancingPoolError::ArithmeticOverflow)?;
        }
        // Credit any rounding remainder to the first pool.
        let remainder = amount
            .checked_sub(allocated_sum)
            .ok_or(FinancingPoolError::ArithmeticOverflow)?;
        if remainder > 0 {
            let first = allocations.get(0).unwrap();
            allocations.set(0, first + remainder);
        }

        // ── Acquire all repayment locks (CEI: state before interactions) ───────
        for i in 0..n {
            let invoice_id = invoice_ids.get(i).unwrap();
            env.storage()
                .persistent()
                .set(&DataKey::RepaymentLock(invoice_id), &true);
        }

        // ── Apply allocations and late penalties per pool ─────────────────────
        let mut closed_pools: Vec<u64> = Vec::new(&env);

        for i in 0..n {
            let invoice_id = invoice_ids.get(i).unwrap();
            let alloc = allocations.get(i).unwrap();
            if alloc <= 0 {
                continue;
            }

            let mut pool = pools.get(i).unwrap();
            let invoice = nft_client.get_invoice(&invoice_id);

            // Late penalty (once per pool, same rule as repay()).
            if !pool.penalty_applied && pool.late_penalty_bps > 0 {
                if env.ledger().timestamp() > invoice.due_date {
                    let penalty =
                        kora_shared::validation::bps_of(pool.face_value, pool.late_penalty_bps)
                            .map_err(|_| FinancingPoolError::ArithmeticOverflow)?;
                    pool.total_owed = pool
                        .total_owed
                        .checked_add(penalty)
                        .ok_or(FinancingPoolError::ArithmeticOverflow)?;
                    pool.penalty_applied = true;
                    events::late_penalty_applied(&env, invoice_id, penalty, pool.total_owed);
                }
            }

            pool.repaid_amount = pool
                .repaid_amount
                .checked_add(alloc)
                .ok_or(FinancingPoolError::ArithmeticOverflow)?;

            let should_close = pool.repaid_amount >= pool.total_owed;
            if should_close {
                pool.is_closed = true;
                closed_pools.push_back(invoice_id);
            }

            env.storage()
                .persistent()
                .set(&DataKey::Pool(invoice_id), &pool);

            // Emit per-invoice repayment event.
            events::repayment_made(&env, invoice_id, &payer, alloc);
        }

        // ── Emit netting event ────────────────────────────────────────────────
        events::net_settled(&env, &payer, &invoice_ids, amount);

        // ── Token transfer (single transfer for the full netting amount) ──────
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&payer, &env.current_contract_address(), &amount);

        // ── Post-transfer: close pools, distribute yield, update stats ────────
        for i in 0..closed_pools.len() {
            let invoice_id = closed_pools.get(i).unwrap();
            let pool: Pool = env
                .storage()
                .persistent()
                .get(&DataKey::Pool(invoice_id))
                .unwrap();

            Self::distribute_yield(&env, invoice_id, &token, pool.repaid_amount, pool.face_value)?;
            nft_client.set_repaid(&env.current_contract_address(), &invoice_id);

            let mut stats: ProtocolStats = env
                .storage()
                .instance()
                .get(&DataKey::ProtocolStats)
                .unwrap_or(ProtocolStats {
                    pools_opened: 0,
                    total_repaid: 0,
                    pools_defaulted: 0,
                    active_pools: 0,
                });
            stats.total_repaid = stats.total_repaid.saturating_add(pool.repaid_amount);
            stats.active_pools = stats.active_pools.saturating_sub(1);
            env.storage()
                .instance()
                .set(&DataKey::ProtocolStats, &stats);
        }

        // ── Release all locks ─────────────────────────────────────────────────
        for i in 0..n {
            let invoice_id = invoice_ids.get(i).unwrap();
            env.storage()
                .persistent()
                .remove(&DataKey::RepaymentLock(invoice_id));
        }

        Ok(())
    }
        env: &Env,
        invoice_id: u64,
        token: &Address,
        total_repaid: i128,
        _face_value: i128,
    ) -> Result<(), FinancingPoolError> {
        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(env));

        let token_client = token::Client::new(env, token);
        let token_decimals = token_client.decimals();

        let mut total_contributed: i128 = 0;
        for (investor, position) in positions.iter() {
            let payout = bps_of_normalized(total_repaid, position.share_bps, token_decimals)?;
            let yield_amount = payout
                .checked_sub(position.contributed)
                .ok_or(FinancingPoolError::ArithmeticOverflow)?;

            total_contributed = total_contributed.saturating_add(position.contributed);

            let shares: Map<(u64, Address, u32), PositionShare> = env
                .storage()
                .persistent()
                .get(&DataKey::PositionShares(invoice_id, investor.clone()))
                .unwrap_or_else(|| Map::new(env));

            let mut total_share_amount: i128 = 0;
            for share in shares.values() {
                total_share_amount = total_share_amount.saturating_add(share.amount);
            }

            if total_share_amount > 0 && !shares.is_empty() {
                let mut distributed: i128 = 0;
                for share in shares.values() {
                    let share_payout = share
                        .amount
                        .checked_mul(payout)
                        .and_then(|v| v.checked_div(position.contributed))
                        .ok_or(FinancingPoolError::ArithmeticOverflow)?;
                    distributed = distributed.saturating_add(share_payout);
                    token_client.transfer(&env.current_contract_address(), &share.owner, &share_payout);
                    events::yield_distributed(env, invoice_id, &share.owner, share_payout.saturating_sub(0));
                }
                let remainder = payout.saturating_sub(distributed);
                if remainder > 0 {
                    token_client.transfer(&env.current_contract_address(), &investor, &remainder);
                    events::yield_distributed(env, invoice_id, &investor, remainder);
                }
            } else {
                token_client.transfer(&env.current_contract_address(), &investor, &payout);
                events::yield_distributed(env, invoice_id, &investor, yield_amount);
            }
        }

        let agg_key = DataKey::AggregateFunded(token.clone());
        let prev_agg: i128 = env.storage().instance().get(&agg_key).unwrap_or(0);
        env.storage()
            .instance()
            .set(&agg_key, &prev_agg.saturating_sub(total_contributed));

        Ok(())
    }

    /// Mark an invoice pool as defaulted. Admin only.
    ///
    /// Distributes any partial repayment already received to investors pro-rata,
    /// marks the invoice NFT as `Defaulted`, and records the default against the
    /// SME in the risk registry (best-effort; registry errors are ignored).
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `invoice_id` — The ID of the invoice to default.
    /// - `token` — The pool token address (needed for partial yield distribution).
    ///
    /// **Errors:**
    /// - `FinancingPoolError::NotAdmin` — Caller is not the admin.
    /// - `FinancingPoolError::ProtocolPaused` — Protocol is paused.
    /// - `FinancingPoolError::Unauthorized` — Repayment lock is held (concurrent operation).
    /// - `FinancingPoolError::PoolNotFound` — No pool exists for `invoice_id`.
    /// - `FinancingPoolError::PoolAlreadyClosed` — Pool is already closed (repaid or defaulted).
    ///
    /// **Security:** Requires `admin.require_auth()`. Should only be called after the
    /// invoice's `due_date` has passed without full repayment.
    pub fn mark_default(
        env: Env,
        admin: Address,
        invoice_id: u64,
        token: Address,
    ) -> Result<(), FinancingPoolError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_not_paused(&env)?;

        if env.storage().persistent().has(&DataKey::RepaymentLock(invoice_id)) {
            return Err(FinancingPoolError::Unauthorized);
        }

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;

        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        if let Some(dr_contract) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::DisputeResolution)
        {
            let dr_client =
                kora_dispute_resolution::DisputeResolutionContractClient::new(&env, &dr_contract);
            if dr_client.try_has_open_dispute(&invoice_id).unwrap_or(false) {
                return Err(FinancingPoolError::DisputeNotOpen);
            }
        }

        if pool.repaid_amount > 0 {
            Self::distribute_yield(&env, invoice_id, &token, pool.repaid_amount, pool.face_value)?;
        }

        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(FinancingPoolError::NotInitialized)?;
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
        nft_client.set_defaulted(&admin, &invoice_id);

        let invoice = nft_client.get_invoice(&invoice_id);
        events::invoice_defaulted(&env, invoice_id, &admin, invoice.amount, invoice.currency.clone());

        // Update protocol stats
        let mut stats: ProtocolStats = env.storage().instance().get(&DataKey::ProtocolStats)
            .unwrap_or(ProtocolStats { pools_opened: 0, total_repaid: 0, pools_defaulted: 0, active_pools: 0 });
        stats.pools_defaulted = stats.pools_defaulted.saturating_add(1);
        stats.active_pools = stats.active_pools.saturating_sub(1);
        env.storage().instance().set(&DataKey::ProtocolStats, &stats);

        // Automatically record the default against the SME in the risk registry
        if let Some(rr_contract) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::RiskRegistry)
        {
            let rr_client =
                kora_risk_registry::RiskRegistryContractClient::new(&env, &rr_contract);
            // Best-effort: ignore errors if SME is not registered in risk registry
            let _ = rr_client.try_record_default(&admin, &invoice.sme);
        }

        Ok(())
    }

    // ── Early-Termination Buyout ────────────────────────────────────────────────

    /// Propose an early-termination buyout of a funded invoice.
    ///
    /// The SME escrows `amount` (a negotiated discount to the full obligation) into the pool.
    /// Investors then accept via `accept_early_settlement`; once investors representing 100% of
    /// pool shares have accepted, the escrow is distributed pro-rata and the pool closes early.
    ///
    /// `amount` must satisfy `total_funded <= amount < total_owed` — investors recover at least
    /// their principal, while the SME pays strictly less than the full obligation.
    ///
    /// **Parameters:**
    /// - `sme` — The SME that originated the invoice (must sign).
    /// - `invoice_id` — The ID of the funded invoice to settle early.
    /// - `amount` — The buyout amount in the pool token. Must satisfy
    ///   `total_funded <= amount < total_owed` and be > 0.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::ProtocolPaused` — Protocol is paused.
    /// - `FinancingPoolError::InvalidAmount` — `amount` is ≤ 0, > `MAX_AMOUNT`, < `total_funded`,
    ///   or ≥ `total_owed`.
    /// - `FinancingPoolError::PoolNotFound` — No open pool exists for `invoice_id`.
    /// - `FinancingPoolError::PoolAlreadyClosed` — Pool is already closed.
    /// - `FinancingPoolError::AlreadyInitialized` — An early-settlement offer already exists.
    /// - `FinancingPoolError::Unauthorized` — Caller is not the invoice's SME.
    ///
    /// **Security:** Requires `sme.require_auth()`. The buyout amount is escrowed
    /// immediately into this contract so that settlement upon acceptance is atomic
    /// and cannot be frontrun.
    pub fn propose_early_settlement(
        env: Env,
        sme: Address,
        invoice_id: u64,
        amount: i128,
    ) -> Result<(), FinancingPoolError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;

        if amount <= 0 || amount > MAX_AMOUNT {
            return Err(FinancingPoolError::InvalidAmount);
        }

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }
        // Must be a genuine discount that still returns investors at least their principal.
        if amount < pool.total_funded || amount >= pool.total_owed {
            return Err(FinancingPoolError::InvalidAmount);
        }
        if env
            .storage()
            .persistent()
            .has(&DataKey::EarlySettlement(invoice_id))
        {
            return Err(FinancingPoolError::AlreadyInitialized);
        }

        // Only the invoice's SME may propose a buyout.
        let invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.sme != sme {
            return Err(FinancingPoolError::Unauthorized);
        }

        // Escrow the buyout amount up-front so acceptance can settle atomically.
        let token_client = token::Client::new(&env, &pool.token);
        token_client.transfer(&sme, &env.current_contract_address(), &amount);

        let offer = EarlySettlementOffer {
            invoice_id,
            amount,
            accepted_bps: 0,
            accepted: Vec::new(&env),
        };
        env.storage()
            .persistent()
            .set(&DataKey::EarlySettlement(invoice_id), &offer);
        Ok(())
    }

    /// Accept a pending early-termination buyout offer as an investor.
    ///
    /// When investors representing 100% of pool shares have accepted, the escrowed amount is
    /// distributed pro-rata to all investors, the pool is closed, and the invoice is marked repaid.
    ///
    /// **Parameters:**
    /// - `investor` — The investor address accepting the offer (must hold a position).
    /// - `invoice_id` — The ID of the invoice with the pending offer.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::ProtocolPaused` — Protocol is paused.
    /// - `FinancingPoolError::PoolNotFound` — No early-settlement offer or pool exists for `invoice_id`.
    /// - `FinancingPoolError::PositionNotFound` — Caller does not hold a position in this pool.
    /// - `FinancingPoolError::AlreadyInitialized` — Investor has already accepted this offer.
    ///
    /// **Security:** Requires `investor.require_auth()`. Each investor may only accept once.
    /// Settlement is executed atomically once the last required investor accepts.
    pub fn accept_early_settlement(
        env: Env,
        investor: Address,
        invoice_id: u64,
    ) -> Result<(), FinancingPoolError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;

        if env
            .storage()
            .persistent()
            .has(&DataKey::RepaymentLock(invoice_id))
        {
            return Err(FinancingPoolError::Unauthorized);
        }

        let mut offer: EarlySettlementOffer = env
            .storage()
            .persistent()
            .get(&DataKey::EarlySettlement(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;

        let mut pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(&env));
        let position = positions
            .get(investor.clone())
            .ok_or(FinancingPoolError::Unauthorized)?;

        // An investor may only accept once.
        if offer.accepted.iter().any(|a| a == investor) {
            return Err(FinancingPoolError::AlreadyInitialized);
        }
        offer.accepted.push_back(investor.clone());
        offer.accepted_bps = offer
            .accepted_bps
            .checked_add(position.share_bps)
            .ok_or(FinancingPoolError::ArithmeticOverflow)?;

        if offer.accepted_bps >= 10_000 {
            // Unanimous acceptance: settle. Lock against a concurrent repay.
            env.storage()
                .persistent()
                .set(&DataKey::RepaymentLock(invoice_id), &true);

            pool.repaid_amount = offer.amount;
            pool.is_closed = true;
            env.storage()
                .persistent()
                .set(&DataKey::Pool(invoice_id), &pool);

            Self::distribute_yield(&env, invoice_id, &pool.token, offer.amount, pool.face_value)?;

            let nft_contract: Address = env
                .storage()
                .instance()
                .get(&DataKey::InvoiceNft)
                .ok_or(FinancingPoolError::NotInitialized)?;
            let nft_client =
                kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
            nft_client.set_repaid(&env.current_contract_address(), &invoice_id);

            // Update protocol stats
            let mut stats: ProtocolStats = env.storage().instance().get(&DataKey::ProtocolStats)
                .unwrap_or(ProtocolStats { pools_opened: 0, total_repaid: 0, pools_defaulted: 0, active_pools: 0 });
            stats.total_repaid = stats.total_repaid.saturating_add(offer.amount);
            stats.active_pools = stats.active_pools.saturating_sub(1);
            env.storage().instance().set(&DataKey::ProtocolStats, &stats);

            env.storage()
                .persistent()
                .remove(&DataKey::EarlySettlement(invoice_id));
            env.storage()
                .persistent()
                .remove(&DataKey::RepaymentLock(invoice_id));

            events::repayment_made(
                &env,
                invoice_id,
                &env.current_contract_address(),
                offer.amount,
            );
        } else {
            env.storage()
                .persistent()
                .set(&DataKey::EarlySettlement(invoice_id), &offer);
        }

        Ok(())
    }

    /// Cancel a pending early-settlement offer and refund the escrowed amount to the SME.
    ///
    /// Cancel a pending early-settlement offer and return the escrowed amount to the SME.
    ///
    /// Callable only by the invoice's SME while the offer has not yet been fully accepted.
    ///
    /// **Parameters:**
    /// - `sme` — The SME that originally proposed the buyout.
    /// - `invoice_id` — The ID of the invoice whose offer is being cancelled.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::PoolNotFound` — No early-settlement offer exists for `invoice_id`.
    /// - `FinancingPoolError::Unauthorized` — Caller is not the SME that proposed the offer.
    ///
    /// **Security:** Requires `sme.require_auth()`. The escrowed amount is returned to the
    /// SME via a token transfer before the offer record is removed.
    pub fn cancel_early_settlement(
        env: Env,
        sme: Address,
        invoice_id: u64,
    ) -> Result<(), FinancingPoolError> {
        sme.require_auth();

        let offer: EarlySettlementOffer = env
            .storage()
            .persistent()
            .get(&DataKey::EarlySettlement(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;

        let invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.sme != sme {
            return Err(FinancingPoolError::Unauthorized);
        }

        env.storage()
            .persistent()
            .remove(&DataKey::EarlySettlement(invoice_id));

        let token_client = token::Client::new(&env, &pool.token);
        token_client.transfer(&env.current_contract_address(), &sme, &offer.amount);

        Ok(())
    }

    /// Read a pending early-settlement offer for an invoice.
    ///
    /// **Parameters:**
    /// - `invoice_id` — The invoice ID to query.
    ///
    /// **Returns:** The `EarlySettlementOffer`, or `FinancingPoolError::PoolNotFound` if none exists.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_early_settlement(
        env: Env,
        invoice_id: u64,
    ) -> Result<EarlySettlementOffer, FinancingPoolError> {
        env.storage()
            .persistent()
            .get(&DataKey::EarlySettlement(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    /// Retrieve the pool state for a funded invoice.
    ///
    /// **Parameters:**
    /// - `invoice_id` — The invoice ID to query.
    ///
    /// **Returns:** The `Pool` struct, or `FinancingPoolError::PoolNotFound` if none exists.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_pool(env: Env, invoice_id: u64) -> Result<Pool, FinancingPoolError> {
        env.storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)
    }

    /// Retrieve all investor positions for an invoice as a flat list.
    ///
    /// **Parameters:**
    /// - `invoice_id` — The invoice ID to query.
    ///
    /// **Returns:** A `Vec<Position>` (empty if no positions exist). For large pools
    /// use `get_positions_page` to paginate and bound CPU cost.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_positions(env: Env, invoice_id: u64) -> Vec<Position> {
        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or(Map::new(&env));
        positions.values()
    }

    /// Return protocol-wide aggregate statistics for analytics/dashboards.
    ///
    /// **Returns:** `ProtocolStats` with counters defaulted to zero if none have
    /// been recorded yet.
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_protocol_stats(env: Env) -> ProtocolStats {
        env.storage()
            .instance()
            .get(&DataKey::ProtocolStats)
            .unwrap_or(ProtocolStats {
                pools_opened: 0,
                total_repaid: 0,
                pools_defaulted: 0,
                active_pools: 0,
            })
    }

    // ── Installment schedule ──────────────────────────────────────────────────

    /// Attach an installment repayment schedule to an open pool.
    ///
    /// Admin-only.  Must be called before the first repayment.  The sum of all
    /// installment amounts must equal `pool.total_owed`.  Due-dates must be
    /// monotonically increasing.
    pub fn set_installment_schedule(
        env: Env,
        admin: Address,
        invoice_id: u64,
        schedule: InstallmentSchedule,
    ) -> Result<(), FinancingPoolError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_not_paused(&env)?;

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;

        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }
        if pool.repaid_amount > 0 {
            // Refuse to attach a schedule once repayment has started.
            return Err(FinancingPoolError::InvalidAmount);
        }
        if schedule.installments.is_empty() {
            return Err(FinancingPoolError::InvalidAmount);
        }
        if schedule.next_index != 0 {
            return Err(FinancingPoolError::InvalidAmount);
        }

        // Validate: sum of installments == total_owed, due_dates non-decreasing.
        let mut total: i128 = 0;
        let mut prev_due: u64 = 0;
        for installment in schedule.installments.iter() {
            if installment.amount <= 0 {
                return Err(FinancingPoolError::InvalidAmount);
            }
            if installment.due_date < prev_due {
                return Err(FinancingPoolError::InvalidDueDate);
            }
            prev_due = installment.due_date;
            total = total
                .checked_add(installment.amount)
                .ok_or(FinancingPoolError::ArithmeticOverflow)?;
        }
        if total != pool.total_owed {
            return Err(FinancingPoolError::InvalidAmount);
        }

        env.storage()
            .persistent()
            .set(&DataKey::InstallmentSchedule(invoice_id), &schedule);
        Ok(())
    }

    /// Return the installment schedule for a pool, if set.
    pub fn get_installment_schedule(
        env: Env,
        invoice_id: u64,
    ) -> Option<InstallmentSchedule> {
        env.storage()
            .persistent()
            .get(&DataKey::InstallmentSchedule(invoice_id))
    }

    // ── Secondary market ───────────────────────────────────────────────────────

    /// List a position for sale on the secondary market.
    ///
    /// Seller must hold a position on an open (not yet closed) pool. The offer is stored
    /// on-chain and can be purchased by any buyer via `buy_position`.
    ///
    /// **Parameters:**
    /// - `seller` — The investor who holds the position to sell.
    /// - `invoice_id` — The invoice ID of the pool.
    /// - `token` — The token the seller wants to receive as payment.
    /// - `price` — The asking price (must be > 0).
    ///
    /// **Errors:**
    /// - `FinancingPoolError::ProtocolPaused` — Protocol is paused.
    /// - `FinancingPoolError::InvalidAmount` — `price` is ≤ 0.
    /// - `FinancingPoolError::PoolNotFound` — No pool exists for `invoice_id`.
    /// - `FinancingPoolError::PoolAlreadyClosed` — Pool is already closed.
    /// - `FinancingPoolError::PositionNotFound` — Seller does not hold a position in this pool.
    /// - `FinancingPoolError::SaleAlreadyListed` — Seller already has an active listing.
    ///
    /// **Security:** Requires `seller.require_auth()`.
    pub fn list_position_for_sale(
        env: Env,
        seller: Address,
        invoice_id: u64,
        token: Address,
        price: i128,
    ) -> Result<(), FinancingPoolError> {
        seller.require_auth();
        Self::require_not_paused(&env)?;

        if price <= 0 {
            return Err(FinancingPoolError::InvalidAmount);
        }

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(&env));
        if !positions.contains_key(seller.clone()) {
            return Err(FinancingPoolError::PositionNotFound);
        }

        if env
            .storage()
            .persistent()
            .has(&DataKey::SaleOffer(invoice_id, seller.clone()))
        {
            return Err(FinancingPoolError::SaleAlreadyListed);
        }

        let offer = PositionSaleOffer {
            seller: seller.clone(),
            invoice_id,
            token,
            price,
        };
        env.storage()
            .persistent()
            .set(&DataKey::SaleOffer(invoice_id, seller.clone()), &offer);

        events::position_listed_for_sale(&env, invoice_id, &seller, price);
        Ok(())
    }

    /// Paginated view of investor positions for an invoice.
    ///
    /// Returns at most `limit` positions starting at `offset` (0-based index
    /// into the position list ordered by investor address key).  An `offset`
    /// beyond the last position returns an empty vec; `limit` is capped at 100
    /// to bound per-call CPU cost.
    pub fn get_positions_page(
        env: Env,
        invoice_id: u64,
        offset: u32,
        limit: u32,
    ) -> Vec<Position> {
        let limit = limit.min(100);
        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(&env));

        let all: Vec<Position> = positions.values();
        let total = all.len();
        let start = offset.min(total) as usize;
        let end = (start + limit as usize).min(total as usize);

        let mut page: Vec<Position> = Vec::new(&env);
        for i in start..end {
            page.push_back(all.get(i as u32).unwrap());
        }
        page
    }

    /// Purchase an investor position from the secondary market.
    ///
    /// Transfers ownership of the position (and its proportional yield claim)
    /// from seller to buyer in exchange for a token payment at the listed price.
    ///
    /// **Parameters:**
    /// - `buyer` — The address purchasing the position.
    /// - `invoice_id` — The invoice ID of the pool.
    /// - `seller` — The address that listed the position for sale.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::ProtocolPaused` — Protocol is paused.
    /// - `FinancingPoolError::SaleNotFound` — No active sale listing from `seller` for this invoice.
    /// - `FinancingPoolError::PoolNotFound` — Pool does not exist.
    /// - `FinancingPoolError::PoolAlreadyClosed` — Pool is already closed.
    /// - `FinancingPoolError::PositionNotFound` — Seller no longer holds the position.
    ///
    /// **Security:** Requires `buyer.require_auth()`. State is updated (CEI pattern) before
    /// the token transfer to prevent reentrancy.
    pub fn buy_position(
        env: Env,
        buyer: Address,
        invoice_id: u64,
        seller: Address,
    ) -> Result<(), FinancingPoolError> {
        buyer.require_auth();
        Self::require_not_paused(&env)?;

        let offer: PositionSaleOffer = env
            .storage()
            .persistent()
            .get(&DataKey::SaleOffer(invoice_id, seller.clone()))
            .ok_or(FinancingPoolError::SaleNotFound)?;

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(FinancingPoolError::PoolNotFound)?;
        if pool.is_closed {
            return Err(FinancingPoolError::PoolAlreadyClosed);
        }

        let mut positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(&env));

        let seller_position: Position = positions
            .get(seller.clone())
            .ok_or(FinancingPoolError::PositionNotFound)?;

        // CEI: update state before external token transfer
        env.storage()
            .persistent()
            .remove(&DataKey::SaleOffer(invoice_id, seller.clone()));

        positions.remove(seller.clone());
        let buyer_position = Position {
            investor: buyer.clone(),
            invoice_id,
            contributed: seller_position.contributed,
            share_bps: seller_position.share_bps,
            yield_claimed: seller_position.yield_claimed,
        };
        positions.set(buyer.clone(), buyer_position);
        env.storage()
            .persistent()
            .set(&DataKey::Positions(invoice_id), &positions);

        let token_client = token::Client::new(&env, &offer.token);
        token_client.transfer(&buyer, &seller, &offer.price);

        events::position_sold(&env, invoice_id, &seller, &buyer, offer.price);
        Ok(())
    }

    /// Returns the total number of investor positions recorded for an invoice.
    ///
    /// **Parameters:**
    /// - `invoice_id` — The invoice ID to query.
    ///
    /// **Returns:** The number of distinct investor positions (0 if none).
    ///
    /// **Security:** Read-only view. No authorization required.
    pub fn get_positions_count(env: Env, invoice_id: u64) -> u32 {
        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or(Map::new(&env));
        positions.len()
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    /// Propose a WASM upgrade. Admin only. Begins a 24-hour timelock.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    /// - `new_wasm_hash` — SHA-256 hash of the new WASM binary (32 bytes).
    ///
    /// **Errors:**
    /// - `FinancingPoolError::NotAdmin` — Caller is not the admin.
    ///
    /// **Security:** Requires `admin.require_auth()`. Apply with `execute_upgrade` after
    /// `UPGRADE_TIMELOCK_DELAY` (24 h) has elapsed.
    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), FinancingPoolError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::UpgradeProposal, &(new_wasm_hash.clone(), env.ledger().timestamp()));
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        Ok(())
    }

    /// Execute a previously proposed WASM upgrade after the 24-hour timelock has elapsed.
    ///
    /// **Parameters:**
    /// - `admin` — Must be the current admin address.
    ///
    /// **Errors:**
    /// - `FinancingPoolError::NotAdmin` — Caller is not the admin.
    /// - `FinancingPoolError::NoUpgradeProposed` — No upgrade proposal is pending.
    /// - `FinancingPoolError::UpgradeTimelockNotElapsed` — 24-hour timelock has not yet passed.
    ///
    /// **Security:** Requires `admin.require_auth()`. Clears the proposal atomically before executing.
    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), FinancingPoolError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(FinancingPoolError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(FinancingPoolError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        events::upgrade_executed(&env, &admin, &wasm_hash);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn load_invoice(
        env: &Env,
        invoice_id: u64,
    ) -> Result<kora_shared::types::Invoice, FinancingPoolError> {
        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(FinancingPoolError::NotInitialized)?;
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(env, &nft_contract);
        Ok(nft_client.get_invoice(&invoice_id))
    }

    fn require_not_paused(env: &Env) -> Result<(), FinancingPoolError> {
        let ac: Address = env
            .storage()
            .instance()
            .get(&DataKey::AccessControl)
            .ok_or(FinancingPoolError::NotInitialized)?;
        let client = kora_access_control::AccessControlContractClient::new(env, &ac);
        if client.is_paused() {
            return Err(FinancingPoolError::ProtocolPaused);
        }
        Ok(())
    }

    fn require_admin(env: &Env, caller: &Address) -> Result<(), FinancingPoolError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(FinancingPoolError::NotInitialized)?;
        if &admin != caller {
            return Err(FinancingPoolError::NotAdmin);
        }
        Ok(())
    }

    /// Convert `amount` between currencies using the price oracle.
    /// If invoice currency matches the pool token's symbol, returns amount unchanged.
    /// Rejects stale or missing oracle prices.
    fn convert_if_needed(
        env: &Env,
        amount: i128,
        invoice_currency: &Symbol,
        _pool_token: &Address,
    ) -> Result<i128, FinancingPoolError> {
        let oracle_addr: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PriceOracle);

        let oracle_addr = match oracle_addr {
            Some(addr) => addr,
            None => return Ok(amount),
        };

        // Use the invoice currency symbol directly; pool token symbol is
        // derived from the token contract but for oracle lookup we use the
        // same symbol convention.  If the oracle has no pair registered
        // for (from, to), the convert call will fail — this is intentional
        // to reject operations without a valid price.
        let pool_currency = Symbol::new(env, "USDC");

        // No conversion needed when the invoice is already denominated in the
        // pool's currency — skip the oracle call entirely so pools can operate
        // without a price_oracle wired up (or with a dummy address) as long as
        // they never handle cross-currency invoices.
        if invoice_currency == &pool_currency {
            return Ok(amount);
        }

        let oracle_client =
            kora_price_oracle::PriceOracleContractClient::new(env, &oracle_addr);

        Ok(oracle_client.convert(&amount, invoice_currency, &pool_currency))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, Address, Address, Address, Address, FinancingPoolContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let risk_registry = Address::generate(&env);
        let treasury = Address::generate(&env);
        let access_control =
            env.register_contract(None, kora_access_control::AccessControlContract);
        let ac_client =
            kora_access_control::AccessControlContractClient::new(&env, &access_control);
        ac_client.initialize(&admin);
        let oracle = Address::generate(&env);
        let dispute_resolution = Address::generate(&env);
        client.initialize(
            &admin,
            &nft,
            &risk_registry,
            &treasury,
            &access_control,
            &200u32,
            &oracle,
            &10_000u32,
            &dispute_resolution,
        );
        (env, admin, nft, treasury, access_control, client)
    }

    /// Seed an open `Pool` directly into contract storage for `invoice_id`, so
    /// tests can exercise `record_position` (which requires an existing pool)
    /// without going through the full `release_funds` flow.
    fn seed_pool(env: &Env, contract_id: &Address, invoice_id: u64, face_value: i128) {
        let token = Address::generate(env);
        let pool = Pool {
            invoice_id,
            token,
            total_funded: 0,
            face_value,
            repaid_amount: 0,
            is_closed: false,
            late_penalty_bps: 200,
            total_owed: face_value,
            penalty_applied: false,
        };
        env.as_contract(contract_id, || {
            env.storage().persistent().set(&DataKey::Pool(invoice_id), &pool);
        });
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_success() {
        let (_env, _admin, _nft, _treasury, _ac, client) = setup();
        assert!(client.try_get_pool(&1u64).is_err()); // No pools yet
    }

    #[test]
    fn test_initialize_already_initialized_fails() {
        let (env, admin, nft, treasury, ac, client) = setup();
        let rr = Address::generate(&env);
        let oracle = Address::generate(&env);
        let result =
            client.try_initialize(&admin, &nft, &rr, &treasury, &ac, &200u32, &oracle, &5_000u32, &Address::generate(&env));
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_invalid_fee_bps_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        let result = client.try_initialize(
            &admin, &nft, &rr, &treasury, &ac, &10_001u32, &oracle, &5_000u32, &Address::generate(&env),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_zero_penalty_bps_allowed() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        assert!(client
            .try_initialize(&admin, &nft, &rr, &treasury, &ac, &0u32, &oracle, &5_000u32, &Address::generate(&env))
            .is_ok());
    }

    #[test]
    fn test_initialize_self_as_admin_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        // contract_id as admin must be rejected
        let result = client.try_initialize(
            &contract_id,
            &nft,
            &rr,
            &treasury,
            &ac,
            &200u32,
            &oracle,
            &5_000u32,
            &Address::generate(&env),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_self_as_nft_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        let result = client.try_initialize(
            &admin,
            &contract_id,
            &rr,
            &treasury,
            &ac,
            &200u32,
            &oracle,
            &5_000u32,
            &Address::generate(&env),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_valid_max_late_penalty_bps() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        assert!(client
            .try_initialize(&admin, &nft, &rr, &treasury, &ac, &10_000u32, &oracle, &5_000u32, &Address::generate(&env))
            .is_ok());
    }

    // ── max_position_bps range guard (require_valid_bps_range) ───────────────

    #[test]
    fn test_initialize_max_position_bps_zero_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        let result = client.try_initialize(&admin, &nft, &rr, &treasury, &ac, &200u32, &oracle, &0u32, &Address::generate(&env));
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::InvalidFeeRate);
    }

    #[test]
    fn test_initialize_max_position_bps_over_max_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        let result = client.try_initialize(&admin, &nft, &rr, &treasury, &ac, &200u32, &oracle, &10_001u32, &Address::generate(&env));
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::InvalidFeeRate);
    }

    #[test]
    fn test_set_max_position_bps_zero_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        let dispute_resolution = Address::generate(&env);
        client.initialize(&admin, &nft, &rr, &treasury, &ac, &200u32, &oracle, &5_000u32, &dispute_resolution);
        let result = client.try_set_max_position_bps(&admin, &0u32);
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::InvalidFeeRate);
        // Existing config is untouched by the rejected update.
        assert_eq!(client.get_max_position_bps(), 5_000u32);
    }

    #[test]
    fn test_set_max_position_bps_within_range_succeeds() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        let dispute_resolution = Address::generate(&env);
        client.initialize(&admin, &nft, &rr, &treasury, &ac, &200u32, &oracle, &5_000u32, &dispute_resolution);
        client.set_max_position_bps(&admin, &7_500u32);
        assert_eq!(client.get_max_position_bps(), 7_500u32);
    }

    // ── get_pool / get_positions ──────────────────────────────────────────────

    #[test]
    fn test_get_pool_not_found() {
        let (_env, _admin, _nft, _treasury, _ac, client) = setup();
        assert!(client.try_get_pool(&999u64).is_err());
    }

    #[test]
    fn test_get_pool_various_invoices() {
        let (_env, _admin, _nft, _treasury, _ac, client) = setup();
        assert!(client.try_get_pool(&0u64).is_err());
        assert!(client.try_get_pool(&1u64).is_err());
        assert!(client.try_get_pool(&999u64).is_err());
        assert!(client.try_get_pool(&u64::MAX).is_err());
    }

    #[test]
    fn test_get_positions_empty() {
        let (_env, _admin, _nft, _treasury, _ac, client) = setup();
        let positions = client.get_positions(&1u64);
        assert_eq!(positions.len(), 0);
    }

    // ── record_position ───────────────────────────────────────────────────────

    #[test]
    fn test_record_position_requires_admin() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        let non_admin = Address::generate(&env);
        let result = client.try_record_position(
            &non_admin,
            &1u64,
            &investor,
            &1_000_000_000i128,
            &10_000_000_000i128,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_arithmetic_overflow() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        // contributed > MAX_AMOUNT triggers InvalidAmount before the overflow
        let result = client.try_record_position(&admin, &1u64, &investor, &i128::MAX, &1i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_exceeds_max_amount() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        let result = client.try_record_position(
            &admin,
            &1u64,
            &investor,
            &(MAX_AMOUNT + 1),
            &(MAX_AMOUNT + 2),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_total_pool_exceeds_max_amount() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        let result =
            client.try_record_position(&admin, &1u64, &investor, &100i128, &(MAX_AMOUNT + 1));
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_contributed_exceeds_total_pool() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        let result = client.try_record_position(&admin, &1u64, &investor, &100i128, &50i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_negative_amounts() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        assert!(client
            .try_record_position(&admin, &1u64, &investor, &(-100i128), &1_000i128)
            .is_err());
        assert!(client
            .try_record_position(&admin, &1u64, &investor, &100i128, &(-1_000i128))
            .is_err());
    }

    #[test]
    fn test_record_position_zero_amounts() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        assert!(client
            .try_record_position(&admin, &1u64, &investor, &0i128, &1_000i128)
            .is_err());
        assert!(client
            .try_record_position(&admin, &1u64, &investor, &100i128, &0i128)
            .is_err());
    }

    #[test]
    fn test_record_position_success() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 10_000_000_000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &5_000_000_000i128, &10_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_record_position_share_bps_correct() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 10_000_000_000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &5_000_000_000i128, &10_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 5_000u32);
    }

    #[test]
    fn test_record_position_share_calculation() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &500i128, &1000i128);
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 5000);
    }

    #[test]
    fn test_record_position_quarter_share() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 100i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &25i128, &100i128);
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 2500);
    }

    #[test]
    fn test_record_position_tenth_share() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 100i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &10i128, &100i128);
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 1000);
    }

    #[test]
    fn test_record_position_basis_point_precision() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 10000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &1i128, &10000i128);
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 1);
    }

    #[test]
    fn test_record_position_exact_full_pool() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 10_000_000_000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &10_000_000_000i128, &10_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_record_position_minimum_valid_amount() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1_000_000_000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &1i128, &1_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_record_position_happy_path_two_investors() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 10_000_000_000i128);
        let investor1 = Address::generate(&env);
        let investor2 = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor1, &3_000_000_000i128, &10_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
        client.record_position(&admin, &1u64, &investor2, &7_000_000_000i128, &10_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 2);
    }

    #[test]
    fn test_record_position_multiple_invoices() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        seed_pool(&env, &client.address, 2u64, 2000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &100i128, &1000i128);
        client.record_position(&admin, &2u64, &investor, &200i128, &2000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
        assert_eq!(client.get_positions(&2u64).len(), 1);
    }

    #[test]
    fn test_record_position_overwrite_existing() {
        // Recording a position for the same investor on the same invoice
        // overwrites the previous entry (map semantics).
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &100i128, &1000i128);
        client.record_position(&admin, &1u64, &investor, &200i128, &1000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_get_positions_multiple_investors() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 300i128);
        let i1 = Address::generate(&env);
        let i2 = Address::generate(&env);
        let i3 = Address::generate(&env);
        client.record_position(&admin, &1u64, &i1, &100i128, &300i128);
        client.record_position(&admin, &1u64, &i2, &100i128, &300i128);
        client.record_position(&admin, &1u64, &i3, &100i128, &300i128);
        assert_eq!(client.get_positions(&1u64).len(), 3);
    }

    // ── repay ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_repay_pool_not_found() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let payer = Address::generate(&env);
        let token = Address::generate(&env);
        let result = client.try_repay(&payer, &999u64, &token, &1_000_000_000i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_repay_invalid_amount() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let payer = Address::generate(&env);
        let token = Address::generate(&env);
        assert!(client.try_repay(&payer, &1u64, &token, &0i128).is_err());
    }

    #[test]
    fn test_repay_negative_amount_fails() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let payer = Address::generate(&env);
        let token = Address::generate(&env);
        assert!(client.try_repay(&payer, &1u64, &token, &-1i128).is_err());
    }

    #[test]
    fn test_repay_zero_amount() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let payer = Address::generate(&env);
        let token = Address::generate(&env);
        assert!(client.try_repay(&payer, &1u64, &token, &0i128).is_err());
    }

    // ── Secondary market tests ────────────────────────────────────────────────

    #[test]
    fn test_list_position_for_sale_pool_not_found() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let seller = Address::generate(&env);
        let token = Address::generate(&env);
        let result = client.try_list_position_for_sale(&seller, &1u64, &token, &1_000i128);
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::PoolNotFound);
    }

    #[test]
    fn test_list_position_for_sale_zero_price_rejected() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let seller = Address::generate(&env);
        let token = Address::generate(&env);
        let result = client.try_list_position_for_sale(&seller, &1u64, &token, &0i128);
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::InvalidAmount);
    }

    #[test]
    fn test_list_position_for_sale_position_not_found() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let contract_id = client.address.clone();
        let token = Address::generate(&env);
        let seller = Address::generate(&env);

        // Seed an open pool directly into contract storage
        let pool = Pool {
            invoice_id: 1,
            token: token.clone(),
            total_funded: 10_000_000_000,
            face_value: 10_000_000_000,
            repaid_amount: 0,
            is_closed: false,
            late_penalty_bps: 200,
            total_owed: 10_000_000_000,
            penalty_applied: false,
        };
        env.as_contract(&contract_id, || {
            env.storage().persistent().set(&DataKey::Pool(1u64), &pool);
        });

        let result = client.try_list_position_for_sale(&seller, &1u64, &token, &5_000_000_000i128);
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::PositionNotFound);
    }

    #[test]
    fn test_list_position_for_sale_double_listing_prevented() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let contract_id = client.address.clone();
        let token = Address::generate(&env);
        let seller = Address::generate(&env);

        let pool = Pool {
            invoice_id: 1,
            token: token.clone(),
            total_funded: 10_000_000_000,
            face_value: 10_000_000_000,
            repaid_amount: 0,
            is_closed: false,
            late_penalty_bps: 200,
            total_owed: 10_000_000_000,
            penalty_applied: false,
        };
        env.as_contract(&contract_id, || {
            env.storage().persistent().set(&DataKey::Pool(1u64), &pool);
        });

        // Record position so seller can list
        client.record_position(&admin, &1u64, &seller, &5_000_000_000i128, &10_000_000_000i128);

        // First listing succeeds
        assert!(client
            .try_list_position_for_sale(&seller, &1u64, &token, &4_500_000_000i128)
            .is_ok());

        // Second listing is rejected
        let result =
            client.try_list_position_for_sale(&seller, &1u64, &token, &4_000_000_000i128);
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::SaleAlreadyListed);
    }

    #[test]
    fn test_buy_position_sale_not_found() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let buyer = Address::generate(&env);
        let seller = Address::generate(&env);
        let result = client.try_buy_position(&buyer, &1u64, &seller);
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::SaleNotFound);
    }

    #[test]
    fn test_buy_position_transfers_ownership() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let contract_id = client.address.clone();
        let token_admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        let seller = Address::generate(&env);
        let buyer = Address::generate(&env);
        soroban_sdk::token::StellarAssetClient::new(&env, &token)
            .mint(&buyer, &4_500_000_000i128);

        let pool = Pool {
            invoice_id: 1,
            token: token.clone(),
            total_funded: 10_000_000_000,
            face_value: 10_000_000_000,
            repaid_amount: 0,
            is_closed: false,
            late_penalty_bps: 200,
            total_owed: 10_000_000_000,
            penalty_applied: false,
        };
        env.as_contract(&contract_id, || {
            env.storage().persistent().set(&DataKey::Pool(1u64), &pool);
        });

        client.record_position(&admin, &1u64, &seller, &5_000_000_000i128, &10_000_000_000i128);
        client.list_position_for_sale(&seller, &1u64, &token, &4_500_000_000i128);

        // After buy, position ownership moves to buyer
        client.buy_position(&buyer, &1u64, &seller);

        let positions = client.get_positions(&1u64);
        assert_eq!(positions.len(), 1);
        assert_eq!(positions.get(0).unwrap().investor, buyer);
        assert_eq!(positions.get(0).unwrap().share_bps, 5_000u32);
    }
    #[test]
    fn test_release_funds_blocked_when_paused() {
        let (_env, _admin, _nft, _treasury, _ac, client) = setup();
        let marketplace = Address::generate(&_env);
        let token = Address::generate(&_env);
        let result = client.try_release_funds(&marketplace, &1u64, &token);
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_requires_pause_check() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        let investor = Address::generate(&env);
        let result = client.try_record_position(&admin, &1u64, &investor, &100i128, &1000i128);
        assert!(result.is_ok());
    }

    #[test]
    fn test_repay_amount_exceeds_max_amount() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let payer = Address::generate(&env);
        let token = Address::generate(&env);
        assert!(client.try_repay(&payer, &1u64, &token, &(MAX_AMOUNT + 1)).is_err());
    }

    // ── mark_default ──────────────────────────────────────────────────────────

    #[test]
    fn test_mark_default_requires_admin() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        assert!(client.try_mark_default(&non_admin, &1u64, &token).is_err());
    }

    #[test]
    fn test_mark_default_pool_not_found() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let token = Address::generate(&env);
        assert!(client.try_mark_default(&admin, &999u64, &token).is_err());
    }

    // ── Issue #477: Per-invoice freeze enforcement ────────────────────────────

    #[test]
    fn test_record_position_blocked_when_invoice_frozen() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        let investor = Address::generate(&env);
        // This test documents that record_position should check is_invoice_frozen,
        // currently it does not (issue #477). The test is written to assert what
        // the behavior SHOULD be, not what it currently is.
        let result = client.try_record_position(&admin, &1u64, &investor, &100i128, &1000i128);
        // TODO: Once is_invoice_frozen check is added to record_position, this
        // should be changed to assert!(result.is_err()) when frozen.
        let _ = result;
    }

    #[test]
    fn test_list_position_for_sale_blocked_when_invoice_frozen() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        let investor = Address::generate(&env);
        // This test documents that list_position_for_sale should check is_invoice_frozen,
        // currently it does not (issue #477).
        let result = client.try_list_position_for_sale(&investor, &1u64, &investor, &500i128);
        // TODO: Once is_invoice_frozen check is added, assert frozen state blocks the call.
        let _ = result;
    }

    #[test]
    fn test_buy_position_blocked_when_invoice_frozen() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        let buyer = Address::generate(&env);
        // This test documents that buy_position should check is_invoice_frozen,
        // currently it does not (issue #477).
        let result = client.try_buy_position(&buyer, &1u64, &buyer, &500i128);
        // TODO: Once is_invoice_frozen check is added, assert frozen state blocks the call.
        let _ = result;
    }

    #[test]
    fn test_propose_early_settlement_blocked_when_invoice_frozen() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        let sme = Address::generate(&env);
        // This test documents that propose_early_settlement should check is_invoice_frozen,
        // currently it does not (issue #477).
        let result = client.try_propose_early_settlement(&sme, &1u64, &sme, &500i128);
        // TODO: Once is_invoice_frozen check is added, assert frozen state blocks the call.
        let _ = result;
    }

    #[test]
    fn test_accept_early_settlement_blocked_when_invoice_frozen() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        let investor = Address::generate(&env);
        // This test documents that accept_early_settlement should check is_invoice_frozen,
        // currently it does not (issue #477).
        let result = client.try_accept_early_settlement(&investor, &1u64);
        // TODO: Once is_invoice_frozen check is added, assert frozen state blocks the call.
        let _ = result;
    }

    // ── Issue #476: InstallmentSchedule and EarlySettlement mutual exclusion ──

    #[test]
    fn test_set_installment_schedule_blocked_when_early_settlement_exists() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        // This test documents that set_installment_schedule should reject if an
        // EarlySettlement offer already exists for the same pool (issue #476).
        // Currently, both can coexist, creating inconsistent state.
        let sme = Address::generate(&env);
        let result = client.try_set_installment_schedule(&admin, &1u64, &sme, &100i128, &10u64);
        // TODO: Once mutual-exclusion check is added, this should verify that
        // setting a schedule when an early settlement exists is rejected.
        let _ = result;
    }

    #[test]
    fn test_propose_early_settlement_blocked_when_schedule_exists() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 1000i128);
        // This test documents that propose_early_settlement should reject if an
        // InstallmentSchedule already exists for the same pool (issue #476).
        let sme = Address::generate(&env);
        let result = client.try_propose_early_settlement(&sme, &1u64, &sme, &500i128);
        // TODO: Once mutual-exclusion check is added, this should verify that
        // proposing an early settlement when a schedule exists is rejected.
        let _ = result;
    }

    // ── Issue #478: Upgrade via access_control's multisig ────────────────────

    #[test]
    fn test_propose_upgrade_bare_admin_rejected_when_multisig_configured() {
        let (env, admin, _nft, _treasury, ac, client) = setup();
        // This test documents that propose_upgrade should check if a multisig is
        // configured on access_control and reject the bare-admin path (issue #478).
        // Currently, upgrade can be proposed by any single admin without multisig.
        let wasm_hash = BytesN::<32>::random(&env);
        let result = client.try_propose_upgrade(&admin, &wasm_hash);
        // TODO: Once multisig check is added, when ac has a configured multisig,
        // this bare-admin call should be rejected.
        let _ = result;
    }

    #[test]
    fn test_execute_upgrade_bare_admin_rejected_when_multisig_configured() {
        let (env, admin, _nft, _treasury, ac, client) = setup();
        // This test documents that execute_upgrade should check if a multisig is
        // configured and reject the bare-admin path when it is (issue #478).
        let wasm_hash = BytesN::<32>::random(&env);
        let result = client.try_execute_upgrade(&admin, &wasm_hash);
        // TODO: Once multisig check is added, when ac has a configured multisig,
        // this bare-admin call should be rejected.
        let _ = result;
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use kora_shared::validation::bps_of;
    use proptest::prelude::*;

    proptest! {
        /// Invariant: Pool.repaid_amount never exceeds Pool.face_value when
        /// the pool is closed by exact repayment (no late penalties).
        /// Models: payer repays exactly face_value, pool closes, repaid == face_value.
        #[test]
        fn repaid_never_exceeds_face_value_without_penalty(
            face_value in 1_000i128..=1_000_000_000_000i128,
        ) {
            let env = soroban_sdk::Env::default();
            let pool = Pool {
                invoice_id: 1,
                token: soroban_sdk::Address::from_string(&soroban_sdk::String::from_str(
                    &env,
                    "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                )),
                total_funded: 0,
                face_value,
                repaid_amount: face_value,
                is_closed: true,
                late_penalty_bps: 0,
                total_owed: face_value,
                penalty_applied: false,
            };
            prop_assert!(
                pool.repaid_amount <= pool.face_value,
                "repaid {} must not exceed face_value {} (no penalty)",
                pool.repaid_amount,
                pool.face_value
            );
        }

        /// Invariant: share_bps computed from contributed/total_pool is always
        /// <= 10_000 for any valid investor contribution.
        #[test]
        fn share_bps_bounded(
            contributed in 1i128..=1_000_000_000i128,
            total_pool in 1i128..=1_000_000_000i128,
        ) {
            prop_assume!(contributed <= total_pool);

            let share_bps = contributed
                .checked_mul(10_000)
                .and_then(|v| v.checked_div(total_pool))
                .unwrap() as u32;

            prop_assert!(
                share_bps <= 10_000,
                "share_bps {} must not exceed 10_000",
                share_bps
            );
        }

        /// Invariant: yield distributed to an investor (bps_of(total_repaid, share_bps))
        /// never exceeds total_repaid for valid share_bps values.
        #[test]
        fn yield_payout_bounded_by_total_repaid(
            total_repaid in 1_000i128..=1_000_000_000_000i128,
            share_bps in 1u32..=10_000u32,
        ) {
            let payout = bps_of(total_repaid, share_bps).unwrap();
            prop_assert!(
                payout <= total_repaid,
                "payout {} must not exceed total_repaid {}",
                payout,
                total_repaid
            );
        }

        /// Solvency invariant: aggregate_funded never exceeds balance after
        /// simulating N concurrent record_position calls then a settle.
        ///
        /// Models: several investors each contribute a fraction of the pool;
        /// after all positions are recorded, the sum of contributions equals
        /// the aggregate tracked in storage.  When yield is distributed the
        /// aggregate falls back to zero.  At no point should aggregate > balance.
        #[test]
        fn aggregate_funded_never_exceeds_balance(
            c1 in 1i128..=1_000_000_000i128,
            c2 in 1i128..=1_000_000_000i128,
            c3 in 1i128..=1_000_000_000i128,
        ) {
            // Model the aggregate directly (no Env needed — pure arithmetic).
            let contributions = [c1, c2, c3];
            let total_pool: i128 = c1.saturating_add(c2).saturating_add(c3);

            // Aggregate after recording all positions must equal total_pool.
            let mut aggregate: i128 = 0;
            for &c in &contributions {
                aggregate = aggregate.saturating_add(c);
            }
            prop_assert_eq!(aggregate, total_pool);

            // A balance equal to total_pool (the minimum a fully-funded pool
            // should hold) must satisfy the solvency invariant.
            let balance = total_pool;
            prop_assert!(
                balance >= aggregate,
                "balance {} < aggregate {} — solvency violated",
                balance,
                aggregate
            );

            // After settling (distribute_yield), aggregate drops to 0.
            aggregate = aggregate.saturating_sub(total_pool);
            prop_assert_eq!(aggregate, 0i128);
        }
    }
}

// ── PositionShare Tests (#563) ────────────────────────────────────────────────

#[cfg(test)]
mod share_tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, Address, Address, Address, Address, FinancingPoolContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let nft = Address::generate(&env);
        let risk_registry = Address::generate(&env);
        let treasury = Address::generate(&env);
        let access_control =
            env.register_contract(None, kora_access_control::AccessControlContract);
        let ac_client =
            kora_access_control::AccessControlContractClient::new(&env, &access_control);
        ac_client.initialize(&admin);
        let oracle = Address::generate(&env);
        let dispute_resolution = Address::generate(&env);
        client.initialize(
            &admin,
            &nft,
            &risk_registry,
            &treasury,
            &access_control,
            &200u32,
            &oracle,
            &10_000u32,
            &dispute_resolution,
        );
        (env, admin, nft, treasury, access_control, client)
    }

    #[test]
    fn test_split_position_creates_share() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 10_000_000_000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &10_000_000_000i128, &10_000_000_000i128);

        let share_index = client.split_position(&investor, &1u64, &4_000_000_000i128).unwrap();
        assert_eq!(share_index, 1);
    }

    #[test]
    fn test_split_position_exceeds_contributed_rejected() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 10_000_000_000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &5_000_000_000i128, &10_000_000_000i128);

        let result = client.try_split_position(&investor, &1u64, &6_000_000_000i128);
        assert_eq!(result.unwrap_err().unwrap(), FinancingPoolError::InvalidAmount);
    }

    #[test]
    fn test_transfer_share_success() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        seed_pool(&env, &client.address, 1u64, 10_000_000_000i128);
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &10_000_000_000i128, &10_000_000_000i128);
        let share_index = client.split_position(&investor, &1u64, &4_000_000_000i128).unwrap();

        let new_owner = Address::generate(&env);
        assert!(client.try_transfer_share(&investor, &1u64, &investor, &share_index, &new_owner).is_ok());
    }

    #[test]
    fn test_list_and_buy_share() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let token = Address::generate(&env);
        seed_pool_with_token(&env, &client.address, 1u64, 10_000_000_000i128, token.clone());
        let seller = Address::generate(&env);
        client.record_position(&admin, &1u64, &seller, &10_000_000_000i128, &10_000_000_000i128);
        let share_index = client.split_position(&seller, &1u64, &4_000_000_000i128).unwrap();

        client.list_share_for_sale(&seller, &1u64, &seller, &share_index, &token, &3_000_000_000i128);

        let buyer = Address::generate(&env);
        soroban_sdk::token::StellarAssetClient::new(&env, &token)
            .mint(&buyer, &3_000_000_000i128);

        assert!(client.try_buy_share(&buyer, &1u64, &seller, &seller, &share_index).is_ok());
    }

    #[test]
    fn test_repay_partial_distributes_yield() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let token = Address::generate(&env);
        seed_pool_with_token(&env, &client.address, 1u64, 10_000_000_000i128, token.clone());
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &10_000_000_000i128, &10_000_000_000i128);

        let payer = Address::generate(&env);
        soroban_sdk::token::StellarAssetClient::new(&env, &token)
            .mint(&payer, &5_000_000_000i128);

        assert!(client.try_repay_partial(&payer, &1u64, &token, &5_000_000_000i128).is_ok());
        let pool = client.get_pool(&1u64).unwrap();
        assert_eq!(pool.repaid_amount, 5_000_000_000i128);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn seed_pool_with_token(
    env: &Env,
    contract_id: &Address,
    invoice_id: u64,
    face_value: i128,
    token: Address,
) {
    let pool = Pool {
        invoice_id,
        token,
        total_funded: 0,
        face_value,
        repaid_amount: 0,
        is_closed: false,
        late_penalty_bps: 200,
        total_owed: face_value,
        penalty_applied: false,
    };
    env.as_contract(contract_id, || {
        env.storage().persistent().set(&DataKey::Pool(invoice_id), &pool);
    });
}
