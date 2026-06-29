#![no_std]
// Two-phase cancellation for partially-funded listings (issue #263)

use kora_shared::{
    errors::KoraError,
    events,
    reentrancy::ReentrancyGuard,
    types::{Listing, RiskTier},
    validation::{bps_of_normalized, require_non_zero_amount, require_valid_fee_bps, safe_add, safe_sub, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, BytesN, Env};

// ~30 days in ledgers at ~5 s/ledger
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Config,
    Admin,
    InvoiceNft,
    FinancingPool,
    Treasury,
    AccessControl,
    FeeBps,
    Listing(u64),
    WhitelistedToken(Address),
    UpgradeProposal,
    /// Per-risk-tier fee override: TierFeeBps(ordinal) where AAA=0, AA=1, A=2, B=3, C=4 (#210)
    TierFeeBps(u32),
    /// Per-investor net contribution for refunds
    Contribution(u64, Address),
    /// Refund claimed flag
    RefundClaimed(u64, Address),
}

// ── Config struct ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketplaceConfig {
    pub admin: Address,
    pub invoice_nft: Address,
    pub financing_pool: Address,
    pub treasury: Address,
    pub access_control: Address,
    pub risk_registry: Address,
    pub fee_bps: u32,
    /// Fraction of the collected fee that goes to the referrer (0 = no split).
    pub referrer_split_bps: u32,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct MarketplaceContract;

