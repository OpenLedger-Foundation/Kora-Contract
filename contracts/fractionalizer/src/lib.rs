#![no_std]

use kora_shared::types::{Pool, Position};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, Map,
};

const MAX_SHARES: u64 = 1_000_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum FractionalizerError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotAdmin = 3,
    ProtocolPaused = 4,
    InvalidAmount = 5,
    InvalidAddress = 6,
    ArithmeticOverflow = 7,
    WrapNotFound = 8,
    SharesOutstanding = 9,
    PoolNotClosed = 10,
    AlreadyClaimed = 11,
    NotShareHolder = 12,
    InvalidShareCount = 13,
    PositionNotFound = 14,
}

#[contracttype]
pub enum DataKey {
    Admin,
    FinancingPool,
    AccessControl,
    /// Wrap record keyed by (invoice_id, original_investor)
    Wrap(u64, Address),
    /// Shares held by an address for a given wrap: (invoice_id, original_investor, holder)
    ShareBalance(u64, Address, Address),
    /// Whether a holder has already claimed for a wrap
    Claimed(u64, Address, Address),
}

/// Represents a wrapped position with fixed-supply fungible shares.
#[contracttype]
#[derive(Clone, Debug)]
pub struct WrapRecord {
    pub invoice_id: u64,
    pub original_investor: Address,
    /// Total number of shares minted (fixed at wrap time)
    pub total_shares: u64,
    /// Shares still outstanding (not yet burned via redeem)
    pub outstanding_shares: u64,
    /// The contributed amount from the underlying position
    pub contributed: i128,
    /// The position's share in bps at wrap time (fixed snapshot; the pool
    /// holds a single position per holder so re-reading later collides
    /// when several investors wrap into this contract).
    pub share_bps: u32,
    /// The pool token address
    pub token: Address,
    /// Total payout received from pool (set when pool closes)
    pub total_payout: i128,
    /// Whether the pool has been settled and payout recorded
    pub settled: bool,
}

#[contract]
pub struct FractionalizerContract;

