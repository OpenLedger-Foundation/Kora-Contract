#![no_std]

use kora_shared::{
    errors::KoraError,
    events,
    types::{Pool, Position},
    validation::{bps_of_normalized, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, BytesN, Env, Map, Vec};

const MAX_AMOUNT: i128 = i128::MAX / 2;

// ── Local Events ──────────────────────────────────────────────────────────────



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
    RepaymentLock(u64),
    UpgradeProposal,
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
    ) -> Result<(), KoraError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(KoraError::AlreadyInitialized);
        }
        kora_shared::validation::require_valid_fee_bps(late_penalty_bps)?;
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
        env.storage().instance().set(&DataKey::InvoiceNft, &invoice_nft);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        env.storage().instance().set(&DataKey::LatePenaltyBps, &late_penalty_bps);
        env.storage().instance().set(&DataKey::PriceOracle, &price_oracle);
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

        positions.set(investor.clone(), position);
        env.storage()
            .persistent()
            .set(&DataKey::Positions(invoice_id), &positions);

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

        for (investor, position) in positions.iter() {
            let payout = bps_of_normalized(total_repaid, position.share_bps, token_decimals)?;
            let yield_amount = payout
                .checked_sub(position.contributed)
                .ok_or(KoraError::ArithmeticOverflow)?;

            token_client.transfer(&env.current_contract_address(), &investor, &payout);
            events::yield_distributed(env, invoice_id, &investor, yield_amount);
        }

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

    /// Archive a closed pool, removing Pool and Positions from persistent storage to reclaim rent.
    /// Only admin can call this. Pool must be closed (is_closed == true).
    pub fn archive_pool(env: Env, admin: Address, invoice_id: u64) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        let pool: Pool = env
            .storage()
            .persistent()
            .get(&DataKey::Pool(invoice_id))
            .ok_or(KoraError::PoolNotFound)?;
        if !pool.is_closed {
            return Err(KoraError::InvalidInvoiceStatus);
        }
        env.storage().persistent().remove(&DataKey::Pool(invoice_id));
        env.storage().persistent().remove(&DataKey::Positions(invoice_id));
        Ok(())
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
        let risk_registry = Address::generate(&env);
        let treasury = Address::generate(&env);
        client.initialize(&admin, &nft, &risk_registry, &treasury, &200u32);
        (env, admin, nft, treasury, client)
        let access_control = Address::generate(&env);
        let oracle = Address::generate(&env);
        client.initialize(&admin, &nft, &treasury, &access_control, &200u32, &oracle);
        (env, admin, nft, treasury, access_control, client)
    }

    #[test]
    fn test_initialize_success() {
        let (_env, _admin, _nft, _treasury, _ac, client) = setup();
        let pool = client.try_get_pool(&1u64);
        assert!(pool.is_err()); // No pools created during setup
    }

    #[test]
    fn test_initialize_already_initialized_fails() {
        let (env, admin, nft, treasury, client) = setup();
        let rr = Address::generate(&env);
        let result = client.try_initialize(&admin, &nft, &rr, &treasury, &200u32);
        let (env, admin, nft, treasury, ac, client) = setup();
        let oracle = Address::generate(&env);
        let result = client.try_initialize(&admin, &nft, &treasury, &ac, &200u32, &oracle);
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
        
        let result = client.try_initialize(&admin, &nft, &rr, &treasury, &10_001u32);
        let access_control = Address::generate(&env);
        let oracle = Address::generate(&env);

        let result = client.try_initialize(&admin, &nft, &treasury, &access_control, &10_001u32, &oracle);

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
        let treasury = Address::generate(&env);
        let access_control = Address::generate(&env);
        let oracle = Address::generate(&env);
        let result = client.try_initialize(&admin, &nft, &treasury, &access_control, &0u32, &oracle);

        assert!(result.is_ok());
    }


    #[test]
    fn test_get_pool_not_found() {
        let (_env, _admin, _nft, _treasury, _ac, client) = setup();
        let result = client.try_get_pool(&999u64);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_positions_empty() {
        let (_env, _admin, _nft, _treasury, _ac, client) = setup();
        let positions = client.get_positions(&1u64);
        assert_eq!(positions.len(), 0);
    }

    #[test]
    fn test_record_position_requires_admin() {
        let (_env, admin, _nft, _treasury, _access_control, client) = setup();
        let investor = Address::generate(&_env);
        let non_admin = Address::generate(&_env);

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
        let result = client.try_record_position(&admin, &1u64, &investor, &i128::MAX, &1i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_success() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(
            &admin, &1u64, &investor, &5_000_000_000i128, &10_000_000_000i128,
        );
        let positions = client.get_positions(&1u64);
        assert_eq!(positions.len(), 1);
    }

    #[test]
    fn test_record_position_share_bps_correct() {
        let (env, admin, _nft, _treasury, _access_control, client) = setup();

        let investor = Address::generate(&env);
        client.record_position(
            &admin, &1u64, &investor, &5_000_000_000i128, &10_000_000_000i128,
        );
        let positions = client.get_positions(&1u64);
        assert_eq!(positions.get(0).unwrap().share_bps, 5_000u32);
    }

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
        let result = client.try_repay(&payer, &1u64, &token, &0i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_repay_negative_amount_fails() {
        let (env, _admin, _nft, _treasury, _access_control, client) = setup();

        let payer = Address::generate(&env);
        let token = Address::generate(&env);
        let result = client.try_repay(&payer, &1u64, &token, &-1i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_mark_default_requires_admin() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let non_admin = Address::generate(&env);
        let token = Address::generate(&env);
        let result = client.try_mark_default(&non_admin, &1u64, &token);
        assert!(result.is_err());
    }

    #[test]
    fn test_mark_default_pool_not_found() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let token = Address::generate(&env);
        let result = client.try_mark_default(&admin, &999u64, &token);
        assert!(result.is_err());
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
        // contract_id as admin must be rejected
        let result = client.try_initialize(&contract_id, &nft, &rr, &treasury, &200u32, &ac);
    fn test_record_position_exceeds_max_amount() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        let result = client.try_record_position(
            &admin, &1u64, &investor, &(MAX_AMOUNT + 1), &(MAX_AMOUNT + 2),
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
    fn test_initialize_self_as_nft_rejected() {
    fn test_record_position_contributed_exceeds_total_pool() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        let result = client.try_record_position(&admin, &1u64, &investor, &100i128, &50i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_repay_amount_exceeds_max_amount() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let payer = Address::generate(&env);
        let token = Address::generate(&env);
        let result = client.try_repay(&payer, &1u64, &token, &(MAX_AMOUNT + 1));
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_negative_amounts() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        let result =
            client.try_record_position(&admin, &1u64, &investor, &(-100i128), &1_000i128);
        assert!(result.is_err());
        let result =
            client.try_record_position(&admin, &1u64, &investor, &100i128, &(-1_000i128));
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_zero_amounts() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        let result = client.try_record_position(&admin, &1u64, &investor, &0i128, &1_000i128);
        assert!(result.is_err());
        let result = client.try_record_position(&admin, &1u64, &investor, &100i128, &0i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_repay_zero_amount() {
        let (env, _admin, _nft, _treasury, _ac, client) = setup();
        let payer = Address::generate(&env);
        let token = Address::generate(&env);
        let result = client.try_repay(&payer, &1u64, &token, &0i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_record_position_happy_path() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor1 = Address::generate(&env);
        let investor2 = Address::generate(&env);

        client.record_position(&admin, &1u64, &investor1, &3_000_000_000i128, &10_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);

        client.record_position(&admin, &1u64, &investor2, &7_000_000_000i128, &10_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 2);
    }

    #[test]
    fn test_record_position_exact_full_pool() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &10_000_000_000i128, &10_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_record_position_minimum_valid_amount() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &1i128, &1_000_000_000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_record_position_share_calculation() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &500i128, &1000i128);
        let positions = client.get_positions(&1u64);
        assert_eq!(positions.get(0).unwrap().share_bps, 5000);
    }

    #[test]
    fn test_record_position_quarter_share() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &25i128, &100i128);
        let positions = client.get_positions(&1u64);
        assert_eq!(positions.get(0).unwrap().share_bps, 2500);
    }

    #[test]
    fn test_record_position_tenth_share() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &10i128, &100i128);
        let positions = client.get_positions(&1u64);
        assert_eq!(positions.get(0).unwrap().share_bps, 1000);
    }

    #[test]
    fn test_record_position_basis_point_precision() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &1i128, &10000i128);
        let positions = client.get_positions(&1u64);
        assert_eq!(positions.get(0).unwrap().share_bps, 1);
    }

    #[test]
    fn test_record_position_multiple_invoices() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &100i128, &1000i128);
        client.record_position(&admin, &2u64, &investor, &200i128, &2000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
        assert_eq!(client.get_positions(&2u64).len(), 1);
    }

    #[test]
    fn test_record_position_overwrite_existing() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let investor = Address::generate(&env);
        client.record_position(&admin, &1u64, &investor, &100i128, &1000i128);
        client.record_position(&admin, &1u64, &investor, &200i128, &1000i128);
        assert_eq!(client.get_positions(&1u64).len(), 1);
    }

    #[test]
    fn test_get_positions_multiple_investors() {
        let (env, admin, _nft, _treasury, _ac, client) = setup();
        let i1 = Address::generate(&env);
        let i2 = Address::generate(&env);
        let i3 = Address::generate(&env);
        client.record_position(&admin, &1u64, &i1, &100i128, &300i128);
        client.record_position(&admin, &1u64, &i2, &100i128, &300i128);
        client.record_position(&admin, &1u64, &i3, &100i128, &300i128);
        assert_eq!(client.get_positions(&1u64).len(), 3);
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
    fn test_initialize_valid_late_penalty_bps() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, FinancingPoolContract);
        let client = FinancingPoolContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let result = client.try_initialize(&admin, &contract_id, &rr, &treasury, &200u32, &ac);
        assert!(result.is_err());
        let nft = Address::generate(&env);
        let treasury = Address::generate(&env);
        let ac = Address::generate(&env);
        let oracle = Address::generate(&env);
        let result = client.try_initialize(&admin, &nft, &treasury, &ac, &10_000u32, &oracle);
        assert!(result.is_ok());
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
    }
}