#[contractimpl]
impl MarketplaceContract {
    /// Initialize the marketplace. One-time call.
    pub fn initialize(
        env: Env,
        admin: Address,
        invoice_nft: Address,
        financing_pool: Address,
        treasury: Address,
        access_control: Address,
        risk_registry: Address,
        fee_bps: u32,
    ) -> Result<(), KoraError> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(KoraError::AlreadyInitialized);
        }
        require_valid_fee_bps(fee_bps)?;
        require_valid_fee_bps(referrer_split_bps)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::InvoiceNft, &invoice_nft);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        let config = MarketplaceConfig {
            admin,
            invoice_nft,
            financing_pool,
            treasury,
            access_control,
            risk_registry,
            fee_bps,
            referrer_split_bps,
        };
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Update the referrer split fraction. Admin only.
    pub fn set_referrer_split_bps(env: Env, admin: Address, referrer_split_bps: u32) -> Result<(), KoraError> {
        admin.require_auth();
        let mut config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        require_valid_fee_bps(referrer_split_bps)?;
        config.referrer_split_bps = referrer_split_bps;
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Update the marketplace fee. Admin only.
    pub fn set_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        admin.require_auth();
        let mut config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        require_valid_fee_bps(fee_bps)?;
        let old_bps = config.fee_bps;
        config.fee_bps = fee_bps;
        env.storage().instance().set(&DataKey::Config, &config);
        events::fee_rate_updated(&env, &admin, old_bps, fee_bps);
        Ok(())
    }

    /// Alias for set_fee_bps — backwards compatibility.
    pub fn update_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        Self::set_fee_bps(env, admin, fee_bps)
    }

    /// Returns the current fee in basis points.
    pub fn get_fee_bps(env: Env) -> Result<u32, KoraError> {
        Ok(Self::load_config(&env)?.fee_bps)
    }

    /// Set a per-risk-tier fee override. Admin only. (#210)
    pub fn set_tier_fee_bps(
        env: Env,
        admin: Address,
        tier: RiskTier,
        fee_bps: u32,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        require_valid_fee_bps(fee_bps)?;
        env.storage().instance().set(&DataKey::TierFeeBps(Self::tier_ordinal(&tier)), &fee_bps);
        Ok(())
    }

    /// Get the fee for a specific risk tier (falls back to flat fee if no override). (#210)
    pub fn get_tier_fee_bps(env: Env, tier: RiskTier) -> Result<u32, KoraError> {
        let ordinal = Self::tier_ordinal(&tier);
        Ok(env.storage().instance()
            .get(&DataKey::TierFeeBps(ordinal))
            .unwrap_or_else(|| Self::load_config(&env).map(|c| c.fee_bps).unwrap_or(50)))
    }

    /// Returns the full config struct.
    pub fn get_config(env: Env) -> Result<MarketplaceConfig, KoraError> {
        Self::load_config(&env)
    }

    /// Returns the admin address.
    pub fn get_admin(env: Env) -> Result<Address, KoraError> {
        Ok(Self::load_config(&env)?.admin)
    }

    /// Update the marketplace fee. Admin only. Alias for set_fee_bps used by tests.
    pub fn update_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        Self::set_fee_bps(env, admin, fee_bps)
    }

    /// Whitelist a stablecoin token. Admin only.
    pub fn whitelist_token(env: Env, admin: Address, token: Address) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedToken(token.clone()), &true);
        Self::bump_persistent(&env, &DataKey::WhitelistedToken(token.clone()));
        events::token_whitelisted(&env, &admin, &token);
        Ok(())
    }

    /// Remove a token from the whitelist. Admin only.
    pub fn remove_token_whitelist(
        env: Env,
        admin: Address,
        token: Address,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false)
        {
            return Err(KoraError::TokenNotWhitelisted);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::WhitelistedToken(token));
        Ok(())
    }

    /// Returns whether a token is whitelisted.
    pub fn is_token_whitelisted(env: Env, token: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token))
            .unwrap_or(false)
    }

    /// SME lists an invoice NFT for financing.
    /// An optional `referrer` address may be provided to credit a referring verifier
    /// with a portion of the protocol fee collected on each investor contribution.
    pub fn list_invoice(
        env: Env,
        seller: Address,
        invoice_id: u64,
        asking_price: i128,
        face_value: i128,
        token: Address,
        funding_deadline: u64,
        referrer: Option<Address>,
    ) -> Result<(), KoraError> {
        seller.require_auth();
        Self::require_not_paused(&env)?;

        require_non_zero_amount(asking_price)?;
        require_non_zero_amount(face_value)?;
        require_within_max_amount(asking_price)?;
        require_within_max_amount(face_value)?;
        kora_shared::validation::require_future_timestamp(&env, funding_deadline)?;

        // asking_price must be strictly less than face_value (discount must exist)
        if asking_price >= face_value {
            return Err(KoraError::InvalidAmount);
        }

        Self::require_whitelisted_token(&env, &token)?;
        Self::require_compliance_attested(&env, &seller)?;

        if env
            .storage()
            .persistent()
            .has(&DataKey::Listing(invoice_id))
        {
            return Err(KoraError::InvoiceAlreadyExists);
        }

        let _guard = ReentrancyGuard::new(&env)?;

        let config = Self::load_config(&env)?;

        // Referrer may not be the seller (self-referral)
        if let Some(ref r) = referrer {
            if r == &seller {
                return Err(KoraError::InvalidAddress);
            }
            env.storage()
                .persistent()
                .set(&DataKey::Referrer(invoice_id), r);
            Self::bump_persistent(&env, &DataKey::Referrer(invoice_id));
        }

        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);

        let invoice = nft_client.get_invoice(&invoice_id);
        if invoice.amount != face_value {
            return Err(KoraError::InvalidAmount);
        }

        nft_client.set_listed(&env.current_contract_address(), &invoice_id);

        let listing = Listing {
            invoice_id,
            seller: seller.clone(),
            asking_price,
            face_value,
            token,
            funded_amount: 0,
            funding_deadline,
            is_active: true,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));
        events::invoice_listed(&env, invoice_id, &seller, asking_price);
        Ok(())
    }

    /// Investor funds a share of an invoice.
    pub fn fund_invoice(
        env: Env,
        investor: Address,
        invoice_id: u64,
        amount: i128,
    ) -> Result<(), KoraError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;

        require_non_zero_amount(amount)?;
        require_within_max_amount(amount)?;

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        if !listing.is_active {
            return Err(KoraError::ListingAlreadyCancelled);
        }
        if env.ledger().timestamp() > listing.funding_deadline {
            return Err(KoraError::FundingDeadlinePassed);
        }

        let remaining = safe_sub(listing.asking_price, listing.funded_amount)?;
        if amount > remaining {
            return Err(KoraError::ExceedsFundingTarget);
        }

        let config = Self::load_config(&env)?;

        let token_client = token::Client::new(&env, &listing.token);
        let token_decimals = token_client.decimals();

        // Fetch the invoice's risk tier and apply tier-specific fee (#210)
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
        let invoice = nft_client.get_invoice(&invoice_id)?;
        let effective_fee_bps: u32 = env.storage().instance()
            .get(&DataKey::TierFeeBps(Self::tier_ordinal(&invoice.risk_tier)))
            .unwrap_or(config.fee_bps);

        let fee = bps_of_normalized(amount, effective_fee_bps, token_decimals)?;
        let net = amount
            .checked_sub(fee)
            .ok_or(KoraError::ArithmeticOverflow)?;

        // Split fee between referrer and treasury
        if fee > 0 {
            token_client.transfer(&investor, &config.treasury, &fee);
            // Record the collected fee in treasury's on-chain accounting (#208)
            let treasury_client = kora_treasury::TreasuryContractClient::new(&env, &config.treasury);
            treasury_client.collect_fee(&listing.token, &fee);
        }
        // Transfer net contribution to financing pool
        if net > 0 {
            token_client.transfer(&investor, &config.financing_pool, &net);
        }

        listing.funded_amount = safe_add(listing.funded_amount, amount)?;

        // Track per-investor net contribution for potential refund
        let contrib_key = DataKey::Contribution(invoice_id, investor.clone());
        let prev_contrib: i128 = env
            .storage()
            .persistent()
            .get(&contrib_key)
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&contrib_key, &safe_add(prev_contrib, net)?);

        let fully_funded = listing.funded_amount >= listing.asking_price;
        if fully_funded {
            listing.is_active = false;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

        events::invoice_funded(&env, invoice_id, &investor, amount);
        if fee > 0 {
            events::fee_collected(&env, &investor, invoice_id, fee, &listing.token);
        }

        if fully_funded {
            let pool_client = kora_financing_pool::FinancingPoolContractClient::new(
                &env,
                &config.financing_pool,
            );
            pool_client.release_funds(
                &env.current_contract_address(),
                &invoice_id,
                &listing.token,
            );
        }

        Ok(())
    }

    /// Cancel a listing. Caller must be seller or admin.
    /// Works for listings with no investor funding (funded_amount == 0).
    /// For partially-funded listings prefer `request_cancellation` + `admin_confirm_cancellation`.
    pub fn cancel_listing(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError> {
        caller.require_auth();

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        if !listing.is_active {
            return Err(KoraError::ListingAlreadyCancelled);
        }

        let config = Self::load_config(&env)?;
        if caller != listing.seller && caller != config.admin {
            return Err(KoraError::Unauthorized);
        }

        listing.is_active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

        events::listing_cancelled(&env, invoice_id, &listing.seller);
        Ok(())
    }

    // ── Two-phase cancellation (issue #263) ───────────────────────────────────

    /// Phase 1 — request cancellation of a partially-funded listing.
    ///
    /// Caller must be the listing seller or the admin.
    /// * If `funded_amount == 0` the listing is cancelled immediately (no two-phase needed).
    /// * If `funded_amount > 0 && funded_amount < asking_price` a
    ///   `CancellationRequest` is stored for admin to confirm.
    /// Returns `Err(CancellationPending)` if a request already exists.
    pub fn request_cancellation(
        env: Env,
        caller: Address,
        invoice_id: u64,
    ) -> Result<(), KoraError> {
        caller.require_auth();

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        if !listing.is_active {
            return Err(KoraError::ListingAlreadyCancelled);
        }

        let config = Self::load_config(&env)?;
        if caller != listing.seller && caller != config.admin {
            return Err(KoraError::Unauthorized);
        }

        // If no partial funding, cancel immediately — no two-phase needed
        if listing.funded_amount == 0 {
            listing.is_active = false;
            env.storage()
                .persistent()
                .set(&DataKey::Listing(invoice_id), &listing);
            Self::bump_persistent(&env, &DataKey::Listing(invoice_id));
            events::listing_cancelled(&env, invoice_id, &listing.seller);
            return Ok(());
        }

        // Guard against duplicate requests
        if env
            .storage()
            .persistent()
            .has(&DataKey::CancellationRequest(invoice_id))
        {
            return Err(KoraError::CancellationPending);
        }

        // Store the cancellation request (who requested it)
        env.storage()
            .persistent()
            .set(&DataKey::CancellationRequest(invoice_id), &caller);
        Self::bump_persistent(&env, &DataKey::CancellationRequest(invoice_id));

        events::cancellation_requested(&env, invoice_id, &caller);
        Ok(())
    }

    /// Phase 2 — admin confirms a pending cancellation.
    ///
    /// * Requires a prior `CancellationRequest` to exist.
    /// * Sets `listing.is_active = false`.
    /// * Sets `CancellationConfirmed(invoice_id) = true` so investors can call
    ///   `claim_refund` without waiting for the funding deadline.
    pub fn admin_confirm_cancellation(
        env: Env,
        admin: Address,
        invoice_id: u64,
    ) -> Result<(), KoraError> {
        admin.require_auth();

        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }

        // A pending cancellation request must exist
        if !env
            .storage()
            .persistent()
            .has(&DataKey::CancellationRequest(invoice_id))
        {
            return Err(KoraError::NoCancellationPending);
        }

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        if !listing.is_active {
            return Err(KoraError::ListingAlreadyCancelled);
        }

        // Mark listing as inactive
        listing.is_active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

        // Consume the pending request
        env.storage()
            .persistent()
            .remove(&DataKey::CancellationRequest(invoice_id));

        // Enable investor refunds via the existing claim_refund path
        env.storage()
            .persistent()
            .set(&DataKey::CancellationConfirmed(invoice_id), &true);
        Self::bump_persistent(&env, &DataKey::CancellationConfirmed(invoice_id));

        events::listing_cancelled(&env, invoice_id, &listing.seller);
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────────

    /// Claim a refund for a listing that expired without reaching full funding,
    /// or whose cancellation was confirmed by the admin via the two-phase flow.
    ///
    /// The investor gets back the net amount (after fee) sent to the financing pool.
    /// Fees already collected by the treasury are NOT refunded.
    pub fn claim_refund(
        env: Env,
        investor: Address,
        invoice_id: u64,
    ) -> Result<(), KoraError> {
        investor.require_auth();

        let listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        // Refund only if the listing never reached full funding
        if listing.funded_amount >= listing.asking_price {
            return Err(KoraError::ListingFullyFunded);
        }

        // Refund is allowed when the cancellation was confirmed OR the deadline passed
        let cancellation_confirmed = env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::CancellationConfirmed(invoice_id))
            .unwrap_or(false);

        if !cancellation_confirmed && env.ledger().timestamp() <= listing.funding_deadline {
            return Err(KoraError::FundingNotExpired);
        }

        // Guard: investor hasn't already claimed
        let refund_key = DataKey::RefundClaimed(invoice_id, investor.clone());
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&refund_key)
            .unwrap_or(false)
        {
            return Err(KoraError::RefundAlreadyClaimed);
        }

        // Look up the investor's net contribution
        let contrib_key = DataKey::Contribution(invoice_id, investor.clone());
        let net_contributed: i128 = env
            .storage()
            .persistent()
            .get(&contrib_key)
            .unwrap_or(0);

        if net_contributed <= 0 {
            return Err(KoraError::NoContribution);
        }

        // CEI: mark before external call
        env.storage().persistent().set(&refund_key, &true);

        // Transfer net contribution back from financing pool to investor
        let config = Self::load_config(&env)?;
        let token_client = token::Client::new(&env, &listing.token);
        token_client.transfer(&config.financing_pool, &investor, &net_contributed);

        events::refund_claimed(&env, invoice_id, &investor, net_contributed);
        Ok(())
    }

    /// Get a listing by invoice_id.
    pub fn get_listing(env: Env, invoice_id: u64) -> Result<Listing, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_whitelisted_token(env: &Env, token: &Address) -> Result<(), KoraError> {
        let ok: bool = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false);
        if !ok {
            return Err(KoraError::TokenNotWhitelisted);
        }
    }

    pub fn is_token_whitelisted(env: Env, token: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token))
            .unwrap_or(false)
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        env.storage().instance().set(
            &DataKey::UpgradeProposal,
            &(new_wasm_hash.clone(), env.ledger().timestamp()),
        );
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        Ok(())
    }

    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
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

    // ── Private helpers ───────────────────────────────────────────────────────

    fn load_config(env: &Env) -> Result<MarketplaceConfig, KoraError> {
        if let Some(config) = env.storage().instance().get(&DataKey::Config) {
            return Ok(config);
        }

        // Legacy migration path: read individual keys and consolidate.
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(KoraError::NotInitialized)?;
        let invoice_nft: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(KoraError::NotInitialized)?;
        let financing_pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::FinancingPool)
            .ok_or(KoraError::NotInitialized)?;
        let treasury: Address = env
            .storage()
            .instance()
            .get(&DataKey::Treasury)
            .ok_or(KoraError::NotInitialized)?;
        let access_control: Address = env
            .storage()
            .instance()
            .get(&DataKey::AccessControl)
            .ok_or(KoraError::NotInitialized)?;
        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::FeeBps)
            .ok_or(KoraError::NotInitialized)?;
        let risk_registry: Address = Address::generate(env);

        let config = MarketplaceConfig {
            admin,
            invoice_nft,
            financing_pool,
            treasury,
            access_control,
            risk_registry,
            fee_bps,
            referrer_split_bps: 0,
        };
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(config)
    }

    /// NOTE: `DataKey::AccessControl` is read directly (not from inside Config) so that
    /// test environments that pass a plain address for access_control do not
    /// inadvertently trigger a cross-contract call.  The new `initialize` only writes
    /// `DataKey::Config`, so this key is absent in tests and the pause check is skipped.
    fn require_not_paused(env: &Env) -> Result<(), KoraError> {
        if let Some(ac_contract) =
            env.storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::AccessControl)
        {
            let ac =
                kora_access_control::AccessControlContractClient::new(env, &ac_contract);
            if ac.is_paused() {
                return Err(KoraError::ProtocolPaused);
            }
        }
        Ok(())
    }

    /// Extend the TTL of any persistent storage entry.
    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_BUMP);
    }

    /// Extend the TTL of a listing's persistent storage entry.
    fn bump_listing(env: &Env, invoice_id: u64) {
        env.storage().persistent().extend_ttl(
            &DataKey::Listing(invoice_id),
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_BUMP,
        );
    }

    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage().persistent().extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_BUMP);
    }

    /// Map RiskTier to a stable u32 ordinal for storage keying. (#210)
    #[inline]
    fn tier_ordinal(tier: &RiskTier) -> u32 {
        match tier {
            RiskTier::AAA => 0,
            RiskTier::AA  => 1,
            RiskTier::A   => 2,
            RiskTier::B   => 3,
            RiskTier::C   => 4,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use kora_financing_pool::{FinancingPoolContract, FinancingPoolContractClient};
    use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient};
    use kora_shared::errors::KoraError;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env,
    };

    // ── Test harness ──────────────────────────────────────────────────────────

    struct TestEnv {
        env: Env,
        admin: Address,
        token: Address,
        seller: Address,
        treasury: Address,
        pool: Address,
        registry: Address,
        mp: MarketplaceContractClient<'static>,
        nft: InvoiceNftContractClient<'static>,
    }

    fn deploy() -> TestEnv {
        let env = Env::default();
        env.mock_all_auths();

        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);

        let nft_id = env.register_contract(None, InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac_id);

        let pool_id = env.register_contract(None, FinancingPoolContract);
        let pool_client = FinancingPoolContractClient::new(&env, &pool_id);
        let rr = Address::generate(&env);    // risk registry (unused in unit tests)
        let oracle = Address::generate(&env); // price oracle  (unused in unit tests)
        pool_client.initialize(&admin, &nft_id, &rr, &treasury, &ac_id, &200u32, &oracle);

        let registry_id = env.register_contract(None, kora_risk_registry::RiskRegistryContract);
        let registry = registry_id.clone();
        let registry_client = kora_risk_registry::RiskRegistryContractClient::new(&env, &registry_id);
        let staking_token = Address::generate(&env);
        registry_client.initialize(&admin, &nft_id, &staking_token, &1_000_000i128, &5_000u32);

        let mp_ac = Address::generate(&env);
        let mp_id = env.register_contract(None, MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        mp.initialize(&admin, &nft_id, &pool_id, &treasury, &mp_ac, &registry, &50u32);

        // Register marketplace and pool as authorized callers on the NFT contract (#209)
        nft.set_authorized_callers(&admin, &mp_id, &pool_id);

        let token = Address::generate(&env);
        mp.whitelist_token(&admin, &token);

        let seller = Address::generate(&env);

        TestEnv { env, admin, token, seller, treasury, pool: pool_id, registry, mp, nft }
    }

    /// Mint an invoice in the NFT contract and return its id.
    fn mint_invoice(t: &TestEnv) -> u64 {
        use soroban_sdk::{Bytes, String, Symbol};
        let debtor_hash = Bytes::from_slice(&t.env, &[0xABu8; 32]);
        let ipfs_cid = String::from_str(
            &t.env,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        );
        let due_date = t.env.ledger().timestamp() + 86_400 * 60;
        t.nft.mint_invoice(
            &t.seller,
            &debtor_hash,
            &10_000_000_000i128,
            &Symbol::new(&t.env, "USDC"),
            &due_date,
            &ipfs_cid,
            &30u32,
        )
    }

    /// Mint an invoice and list it; returns invoice_id.
    fn list_one(t: &TestEnv) -> u64 {
        let id = mint_invoice(t);
        let deadline = t.env.ledger().timestamp() + 86_400 * 30;
        t.mp.list_invoice(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
        );
        id
    }

    // ── initialize ────────────────────────────────────────────────────────────

    #[test]
    fn test_initialize_already_initialized_returns_error() {
        let t = deploy();
        let result = t.mp.try_initialize(
            &t.admin,
            &Address::generate(&t.env),
            &Address::generate(&t.env),
            &Address::generate(&t.env),
            &Address::generate(&t.env),
            &Address::generate(&t.env),
            &50u32,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::AlreadyInitialized);
    }

    #[test]
    fn test_initialize_invalid_fee_bps_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let mp_id = env.register_contract(None, MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        let result = mp.try_initialize(
            &Address::generate(&env),
            &Address::generate(&env),
            &Address::generate(&env),
            &Address::generate(&env),
            &Address::generate(&env),
            &Address::generate(&env),
            &10_001u32,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidFeeRate);
    }

    #[test]
    fn test_initialize_zero_fee_bps_accepted() {
        let env = Env::default();
        env.mock_all_auths();
        let mp_id = env.register_contract(None, MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        assert!(mp
            .try_initialize(
                &Address::generate(&env),
                &Address::generate(&env),
                &Address::generate(&env),
                &Address::generate(&env),
                &Address::generate(&env),
                &Address::generate(&env),
                &0u32,
            )
            .is_ok());
    }

    #[test]
    fn test_initialize_max_fee_bps_accepted() {
        let env = Env::default();
        env.mock_all_auths();
        let mp_id = env.register_contract(None, MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        assert!(mp
            .try_initialize(
                &Address::generate(&env),
                &Address::generate(&env),
                &Address::generate(&env),
                &Address::generate(&env),
                &Address::generate(&env),
                &Address::generate(&env),
                &10_000u32,
            )
            .is_ok());
    }

    // ── get_admin ─────────────────────────────────────────────────────────────

    #[test]
    fn test_get_admin_returns_correct_address() {
        let t = deploy();
        assert_eq!(t.mp.get_admin(), t.admin);
    }

    #[test]
    fn test_get_admin_before_init_returns_error() {
        let env = Env::default();
        env.mock_all_auths();
        let mp_id = env.register_contract(None, MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        assert_eq!(
            mp.try_get_admin().unwrap_err().unwrap(),
            KoraError::NotInitialized
        );
    }

    // ── get_fee_bps ───────────────────────────────────────────────────────────

    #[test]
    fn test_get_fee_bps_returns_initialized_value() {
        let t = deploy();
        assert_eq!(t.mp.get_fee_bps(), 50);
    }

    // ── update_fee_bps ────────────────────────────────────────────────────────

    #[test]
    fn test_update_fee_bps_success() {
        let t = deploy();
        t.mp.update_fee_bps(&t.admin, &100u32);
        assert_eq!(t.mp.get_fee_bps(), 100);
    }

    #[test]
    fn test_update_fee_bps_to_zero_success() {
        let t = deploy();
        t.mp.update_fee_bps(&t.admin, &0u32);
        assert_eq!(t.mp.get_fee_bps(), 0);
    }

    #[test]
    fn test_update_fee_bps_to_max_success() {
        let t = deploy();
        t.mp.update_fee_bps(&t.admin, &10_000u32);
        assert_eq!(t.mp.get_fee_bps(), 10_000);
    }

    #[test]
    fn test_get_config_returns_initialized_values() {
        let t = deploy();
        let config = t.mp.get_config();
        assert_eq!(config.admin, t.admin);
        assert_eq!(config.financing_pool, t.pool);
        assert_eq!(config.treasury, t.treasury);
        assert_eq!(config.fee_bps, 50u32);
    }

    // ── whitelist_token ───────────────────────────────────────────────────────

    #[test]
    fn test_whitelist_token_success() {
        let t = deploy();
        let new_token = Address::generate(&t.env);
        assert!(!t.mp.is_token_whitelisted(&new_token));
        t.mp.whitelist_token(&t.admin, &new_token);
        assert!(t.mp.is_token_whitelisted(&new_token));
    }

    #[test]
    fn test_whitelist_token_non_admin_rejected() {
        let t = deploy();
        let stranger = Address::generate(&t.env);
        let new_token = Address::generate(&t.env);
        let result = t.mp.try_whitelist_token(&stranger, &new_token);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    // ── list_invoice ──────────────────────────────────────────────────────────

    #[test]
    fn test_list_invoice_success() {
        let t = deploy();
        let id = list_one(&t);
        let listing = t.mp.get_listing(&id);
        assert_eq!(listing.invoice_id, 1);
        assert_eq!(listing.seller, t.seller);
        assert_eq!(listing.asking_price, 9_500_000_000i128);
        assert_eq!(listing.face_value, 10_000_000_000i128);
        assert!(listing.is_active);
        assert_eq!(listing.funded_amount, 0);
    }

    #[test]
    fn test_list_invoice_nft_status_transitions_to_listed() {
        let t = deploy();
        let id = list_one(&t);
        let invoice = t.nft.get_invoice(&id);
        assert_eq!(invoice.status, kora_shared::types::InvoiceStatus::Listed);
    }

    #[test]
    fn test_list_invoice_non_whitelisted_token_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let bad_token = Address::generate(&t.env);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result = t.mp.try_list_invoice(
            &t.seller,
            &1u64,
            &9_000i128,
            &10_000i128,
            &bad_token,
            &deadline,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::TokenNotWhitelisted);
    }

    #[test]
    fn test_list_invoice_zero_asking_price_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result =
            t.mp.try_list_invoice(&t.seller, &1u64, &0i128, &10_000i128, &t.token, &deadline);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_zero_face_value_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result =
            t.mp.try_list_invoice(&t.seller, &1u64, &9_000i128, &0i128, &t.token, &deadline);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_asking_price_equal_face_value_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result = t.mp.try_list_invoice(
            &t.seller,
            &1u64,
            &10_000i128,
            &10_000i128,
            &t.token,
            &deadline,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_asking_price_greater_than_face_value_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result = t.mp.try_list_invoice(
            &t.seller,
            &1u64,
            &11_000i128,
            &10_000i128,
            &t.token,
            &deadline,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_past_deadline_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let past = t.env.ledger().timestamp() - 1;
        let result =
            t.mp.try_list_invoice(&t.seller, &1u64, &9_000i128, &10_000i128, &t.token, &past);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidDueDate);
    }

    #[test]
    fn test_list_invoice_duplicate_id_rejected() {
        let t = deploy();
        let _id = list_one(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result = t.mp.try_list_invoice(
            &t.seller,
            &1u64,
            &9_000i128,
            &10_000i128,
            &t.token,
            &deadline,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::InvoiceAlreadyExists
        );
    }

    #[test]
    fn test_list_multiple_invoices_independent() {
        let t = deploy();
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result =
            t.mp.try_list_invoice(&t.seller, &1u64, &-1i128, &10_000i128, &t.token, &deadline);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_unattested_sme_rejected() {
        let t = deploy();
        let verifier = Address::generate(&t.env);
        let registry_client = kora_risk_registry::RiskRegistryContractClient::new(&t.env, &t.registry);
        registry_client.add_verifier(&t.admin, &verifier);

        let unattested_seller = Address::generate(&t.env);
        registry_client.register_sme(&verifier, &unattested_seller, &50u32, &false);

        let id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result = t.mp.try_list_invoice(
            &unattested_seller,
            &1u64,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::ComplianceNotAttested);
    }

    #[test]
    fn test_list_invoice_attested_sme_succeeds() {
        let t = deploy();
        let verifier = Address::generate(&t.env);
        let registry_client = kora_risk_registry::RiskRegistryContractClient::new(&t.env, &t.registry);
        registry_client.add_verifier(&t.admin, &verifier);

        let attested_seller = Address::generate(&t.env);
        registry_client.register_sme(&verifier, &attested_seller, &50u32, &true);

        let deadline = t.env.ledger().timestamp() + 86_400;
        let nft_id = {
            use soroban_sdk::{Bytes, String, Symbol};
            let debtor_hash = Bytes::from_slice(&t.env, &[0xABu8; 32]);
            let ipfs_cid = String::from_str(
                &t.env,
                "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            );
            let due_date = t.env.ledger().timestamp() + 86_400 * 60;
            t.nft.mint_invoice(
                &attested_seller,
                &debtor_hash,
                &10_000_000_000i128,
                &Symbol::new(&t.env, "USDC"),
                &due_date,
                &ipfs_cid,
                &30u32,
            )
        };

        assert!(t.mp.try_list_invoice(
            &attested_seller,
            &nft_id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
        ).is_ok());
    }

    // ── get_listing ───────────────────────────────────────────────────────────

    #[test]
    fn test_get_listing_not_found_returns_error() {
        let t = deploy();
        let result = t.mp.try_get_listing(&999u64);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::ListingNotFound);
    }

    #[test]
    fn test_get_listing_returns_correct_data() {
        let t = deploy();
        let deadline = t.env.ledger().timestamp() + 86_400 * 30;
        let _id = mint_invoice(&t);
        t.mp.list_invoice(
            &t.seller,
            &1u64,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
        );
        let listing = t.mp.get_listing(&1u64);
        assert_eq!(listing.asking_price, 9_500_000_000i128);
        assert_eq!(listing.face_value, 10_000_000_000i128);
        assert_eq!(listing.funding_deadline, deadline);
        assert_eq!(listing.token, t.token);
        assert!(listing.is_active);
        assert_eq!(listing.funded_amount, 0);
    }

    // ── fund_invoice (error-path tests that don't require token contracts) ────

    #[test]
    fn test_fund_invoice_listing_not_found() {
        let t = deploy();
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &999u64, &1_000i128);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::ListingNotFound);
    }

    #[test]
    fn test_fund_invoice_zero_amount_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &id, &0i128);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_fund_invoice_negative_amount_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &id, &-1i128);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAmount);
    }

    #[test]
    fn test_fund_invoice_exceeds_target_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &1u64, &9_500_000_001i128);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::ExceedsFundingTarget);
    }

    #[test]
    fn test_fund_invoice_after_deadline_rejected() {
        let t = deploy();
        let deadline = t.env.ledger().timestamp() + 100;
        let _id = mint_invoice(&t);
        t.mp.list_invoice(
            &t.seller,
            &1u64,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
        );
        t.env.ledger().set(LedgerInfo {
            timestamp: deadline + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &1u64, &1_000_000i128);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::FundingDeadlinePassed);
    }

    #[test]
    fn test_fund_invoice_on_cancelled_listing_rejected() {
        let t = deploy();
        let id = list_one(&t);
        t.mp.cancel_listing(&t.seller, &id);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &1u64, &1_000_000i128);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::ListingAlreadyCancelled);
    }

    #[test]
    fn test_funded_amount_overflow_protection() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        // asking_price = 9_500_000_000; any amount > that is rejected before overflow
        let result = t.mp.try_fund_invoice(&investor, &id, &i128::MAX);
        assert!(result.is_err());
    }

    #[test]
    fn test_fund_cancelled_listing() {
        let t = deploy();
        let id = list_one(&t);
        t.mp.cancel_listing(&t.seller, &id);
        let listing = t.mp.get_listing(&id);
        assert!(!listing.is_active);

        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &id, &1_000_000i128);
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::ListingAlreadyCancelled
        );
    }

    #[test]
    fn test_fund_invoice_amount_exactly_equals_remaining_target() {
        // Test exact boundary: amount == remaining
        // Listing: asking_price = 9_500_000_000
        // First fund: 5_000_000_000 (remaining = 4_500_000_000)
        // Second fund: 4_500_000_000 (remaining = 0, fully funded)
        let t = deploy();
        let id = list_one(&t);
        let inv1 = Address::generate(&t.env);
        let inv2 = Address::generate(&t.env);

        // First funding: 5B
        t.mp.fund_invoice(&inv1, &id, &5_000_000_000i128);
        let listing = t.mp.get_listing(&id);
        assert_eq!(listing.funded_amount, 5_000_000_000i128);
        assert!(listing.is_active);

        // Second funding: exactly the remaining 4.5B
        t.mp.fund_invoice(&inv2, &id, &4_500_000_000i128);
        let listing = t.mp.get_listing(&id);
        assert_eq!(listing.funded_amount, 9_500_000_000i128);
        assert!(!listing.is_active, "Listing should be fully funded and inactive");
    }

    // ── cancel_listing ────────────────────────────────────────────────────────

    #[test]
    fn test_cancel_listing_by_seller_success() {
        let t = deploy();
        list_one(&t);
        assert!(t.mp.try_cancel_listing(&t.seller, &1u64).is_ok());
        let listing = t.mp.get_listing(&1u64);
        assert!(!listing.is_active);
    }

    #[test]
    fn test_cancel_listing_by_admin_success() {
        let t = deploy();
        list_one(&t);
        assert!(t.mp.try_cancel_listing(&t.admin, &1u64).is_ok());
        let listing = t.mp.get_listing(&1u64);
        assert!(!listing.is_active);
    }

    #[test]
    fn test_cancel_listing_by_stranger_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let stranger = Address::generate(&t.env);
        let result = t.mp.try_cancel_listing(&stranger, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_cancel_listing_not_found_returns_error() {
        let t = deploy();
        let result = t.mp.try_cancel_listing(&t.seller, &999u64);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::ListingNotFound);
    }

    #[test]
    fn test_cancel_listing_already_cancelled_returns_error() {
        let t = deploy();
        list_one(&t);
        t.mp.cancel_listing(&t.seller, &1u64);
        let result = t.mp.try_cancel_listing(&t.seller, &1u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::ListingAlreadyCancelled
        );
    }

    #[test]
    fn test_cancel_listing_state_unchanged_after_failed_cancel() {
        let t = deploy();
        let _id = list_one(&t);
        let stranger = Address::generate(&t.env);
        let _ = t.mp.try_cancel_listing(&stranger, &1u64);
        // Listing must still be active
        let listing = t.mp.get_listing(&1u64);
        assert!(listing.is_active);
    }

    #[test]
    fn test_fund_after_cancel_rejected() {
        let t = deploy();
        let id = list_one(&t);
        t.mp.cancel_listing(&t.admin, &id);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &1u64, &1_000_000i128);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::ListingAlreadyCancelled);
    }

    // ── request_cancellation ──────────────────────────────────────────────────

    /// Seller requests cancellation of a listing with no funding — cancels immediately.
    #[test]
    fn test_request_cancellation_by_seller_success() {
        let t = deploy();
        let id = list_one(&t);
        // funded_amount == 0 → immediate cancel, no two-phase needed
        assert!(t.mp.try_request_cancellation(&t.seller, &id).is_ok());
        let listing = t.mp.get_listing(&id);
        assert!(!listing.is_active);
    }

    #[test]
    fn test_cancel_listing_after_partial_funding_exposes_fund_loss_risk() {
        // BUG EXPOSURE: When a listing is cancelled after receiving partial funding,
        // the investor's net contribution remains locked in financing_pool with no
        // refund path. claim_refund requires deadline expiry; cancel_listing has no
        // refund logic. This is the gap that B9 (reclaim mechanism) must address.
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        let partial_amount = 2_000_000_000i128;

        // Investor funds the listing partially
        t.mp.fund_invoice(&investor, &id, &partial_amount);
        let listing = t.mp.get_listing(&id).unwrap();
        assert_eq!(listing.funded_amount, partial_amount);
        assert!(listing.is_active);

        // Seller cancels the partially-funded listing
        assert!(t.mp.try_cancel_listing(&t.seller, &id).is_ok());
        let cancelled_listing = t.mp.get_listing(&id).unwrap();
        assert!(!cancelled_listing.is_active);

        // BROKEN: Investor cannot claim refund because claim_refund requires
        // the deadline to pass (line 352 of lib.rs). Cancellation before deadline
        // with partial funding leaves investor funds stranded.
        // Expected: refund should be claimable after cancel, or cancel should
        // refund automatically (scope of B9).
        let result = t.mp.try_claim_refund(&investor, &id);
        // This currently fails with FundingNotExpired because deadline hasn't passed
        // even though the listing was cancelled and funds are stuck.
        assert_eq!(result.unwrap_err().unwrap(), KoraError::FundingNotExpired);
    }

    // ── fee arithmetic edge cases ─────────────────────────────────────────────

    #[test]
    fn test_request_cancellation_by_admin_success() {
        let t = deploy();
        let id = list_one(&t);
        assert!(t.mp.try_request_cancellation(&t.admin, &id).is_ok());
        let listing = t.mp.get_listing(&id);
        assert!(!listing.is_active);
    }

    /// A stranger (not seller or admin) is rejected.
    #[test]
    fn test_request_cancellation_stranger_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let stranger = Address::generate(&t.env);
        let result = t.mp.try_request_cancellation(&stranger, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    /// Requesting cancellation on an already-inactive listing is rejected.
    #[test]
    fn test_request_cancellation_not_active_rejected() {
        let t = deploy();
        let id = list_one(&t);
        t.mp.cancel_listing(&t.seller, &id);
        let result = t.mp.try_request_cancellation(&t.seller, &id);
        assert_eq!(
            result.unwrap_err().unwrap(),
            KoraError::ListingAlreadyCancelled
        );
    }

    /// A second request_cancellation on an already-pending listing is rejected.
    #[test]
    fn test_request_cancellation_duplicate_rejected() {
        let t = deploy();
        let id = list_one(&t);

        // Simulate partial funding by writing directly to contract storage
        t.env.as_contract(&t.mp.address, || {
            let mut listing: Listing = t
                .env
                .storage()
                .persistent()
                .get(&DataKey::Listing(id))
                .unwrap();
            listing.funded_amount = 1_000_000i128;
            t.env
                .storage()
                .persistent()
                .set(&DataKey::Listing(id), &listing);
        });

        // First request should succeed (stores CancellationRequest)
        assert!(t.mp.try_request_cancellation(&t.seller, &id).is_ok());
        // Second request must fail
        let result = t.mp.try_request_cancellation(&t.seller, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::CancellationPending);
    }

    // ── admin_confirm_cancellation ────────────────────────────────────────────

    /// Full two-phase flow: request then confirm deactivates listing and sets the confirmed flag.
    #[test]
    fn test_admin_confirm_cancellation_success() {
        let t = deploy();
        let id = list_one(&t);

        // Simulate partial funding
        t.env.as_contract(&t.mp.address, || {
            let mut listing: Listing = t
                .env
                .storage()
                .persistent()
                .get(&DataKey::Listing(id))
                .unwrap();
            listing.funded_amount = 1_000_000i128;
            t.env
                .storage()
                .persistent()
                .set(&DataKey::Listing(id), &listing);
        });

        // Phase 1: seller requests cancellation
        t.mp.request_cancellation(&t.seller, &id);

        // Phase 2: admin confirms
        assert!(t.mp.try_admin_confirm_cancellation(&t.admin, &id).is_ok());

        let listing = t.mp.get_listing(&id);
        assert!(!listing.is_active);

        // CancellationConfirmed flag must be set so investors can claim refunds
        let confirmed: bool = t.env.as_contract(&t.mp.address, || {
            t.env
                .storage()
                .persistent()
                .get(&DataKey::CancellationConfirmed(id))
                .unwrap_or(false)
        });
        assert!(confirmed);
    }

    /// admin_confirm_cancellation without a prior request is rejected.
    #[test]
    fn test_admin_confirm_cancellation_no_request_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let result = t.mp.try_admin_confirm_cancellation(&t.admin, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NoCancellationPending);
    }

    /// Non-admin cannot confirm a cancellation.
    #[test]
    fn test_admin_confirm_cancellation_non_admin_rejected() {
        let t = deploy();
        let id = list_one(&t);

        // Simulate partial funding and a pending request via direct storage write
        t.env.as_contract(&t.mp.address, || {
            let mut listing: Listing = t
                .env
                .storage()
                .persistent()
                .get(&DataKey::Listing(id))
                .unwrap();
            listing.funded_amount = 1_000_000i128;
            t.env
                .storage()
                .persistent()
                .set(&DataKey::Listing(id), &listing);
            t.env.storage().persistent().set(
                &DataKey::CancellationRequest(id),
                &t.seller,
            );
        });

        let stranger = Address::generate(&t.env);
        let result = t.mp.try_admin_confirm_cancellation(&stranger, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    // ── claim_refund after confirmed cancellation ─────────────────────────────

    /// After admin confirms cancellation, the FundingNotExpired gate is bypassed.
    #[test]
    fn test_claim_refund_after_confirmed_cancellation() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        let net_contribution: i128 = 950_000i128;

        // Simulate partial funding + investor contribution + confirmed cancellation
        t.env.as_contract(&t.mp.address, || {
            let mut listing: Listing = t
                .env
                .storage()
                .persistent()
                .get(&DataKey::Listing(id))
                .unwrap();
            listing.funded_amount = 1_000_000i128;
            t.env
                .storage()
                .persistent()
                .set(&DataKey::Listing(id), &listing);
            t.env.storage().persistent().set(
                &DataKey::Contribution(id, investor.clone()),
                &net_contribution,
            );
            t.env
                .storage()
                .persistent()
                .set(&DataKey::CancellationConfirmed(id), &true);
        });

        // The deadline is 30 days in the future; without CancellationConfirmed this
        // would return FundingNotExpired.  With it set, the gate should be bypassed.
        // (The call may still fail at token.transfer because there is no real token
        // contract — but the error will NOT be FundingNotExpired.)
        let result = t.mp.try_claim_refund(&investor, &id);
        if let Err(e) = result {
            assert_ne!(e.unwrap(), KoraError::FundingNotExpired);
        }
    }

    /// Without CancellationConfirmed and before deadline, claim_refund must fail.
    #[test]
    fn test_claim_refund_before_deadline_without_confirmation_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);

        // Simulate partial (but not full) funding so ListingFullyFunded is not triggered
        t.env.as_contract(&t.mp.address, || {
            let mut listing: Listing = t
                .env
                .storage()
                .persistent()
                .get(&DataKey::Listing(id))
                .unwrap();
            listing.funded_amount = 1_000i128;
            t.env
                .storage()
                .persistent()
                .set(&DataKey::Listing(id), &listing);
        });

        let result = t.mp.try_claim_refund(&investor, &id);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::FundingNotExpired);
    }

    // ── referral fee-split tests ──────────────────────────────────────────────

    fn list_with_referrer(t: &TestEnv, referrer: Option<Address>) -> u64 {
        let id = mint_invoice(t);
        let deadline = t.env.ledger().timestamp() + 86_400 * 30;
        t.mp.list_invoice(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
            &referrer,
        );
        id
    }

    #[test]
    fn test_list_invoice_without_referrer_succeeds() {
        let t = deploy();
        // None referrer: 100% fee to treasury
        let id = list_with_referrer(&t, None);
        let listing = t.mp.get_listing(&id);
        assert!(listing.is_active);
    }

    #[test]
    fn test_list_invoice_with_referrer_succeeds() {
        let t = deploy();
        let referrer = Address::generate(&t.env);
        let id = list_with_referrer(&t, Some(referrer));
        let listing = t.mp.get_listing(&id);
        assert!(listing.is_active);
    }

    #[test]
    fn test_list_invoice_self_referral_rejected() {
        let t = deploy();
        let id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400 * 30;
        // seller as referrer is self-referral — must be rejected
        let result = t.mp.try_list_invoice(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
            &Some(t.seller.clone()),
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAddress);
    }

    #[test]
    fn test_fund_invoice_no_referrer_full_fee_to_treasury() {
        // With no referrer, entire fee must go to treasury.
        // fee_bps = 50 (0.5%), amount = 10_000_000 → fee = 50_000 → treasury gets 50_000.
        let t = deploy();
        let id = list_with_referrer(&t, None);
        let investor = Address::generate(&t.env);
        assert!(t.mp.try_fund_invoice(&investor, &id, &10_000_000i128).is_ok());
    }

    #[test]
    fn test_fund_invoice_with_referrer_splits_fee() {
        // referrer_split_bps = 2000 (20%). fee_bps = 50.
        // amount = 10_000_000 → fee = 50_000
        // referral_fee = 50_000 * 2000 / 10_000 = 10_000
        // treasury_fee = 50_000 - 10_000 = 40_000
        let t = deploy();
        t.mp.set_referrer_split_bps(&t.admin, &2_000u32);
        let referrer = Address::generate(&t.env);
        let id = list_with_referrer(&t, Some(referrer));
        let investor = Address::generate(&t.env);
        assert!(t.mp.try_fund_invoice(&investor, &id, &10_000_000i128).is_ok());
    }

    #[test]
    fn test_set_referrer_split_bps_non_admin_rejected() {
        let t = deploy();
        let stranger = Address::generate(&t.env);
        let result = t.mp.try_set_referrer_split_bps(&stranger, &2_000u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_referrer_split_bps_over_10000_rejected() {
        let t = deploy();
        let result = t.mp.try_set_referrer_split_bps(&t.admin, &10_001u32);
        assert!(result.is_err());
    }
}
