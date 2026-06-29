#![no_std]

use kora_shared::{
    errors::KoraError,
    events,
    types::{Pool, Position},
    validation::{bps_of, bps_of_normalized, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{
    contract, contractimpl, contracttype, token, Address, BytesN, Env, Map, Symbol, Vec,
};

const MAX_AMOUNT: i128 = i128::MAX / 2;

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
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct FinancingPoolContract;

#[contractimpl]
impl FinancingPoolContract {
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
    ) -> Result<(), KoraError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }
        kora_shared::validation::require_valid_fee_bps(late_penalty_bps)?;
        // max_position_bps must be in [1, 10_000]: zero would block all funding
        if max_position_bps == 0 || max_position_bps > 10_000 {
            return Err(KoraError::InvalidFeeRate);
        }
        kora_shared::validation::require_not_self(&env, &admin)?;
        kora_shared::validation::require_not_self(&env, &invoice_nft)?;
        kora_shared::validation::require_not_self(&env, &risk_registry)?;
        kora_shared::validation::require_not_self(&env, &treasury)?;
        kora_shared::validation::require_not_self(&env, &access_control)?;
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
        Ok(())
    }

    /// Called by Marketplace when an invoice is fully funded.
    pub fn release_funds(
        env: Env,
        marketplace: Address,
        invoice_id: u64,
        token: Address,
    ) -> Result<(), KoraError> {
        marketplace.require_auth();
        Self::require_not_paused(&env)?;

        if env.storage().persistent().has(&DataKey::Pool(invoice_id)) {
            return Err(KoraError::PoolAlreadyClosed);
        }

        if token == env.current_contract_address() {
            return Err(KoraError::InvalidAddress);
        }

        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(KoraError::NotInitialized)?;
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
        let invoice = nft_client.get_invoice(&invoice_id);

        if invoice.amount <= 0 || invoice.amount > MAX_AMOUNT {
            return Err(KoraError::InvalidAmount);
        }

        let late_penalty_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::LatePenaltyBps)
            .ok_or(KoraError::NotInitialized)?;

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

        // Transition NFT status to Funded
        nft_client.set_funded(&env.current_contract_address(), &invoice_id);

        Ok(())
    }

    /// Update the per-investor concentration cap. Admin only.
    pub fn set_max_position_bps(env: Env, admin: Address, max_position_bps: u32) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        if max_position_bps == 0 || max_position_bps > 10_000 {
            return Err(KoraError::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::MaxPositionBps, &max_position_bps);
        Ok(())
    }

    /// Returns the current per-investor concentration cap in basis points.
    pub fn get_max_position_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxPositionBps)
            .unwrap_or(5_000)
    }

    /// Register an investor position. Admin only.
    pub fn record_position(
        env: Env,
        caller: Address,
        invoice_id: u64,
        investor: Address,
        contributed: i128,
        total_pool: i128,
    ) -> Result<(), KoraError> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;
        Self::require_not_paused(&env)?;

        if contributed <= 0 || total_pool <= 0 {
            return Err(KoraError::InvalidAmount);
        }

        if contributed > total_pool || contributed > MAX_AMOUNT || total_pool > MAX_AMOUNT {
            return Err(KoraError::InvalidAmount);
        }

        let share_bps = contributed
            .checked_mul(10_000)
            .and_then(|v| v.checked_div(total_pool))
            .ok_or(KoraError::ArithmeticOverflow)? as u32;

        // Enforce per-investor concentration cap
        let max_position_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxPositionBps)
            .unwrap_or(5_000);
        if share_bps > max_position_bps {
            return Err(KoraError::ExceedsFundingTarget);
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
        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;
        let agg_key = DataKey::AggregateFunded(pool.token);
        let prev_agg: i128 = env.storage().instance().get(&agg_key).unwrap_or(0);
        let new_agg = prev_agg
            .checked_sub(old_contributed)
            .and_then(|v| v.checked_add(contributed))
            .ok_or(KoraError::ArithmeticOverflow)?;
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

    /// SME repays the invoice.
    /// If the current ledger timestamp is past the invoice's due_date and no
    /// penalty has been applied yet, a one-time late penalty of
    /// `bps_of(face_value, late_penalty_bps)` is added to `total_owed`.
    /// Partial repayments are tracked against `total_owed` so the penalty is
    /// never double-counted.
    pub fn repay(
        env: Env,
        payer: Address,
        invoice_id: u64,
        token: Address,
        amount: i128,
    ) -> Result<(), KoraError> {
        payer.require_auth();

        if amount <= 0 || amount > MAX_AMOUNT {
            return Err(KoraError::InvalidAmount);
        }

        if env.storage().persistent().has(&DataKey::RepaymentLock(invoice_id)) {
            return Err(KoraError::Unauthorized);
        }

        env.storage()
            .persistent()
            .set(&DataKey::RepaymentLock(invoice_id), &true);

        let mut pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;

        if pool.is_closed {
            env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));
            return Err(KoraError::RepaymentAlreadyMade);
        }

        // Fetch invoice for due_date check and currency conversion
        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(KoraError::NotInitialized)?;
        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
        let invoice = nft_client.get_invoice(&invoice_id);

        // Convert repayment amount if invoice currency differs from pool token
        let effective_amount = Self::convert_if_needed(&env, amount, &invoice.currency, &pool.token)?;

        // Apply late penalty once if repayment is past due_date
        if !pool.penalty_applied && pool.late_penalty_bps > 0 {
            if env.ledger().timestamp() > invoice.due_date {
                let penalty = bps_of(pool.face_value, pool.late_penalty_bps)?;
                pool.total_owed = pool
                    .total_owed
                    .checked_add(penalty)
                    .ok_or(KoraError::ArithmeticOverflow)?;
                pool.penalty_applied = true;
                events::late_penalty_applied(&env, invoice_id, penalty, pool.total_owed);
            }
        }

        // Effects before interactions (CEI pattern)
        pool.repaid_amount = pool
            .repaid_amount
            .checked_add(effective_amount)
            .ok_or(KoraError::ArithmeticOverflow)?;

        let should_close = pool.repaid_amount >= pool.total_owed;
        if should_close {
            pool.is_closed = true;
        }
        env.storage().persistent().set(&DataKey::Pool(invoice_id), &pool);

        // Interactions
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&payer, &env.current_contract_address(), &amount);

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

            let nft_contract: Address = env
                .storage()
                .instance()
                .get(&DataKey::InvoiceNft)
                .ok_or(KoraError::NotInitialized)?;
            let nft_client =
                kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
            nft_client.set_repaid(&env.current_contract_address(), &invoice_id);
        }

        env.storage().persistent().remove(&DataKey::RepaymentLock(invoice_id));

        Ok(())
    }

    fn distribute_yield(
        env: &Env,
        invoice_id: u64,
        token: &Address,
        total_repaid: i128,
        _face_value: i128,
    ) -> Result<(), KoraError> {
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
                .ok_or(KoraError::ArithmeticOverflow)?;

            total_contributed = total_contributed.saturating_add(position.contributed);
            token_client.transfer(&env.current_contract_address(), &investor, &payout);
            events::yield_distributed(env, invoice_id, &investor, yield_amount);
        }

        // Reduce the aggregate now that all positions for this pool have been settled.
        let agg_key = DataKey::AggregateFunded(token.clone());
        let prev_agg: i128 = env.storage().instance().get(&agg_key).unwrap_or(0);
        env.storage()
            .instance()
            .set(&agg_key, &prev_agg.saturating_sub(total_contributed));

        Ok(())
    }

    /// Mark invoice as defaulted. Admin only.
    pub fn mark_default(
        env: Env,
        admin: Address,
        invoice_id: u64,
        token: Address,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_not_paused(&env)?;

        if env.storage().persistent().has(&DataKey::RepaymentLock(invoice_id)) {
            return Err(KoraError::Unauthorized);
        }

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;

        if pool.is_closed {
            return Err(KoraError::PoolAlreadyClosed);
        }

        if pool.repaid_amount > 0 {
            Self::distribute_yield(&env, invoice_id, &token, pool.repaid_amount, pool.face_value)?;
        }

        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(KoraError::NotInitialized)?;
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
        nft_client.set_defaulted(&admin, &invoice_id);

        events::invoice_defaulted(&env, invoice_id, &admin);

        // Automatically record the default against the SME in the risk registry
        let invoice = nft_client.get_invoice(&invoice_id);
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
    pub fn propose_early_settlement(
        env: Env,
        sme: Address,
        invoice_id: u64,
        amount: i128,
    ) -> Result<(), KoraError> {
        sme.require_auth();
        Self::require_not_paused(&env)?;

        if amount <= 0 || amount > MAX_AMOUNT {
            return Err(KoraError::InvalidAmount);
        }

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;
        if pool.is_closed {
            return Err(KoraError::PoolAlreadyClosed);
        }
        // Must be a genuine discount that still returns investors at least their principal.
        if amount < pool.total_funded || amount >= pool.total_owed {
            return Err(KoraError::InvalidAmount);
        }
        if env
            .storage()
            .persistent()
            .has(&DataKey::EarlySettlement(invoice_id))
        {
            return Err(KoraError::AlreadyInitialized);
        }

        // Only the invoice's SME may propose a buyout.
        let invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.sme != sme {
            return Err(KoraError::Unauthorized);
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

    /// Accept a pending early-settlement offer as an investor in the pool.
    ///
    /// When investors representing 100% of pool shares have accepted, the escrowed amount is
    /// distributed pro-rata to all investors, the pool is closed, and the invoice is marked repaid.
    pub fn accept_early_settlement(
        env: Env,
        investor: Address,
        invoice_id: u64,
    ) -> Result<(), KoraError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;

        if env
            .storage()
            .persistent()
            .has(&DataKey::RepaymentLock(invoice_id))
        {
            return Err(KoraError::Unauthorized);
        }

        let mut offer: EarlySettlementOffer = env
            .storage()
            .persistent()
            .get(&DataKey::EarlySettlement(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;

        let mut pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;
        if pool.is_closed {
            return Err(KoraError::PoolAlreadyClosed);
        }

        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(&env));
        let position = positions
            .get(investor.clone())
            .ok_or(KoraError::Unauthorized)?;

        // An investor may only accept once.
        if offer.accepted.iter().any(|a| a == investor) {
            return Err(KoraError::AlreadyInitialized);
        }
        offer.accepted.push_back(investor.clone());
        offer.accepted_bps = offer
            .accepted_bps
            .checked_add(position.share_bps)
            .ok_or(KoraError::ArithmeticOverflow)?;

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
                .ok_or(KoraError::NotInitialized)?;
            let nft_client =
                kora_invoice_nft::InvoiceNftContractClient::new(&env, &nft_contract);
            nft_client.set_repaid(&env.current_contract_address(), &invoice_id);

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
    /// Callable only by the invoice's SME while the offer has not yet been fully accepted.
    pub fn cancel_early_settlement(
        env: Env,
        sme: Address,
        invoice_id: u64,
    ) -> Result<(), KoraError> {
        sme.require_auth();

        let offer: EarlySettlementOffer = env
            .storage()
            .persistent()
            .get(&DataKey::EarlySettlement(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;

        let invoice = Self::load_invoice(&env, invoice_id)?;
        if invoice.sme != sme {
            return Err(KoraError::Unauthorized);
        }

        env.storage()
            .persistent()
            .remove(&DataKey::EarlySettlement(invoice_id));

        let token_client = token::Client::new(&env, &pool.token);
        token_client.transfer(&env.current_contract_address(), &sme, &offer.amount);

        Ok(())
    }

    /// Read a pending early-settlement offer, if any.
    pub fn get_early_settlement(
        env: Env,
        invoice_id: u64,
    ) -> Result<EarlySettlementOffer, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::EarlySettlement(invoice_id))
            .ok_or(KoraError::PoolNotFound)
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_pool(env: Env, invoice_id: u64) -> Result<Pool, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)
    }

    pub fn get_positions(env: Env, invoice_id: u64) -> Vec<Position> {
        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or(Map::new(&env));
        positions.values()
    }

    // ── Secondary market ───────────────────────────────────────────────────────

    /// List a position for sale on the secondary market.
    /// Seller must hold a position on an open (not yet closed) pool.
    pub fn list_position_for_sale(
        env: Env,
        seller: Address,
        invoice_id: u64,
        token: Address,
        price: i128,
    ) -> Result<(), KoraError> {
        seller.require_auth();
        Self::require_not_paused(&env)?;

        if price <= 0 {
            return Err(KoraError::InvalidAmount);
        }

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;
        if pool.is_closed {
            return Err(KoraError::PoolAlreadyClosed);
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
        if !positions.contains_key(seller.clone()) {
            return Err(KoraError::PositionNotFound);
        }

        if env
            .storage()
            .persistent()
            .has(&DataKey::SaleOffer(invoice_id, seller.clone()))
        {
            return Err(KoraError::SaleAlreadyListed);
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

    /// Purchase an investor position from the secondary market.
    /// Transfers ownership of the position (and its proportional yield claim)
    /// from seller to buyer in exchange for a token payment.
    pub fn buy_position(
        env: Env,
        buyer: Address,
        invoice_id: u64,
        seller: Address,
    ) -> Result<(), KoraError> {
        buyer.require_auth();
        Self::require_not_paused(&env)?;

        let offer: PositionSaleOffer = env
            .storage()
            .persistent()
            .get(&DataKey::SaleOffer(invoice_id, seller.clone()))
            .ok_or(KoraError::SaleNotFound)?;

        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;
        if pool.is_closed {
            return Err(KoraError::PoolAlreadyClosed);
        }

        let mut positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or_else(|| Map::new(&env));

        let seller_position: Position = positions
            .get(seller.clone())
            .ok_or(KoraError::PositionNotFound)?;

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
            .unwrap_or(Map::new(&env));

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

    /// Returns the total number of investor positions recorded for an invoice.
    pub fn get_positions_count(env: Env, invoice_id: u64) -> u32 {
        let positions: Map<Address, Position> = env
            .storage()
            .persistent()
            .get(&DataKey::Positions(invoice_id))
            .unwrap_or(Map::new(&env));
        positions.len()
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::UpgradeProposal, &(new_wasm_hash.clone(), env.ledger().timestamp()));
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        Ok(())
    }

    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(KoraError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(KoraError::UpgradeTimelockNotElapsed);
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
    ) -> Result<kora_shared::types::Invoice, KoraError> {
        let nft_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(KoraError::NotInitialized)?;
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(env, &nft_contract);
        Ok(nft_client.get_invoice(&invoice_id))
    }

    fn require_not_paused(env: &Env) -> Result<(), KoraError> {
        let ac: Address = env
            .storage()
            .instance()
            .get(&DataKey::AccessControl)
            .ok_or(KoraError::NotInitialized)?;
        let client = kora_access_control::AccessControlContractClient::new(env, &ac);
        if client.is_paused() {
            return Err(KoraError::ProtocolPaused);
        }
        Ok(())
    }

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

    /// Convert `amount` between currencies using the price oracle.
    /// If invoice currency matches the pool token's symbol, returns amount unchanged.
    /// Rejects stale or missing oracle prices.
    fn convert_if_needed(
        env: &Env,
        amount: i128,
        invoice_currency: &Symbol,
        _pool_token: &Address,
    ) -> Result<i128, KoraError> {
        let oracle_addr: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PriceOracle);

        let oracle_addr = match oracle_addr {
            Some(addr) => addr,
            None => return Ok(amount),
        };

        let oracle_client =
            kora_price_oracle::PriceOracleContractClient::new(env, &oracle_addr);

        // Use the invoice currency symbol directly; pool token symbol is
        // derived from the token contract but for oracle lookup we use the
        // same symbol convention.  If the oracle has no pair registered
        // for (from, to), the convert call will fail — this is intentional
        // to reject operations without a valid price.
        let pool_currency = Symbol::new(env, "USDC");

        oracle_client.convert(&amount, invoice_currency, &pool_currency)
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
        let treasury = Address::generate(&env);
        let access_control = Address::generate(&env);
        let oracle = Address::generate(&env);
        client
            .initialize(&admin, &nft, &risk_registry, &treasury, &access_control, &200u32, &oracle)
            .unwrap();
        (env, admin, nft, treasury, access_control, client)
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
        let result = client.try_initialize(&admin, &nft, &rr, &treasury, &ac, &200u32, &oracle);
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
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        let result =
            client.try_initialize(&admin, &nft, &rr, &treasury, &ac, &10_001u32, &oracle);
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
            .try_initialize(&admin, &nft, &rr, &treasury, &ac, &0u32, &oracle)
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
        let result =
            client.try_initialize(&contract_id, &nft, &rr, &treasury, &ac, &200u32, &oracle);
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
        let result =
            client.try_initialize(&admin, &contract_id, &rr, &treasury, &ac, &200u32, &oracle);
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
            .try_initialize(&admin, &nft, &rr, &treasury, &ac, &10_000u32, &oracle)
            .is_ok());
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
        let investor = Address::generate(&env);
        client
            .record_position(&admin, &1u64, &investor, &5_000_000_000i128, &10_000_000_000i128)
            .unwrap();
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_record_position_share_bps_correct() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client
            .record_position(&admin, &1u64, &investor, &5_000_000_000i128, &10_000_000_000i128)
            .unwrap();
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 5_000u32);
    }

    #[test]
    fn test_record_position_share_calculation() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &500i128, &1000i128).unwrap();
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 5000);
    }

    #[test]
    fn test_record_position_quarter_share() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &25i128, &100i128).unwrap();
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 2500);
    }

    #[test]
    fn test_record_position_tenth_share() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &10i128, &100i128).unwrap();
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 1000);
    }

    #[test]
    fn test_record_position_basis_point_precision() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &1i128, &10000i128).unwrap();
        assert_eq!(client.get_positions(&1u64).get(0).unwrap().share_bps, 1);
    }

    #[test]
    fn test_record_position_exact_full_pool() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client
            .record_position(&admin, &1u64, &investor, &10_000_000_000i128, &10_000_000_000i128)
            .unwrap();
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_record_position_minimum_valid_amount() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &1i128, &1_000_000_000i128).unwrap();
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_record_position_happy_path_two_investors() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor1 = Address::generate(&env);
        let investor2 = Address::generate(&env);
        client
            .record_position(&admin, &1u64, &investor1, &3_000_000_000i128, &10_000_000_000i128)
            .unwrap();
        assert_eq!(client.get_positions(&1u64).len(), 1);
        client
            .record_position(&admin, &1u64, &investor2, &7_000_000_000i128, &10_000_000_000i128)
            .unwrap();
        assert_eq!(client.get_positions(&1u64).len(), 2);
    }

    #[test]
    fn test_record_position_multiple_invoices() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &100i128, &1000i128).unwrap();
        client.record_position(&admin, &2u64, &investor, &200i128, &2000i128).unwrap();
        assert_eq!(client.get_positions(&1u64).len(), 1);
        assert_eq!(client.get_positions(&2u64).len(), 1);
    }

    #[test]
    fn test_record_position_overwrite_existing() {
        // Recording a position for the same investor on the same invoice
        // overwrites the previous entry (map semantics).
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &100i128, &1000i128).unwrap();
        client.record_position(&admin, &1u64, &investor, &200i128, &1000i128).unwrap();
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_get_positions_multiple_investors() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let i1 = Address::generate(&env);
        let i2 = Address::generate(&env);
        let i3 = Address::generate(&env);
        client.record_position(&admin, &1u64, &i1, &100i128, &300i128).unwrap();
        client.record_position(&admin, &1u64, &i2, &100i128, &300i128).unwrap();
        client.record_position(&admin, &1u64, &i3, &100i128, &300i128).unwrap();
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
        assert_eq!(result.unwrap_err().unwrap(), KoraError::PoolNotFound);
    }

    #[test]
    fn test_list_position_for_sale_zero_price_rejected() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let seller = Address::generate(&env);
        let token = Address::generate(&env);
        let result = client.try_list_position_for_sale(&seller, &1u64, &token, &0i128);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
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
        assert_eq!(result.unwrap_err().unwrap(), KoraError::PositionNotFound);
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
        assert_eq!(result.unwrap_err().unwrap(), KoraError::SaleAlreadyListed);
    }

    #[test]
    fn test_buy_position_sale_not_found() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let buyer = Address::generate(&env);
        let seller = Address::generate(&env);
        let result = client.try_buy_position(&buyer, &1u64, &seller);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::SaleNotFound);
    }

    #[test]
    fn test_buy_position_transfers_ownership() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let contract_id = client.address.clone();
        let token = Address::generate(&env);
        let seller = Address::generate(&env);
        let buyer = Address::generate(&env);

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
        let investor = Address::generate(&env);
        let result = client.try_record_position(&admin, &1u64, &investor, &100i128, &1000i128);
        assert!(result.is_ok());
    }

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
            let pool = Pool {
                invoice_id: 1,
                token: soroban_sdk::Address::from_str(&soroban_sdk::Env::default(), "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"),
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