#[contractimpl]
impl FractionalizerContract {
    /// One-time initialization.
    pub fn initialize(
        env: Env,
        admin: Address,
        financing_pool: Address,
        access_control: Address,
    ) -> Result<(), FractionalizerError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(FractionalizerError::AlreadyInitialized);
        }
        if admin == env.current_contract_address()
            || financing_pool == env.current_contract_address()
            || access_control == env.current_contract_address()
        {
            return Err(FractionalizerError::InvalidAddress);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        Ok(())
    }

    /// Wrap a financing_pool position into `total_shares` fungible shares.
    ///
    /// The caller must hold the position in the financing_pool. This contract
    /// calls `transfer_position` to take ownership, then records the wrap and
    /// mints `total_shares` to the caller.
    ///
    /// - `investor` — current position owner (must sign)
    /// - `invoice_id` — the pool invoice ID
    /// - `total_shares` — number of fungible shares to mint (1–MAX_SHARES)
    pub fn wrap(
        env: Env,
        investor: Address,
        invoice_id: u64,
        total_shares: u64,
    ) -> Result<(), FractionalizerError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;

        if total_shares == 0 || total_shares > MAX_SHARES {
            return Err(FractionalizerError::InvalidShareCount);
        }

        let pool_addr = Self::financing_pool(&env)?;
        let pool_client = kora_financing_pool::FinancingPoolContractClient::new(&env, &pool_addr);

        // Read the position before transferring ownership
        let position: Position = pool_client.get_position(&invoice_id, &investor);
        if position.contributed <= 0 {
            return Err(FractionalizerError::PositionNotFound);
        }

        // Read pool to get token
        let pool: Pool = pool_client
            .try_get_pool(&invoice_id)
            .map_err(|_| FractionalizerError::PositionNotFound)?
            .map_err(|_| FractionalizerError::PositionNotFound)?;

        // Transfer position ownership to this contract (CEI: state before interaction)
        let wrap = WrapRecord {
            invoice_id,
            original_investor: investor.clone(),
            total_shares,
            outstanding_shares: total_shares,
            contributed: position.contributed,
            share_bps: position.share_bps,
            token: pool.token,
            total_payout: 0,
            settled: false,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Wrap(invoice_id, investor.clone()), &wrap);

        // Mint all shares to the investor
        let mut balances: Map<Address, u64> = Map::new(&env);
        balances.set(investor.clone(), total_shares);
        env.storage()
            .persistent()
            .set(&DataKey::ShareBalance(invoice_id, investor.clone(), investor.clone()), &total_shares);

        // Transfer position to this contract
        pool_client.transfer_position(&invoice_id, &investor, &env.current_contract_address());

        env.events().publish(
            (symbol_short!("FRAC_WRAP"), invoice_id),
            (investor, total_shares, position.contributed),
        );
        Ok(())
    }

    /// Transfer `amount` shares from caller to `to`.
    pub fn transfer_shares(
        env: Env,
        from: Address,
        original_investor: Address,
        invoice_id: u64,
        to: Address,
        amount: u64,
    ) -> Result<(), FractionalizerError> {
        from.require_auth();
        if amount == 0 {
            return Err(FractionalizerError::InvalidAmount);
        }

        let from_bal = Self::share_balance(&env, invoice_id, &original_investor, &from);
        if from_bal < amount {
            return Err(FractionalizerError::NotShareHolder);
        }

        env.storage().persistent().set(
            &DataKey::ShareBalance(invoice_id, original_investor.clone(), from.clone()),
            &(from_bal - amount),
        );
        let to_bal = Self::share_balance(&env, invoice_id, &original_investor, &to);
        env.storage().persistent().set(
            &DataKey::ShareBalance(invoice_id, original_investor.clone(), to.clone()),
            &(to_bal + amount),
        );
        Ok(())
    }

    /// Record the payout for a settled wrap. Must be called after the pool closes.
    ///
    /// Anyone can call this — it reads the pool's repaid_amount and records it.
    /// The pool must be closed (is_closed == true).
    pub fn settle(
        env: Env,
        invoice_id: u64,
        original_investor: Address,
    ) -> Result<(), FractionalizerError> {
        let mut wrap: WrapRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Wrap(invoice_id, original_investor.clone()))
            .ok_or(FractionalizerError::WrapNotFound)?;

        if wrap.settled {
            return Ok(());
        }

        let pool_addr = Self::financing_pool(&env)?;
        let pool_client = kora_financing_pool::FinancingPoolContractClient::new(&env, &pool_addr);
        let pool: Pool = pool_client
            .try_get_pool(&invoice_id)
            .map_err(|_| FractionalizerError::PoolNotClosed)?
            .map_err(|_| FractionalizerError::PoolNotClosed)?;

        if !pool.is_closed {
            return Err(FractionalizerError::PoolNotClosed);
        }

        // The financing_pool already distributed yield to position holders.
        // The payout this wrap is entitled to = repaid_amount * share_bps / 10_000
        // using the share snapshot taken at wrap time (the pool holds a single
        // position per holder, so re-reading after several wraps collide).
        // payout = repaid_amount * share_bps / 10_000
        let payout = pool
            .repaid_amount
            .checked_mul(wrap.share_bps as i128)
            .and_then(|v| v.checked_div(10_000))
            .ok_or(FractionalizerError::ArithmeticOverflow)?;

        wrap.total_payout = payout;
        wrap.settled = true;
        env.storage()
            .persistent()
            .set(&DataKey::Wrap(invoice_id, original_investor.clone()), &wrap);

        env.events().publish(
            (symbol_short!("FRAC_SETL"), invoice_id),
            (original_investor, payout),
        );
        Ok(())
    }

    /// Redeem shares for a pro-rata portion of the payout.
    ///
    /// The pool must be settled first (call `settle`). Each share holder
    /// may call this once. Dust from integer division is given to the first
    /// redeemer (the one with the most shares, or simply the first caller).
    pub fn redeem(
        env: Env,
        holder: Address,
        invoice_id: u64,
        original_investor: Address,
    ) -> Result<i128, FractionalizerError> {
        holder.require_auth();

        let mut wrap: WrapRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Wrap(invoice_id, original_investor.clone()))
            .ok_or(FractionalizerError::WrapNotFound)?;

        if !wrap.settled {
            return Err(FractionalizerError::PoolNotClosed);
        }

        // Double-claim prevention
        if env.storage().persistent().has(&DataKey::Claimed(
            invoice_id,
            original_investor.clone(),
            holder.clone(),
        )) {
            return Err(FractionalizerError::AlreadyClaimed);
        }

        let holder_shares = Self::share_balance(&env, invoice_id, &original_investor, &holder);
        if holder_shares == 0 {
            return Err(FractionalizerError::NotShareHolder);
        }

        // Pro-rata against the REMAINING payout and REMAINING shares: each
        // redeem shrinks both sides proportionally, so later redeemers get
        // the same per-share value as earlier ones (no dilution).
        // holder_payout = holder_shares * remaining_payout / outstanding_shares
        let base_payout = (wrap.total_payout as u128)
            .checked_mul(holder_shares as u128)
            .and_then(|v| v.checked_div(wrap.outstanding_shares as u128))
            .ok_or(FractionalizerError::ArithmeticOverflow)? as i128;

        // Dust: integer division can leave up to a stroop unassigned; the last
        // redeemer (whose shares equal all outstanding shares) takes the exact
        // remainder so nothing is stranded.
        let is_last = wrap.outstanding_shares == holder_shares;
        let payout = if is_last {
            // Last redeemer gets whatever is left (handles dust)
            wrap.total_payout
        } else {
            base_payout
        };

        // Mark claimed and burn shares
        env.storage().persistent().set(
            &DataKey::Claimed(invoice_id, original_investor.clone(), holder.clone()),
            &true,
        );
        env.storage().persistent().set(
            &DataKey::ShareBalance(invoice_id, original_investor.clone(), holder.clone()),
            &0u64,
        );
        wrap.outstanding_shares = wrap
            .outstanding_shares
            .checked_sub(holder_shares)
            .ok_or(FractionalizerError::ArithmeticOverflow)?;
        wrap.total_payout = wrap
            .total_payout
            .checked_sub(payout)
            .ok_or(FractionalizerError::ArithmeticOverflow)?;
        env.storage()
            .persistent()
            .set(&DataKey::Wrap(invoice_id, original_investor.clone()), &wrap);

        // Transfer tokens to holder
        let token_client = token::Client::new(&env, &wrap.token);
        token_client.transfer(&env.current_contract_address(), &holder, &payout);

        env.events().publish(
            (symbol_short!("FRAC_REDM"), invoice_id),
            (holder, holder_shares, payout),
        );
        Ok(payout)
    }

    /// Unwrap: return the position to the original investor.
    ///
    /// Only allowed when all shares are held by the original investor
    /// (i.e., no shares have been distributed to third parties).
    /// Blocked if any shares are outstanding with other holders.
    pub fn unwrap(
        env: Env,
        investor: Address,
        invoice_id: u64,
    ) -> Result<(), FractionalizerError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;

        let wrap: WrapRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Wrap(invoice_id, investor.clone()))
            .ok_or(FractionalizerError::WrapNotFound)?;

        // All shares must be held by the investor themselves
        let investor_shares =
            Self::share_balance(&env, invoice_id, &investor, &investor);
        if investor_shares != wrap.total_shares {
            return Err(FractionalizerError::SharesOutstanding);
        }

        // Burn all shares
        env.storage().persistent().set(
            &DataKey::ShareBalance(invoice_id, investor.clone(), investor.clone()),
            &0u64,
        );
        env.storage()
            .persistent()
            .remove(&DataKey::Wrap(invoice_id, investor.clone()));

        // Return position to investor
        let pool_addr = Self::financing_pool(&env)?;
        let pool_client = kora_financing_pool::FinancingPoolContractClient::new(&env, &pool_addr);
        pool_client.transfer_position(&invoice_id, &env.current_contract_address(), &investor);

        env.events().publish(
            (symbol_short!("FRAC_UNWP"), invoice_id),
            (investor,),
        );
        Ok(())
    }

    // ── Views ──────────────────────────────────────────────────────────────────

    pub fn get_wrap(
        env: Env,
        invoice_id: u64,
        original_investor: Address,
    ) -> Result<WrapRecord, FractionalizerError> {
        env.storage()
            .persistent()
            .get(&DataKey::Wrap(invoice_id, original_investor))
            .ok_or(FractionalizerError::WrapNotFound)
    }

    pub fn get_share_balance(
        env: Env,
        invoice_id: u64,
        original_investor: Address,
        holder: Address,
    ) -> u64 {
        Self::share_balance(&env, invoice_id, &original_investor, &holder)
    }

    // ── Helpers ────────────────────────────────────────────────────────────────

    fn share_balance(
        env: &Env,
        invoice_id: u64,
        original_investor: &Address,
        holder: &Address,
    ) -> u64 {
        env.storage()
            .persistent()
            .get(&DataKey::ShareBalance(
                invoice_id,
                original_investor.clone(),
                holder.clone(),
            ))
            .unwrap_or(0u64)
    }

    fn financing_pool(env: &Env) -> Result<Address, FractionalizerError> {
        env.storage()
            .instance()
            .get(&DataKey::FinancingPool)
            .ok_or(FractionalizerError::NotInitialized)
    }

    fn require_not_paused(env: &Env) -> Result<(), FractionalizerError> {
        let ac: Address = env
            .storage()
            .instance()
            .get(&DataKey::AccessControl)
            .ok_or(FractionalizerError::NotInitialized)?;
        let client = kora_access_control::AccessControlContractClient::new(env, &ac);
        if client.is_paused() {
            return Err(FractionalizerError::ProtocolPaused);
        }
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use kora_shared::types::Pool;
    use soroban_sdk::{testutils::Address as _, Address, Env};

    // ── helpers ────────────────────────────────────────────────────────────────

    struct Setup {
        env: Env,
        admin: Address,
        investor: Address,
        token: Address,
        pool_id: Address,
        ac_id: Address,
        frac_id: Address,
        client: FractionalizerContractClient<'static>,
        pool_client: kora_financing_pool::FinancingPoolContractClient<'static>,
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(token_admin.clone())
            .address();

        // access_control
        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        kora_access_control::AccessControlContractClient::new(&env, &ac_id).initialize(&admin);

        // financing_pool dependencies (stubs)
        let nft = Address::generate(&env);
        let rr = Address::generate(&env);
        let treasury = Address::generate(&env);
        let oracle = Address::generate(&env);
        let dr = Address::generate(&env);

        let pool_id =
            env.register_contract(None, kora_financing_pool::FinancingPoolContract);
        let pool_client =
            kora_financing_pool::FinancingPoolContractClient::new(&env, &pool_id);
        pool_client.initialize(
            &admin, &nft, &rr, &treasury, &ac_id, &200u32, &oracle, &10_000u32, &0u64, &dr,
        );

        // fractionalizer
        let frac_id =
            env.register_contract(None, FractionalizerContract);
        let client = FractionalizerContractClient::new(&env, &frac_id);
        client.initialize(&admin, &pool_id, &ac_id);

        let investor = Address::generate(&env);

        Setup {
            env,
            admin,
            investor,
            token,
            pool_id,
            ac_id,
            frac_id,
            client,
            pool_client,
        }
    }

    /// Seed an open pool and record a position for `investor`.
    fn seed_pool_and_position(s: &Setup, invoice_id: u64, contributed: i128) {
        use kora_financing_pool::DataKey;
        let pool = Pool {
            invoice_id,
            token: s.token.clone(),
            total_funded: contributed,
            face_value: contributed,
            repaid_amount: 0,
            is_closed: false,
            late_penalty_bps: 0,
            total_owed: contributed,
            penalty_applied: false,
        };
        s.env.as_contract(&s.pool_id, || {
            s.env
                .storage()
                .persistent()
                .set(&DataKey::Pool(invoice_id), &pool);
        });
        s.pool_client.record_position(
            &s.admin,
            &invoice_id,
            &s.investor,
            &contributed,
            &contributed,
        );
    }

    /// Close a pool and set repaid_amount.
    fn close_pool(s: &Setup, invoice_id: u64, repaid: i128) {
        use kora_financing_pool::DataKey;
        s.env.as_contract(&s.pool_id, || {
            let mut pool: Pool = s
                .env
                .storage()
                .persistent()
                .get(&DataKey::Pool(invoice_id))
                .unwrap();
            pool.repaid_amount = repaid;
            pool.is_closed = true;
            s.env
                .storage()
                .persistent()
                .set(&DataKey::Pool(invoice_id), &pool);
        });
    }

    // ── initialize ─────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_success() {
        let s = setup();
        // already initialized in setup — second call must fail
        let result = s.client.try_initialize(&s.admin, &s.pool_id, &s.ac_id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::AlreadyInitialized
        );
    }

    #[test]
    fn test_initialize_self_address_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let frac_id = env.register_contract(None, FractionalizerContract);
        let client = FractionalizerContractClient::new(&env, &frac_id);
        let admin = Address::generate(&env);
        let pool = Address::generate(&env);
        // passing contract itself as access_control
        let result = client.try_initialize(&admin, &pool, &frac_id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::InvalidAddress
        );
    }

    // ── wrap ───────────────────────────────────────────────────────────────────

    #[test]
    fn test_wrap_success() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);
        let wrap = s.client.get_wrap(&1u64, &s.investor);
        assert_eq!(wrap.total_shares, 100);
        assert_eq!(wrap.outstanding_shares, 100);
        assert_eq!(wrap.contributed, 10_000_000_000);
    }

    #[test]
    fn test_wrap_zero_shares_rejected() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        let result = s.client.try_wrap(&s.investor, &1u64, &0u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::InvalidShareCount
        );
    }

    #[test]
    fn test_wrap_exceeds_max_shares_rejected() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        let result = s.client.try_wrap(&s.investor, &1u64, &(MAX_SHARES + 1));
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::InvalidShareCount
        );
    }

    #[test]
    fn test_wrap_no_position_rejected() {
        let s = setup();
        // no position seeded
        let result = s.client.try_wrap(&s.investor, &99u64, &10u64);
        assert!(result.is_err());
    }

    #[test]
    fn test_wrap_mints_all_shares_to_investor() {
        let s = setup();
        seed_pool_and_position(&s, 1, 5_000_000_000);
        s.client.wrap(&s.investor, &1u64, &1000u64);
        let bal = s
            .client
            .get_share_balance(&1u64, &s.investor, &s.investor);
        assert_eq!(bal, 1000);
    }

    // ── transfer_shares ────────────────────────────────────────────────────────

    #[test]
    fn test_transfer_shares_success() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);

        let buyer = Address::generate(&s.env);
        s.client
            .transfer_shares(&s.investor, &s.investor, &1u64, &buyer, &40u64);

        assert_eq!(
            s.client.get_share_balance(&1u64, &s.investor, &s.investor),
            60
        );
        assert_eq!(
            s.client.get_share_balance(&1u64, &s.investor, &buyer),
            40
        );
    }

    #[test]
    fn test_transfer_shares_insufficient_balance_rejected() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);

        let buyer = Address::generate(&s.env);
        let result =
            s.client
                .try_transfer_shares(&s.investor, &s.investor, &1u64, &buyer, &101u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::NotShareHolder
        );
    }

    #[test]
    fn test_transfer_shares_zero_amount_rejected() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);
        let buyer = Address::generate(&s.env);
        let result =
            s.client
                .try_transfer_shares(&s.investor, &s.investor, &1u64, &buyer, &0u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::InvalidAmount
        );
    }

    // ── unwrap ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_unwrap_success_when_all_shares_held() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);
        // investor holds all shares — unwrap must succeed
        s.client.unwrap(&s.investor, &1u64);
        // wrap record removed
        assert!(s.client.try_get_wrap(&1u64, &s.investor).is_err());
    }

    #[test]
    fn test_unwrap_blocked_when_shares_distributed() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);

        let buyer = Address::generate(&s.env);
        s.client
            .transfer_shares(&s.investor, &s.investor, &1u64, &buyer, &1u64);

        let result = s.client.try_unwrap(&s.investor, &1u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::SharesOutstanding
        );
    }

    #[test]
    fn test_unwrap_not_found_rejected() {
        let s = setup();
        let result = s.client.try_unwrap(&s.investor, &99u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::WrapNotFound
        );
    }

    // ── settle ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_settle_pool_not_closed_rejected() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);
        let result = s.client.try_settle(&1u64, &s.investor);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::PoolNotClosed
        );
    }

    #[test]
    fn test_settle_wrap_not_found_rejected() {
        let s = setup();
        let result = s.client.try_settle(&99u64, &s.investor);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::WrapNotFound
        );
    }

    // ── redeem ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_redeem_not_settled_rejected() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);
        let result = s.client.try_redeem(&s.investor, &1u64, &s.investor);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::PoolNotClosed
        );
    }

    #[test]
    fn test_redeem_no_shares_rejected() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);
        close_pool(&s, 1, 10_000_000_000);

        // Manually mark settled in storage
        s.env.as_contract(&s.frac_id, || {
            let mut wrap: WrapRecord = s
                .env
                .storage()
                .persistent()
                .get(&DataKey::Wrap(1u64, s.investor.clone()))
                .unwrap();
            wrap.settled = true;
            wrap.total_payout = 10_000_000_000;
            s.env
                .storage()
                .persistent()
                .set(&DataKey::Wrap(1u64, s.investor.clone()), &wrap);
        });

        let stranger = Address::generate(&s.env);
        let result = s.client.try_redeem(&stranger, &1u64, &s.investor);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::NotShareHolder
        );
    }

    #[test]
    fn test_redeem_double_claim_rejected() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &1u64);

        // Mint tokens to fractionalizer so it can pay out
        soroban_sdk::token::StellarAssetClient::new(&s.env, &s.token)
            .mint(&s.frac_id, &10_000_000_000i128);

        s.env.as_contract(&s.frac_id, || {
            let mut wrap: WrapRecord = s
                .env
                .storage()
                .persistent()
                .get(&DataKey::Wrap(1u64, s.investor.clone()))
                .unwrap();
            wrap.settled = true;
            wrap.total_payout = 10_000_000_000;
            s.env
                .storage()
                .persistent()
                .set(&DataKey::Wrap(1u64, s.investor.clone()), &wrap);
        });

        s.client.redeem(&s.investor, &1u64, &s.investor);

        let result = s.client.try_redeem(&s.investor, &1u64, &s.investor);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::AlreadyClaimed
        );
    }

    // ── pro-rata distribution ──────────────────────────────────────────────────

    #[test]
    fn test_redeem_pro_rata_two_holders() {
        let s = setup();
        seed_pool_and_position(&s, 1, 10_000_000_000);
        s.client.wrap(&s.investor, &1u64, &100u64);

        let buyer = Address::generate(&s.env);
        s.client
            .transfer_shares(&s.investor, &s.investor, &1u64, &buyer, &40u64);

        // Mint payout to fractionalizer
        let payout: i128 = 10_000_000_000;
        soroban_sdk::token::StellarAssetClient::new(&s.env, &s.token)
            .mint(&s.frac_id, &payout);

        s.env.as_contract(&s.frac_id, || {
            let mut wrap: WrapRecord = s
                .env
                .storage()
                .persistent()
                .get(&DataKey::Wrap(1u64, s.investor.clone()))
                .unwrap();
            wrap.settled = true;
            wrap.total_payout = payout;
            s.env
                .storage()
                .persistent()
                .set(&DataKey::Wrap(1u64, s.investor.clone()), &wrap);
        });

        let token_client = soroban_sdk::token::Client::new(&s.env, &s.token);

        // buyer redeems 40/100 shares → 4_000_000_000
        let buyer_payout = s.client.redeem(&buyer, &1u64, &s.investor);
        assert_eq!(buyer_payout, 4_000_000_000);
        assert_eq!(token_client.balance(&buyer), 4_000_000_000);

        // investor redeems remaining 60/100 shares → 6_000_000_000 (last redeemer gets dust)
        let investor_payout = s.client.redeem(&s.investor, &1u64, &s.investor);
        assert_eq!(investor_payout, 6_000_000_000);
        assert_eq!(token_client.balance(&s.investor), 6_000_000_000);
    }

    #[test]
    fn test_redeem_non_divisible_dust_to_last_redeemer() {
        let s = setup();
        // 3 shares, payout = 10 → each gets 3, last gets 4 (dust=1)
        seed_pool_and_position(&s, 2, 10_000_000_000);
        s.client.wrap(&s.investor, &2u64, &3u64);

        let b1 = Address::generate(&s.env);
        let b2 = Address::generate(&s.env);
        s.client
            .transfer_shares(&s.investor, &s.investor, &2u64, &b1, &1u64);
        s.client
            .transfer_shares(&s.investor, &s.investor, &2u64, &b2, &1u64);

        let payout: i128 = 10;
        soroban_sdk::token::StellarAssetClient::new(&s.env, &s.token)
            .mint(&s.frac_id, &payout);

        s.env.as_contract(&s.frac_id, || {
            let mut wrap: WrapRecord = s
                .env
                .storage()
                .persistent()
                .get(&DataKey::Wrap(2u64, s.investor.clone()))
                .unwrap();
            wrap.settled = true;
            wrap.total_payout = payout;
            s.env
                .storage()
                .persistent()
                .set(&DataKey::Wrap(2u64, s.investor.clone()), &wrap);
        });

        let p1 = s.client.redeem(&b1, &2u64, &s.investor);
        let p2 = s.client.redeem(&b2, &2u64, &s.investor);
        // investor holds 1 share and is last redeemer
        let p3 = s.client.redeem(&s.investor, &2u64, &s.investor);

        // total must equal payout exactly
        assert_eq!(p1 + p2 + p3, payout);
        // first two get floor(10/3)=3 each
        assert_eq!(p1, 3);
        assert_eq!(p2, 3);
        // last gets remainder: 10 - 3 - 3 = 4
        assert_eq!(p3, 4);
    }

    #[test]
    fn test_redeem_single_share_gets_full_payout() {
        let s = setup();
        seed_pool_and_position(&s, 3, 5_000_000_000);
        s.client.wrap(&s.investor, &3u64, &1u64);

        let payout: i128 = 5_500_000_000;
        soroban_sdk::token::StellarAssetClient::new(&s.env, &s.token)
            .mint(&s.frac_id, &payout);

        s.env.as_contract(&s.frac_id, || {
            let mut wrap: WrapRecord = s
                .env
                .storage()
                .persistent()
                .get(&DataKey::Wrap(3u64, s.investor.clone()))
                .unwrap();
            wrap.settled = true;
            wrap.total_payout = payout;
            s.env
                .storage()
                .persistent()
                .set(&DataKey::Wrap(3u64, s.investor.clone()), &wrap);
        });

        let got = s.client.redeem(&s.investor, &3u64, &s.investor);
        assert_eq!(got, payout);
    }

    // ── wrap/unwrap lifecycle ──────────────────────────────────────────────────

    #[test]
    fn test_wrap_unwrap_lifecycle() {
        let s = setup();
        seed_pool_and_position(&s, 4, 10_000_000_000);
        s.client.wrap(&s.investor, &4u64, &50u64);

        // Confirm position is now owned by fractionalizer
        let pos = s
            .pool_client
            .get_position(&4u64, &s.frac_id);
        assert_eq!(pos.contributed, 10_000_000_000);

        // Unwrap returns position to investor
        s.client.unwrap(&s.investor, &4u64);
        let pos_after = s.pool_client.get_position(&4u64, &s.investor);
        assert_eq!(pos_after.contributed, 10_000_000_000);
    }

    // ── paused protocol ────────────────────────────────────────────────────────

    #[test]
    fn test_wrap_blocked_when_paused() {
        let s = setup();
        seed_pool_and_position(&s, 5, 10_000_000_000);

        let ac_client =
            kora_access_control::AccessControlContractClient::new(&s.env, &s.ac_id);
        ac_client.pause(&s.admin);

        let result = s.client.try_wrap(&s.investor, &5u64, &10u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::ProtocolPaused
        );
    }

    #[test]
    fn test_unwrap_blocked_when_paused() {
        let s = setup();
        seed_pool_and_position(&s, 6, 10_000_000_000);
        s.client.wrap(&s.investor, &6u64, &10u64);

        let ac_client =
            kora_access_control::AccessControlContractClient::new(&s.env, &s.ac_id);
        ac_client.pause(&s.admin);

        let result = s.client.try_unwrap(&s.investor, &6u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            FractionalizerError::ProtocolPaused
        );
    }
}
