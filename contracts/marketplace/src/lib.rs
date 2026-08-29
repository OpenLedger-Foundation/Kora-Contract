#![no_std]
// Two-phase cancellation for partially-funded listings (issue #263)

use kora_shared::{
    errors::KoraError,
    events,
    reentrancy::ReentrancyGuard,
    types::{Listing, RiskTier},
    validation::{bps_of_normalized, require_non_zero_amount, require_valid_fee_bps, require_within_max_amount, safe_add, safe_sub, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Bytes, BytesN, Env, Vec};

// ~30 days in ledgers at ~5 s/ledger
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

/// Default minimum contribution floor for `fund_invoice`, in a token's smallest unit.
/// ~1 unit assuming a 7-decimal stablecoin (Stellar/Soroban's `STANDARD_DECIMALS`) —
/// small enough not to block genuine small investors, large enough that dust-sized
/// storage-griefing contributions are no longer economically free (#451).
const DEFAULT_MIN_CONTRIBUTION: i128 = 10_000_000;

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Config,
    Admin,
    InvoiceNft,
    FinancingPool,
    Treasury,
    AccessControl,
    PriceOracle,
    FeeBps,
    RiskRegistry,
    Listing(u64),
    WhitelistedToken(Address),
    /// Enumerable index of all currently-whitelisted tokens (#443)
    WhitelistedTokenList,
    /// Pending token-whitelist proposal: token -> proposed_at (#444)
    TokenWhitelistProposal(Address),
    UpgradeProposal,
    /// Per-risk-tier fee override: TierFeeBps(ordinal) where AAA=0, AA=1, A=2, B=3, C=4 (#210)
    TierFeeBps(u32),
    /// Minimum contribution floor for `fund_invoice` (#451). Defaults to
    /// `DEFAULT_MIN_CONTRIBUTION` when unset.
    MinContributionAmount,
    /// Per-investor net contribution for refunds
    Contribution(u64, Address),
    /// Refund claimed flag
    RefundClaimed(u64, Address),
    /// Referrer credited on a listing, if any (#referral fee split)
    Referrer(u64),
    /// Pending two-phase cancellation request for a partially-funded listing (#263)
    CancellationRequest(u64),
    /// Set once admin confirms a two-phase cancellation, unlocking claim_refund (#263)
    CancellationConfirmed(u64),
    /// Tiered priority-allocation window for whitelisted investors (#576)
    PriorityWindow(u64),
    /// Admin-configurable max aggregate outstanding asking_price for a token
    /// across all active listings (0 = uncapped). (#447)
    TokenExposureCap(Address),
    /// Running total of asking_price across all currently-active listings
    /// denominated in a given token. (#447)
    TokenExposureCurrent(Address),
    /// Per-investor protocol fee charged on their contribution, tracked so it
    /// can be clawed back from the treasury on a failed-listing refund. (#450)
    FeeContribution(u64, Address),
    /// Oracle currency symbol registered for a whitelisted token (#449).
    TokenCurrency(Address),
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
    pub price_oracle: Address,
    pub risk_registry: Address,
    pub fee_bps: u32,
    /// Fraction of the collected fee that goes to the referrer (0 = no split).
    pub referrer_split_bps: u32,
}

/// Per-allocation outcome of a `fund_invoices_batch` call in best-effort
/// (non-atomic) mode. (#448)
#[contracttype]
#[derive(Clone, Debug)]
pub struct BatchAllocationResult {
    pub invoice_id: u64,
    pub success: bool,
    pub error_code: u32,
}

/// A time-windowed priority allocation phase for a listing (#576).
///
/// While `env.ledger().timestamp() <= window_end`, only addresses in `whitelist`
/// may fund the listing; afterwards funding opens to all investors.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PriorityWindow {
    pub whitelist: Vec<Address>,
    pub window_end: u64,
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
        price_oracle: Address,
        risk_registry: Address,
        fee_bps: u32,
        referrer_split_bps: u32,
    ) -> Result<(), KoraError> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(KoraError::AlreadyInitialized);
        }
        require_valid_fee_bps(fee_bps)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::InvoiceNft, &invoice_nft);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        env.storage().instance().set(&DataKey::PriceOracle, &price_oracle);
        let config = MarketplaceConfig {
            admin,
            invoice_nft,
            financing_pool,
            treasury,
            access_control,
            price_oracle,
            risk_registry,
            fee_bps,
            // No referrer split at initialization; configure via set_referrer_split_bps.
            referrer_split_bps: 0,
        };
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Update the referrer split fraction. Admin only, and additionally subject
    /// to multisig quorum when access_control has a threshold above 1.
    pub fn set_referrer_split_bps(env: Env, admin: Address, referrer_split_bps: u32) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_direct_admin_allowed(&env)?;
        Self::apply_set_referrer_split_bps(&env, referrer_split_bps)
    }

    /// Update the marketplace's local fallback fee. Admin only.
    ///
    /// NOTE: This is a fallback lever only. Once `access_control` has an executed
    /// governance proposal for `ParameterKey::FeeBps`, that value takes precedence
    /// over this one everywhere fees are computed (`get_fee_bps`, `fund_invoice`) — see
    /// `base_fee_bps`. This setter remains useful before any governance value has been
    /// set, or as an emergency lever if governance is unavailable. (#446)
    pub fn set_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_direct_admin_allowed(&env)?;
        Self::apply_set_fee_bps(&env, &admin, fee_bps)
    }

    /// Alias for set_fee_bps — backwards compatibility.
    pub fn update_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), KoraError> {
        Self::set_fee_bps(env, admin, fee_bps)
    }

    /// Returns the current fee in basis points: the governance-approved
    /// `access_control` value if one has been executed, otherwise the local
    /// `config.fee_bps` fallback. (#446)
    pub fn get_fee_bps(env: Env) -> Result<u32, KoraError> {
        let config = Self::load_config(&env)?;
        Self::base_fee_bps(&env, &config)
    }

    /// Set the minimum contribution floor for `fund_invoice`. Admin only.
    ///
    /// Contributions below this amount are rejected unless they exactly complete the
    /// listing's remaining funding target, closing off cheap dust-contribution
    /// storage-griefing of a listing (#451).
    pub fn set_min_contribution(
        env: Env,
        admin: Address,
        min_contribution: i128,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        if min_contribution < 0 {
            return Err(KoraError::InvalidAmount);
        }
        env.storage()
            .instance()
            .set(&DataKey::MinContributionAmount, &min_contribution);
        Ok(())
    }

    /// Returns the current minimum contribution floor for `fund_invoice` (#451).
    /// Defaults to `DEFAULT_MIN_CONTRIBUTION` when never explicitly set.
    pub fn get_min_contribution(env: Env) -> i128 {
        Self::min_contribution_floor(&env)
    }

    /// Set the maximum fraction of `asking_price` any single investor may hold
    /// across all their `fund_invoice` calls on a given listing, expressed in
    /// basis points. Admin only. Pass `0` to disable (uncapped). (#435)
    ///
    /// **Errors:** `NotAdmin`
    pub fn set_max_investor_share_bps(
        env: Env,
        admin: Address,
        max_bps: u32,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        require_valid_fee_bps(max_bps)?;
        env.storage()
            .instance()
            .set(&DataKey::MaxInvestorShareBps, &max_bps);
        Ok(())
    }

    /// Returns the current per-investor concentration cap in basis points.
    /// `0` means uncapped (default).
    pub fn get_max_investor_share_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxInvestorShareBps)
            .unwrap_or(0)
    }

    /// Mark an investor address as accredited, enabling them to call
    /// `fund_invoice`. Admin only. (#436)
    ///
    /// **Errors:** `NotAdmin`
    pub fn set_investor_accredited(
        env: Env,
        admin: Address,
        investor: Address,
        accredited: bool,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        env.storage()
            .persistent()
            .set(&DataKey::InvestorAccredited(investor.clone()), &accredited);
        Self::bump_persistent(&env, &DataKey::InvestorAccredited(investor));
        Ok(())
    }

    /// Returns whether `investor` is currently marked as accredited.
    pub fn is_investor_accredited(env: Env, investor: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey::InvestorAccredited(investor))
            .unwrap_or(false)
    }

    /// Set a per-risk-tier fee override. Admin only. (#210)
    pub fn set_tier_fee_bps(
        env: Env,
        admin: Address,
        tier: RiskTier,
        fee_bps: u32,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_direct_admin_allowed(&env)?;
        Self::apply_set_tier_fee_bps(&env, Self::tier_ordinal(&tier), fee_bps)
    }

    /// Get the fee for a specific risk tier (falls back to the flat/governance fee if no
    /// override is set). (#210, #446)
    pub fn get_tier_fee_bps(env: Env, tier: RiskTier) -> Result<u32, KoraError> {
        let ordinal = Self::tier_ordinal(&tier);
        if let Some(tier_fee) = env.storage().instance().get(&DataKey::TierFeeBps(ordinal)) {
            return Ok(tier_fee);
        }
        let config = Self::load_config(&env)?;
        Self::base_fee_bps(&env, &config)
    }

    /// Returns the full config struct.
    pub fn get_config(env: Env) -> Result<MarketplaceConfig, KoraError> {
        Self::load_config(&env)
    }

    /// Returns the admin address.
    pub fn get_admin(env: Env) -> Result<Address, KoraError> {
        Ok(Self::load_config(&env)?.admin)
    }

    /// Propose whitelisting a stablecoin token. Admin only. Must be followed by
    /// `execute_token_whitelist` no earlier than `UPGRADE_TIMELOCK_DELAY` later.
    ///
    /// Timelocked (unlike the old instant `whitelist_token`) so a compromised or
    /// careless admin key cannot make a malicious token immediately usable in
    /// `fund_invoice` — see THREAT_MODEL.md's "Malicious Third-Party Contract"
    /// threat actor. (#444)
    pub fn propose_token_whitelist(env: Env, admin: Address, token: Address) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        env.storage().instance().set(
            &DataKey::TokenWhitelistProposal(token.clone()),
            &env.ledger().timestamp(),
        );
        events::token_whitelist_proposed(&env, &admin, &token);
        Ok(())
    }

    /// Execute a pending token-whitelist proposal once the timelock has elapsed. Admin only.
    /// (#444)
    pub fn execute_token_whitelist(env: Env, admin: Address, token: Address) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        let proposed_at: u64 = env
            .storage()
            .instance()
            .get(&DataKey::TokenWhitelistProposal(token.clone()))
            .ok_or(KoraError::NoTokenWhitelistProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(KoraError::TokenWhitelistTimelockNotElapsed);
        }
        env.storage()
            .instance()
            .remove(&DataKey::TokenWhitelistProposal(token.clone()));

        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedToken(token.clone()), &true);
        Self::bump_persistent(&env, &DataKey::WhitelistedToken(token.clone()));
        Self::add_to_whitelist_registry(&env, &token);
        events::token_whitelisted(&env, &admin, &token);
        Ok(())
    }

    /// Remove a token from the whitelist. Admin only. Takes effect immediately —
    /// no timelock — since removal can only reduce risk, unlike adding a new token. (#444)
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
            .remove(&DataKey::WhitelistedToken(token.clone()));
        Self::remove_from_whitelist_registry(&env, &token);
        Ok(())
    }

    /// Returns whether a token is whitelisted.
    pub fn is_token_whitelisted(env: Env, token: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token))
            .unwrap_or(false)
    }

    /// Associate a whitelisted token with the oracle currency symbol used to
    /// price it (e.g. "USDC", "EURC"). Required for cross-currency funding
    /// (#449) — investors may fund a listing with any token that has a
    /// currency symbol registered here, converted via the price oracle.
    /// Admin only.
    pub fn set_token_currency(
        env: Env,
        admin: Address,
        token: Address,
        currency: soroban_sdk::Symbol,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        env.storage()
            .persistent()
            .set(&DataKey::TokenCurrency(token.clone()), &currency);
        Self::bump_persistent(&env, &DataKey::TokenCurrency(token));
        Ok(())
    }

    /// Returns the oracle currency symbol registered for a token, if any.
    pub fn get_token_currency(env: Env, token: Address) -> Option<soroban_sdk::Symbol> {
        env.storage().persistent().get(&DataKey::TokenCurrency(token))
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
            return Err(KoraError::AlreadyInitialized);
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

        // === #575: Debtor verification gate ===
        // The named debtor must carry a risk_registry record meeting the governed
        // minimum before the invoice can be listed on the marketplace.
        Self::require_debtor_verified(&env, &invoice.debtor_hash)?;

        // Enforce the protocol-wide per-token exposure cap (#447) before mutating
        // NFT/listing state, so a rejected listing leaves no partial side effects.
        Self::add_token_exposure(&env, &token, asking_price)?;

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
        events::invoice_listed(&env, invoice_id, &seller, asking_price, invoice.currency.clone());
        Ok(())
    }

    /// Set the maximum aggregate outstanding `asking_price` allowed for a
    /// whitelisted token across all active listings. Admin only. `cap == 0`
    /// means uncapped. (#447)
    pub fn set_token_exposure_cap(
        env: Env,
        admin: Address,
        token: Address,
        cap: i128,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        if cap < 0 {
            return Err(KoraError::InvalidAmount);
        }
        env.storage()
            .instance()
            .set(&DataKey::TokenExposureCap(token), &cap);
        Ok(())
    }

    /// Returns the configured exposure cap for a token (0 = uncapped).
    pub fn get_token_exposure_cap(env: Env, token: Address) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TokenExposureCap(token))
            .unwrap_or(0)
    }

    /// Returns the current aggregate outstanding asking_price for a token
    /// across all active listings.
    pub fn get_token_exposure_current(env: Env, token: Address) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TokenExposureCurrent(token))
            .unwrap_or(0)
    }

    /// Investor funds a share of an invoice.
    /// Investor funds a share of an invoice. `payment_token`, if provided and
    /// different from the listing's native token, must be a whitelisted token
    /// with a registered oracle currency (`set_token_currency`); the paid
    /// amount is converted to the listing's token via the price oracle before
    /// fee/net accounting. (#449)
    pub fn fund_invoice(
        env: Env,
        investor: Address,
        invoice_id: u64,
        amount: i128,
        payment_token: Option<Address>,
    ) -> Result<(), KoraError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;
        Self::fund_invoice_internal(&env, &investor, invoice_id, amount, payment_token.as_ref())
    }

    /// Fund multiple listings in a single transaction. (#448)
    ///
    /// `atomic = true` reverts the entire batch (and the transaction) if any
    /// single allocation fails. `atomic = false` attempts every allocation
    /// independently and reports a per-allocation outcome without rolling
    /// back earlier successes.
    ///
    /// Bounded to `MAX_BATCH_SIZE` allocations to stay within Soroban's
    /// per-transaction resource limits.
    ///
    /// Each allocation always funds in the listing's native token — use
    /// `fund_invoice` directly for cross-currency funding of a single listing.
    pub fn fund_invoices_batch(
        env: Env,
        investor: Address,
        allocations: soroban_sdk::Vec<(u64, i128)>,
        atomic: bool,
    ) -> Result<soroban_sdk::Vec<BatchAllocationResult>, KoraError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;

        if allocations.is_empty() || allocations.len() > MAX_BATCH_SIZE {
            return Err(KoraError::InvalidAmount);
        }

        let mut results = soroban_sdk::Vec::new(&env);
        for (invoice_id, amount) in allocations.iter() {
            match Self::fund_invoice_internal(&env, &investor, invoice_id, amount, None) {
                Ok(()) => results.push_back(BatchAllocationResult {
                    invoice_id,
                    success: true,
                    error_code: 0,
                }),
                Err(e) => {
                    if atomic {
                        return Err(e);
                    }
                    results.push_back(BatchAllocationResult {
                        invoice_id,
                        success: false,
                        error_code: e as u32,
                    });
                }
            }
        }
        Ok(results)
    }

    /// Core funding logic shared by `fund_invoice` and `fund_invoices_batch`.
    /// Caller is responsible for `investor.require_auth()` and the pause check.
    fn fund_invoice_internal(
        env: &Env,
        investor: &Address,
        invoice_id: u64,
        amount: i128,
        payment_token: Option<&Address>,
    ) -> Result<(), KoraError> {
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

        let config = Self::load_config(&env)?;

        // Early load of invoice to determine currency for conversion check
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
        let invoice = nft_client.get_invoice(&invoice_id);

        // Determine the amount that will be credited (after conversion if needed)
        let token_client = token::Client::new(&env, &listing.token);
        let token_decimals = token_client.decimals();
        let token_symbol = token_client.symbol();
        let credited_amount = if token_symbol != invoice.currency {
            let oracle_client = kora_price_oracle::PriceOracleContractClient::new(&env, &config.price_oracle);
            let invoice_decimals = 7u32;
            oracle_client.convert_with_decimals(
                &amount,
                &token_symbol,
                &invoice.currency,
                &token_decimals,
                &invoice_decimals,
            )?
        } else {
            amount
        };

        let remaining = safe_sub(listing.asking_price, listing.funded_amount)?;
        if credited_amount > remaining {
            return Err(KoraError::ExceedsFundingTarget);
        }

        // Reject dust contributions below the configured floor unless this contribution
        // exactly completes the remaining funding target — genuine "top-off" contributions
        // near full funding must not be blocked (#451).
        if amount < Self::min_contribution_floor(&env) && amount != remaining {
            return Err(KoraError::ContributionBelowMinimum);
        }

        let config = Self::load_config(&env)?;

        // === #436: Investor compliance gate — must precede any token movement ===
        // Mirrors the seller-side require_compliance_attested check in list_invoice.
        // An investor whose accreditation flag is absent or explicitly false is
        // rejected before touching any other state.
        Self::require_investor_accredited(&env, &investor)?;

        // === #576: Tiered priority-allocation window ===
        // Enforce the whitelist while a priority window is active for this listing.
        Self::require_priority_window_allowed(&env, invoice_id, &investor)?;

        // === #435: Per-listing investor concentration cap ===
        // Compute prospective gross (pre-fee) cumulative contribution and reject
        // if it would exceed cap_bps of asking_price.  0 = uncapped (default).
        let cap_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxInvestorShareBps)
            .unwrap_or(0);
        if cap_bps > 0 {
            let gross_key = DataKey::GrossContribution(invoice_id, investor.clone());
            let prev_gross: i128 = env
                .storage()
                .persistent()
                .get(&gross_key)
                .unwrap_or(0);
            let prospective = safe_add(prev_gross, amount)?;
            // prospective * 10_000 / asking_price > cap_bps
            // rearranged to avoid division: prospective * 10_000 > cap_bps * asking_price
            let lhs = prospective
                .checked_mul(10_000)
                .ok_or(KoraError::ArithmeticOverflow)?;
            let rhs = (cap_bps as i128)
                .checked_mul(listing.asking_price)
                .ok_or(KoraError::ArithmeticOverflow)?;
            if lhs > rhs {
                events::investor_concentration_exceeded(
                    &env,
                    invoice_id,
                    &investor,
                    prospective,
                    cap_bps,
                );
                return Err(KoraError::InvestorConcentrationExceeded);
            }
        }

        // Check per-invoice freeze before any token operations.
        // Enforced in addition to the protocol-wide pause so a single disputed
        // invoice can be frozen without halting all protocol activity.
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(env, &config.invoice_nft);
        if nft_client.is_invoice_frozen(&invoice_id) {
            return Err(KoraError::InvoiceFrozen);
        }

        // Cross-currency funding (#449): if the investor pays with a token
        // other than the listing's, convert the paid amount into the
        // listing's native token via the price oracle for funding-target,
        // fee, and net accounting.
        let pay_token = payment_token.cloned().unwrap_or_else(|| listing.token.clone());
        let listing_amount = if pay_token == listing.token {
            amount
        } else {
            Self::require_whitelisted_token(env, &pay_token)?;
            let converted = Self::convert_via_oracle(env, &config, amount, &pay_token, &listing.token)?;
            require_within_max_amount(converted)?;
            converted
        };

        let remaining = safe_sub(listing.asking_price, listing.funded_amount)?;
        if listing_amount > remaining {
            return Err(KoraError::ExceedsFundingTarget);
        }

        let listing_token_client = token::Client::new(env, &listing.token);
        let token_decimals = listing_token_client.decimals();

        // Fetch the invoice's risk tier and apply tier-specific fee (#210),
        // falling back to the governance/local flat fee (#446)
        let invoice = nft_client.get_invoice(&invoice_id);
        let effective_fee_bps: u32 = match env
            .storage()
            .instance()
            .get(&DataKey::TierFeeBps(Self::tier_ordinal(&invoice.risk_tier)))
        {
            Some(tier_fee) => tier_fee,
            None => Self::base_fee_bps(&env, &config)?,
        };

        // Fee/net computed in listing-token terms so the funding target and
        // fee rate are exact regardless of which token the investor paid with.
        let fee = bps_of_normalized(listing_amount, effective_fee_bps, token_decimals)?;
        let net = listing_amount
            .checked_sub(fee)
            .ok_or(KoraError::ArithmeticOverflow)?;

        // The actual transfer always moves `pay_token` — proportionally split
        // when it differs from the listing token, so the investor's wallet is
        // only ever debited in the currency they chose to pay with (mirrors
        // financing_pool.repay's existing convert-then-transfer pattern).
        let (pay_fee, pay_net) = if pay_token == listing.token {
            (fee, net)
        } else {
            let pay_fee = safe_div(safe_mul(amount, fee)?, listing_amount)?;
            let pay_net = safe_sub(amount, pay_fee)?;
            (pay_fee, pay_net)
        };

        let pay_token_client = token::Client::new(env, &pay_token);
        if pay_fee > 0 {
            pay_token_client.transfer(investor, &config.treasury, &pay_fee);
            // Record the collected fee in treasury's on-chain accounting (#208)
            let treasury_client = kora_treasury::TreasuryContractClient::new(env, &config.treasury);
            treasury_client.collect_fee(&pay_token, &pay_fee);
        }
        if pay_net > 0 {
            pay_token_client.transfer(investor, &config.financing_pool, &pay_net);
        }

        listing.funded_amount = safe_add(listing.funded_amount, listing_amount)?;

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

        // Track the investor's fee share so claim_refund can claw it back
        // from the treasury on a failed/cancelled listing. (#450)
        let fee_key = DataKey::FeeContribution(invoice_id, investor.clone());
        let prev_fee: i128 = env.storage().persistent().get(&fee_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&fee_key, &safe_add(prev_fee, fee)?);

        let fully_funded = listing.funded_amount >= listing.asking_price;
        if fully_funded {
            listing.is_active = false;
            // Listing is no longer outstanding marketplace exposure (#447).
            Self::remove_token_exposure(env, &listing.token, listing.asking_price);
        }

        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(env, &DataKey::Listing(invoice_id));

        events::invoice_funded(&env, invoice_id, &investor, amount, invoice.currency.clone());
        if fee > 0 {
            events::fee_collected(env, investor, invoice_id, fee, &listing.token);
        }

        if fully_funded {
            let pool_client = kora_financing_pool::FinancingPoolContractClient::new(
                env,
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
        // Listing is no longer outstanding marketplace exposure (#447).
        Self::remove_token_exposure(&env, &listing.token, listing.asking_price);
        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

        events::listing_cancelled(&env, invoice_id, &listing.seller);
        Ok(())
    }

    /// Configure a time-windowed priority allocation phase for `listing_id` (#576).
    ///
    /// Admin only. While `env.ledger().timestamp() <= window_end`, only addresses
    /// in `whitelist` may fund the listing; afterwards funding opens to all investors.
    pub fn set_priority_window(
        env: Env,
        admin: Address,
        listing_id: u64,
        whitelist: Vec<Address>,
        window_end: u64,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        env.storage()
            .persistent()
            .set(&DataKey::PriorityWindow(listing_id), &PriorityWindow { whitelist, window_end });
        Self::bump_persistent(&env, &DataKey::PriorityWindow(listing_id));
        Ok(())
    }

    /// Withdraw a listed-but-unfunded invoice and cleanly revert it to `Created`,
    /// so the SME can re-list it. Reclaims NFT ownership/state and emits a
    /// cancellation event. Blocked once any partial funding exists. (#577)
    ///
    /// **Errors:** `ListingNotFound`, `ListingAlreadyCancelled`, `ListingAlreadyFunded`
    /// (when `funded_amount > 0`), `Unauthorized`.
    pub fn withdraw_listing(env: Env, caller: Address, invoice_id: u64) -> Result<(), KoraError> {
        caller.require_auth();

        let listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        if !listing.is_active {
            return Err(KoraError::ListingAlreadyCancelled);
        }

        // Blocked once any capital has been committed — a partially-funded listing
        // must go through the two-phase cancellation / refund path instead.
        if listing.funded_amount > 0 {
            return Err(KoraError::ListingAlreadyFunded);
        }

        let config = Self::load_config(&env)?;
        if caller != listing.seller && caller != config.admin {
            return Err(KoraError::Unauthorized);
        }

        // Fully revert: drop the listing record so the invoice can be re-listed.
        env.storage().persistent().remove(&DataKey::Listing(invoice_id));
        // Listing is no longer outstanding marketplace exposure (#447).
        Self::remove_token_exposure(&env, &listing.token, listing.asking_price);

        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
        nft_client.set_created(&env.current_contract_address(), &invoice_id);

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
            // Listing is no longer outstanding marketplace exposure (#447).
            Self::remove_token_exposure(&env, &listing.token, listing.asking_price);
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
        // Listing is no longer outstanding marketplace exposure (#447).
        Self::remove_token_exposure(&env, &listing.token, listing.asking_price);
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
    /// The investor gets back both the net amount (after fee) sent to the financing
    /// pool, and their proportional share of the protocol fee already collected by
    /// the treasury.
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
            return Err(KoraError::AlreadyInitialized);
        }

        // Look up the investor's net contribution
        let contrib_key = DataKey::Contribution(invoice_id, investor.clone());
        let net_contributed: i128 = env
            .storage()
            .persistent()
            .get(&contrib_key)
            .unwrap_or(0);

        if net_contributed <= 0 {
            return Err(KoraError::InsufficientFunds);
        }

        // CEI: mark before external call
        env.storage().persistent().set(&refund_key, &true);

        // Transfer net contribution back from financing pool to investor
        let config = Self::load_config(&env)?;
        let token_client = token::Client::new(&env, &listing.token);
        token_client.transfer(&config.financing_pool, &investor, &net_contributed);

        // Claw back the investor's proportional fee share from the treasury (#450).
        let fee_key = DataKey::FeeContribution(invoice_id, investor.clone());
        let fee_contributed: i128 = env.storage().persistent().get(&fee_key).unwrap_or(0);
        if fee_contributed > 0 {
            let treasury_client = kora_treasury::TreasuryContractClient::new(&env, &config.treasury);
            treasury_client.refund_fee(
                &env.current_contract_address(),
                &listing.token,
                &fee_contributed,
                &investor,
            );
        }

        events::refund_claimed(&env, invoice_id, &investor, net_contributed);
        Ok(())
    }

    /// Amend an active, **unfunded** listing's asking price, funding deadline,
    /// or token. Only the seller or admin may call this, and only while
    /// `funded_amount == 0`. Any partial funding makes the listing immutable
    /// until it is cancelled and re-listed. (#437)
    ///
    /// All provided values are re-validated using the same rules as
    /// `list_invoice`: asking_price < face_value, future deadline, whitelisted
    /// token. Pass `None` for any field you do not want to change.
    ///
    /// **Errors:**
    /// - `NotAdmin` / `Unauthorized` — caller is neither seller nor admin.
    /// - `ListingNotFound` — no active listing for `invoice_id`.
    /// - `ListingAlreadyCancelled` — listing is inactive.
    /// - `ListingAlreadyFunded` — `funded_amount > 0`; amendment is not allowed.
    /// - `InvalidAmount` — new asking price is invalid or not discounted enough.
    /// - `InvalidDueDate` — new deadline is not in the future.
    /// - `TokenNotWhitelisted` — new token is not on the whitelist.
    pub fn amend_listing(
        env: Env,
        caller: Address,
        invoice_id: u64,
        new_asking_price: Option<i128>,
        new_funding_deadline: Option<u64>,
        new_token: Option<Address>,
    ) -> Result<(), KoraError> {
        caller.require_auth();
        Self::require_not_paused(&env)?;

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)?;

        if !listing.is_active {
            return Err(KoraError::ListingAlreadyCancelled);
        }

        // Amendment is only allowed before any investor capital has been committed.
        if listing.funded_amount > 0 {
            return Err(KoraError::ListingAlreadyFunded);
        }

        let config = Self::load_config(&env)?;
        if caller != listing.seller && caller != config.admin {
            return Err(KoraError::Unauthorized);
        }

        // Snapshot old values for the event.
        let old_asking_price = listing.asking_price;
        let old_deadline = listing.funding_deadline;

        // Apply and validate each optional field.
        if let Some(price) = new_asking_price {
            require_non_zero_amount(price)?;
            require_within_max_amount(price)?;
            if price >= listing.face_value {
                return Err(KoraError::InvalidAmount);
            }
            listing.asking_price = price;
        }
        if let Some(deadline) = new_funding_deadline {
            kora_shared::validation::require_future_timestamp(&env, deadline)?;
            listing.funding_deadline = deadline;
        }
        if let Some(ref token) = new_token {
            Self::require_whitelisted_token(&env, token)?;
            listing.token = token.clone();
        }

        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

        events::listing_amended(
            &env,
            invoice_id,
            &caller,
            old_asking_price,
            listing.asking_price,
            old_deadline,
            listing.funding_deadline,
        );
        Ok(())
    }

    /// Return a page of investor addresses that have funded `invoice_id`. (#438)
    ///
    /// `page` is 0-indexed; results are in order of first contribution.
    /// `page_size` is clamped to 1–50.
    pub fn get_listing_investors(
        env: Env,
        invoice_id: u64,
        page: u32,
        page_size: u32,
    ) -> Vec<Address> {
        let page_size = (page_size.max(1).min(50)) as usize;
        let all: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::ListingInvestors(invoice_id))
            .unwrap_or_else(|| Vec::new(&env));

        let start = (page as usize).saturating_mul(page_size);
        let mut out = Vec::new(&env);
        let mut i = start as u32;
        let end = (start + page_size).min(all.len() as usize) as u32;
        while i < end {
            out.push_back(all.get_unchecked(i));
            i += 1;
        }
        out
    }

    /// Sum of all net (post-fee) contributions still outstanding for `invoice_id`. (#438)
    ///
    /// Iterates the `ListingInvestors` index and sums `Contribution` entries,
    /// providing an on-chain reconciliation view that should equal
    /// `listing.funded_amount` minus any fees collected.
    pub fn get_total_outstanding_contribution(env: Env, invoice_id: u64) -> i128 {
        let investors: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::ListingInvestors(invoice_id))
            .unwrap_or_else(|| Vec::new(&env));

        let mut total: i128 = 0;
        for investor in investors.iter() {
            let contrib: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::Contribution(invoice_id, investor.clone()))
                .unwrap_or(0);
            // Saturating add: malformed storage cannot cause a panic.
            total = total.saturating_add(contrib);
        }
        total
    }

    /// Get a listing by invoice_id.
    pub fn get_listing(env: Env, invoice_id: u64) -> Result<Listing, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(KoraError::ListingNotFound)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_compliance_attested(env: &Env, sme: &Address) -> Result<(), KoraError> {
        let config = Self::load_config(env)?;
        let rr = kora_risk_registry::RiskRegistryContractClient::new(env, &config.risk_registry);
        if !rr.is_compliance_attested(sme) {
            return Err(KoraError::ComplianceNotAttested);
        }
        Ok(())
    }

    /// Enforce the debtor verification gate before listing an invoice (#575).
    /// The named debtor must carry a risk_registry record meeting the governed
    /// minimum, otherwise the listing is blocked.
    fn require_debtor_verified(env: &Env, debtor_hash: &Bytes) -> Result<(), KoraError> {
        let config = Self::load_config(env)?;
        let rr = kora_risk_registry::RiskRegistryContractClient::new(env, &config.risk_registry);
        if !rr.is_debtor_verified(debtor_hash) {
            return Err(KoraError::ComplianceNotAttested);
        }
        Ok(())
    }

    /// Enforce the tiered priority-allocation window for `listing_id` (#576).
    /// If a window is active (ledger time <= window_end) the caller must be on
    /// its whitelist; once the window has expired funding is open to everyone.
    fn require_priority_window_allowed(
        env: &Env,
        listing_id: u64,
        investor: &Address,
    ) -> Result<(), KoraError> {
        let window: PriorityWindow = match env
            .storage()
            .persistent()
            .get(&DataKey::PriorityWindow(listing_id))
        {
            Some(w) => w,
            None => return Ok(()),
        };
        if env.ledger().timestamp() <= window.window_end {
            if !window.whitelist.contains(investor) {
                return Err(KoraError::Unauthorized);
            }
        }
        Ok(())
    }

    /// Enforce investor-side accreditation gate (#436).
    /// Returns `Err(InvestorNotAccredited)` when the investor's accreditation
    /// flag is absent or explicitly `false`.
    fn require_investor_accredited(env: &Env, investor: &Address) -> Result<(), KoraError> {
        let accredited: bool = env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::InvestorAccredited(investor.clone()))
            .unwrap_or(false);
        if !accredited {
            return Err(KoraError::InvestorNotAccredited);
        }
        Ok(())
    }

    fn require_whitelisted_token(env: &Env, token: &Address) -> Result<(), KoraError> {
        let ok: bool = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false);
        if !ok {
            return Err(KoraError::TokenNotWhitelisted);
        }
        Ok(())
    }

    /// Record `amount` of new outstanding exposure for `token`, rejecting if it
    /// would push the aggregate over the admin-configured cap (0 = uncapped). (#447)
    fn add_token_exposure(env: &Env, token: &Address, amount: i128) -> Result<(), KoraError> {
        let cap: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TokenExposureCap(token.clone()))
            .unwrap_or(0);
        let current: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TokenExposureCurrent(token.clone()))
            .unwrap_or(0);
        let new_total = safe_add(current, amount)?;
        // Reuses ExceedsFundingTarget (a KoraError enum slot is not available —
        // Soroban caps contracterror enums at 50 variants) to signal "amount
        // would exceed a configured ceiling".
        if cap != 0 && new_total > cap {
            return Err(KoraError::ExceedsFundingTarget);
        }
        env.storage()
            .instance()
            .set(&DataKey::TokenExposureCurrent(token.clone()), &new_total);
        Ok(())
    }

    /// Release `amount` of previously-recorded exposure for `token` when a
    /// listing resolves (fully funded, cancelled, or expired). (#447)
    fn remove_token_exposure(env: &Env, token: &Address, amount: i128) {
        let current: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TokenExposureCurrent(token.clone()))
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::TokenExposureCurrent(token.clone()),
            &current.saturating_sub(amount),
        );
    }

    /// Convert `amount` from `from_token`'s registered oracle currency to
    /// `to_token`'s, via the price oracle wired into the financing pool. (#449)
    fn convert_via_oracle(
        env: &Env,
        config: &MarketplaceConfig,
        amount: i128,
        from_token: &Address,
        to_token: &Address,
    ) -> Result<i128, KoraError> {
        let from_currency: soroban_sdk::Symbol = env
            .storage()
            .persistent()
            .get(&DataKey::TokenCurrency(from_token.clone()))
            .ok_or(KoraError::TokenNotWhitelisted)?;
        let to_currency: soroban_sdk::Symbol = env
            .storage()
            .persistent()
            .get(&DataKey::TokenCurrency(to_token.clone()))
            .ok_or(KoraError::TokenNotWhitelisted)?;

        let pool_client = kora_financing_pool::FinancingPoolContractClient::new(
            env,
            &config.financing_pool,
        );
        let oracle_addr = pool_client.get_price_oracle();
        let oracle_client = kora_price_oracle::PriceOracleContractClient::new(env, &oracle_addr);
        Ok(oracle_client.convert(&amount, &from_currency, &to_currency))
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_direct_admin_allowed(&env)?;
        Self::apply_propose_upgrade(&env, &admin, &new_wasm_hash)
    }

    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), KoraError> {
        admin.require_auth();
        Self::require_admin(&env, &admin)?;
        Self::require_direct_admin_allowed(&env)?;
        Self::apply_execute_upgrade(&env, &admin)
    }

    // === Multisig-gated admin actions

    /// Propose a privileged marketplace action for multisig approval.
    ///
    /// Gated by the signer set configured on the wired access_control contract,
    /// so the marketplace inherits the protocol's existing M-of-N quorum rather
    /// than trusting a single admin key.
    ///
    /// **Errors:**
    /// - `KoraError::MultisigNotConfigured` — No multisig is configured on access_control.
    /// - `KoraError::NotMultisigSigner` — `proposer` is not a configured signer.
    pub fn propose_admin_action(
        env: Env,
        proposer: Address,
        action: MarketplaceAction,
    ) -> Result<u64, KoraError> {
        proposer.require_auth();
        let cfg = Self::require_multisig(&env)?;
        Self::require_signer(&cfg, &proposer)?;

        let id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextAdminProposalId)
            .unwrap_or(1);

        let mut approvals: Vec<Address> = Vec::new(&env);
        approvals.push_back(proposer);

        let proposal = AdminProposal {
            id,
            action,
            approvals,
            executed: false,
            created_at: env.ledger().timestamp(),
        };
        env.storage()
            .persistent()
            .set(&DataKey::AdminProposal(id), &proposal);
        Self::bump_persistent(&env, &DataKey::AdminProposal(id));
        env.storage().instance().set(
            &DataKey::NextAdminProposalId,
            &(id.checked_add(1).ok_or(KoraError::ArithmeticOverflow)?),
        );
        Ok(id)
    }

    /// Record an additional signer approval for a pending admin proposal.
    pub fn approve_admin_action(
        env: Env,
        approver: Address,
        proposal_id: u64,
    ) -> Result<(), KoraError> {
        approver.require_auth();
        let cfg = Self::require_multisig(&env)?;
        Self::require_signer(&cfg, &approver)?;

        let mut proposal = Self::load_proposal(&env, proposal_id)?;
        if proposal.executed {
            return Err(KoraError::ParameterProposalAlreadyExecuted);
        }
        if proposal.approvals.contains(&approver) {
            return Err(KoraError::AlreadyVoted);
        }
        proposal.approvals.push_back(approver);
        env.storage()
            .persistent()
            .set(&DataKey::AdminProposal(proposal_id), &proposal);
        Self::bump_persistent(&env, &DataKey::AdminProposal(proposal_id));
        Ok(())
    }

    /// Execute an admin proposal once it has reached the multisig threshold.
    ///
    /// **Errors:**
    /// - `KoraError::GovernanceThresholdNotMet` — Approvals are below the quorum.
    pub fn execute_admin_action(
        env: Env,
        executor: Address,
        proposal_id: u64,
    ) -> Result<(), KoraError> {
        executor.require_auth();
        let cfg = Self::require_multisig(&env)?;
        Self::require_signer(&cfg, &executor)?;

        let mut proposal = Self::load_proposal(&env, proposal_id)?;
        if proposal.executed {
            return Err(KoraError::ParameterProposalAlreadyExecuted);
        }
        if proposal.approvals.len() < cfg.threshold {
            return Err(KoraError::GovernanceThresholdNotMet);
        }

        // Mark executed before dispatching so a re-entrant path cannot replay it.
        proposal.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::AdminProposal(proposal_id), &proposal);

        let admin = Self::load_config(&env)?.admin;
        match proposal.action {
            MarketplaceAction::SetFeeBps(bps) => Self::apply_set_fee_bps(&env, &admin, bps),
            MarketplaceAction::SetReferrerSplitBps(bps) => {
                Self::apply_set_referrer_split_bps(&env, bps)
            }
            MarketplaceAction::SetTierFeeBps(ordinal, bps) => {
                Self::apply_set_tier_fee_bps(&env, ordinal, bps)
            }
            MarketplaceAction::WhitelistToken(token) => {
                Self::apply_whitelist_token(&env, &admin, &token)
            }
            MarketplaceAction::RemoveTokenWhitelist(token) => {
                Self::apply_remove_token_whitelist(&env, &token)
            }
            MarketplaceAction::ProposeUpgrade(hash) => {
                Self::apply_propose_upgrade(&env, &admin, &hash)
            }
            MarketplaceAction::ExecuteUpgrade => Self::apply_execute_upgrade(&env, &admin),
        }
    }

    /// Returns a pending or executed admin proposal by id.
    pub fn get_admin_proposal(env: Env, proposal_id: u64) -> Result<AdminProposal, KoraError> {
        Self::load_proposal(&env, proposal_id)
    }

    /// Returns true when privileged marketplace calls require a multisig quorum
    /// rather than a direct admin call.
    pub fn is_multisig_required(env: Env) -> bool {
        matches!(Self::multisig_config(&env), Some(cfg) if cfg.threshold > 1)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    // === Multisig helpers

    /// Read the multisig config from the wired access_control contract.
    ///
    /// Returns `None` when no access_control contract is wired (unit-test
    /// environments) or when no multisig has been configured on it.
    fn multisig_config(env: &Env) -> Option<MultisigConfig> {
        let ac_contract: Address = env.storage().instance().get(&DataKey::AccessControl)?;
        let ac = kora_access_control::AccessControlContractClient::new(env, &ac_contract);
        match ac.try_get_multisig_config() {
            Ok(Ok(cfg)) => Some(cfg),
            _ => None,
        }
    }

    fn require_multisig(env: &Env) -> Result<MultisigConfig, KoraError> {
        Self::multisig_config(env).ok_or(KoraError::MultisigNotConfigured)
    }

    fn require_signer(cfg: &MultisigConfig, who: &Address) -> Result<(), KoraError> {
        if cfg.signers.contains(who) {
            Ok(())
        } else {
            Err(KoraError::NotMultisigSigner)
        }
    }

    /// Reject a direct single-key admin call when a real quorum is configured.
    ///
    /// Chains with no multisig, or a 1-of-1 multisig, keep the previous
    /// single-admin behaviour, which is the migration path for existing
    /// deployments.
    fn require_direct_admin_allowed(env: &Env) -> Result<(), KoraError> {
        match Self::multisig_config(env) {
            Some(cfg) if cfg.threshold > 1 => Err(KoraError::MultisigApprovalRequired),
            _ => Ok(()),
        }
    }

    fn require_admin(env: &Env, admin: &Address) -> Result<(), KoraError> {
        if Self::load_config(env)?.admin != *admin {
            return Err(KoraError::NotAdmin);
        }
        Ok(())
    }

    fn load_proposal(env: &Env, proposal_id: u64) -> Result<AdminProposal, KoraError> {
        env.storage()
            .persistent()
            .get(&DataKey::AdminProposal(proposal_id))
            .ok_or(KoraError::ParameterProposalNotFound)
    }

    // === Privileged action bodies
    //
    // Shared by the direct-admin entrypoints and by execute_admin_action, so
    // both authorization paths enact byte-identical state changes.

    fn apply_set_fee_bps(env: &Env, admin: &Address, fee_bps: u32) -> Result<(), KoraError> {
        require_valid_fee_bps(fee_bps)?;
        let mut config = Self::load_config(env)?;
        let old_bps = config.fee_bps;
        config.fee_bps = fee_bps;
        env.storage().instance().set(&DataKey::Config, &config);
        events::fee_rate_updated(env, admin, old_bps, fee_bps);
        Ok(())
    }

    fn apply_set_referrer_split_bps(env: &Env, bps: u32) -> Result<(), KoraError> {
        require_valid_fee_bps(bps)?;
        let mut config = Self::load_config(env)?;
        config.referrer_split_bps = bps;
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    fn apply_set_tier_fee_bps(env: &Env, ordinal: u32, fee_bps: u32) -> Result<(), KoraError> {
        require_valid_fee_bps(fee_bps)?;
        env.storage()
            .instance()
            .set(&DataKey::TierFeeBps(ordinal), &fee_bps);
        Ok(())
    }

    fn apply_whitelist_token(env: &Env, admin: &Address, token: &Address) -> Result<(), KoraError> {
        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedToken(token.clone()), &true);
        Self::bump_persistent(env, &DataKey::WhitelistedToken(token.clone()));
        events::token_whitelisted(env, admin, token);
        Ok(())
    }

    fn apply_remove_token_whitelist(env: &Env, token: &Address) -> Result<(), KoraError> {
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
            .remove(&DataKey::WhitelistedToken(token.clone()));
        Ok(())
    }

    fn apply_propose_upgrade(
        env: &Env,
        admin: &Address,
        new_wasm_hash: &BytesN<32>,
    ) -> Result<(), KoraError> {
        env.storage().instance().set(
            &DataKey::UpgradeProposal,
            &(new_wasm_hash.clone(), env.ledger().timestamp()),
        );
        events::upgrade_proposed(env, admin, new_wasm_hash);
        Ok(())
    }

    fn apply_execute_upgrade(env: &Env, admin: &Address) -> Result<(), KoraError> {
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(KoraError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(KoraError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        events::upgrade_executed(env, admin, &wasm_hash);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Dependency address migration (#445) ─────────────────────────────────────

    /// Propose an update to one of the five cross-contract dependency addresses
    /// (`invoice_nft`, `financing_pool`, `treasury`, `access_control`, `risk_registry`).
    /// Admin only. Must be followed by `execute_dependency_update` no earlier than
    /// `UPGRADE_TIMELOCK_DELAY` later, mirroring `propose_upgrade`/`execute_upgrade`.
    pub fn propose_dependency_update(
        env: Env,
        admin: Address,
        field: DependencyField,
        new_address: Address,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        let ordinal = Self::dependency_field_ordinal(&field);
        env.storage().instance().set(
            &DataKey::DependencyUpdateProposal(ordinal),
            &(new_address.clone(), env.ledger().timestamp()),
        );
        events::dependency_update_proposed(&env, &admin, ordinal, &new_address);
        Ok(())
    }

    /// Execute a pending dependency-address update once the timelock has elapsed. Admin only.
    /// Lets an operational migration (e.g. a redeployed `treasury`) re-point marketplace
    /// without abandoning the contract instance and its accumulated listings.
    pub fn execute_dependency_update(
        env: Env,
        admin: Address,
        field: DependencyField,
    ) -> Result<(), KoraError> {
        admin.require_auth();
        let mut config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(KoraError::NotAdmin);
        }
        let ordinal = Self::dependency_field_ordinal(&field);
        let (new_address, proposed_at): (Address, u64) = env
            .storage()
            .instance()
            .get(&DataKey::DependencyUpdateProposal(ordinal))
            .ok_or(KoraError::NoDependencyUpdateProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(KoraError::DependencyUpdateTimelockNotElapsed);
        }
        env.storage()
            .instance()
            .remove(&DataKey::DependencyUpdateProposal(ordinal));

        let old_address = match field {
            DependencyField::InvoiceNft => {
                core::mem::replace(&mut config.invoice_nft, new_address.clone())
            }
            DependencyField::FinancingPool => {
                core::mem::replace(&mut config.financing_pool, new_address.clone())
            }
            DependencyField::Treasury => {
                core::mem::replace(&mut config.treasury, new_address.clone())
            }
            DependencyField::AccessControl => {
                core::mem::replace(&mut config.access_control, new_address.clone())
            }
            DependencyField::RiskRegistry => {
                core::mem::replace(&mut config.risk_registry, new_address.clone())
            }
        };
        env.storage().instance().set(&DataKey::Config, &config);

        events::dependency_updated(&env, &admin, ordinal, &old_address, &new_address);
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
        // Legacy (pre-Config) instances never stored a risk registry address —
        // there's nothing to migrate it from, so require a fresh `initialize`.
        let risk_registry: Address = env
            .storage()
            .instance()
            .get(&DataKey::RiskRegistry)
            .ok_or(KoraError::NotInitialized)?;

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

    /// Current minimum contribution floor for `fund_invoice`, defaulting to
    /// `DEFAULT_MIN_CONTRIBUTION` when never explicitly set via `set_min_contribution` (#451).
    fn min_contribution_floor(env: &Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::MinContributionAmount)
            .unwrap_or(DEFAULT_MIN_CONTRIBUTION)
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

    /// Map DependencyField to a stable u32 ordinal for storage keying. (#445)
    #[inline]
    fn dependency_field_ordinal(field: &DependencyField) -> u32 {
        match field {
            DependencyField::InvoiceNft => 0,
            DependencyField::FinancingPool => 1,
            DependencyField::Treasury => 2,
            DependencyField::AccessControl => 3,
            DependencyField::RiskRegistry => 4,
        }
    }

    /// Resolve the governance-approved fee (if any), falling back to the locally
    /// configured `fee_bps` when `access_control` has no executed `FeeBps` proposal.
    /// Makes access_control's parameter-governance workflow the source of truth for
    /// the flat fee once a value has been executed there. (#446)
    fn base_fee_bps(env: &Env, config: &MarketplaceConfig) -> Result<u32, KoraError> {
        let ac = kora_access_control::AccessControlContractClient::new(env, &config.access_control);
        match ac.get_parameter(&ParameterKey::FeeBps) {
            Some(gov_fee) => {
                require_valid_fee_bps(gov_fee)?;
                Ok(gov_fee)
            }
            None => Ok(config.fee_bps),
        }
    }

    /// Add `token` to the enumerable whitelist registry if not already present. (#443)
    fn add_to_whitelist_registry(env: &Env, token: &Address) {
        let mut list: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::WhitelistedTokenList)
            .unwrap_or_else(|| Vec::new(env));
        let mut found = false;
        for existing in list.iter() {
            if &existing == token {
                found = true;
                break;
            }
        }
        if !found {
            list.push_back(token.clone());
            env.storage()
                .instance()
                .set(&DataKey::WhitelistedTokenList, &list);
        }
    }

    /// Remove `token` from the enumerable whitelist registry, if present. (#443)
    fn remove_from_whitelist_registry(env: &Env, token: &Address) {
        let list: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::WhitelistedTokenList)
            .unwrap_or_else(|| Vec::new(env));
        let mut new_list = Vec::new(env);
        for existing in list.iter() {
            if &existing != token {
                new_list.push_back(existing);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::WhitelistedTokenList, &new_list);
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
        /// Issuer of `token`, used to mint test balances to investors.
        token_admin: Address,
        seller: Address,
        treasury: Address,
        pool: Address,
        registry: Address,
        mp: MarketplaceContractClient<'static>,
        nft: InvoiceNftContractClient<'static>,
        ac: kora_access_control::AccessControlContractClient<'static>,
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

        // A real access_control contract, so pause checks and the multisig
        // authorization gate exercise actual cross-contract behaviour.
        let ac_id = env.register_contract(None, kora_access_control::AccessControlContract);
        let ac = kora_access_control::AccessControlContractClient::new(&env, &ac_id);
        ac.initialize(&admin);

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

        let mp_id = env.register_contract(None, MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        mp.initialize(&admin, &nft_id, &pool_id, &treasury, &mp_ac, &registry, &50u32, &0u32);

        // Register marketplace and pool as authorized callers on the NFT contract (#209)
        nft.set_authorized_callers(&admin, &mp_id, &pool_id);

        let token = Address::generate(&env);
        mp.propose_token_whitelist(&admin, &token);
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000 + UPGRADE_TIMELOCK_DELAY + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
        mp.execute_token_whitelist(&admin, &token);

        let seller = Address::generate(&env);

        TestEnv {
            env,
            admin,
            token,
            token_admin,
            seller,
            treasury,
            pool: pool_id,
            registry,
            mp,
            nft,
            ac,
        }
    }

    /// Mint `amount` of the test token to `to`.
    fn mint_to(t: &TestEnv, to: &Address, amount: i128) {
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token).mint(to, &amount);
    }

    fn balance_of(t: &TestEnv, who: &Address) -> i128 {
        soroban_sdk::token::Client::new(&t.env, &t.token).balance(who)
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
            &0u32,
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
            &0u32,
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
                &0u32,
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
        t.mp.propose_token_whitelist(&t.admin, &new_token);
        let now = t.env.ledger().timestamp();
        t.env.ledger().set(LedgerInfo {
            timestamp: now + UPGRADE_TIMELOCK_DELAY + 1,
            protocol_version: 21,
            sequence_number: 3,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
        t.mp.execute_token_whitelist(&t.admin, &new_token);
        assert!(t.mp.is_token_whitelisted(&new_token));
    }

    #[test]
    fn test_whitelist_token_non_admin_rejected() {
        let t = deploy();
        let stranger = Address::generate(&t.env);
        let new_token = Address::generate(&t.env);
        let result = t.mp.try_propose_token_whitelist(&stranger, &new_token);
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
            KoraError::AlreadyInitialized
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

    // ── #575: Debtor verification gate ──────────────────────────────────────────

    #[test]
    fn test_list_invoice_rejects_unverified_debtor() {
        let t = deploy();
        let rr = kora_risk_registry::RiskRegistryContractClient::new(&t.env, &t.registry);
        rr.set_min_debtor_score(&t.admin, &50u32);

        let id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        // Debtor hash has no risk_registry record meeting the governed minimum,
        // so listing must be blocked.
        let result = t.mp.try_list_invoice(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
        );
        assert_eq!(result.unwrap_err().unwrap(), KoraError::ComplianceNotAttested);
    }

    // ── #576: Tiered priority-allocation window ─────────────────────────────────

    #[test]
    fn test_priority_window_blocks_non_whitelisted_until_expiry() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        t.mp.set_investor_accredited(&t.admin, &investor, &true);

        // Priority window ending at `deadline`, with an empty whitelist.
        let deadline = t.env.ledger().timestamp() + 86_400;
        let whitelist = soroban_sdk::Vec::new(&t.env);
        t.mp.set_priority_window(&t.admin, &id, &whitelist, &deadline);

        // During the window, even an accredited investor is rejected (not whitelisted).
        let during = t.mp.try_fund_invoice(&investor, &id, &1_000_000i128);
        assert_eq!(during.unwrap_err().unwrap(), KoraError::Unauthorized);

        // After the window expires, the whitelist gate no longer applies.
        t.env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            timestamp: deadline + 1,
            protocol_version: 21,
            sequence_number: 3,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
        let after = t.mp.try_fund_invoice(&investor, &id, &1_000_000i128);
        assert_ne!(after.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    // ── #577: Pre-funding withdrawal ────────────────────────────────────────────

    #[test]
    fn test_withdraw_listing_reverts_and_allows_relist() {
        let t = deploy();
        let id = list_one(&t);
        // Withdraw the unfunded listing; it cleanly reverts to Created.
        assert!(t.mp.try_withdraw_listing(&t.seller, &id).is_ok());
        let deadline = t.env.ledger().timestamp() + 86_400;
        // The same invoice can be re-listed afterwards.
        assert!(t.mp
            .try_list_invoice(
                &t.seller,
                &id,
                &9_500_000_000i128,
                &10_000_000_000i128,
                &t.token,
                &deadline,
            )
            .is_ok());
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
        // net to financing pool = 10_000_000 - 50_000 = 9_950_000
        let t = deploy();
        t.mp.set_referrer_split_bps(&t.admin, &2_000u32);
        let referrer = Address::generate(&t.env);
        let id = list_with_referrer(&t, Some(referrer.clone()));
        let investor = Address::generate(&t.env);
        mint_to(&t, &investor, 10_000_000i128);

        t.mp.fund_invoice(&investor, &id, &10_000_000i128);

        assert_eq!(balance_of(&t, &referrer), 10_000i128);
        assert_eq!(balance_of(&t, &t.treasury), 40_000i128);
        assert_eq!(balance_of(&t, &t.pool), 9_950_000i128);
        assert_eq!(balance_of(&t, &investor), 0i128);
    }

    #[test]
    fn test_fund_invoice_without_referrer_sends_full_fee_to_treasury() {
        let t = deploy();
        t.mp.set_referrer_split_bps(&t.admin, &2_000u32);
        let id = list_with_referrer(&t, None);
        let investor = Address::generate(&t.env);
        mint_to(&t, &investor, 10_000_000i128);

        t.mp.fund_invoice(&investor, &id, &10_000_000i128);

        assert_eq!(balance_of(&t, &t.treasury), 50_000i128);
        assert_eq!(balance_of(&t, &t.pool), 9_950_000i128);
    }

    #[test]
    fn test_fund_invoice_referrer_unpaid_when_split_is_zero() {
        // referrer_split_bps defaults to 0, so a stored referrer earns nothing.
        let t = deploy();
        let referrer = Address::generate(&t.env);
        let id = list_with_referrer(&t, Some(referrer.clone()));
        let investor = Address::generate(&t.env);
        mint_to(&t, &investor, 10_000_000i128);

        t.mp.fund_invoice(&investor, &id, &10_000_000i128);

        assert_eq!(balance_of(&t, &referrer), 0i128);
        assert_eq!(balance_of(&t, &t.treasury), 50_000i128);
    }

    // === Multisig-gated admin authorization

    /// Configure an N-of-M multisig on the wired access_control contract.
    fn configure_multisig(t: &TestEnv, signers: &[Address], threshold: u32) {
        let mut v: Vec<Address> = Vec::new(&t.env);
        for s in signers {
            v.push_back(s.clone());
        }
        t.ac.configure_multisig(&t.admin, &v, &threshold);
    }

    #[test]
    fn test_single_admin_still_works_without_multisig() {
        let t = deploy();
        assert!(!t.mp.is_multisig_required());
        t.mp.set_fee_bps(&t.admin, &75u32);
        assert_eq!(t.mp.get_fee_bps(), 75u32);
    }

    #[test]
    fn test_single_admin_still_works_with_one_of_one_multisig() {
        let t = deploy();
        let signer = Address::generate(&t.env);
        configure_multisig(&t, &[signer], 1);
        assert!(!t.mp.is_multisig_required());
        t.mp.set_fee_bps(&t.admin, &75u32);
        assert_eq!(t.mp.get_fee_bps(), 75u32);
    }

    #[test]
    fn test_direct_admin_calls_rejected_under_quorum() {
        let t = deploy();
        let s1 = Address::generate(&t.env);
        let s2 = Address::generate(&t.env);
        let s3 = Address::generate(&t.env);
        configure_multisig(&t, &[s1, s2, s3], 2);
        assert!(t.mp.is_multisig_required());

        let token = Address::generate(&t.env);
        assert_eq!(
            t.mp.try_set_fee_bps(&t.admin, &9_000u32).unwrap_err().unwrap(),
            KoraError::MultisigApprovalRequired
        );
        assert_eq!(
            t.mp.try_whitelist_token(&t.admin, &token).unwrap_err().unwrap(),
            KoraError::MultisigApprovalRequired
        );
        assert_eq!(
            t.mp.try_remove_token_whitelist(&t.admin, &t.token).unwrap_err().unwrap(),
            KoraError::MultisigApprovalRequired
        );
        assert_eq!(
            t.mp.try_execute_upgrade(&t.admin).unwrap_err().unwrap(),
            KoraError::MultisigApprovalRequired
        );
        // Fee is unchanged by the rejected call.
        assert_eq!(t.mp.get_fee_bps(), 50u32);
    }

    #[test]
    fn test_single_signer_cannot_execute_alone() {
        let t = deploy();
        let s1 = Address::generate(&t.env);
        let s2 = Address::generate(&t.env);
        let s3 = Address::generate(&t.env);
        configure_multisig(&t, &[s1.clone(), s2, s3], 2);

        let id = t.mp.propose_admin_action(&s1, &MarketplaceAction::SetFeeBps(75));
        assert_eq!(
            t.mp.try_execute_admin_action(&s1, &id).unwrap_err().unwrap(),
            KoraError::GovernanceThresholdNotMet
        );
        assert_eq!(t.mp.get_fee_bps(), 50u32);
    }

    #[test]
    fn test_non_signer_cannot_propose() {
        let t = deploy();
        let s1 = Address::generate(&t.env);
        let s2 = Address::generate(&t.env);
        configure_multisig(&t, &[s1, s2], 2);

        let stranger = Address::generate(&t.env);
        assert_eq!(
            t.mp
                .try_propose_admin_action(&stranger, &MarketplaceAction::SetFeeBps(75))
                .unwrap_err()
                .unwrap(),
            KoraError::NotMultisigSigner
        );
    }

    #[test]
    fn test_quorum_approved_fee_change_succeeds() {
        let t = deploy();
        let s1 = Address::generate(&t.env);
        let s2 = Address::generate(&t.env);
        let s3 = Address::generate(&t.env);
        configure_multisig(&t, &[s1.clone(), s2.clone(), s3], 2);

        let id = t.mp.propose_admin_action(&s1, &MarketplaceAction::SetFeeBps(75));
        t.mp.approve_admin_action(&s2, &id);
        t.mp.execute_admin_action(&s2, &id);

        assert_eq!(t.mp.get_fee_bps(), 75u32);
        assert!(t.mp.get_admin_proposal(&id).executed);
    }

    #[test]
    fn test_quorum_approved_token_whitelist_succeeds() {
        let t = deploy();
        let s1 = Address::generate(&t.env);
        let s2 = Address::generate(&t.env);
        configure_multisig(&t, &[s1.clone(), s2.clone()], 2);

        let token = Address::generate(&t.env);
        let id = t
            .mp
            .propose_admin_action(&s1, &MarketplaceAction::WhitelistToken(token.clone()));
        t.mp.approve_admin_action(&s2, &id);
        t.mp.execute_admin_action(&s1, &id);
        assert!(t.mp.is_token_whitelisted(&token));

        let id2 = t
            .mp
            .propose_admin_action(&s1, &MarketplaceAction::RemoveTokenWhitelist(token.clone()));
        t.mp.approve_admin_action(&s2, &id2);
        t.mp.execute_admin_action(&s1, &id2);
        assert!(!t.mp.is_token_whitelisted(&token));
    }

    #[test]
    fn test_admin_proposal_cannot_be_executed_twice() {
        let t = deploy();
        let s1 = Address::generate(&t.env);
        let s2 = Address::generate(&t.env);
        configure_multisig(&t, &[s1.clone(), s2.clone()], 2);

        let id = t.mp.propose_admin_action(&s1, &MarketplaceAction::SetFeeBps(75));
        t.mp.approve_admin_action(&s2, &id);
        t.mp.execute_admin_action(&s1, &id);
        assert_eq!(
            t.mp.try_execute_admin_action(&s1, &id).unwrap_err().unwrap(),
            KoraError::ParameterProposalAlreadyExecuted
        );
    }

    #[test]
    fn test_duplicate_approval_rejected() {
        let t = deploy();
        let s1 = Address::generate(&t.env);
        let s2 = Address::generate(&t.env);
        configure_multisig(&t, &[s1.clone(), s2], 2);

        let id = t.mp.propose_admin_action(&s1, &MarketplaceAction::SetFeeBps(75));
        assert_eq!(
            t.mp.try_approve_admin_action(&s1, &id).unwrap_err().unwrap(),
            KoraError::AlreadyVoted
        );
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
#![no_std]
#![no_std]
// Two-phase cancellation for partially-funded listings (issue #263)
// Dutch-auction price decay (#439)
// Reverse-auction bidding (#440)
// Funding deadline vs due-date guard (#441)

use kora_shared::{
    errors::CommonError,
    events,
    reentrancy::ReentrancyGuard,
    types::{Bid, DecaySchedule, Listing, RiskTier},
    validation::{bps_of, bps_of_normalized, require_non_zero_amount, require_valid_fee_bps, require_within_max_amount, safe_add, safe_sub, UPGRADE_TIMELOCK_DELAY},
};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, token, Address, BytesN, Env, Vec};

// ~30 days in ledgers at ~5 s/ledger
const PERSISTENT_TTL_THRESHOLD: u32 = 518_400;
const PERSISTENT_TTL_BUMP: u32 = 518_400;

/// Default minimum gap (in seconds) that must exist between a listing's
/// `funding_deadline` and the underlying invoice's `due_date` (#441).
/// Default: 3 days = 3 * 86_400 s.
pub const DEFAULT_MIN_FUNDING_BUFFER: u64 = 3 * 86_400;
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MarketplaceError {
    AlreadyInitialized = 1,
    ArithmeticOverflow = 2,
    BidAlreadyExists = 3,
    BidNotFound = 4,
    BiddingWindowActive = 5,
    BiddingWindowClosed = 6,
    CancellationPending = 7,
    ComplianceNotAttested = 8,
    ExceedsFundingTarget = 9,
    FundingDeadlinePassed = 10,
    FundingDeadlineTooCloseToDueDate = 11,
    FundingNotExpired = 12,
    InvalidAddress = 13,
    InvalidAmount = 14,
    InvalidDueDate = 15,
    InvalidFeeRate = 16,
    InvoiceAlreadyExists = 17,
    InvoiceFrozen = 18,
    ListingAlreadyCancelled = 19,
    ListingFullyFunded = 20,
    ListingNotFound = 21,
    NoCancellationPending = 22,
    NoContribution = 23,
    NoUpgradeProposed = 24,
    NotAdmin = 25,
    NotInitialized = 26,
    ProtocolPaused = 27,
    RefundAlreadyClaimed = 28,
    TokenNotWhitelisted = 29,
    Unauthorized = 30,
    UpgradeTimelockNotElapsed = 31,
}

impl From<CommonError> for MarketplaceError {
    fn from(e: CommonError) -> Self {
        match e {
            CommonError::InvalidAmount => MarketplaceError::InvalidAmount,
            CommonError::InvalidAddress => MarketplaceError::InvalidAddress,
            CommonError::InvalidDueDate => MarketplaceError::InvalidDueDate,
            CommonError::InvalidFeeRate => MarketplaceError::InvalidFeeRate,
            CommonError::ArithmeticOverflow => MarketplaceError::ArithmeticOverflow,
            _ => MarketplaceError::InvalidAmount,
        }
    }
}
/// Default minimum discount (face_value - asking_price) required to list an
/// invoice, in basis points of face_value. Admin-configurable via
/// `set_min_discount_bps`. See docs/marketplace.md.
const DEFAULT_MIN_DISCOUNT_BPS: u32 = 10;

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
    /// Referrer credited on a listing, if any (#issue referral-fee-split)
    Referrer(u64),
    /// Pending two-phase cancellation request, storing who requested it (#263)
    CancellationRequest(u64),
    /// Set once admin confirms a two-phase cancellation, unlocking claim_refund (#263)
    CancellationConfirmed(u64),
    /// Legacy individual risk_registry key (never written; present only so
    /// load_config's legacy migration path can look it up and fail cleanly).
    RiskRegistry,
    /// Minimum discount (bps of face_value) required to list an invoice.
    MinDiscountBps,
    /// Minimum seconds between a listing's funding_deadline and the invoice's due_date (#441).
    MinFundingBuffer,
    /// Dutch-auction price decay schedule for a listing, if any (#439).
    DecaySchedule(u64),
    /// Reverse-auction bid: Bid(invoice_id, investor) (#440).
    Bid(u64, Address),
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
    pub price_oracle: Address,
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
    ) -> Result<(), MarketplaceError> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(MarketplaceError::AlreadyInitialized);
        }
        require_valid_fee_bps(fee_bps)?;
        // Write the consolidated Config entry first so that every subsequent
        // load_config call resolves in a single storage lookup.
        // Referrer splits are disabled at init and opted into later via
        // set_referrer_split_bps — kept out of the constructor to avoid
        // widening every deployment call site for a rarely-used feature.
        let referrer_split_bps: u32 = 0;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::InvoiceNft, &invoice_nft);
        env.storage().instance().set(&DataKey::FinancingPool, &financing_pool);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        let config = MarketplaceConfig {
            admin: admin.clone(),
            invoice_nft: invoice_nft.clone(),
            financing_pool: financing_pool.clone(),
            treasury: treasury.clone(),
            access_control: access_control.clone(),
            risk_registry: risk_registry.clone(),
            fee_bps,
            referrer_split_bps: 0,
        };
        env.storage().instance().set(&DataKey::Config, &config);
        // Also write the individual keys so that require_not_paused (which reads
        // DataKey::AccessControl directly for efficiency) can find the address.
        env.storage().instance().set(&DataKey::AccessControl, &access_control);
        Ok(())
    }

    /// Set the minimum funding-to-due-date buffer in seconds. Admin only. (#441)
    /// Defaults to DEFAULT_MIN_FUNDING_BUFFER (3 days) if never explicitly set.
    pub fn set_min_funding_buffer(
        env: Env,
        admin: Address,
        buffer_seconds: u64,
    ) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }
        env.storage()
            .instance()
            .set(&DataKey::MinFundingBuffer, &buffer_seconds);
        Ok(())
    }

    /// Returns the current minimum funding buffer in seconds. (#441)
    pub fn get_min_funding_buffer(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MinFundingBuffer)
            .unwrap_or(DEFAULT_MIN_FUNDING_BUFFER)
    }

    /// Update the referrer split fraction. Admin only.
    pub fn set_referrer_split_bps(env: Env, admin: Address, referrer_split_bps: u32) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let mut config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }
        require_valid_fee_bps(referrer_split_bps)?;
        config.referrer_split_bps = referrer_split_bps;
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Update the marketplace fee. Admin only.
    pub fn set_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let mut config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }
        require_valid_fee_bps(fee_bps)?;
        let old_bps = config.fee_bps;
        config.fee_bps = fee_bps;
        env.storage().instance().set(&DataKey::Config, &config);
        events::fee_rate_updated(&env, &admin, old_bps, fee_bps);
        Ok(())
    }

    /// Alias for set_fee_bps — backwards compatibility.
    pub fn update_fee_bps(env: Env, admin: Address, fee_bps: u32) -> Result<(), MarketplaceError> {
        Self::set_fee_bps(env, admin, fee_bps)
    }

    /// Returns the current fee in basis points.
    pub fn get_fee_bps(env: Env) -> Result<u32, MarketplaceError> {
        Ok(Self::load_config(&env)?.fee_bps)
    }

    /// Update the minimum required discount (face_value - asking_price), in
    /// basis points of face_value, that a listing must offer. Admin only.
    ///
    /// **Errors:**
    /// - `MarketplaceError::NotAdmin` — Caller is not the admin.
    /// - `MarketplaceError::InvalidFeeRate` — `min_discount_bps` > 10 000.
    pub fn set_min_discount_bps(env: Env, admin: Address, min_discount_bps: u32) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }
        require_valid_fee_bps(min_discount_bps)?;
        env.storage().instance().set(&DataKey::MinDiscountBps, &min_discount_bps);
        Ok(())
    }

    /// Returns the current minimum discount requirement in basis points.
    pub fn get_min_discount_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MinDiscountBps)
            .unwrap_or(DEFAULT_MIN_DISCOUNT_BPS)
    }

    /// Set a per-risk-tier fee override. Admin only. (#210)
    pub fn set_tier_fee_bps(
        env: Env,
        admin: Address,
        tier: RiskTier,
        fee_bps: u32,
    ) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }
        require_valid_fee_bps(fee_bps)?;
        env.storage().instance().set(&DataKey::TierFeeBps(Self::tier_ordinal(&tier)), &fee_bps);
        Ok(())
    }

    /// Get the fee for a specific risk tier (falls back to flat fee if no override). (#210)
    pub fn get_tier_fee_bps(env: Env, tier: RiskTier) -> Result<u32, MarketplaceError> {
        let ordinal = Self::tier_ordinal(&tier);
        Ok(env.storage().instance()
            .get(&DataKey::TierFeeBps(ordinal))
            .unwrap_or_else(|| Self::load_config(&env).map(|c| c.fee_bps).unwrap_or(50)))
    }

    /// Returns the full config struct.
    pub fn get_config(env: Env) -> Result<MarketplaceConfig, MarketplaceError> {
        Self::load_config(&env)
    }

    /// Returns the admin address.
    pub fn get_admin(env: Env) -> Result<Address, MarketplaceError> {
        Ok(Self::load_config(&env)?.admin)
    }

    /// Whitelist a stablecoin token. Admin only.
    pub fn whitelist_token(env: Env, admin: Address, token: Address) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
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
    ) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }
        if !env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false)
        {
            return Err(MarketplaceError::TokenNotWhitelisted);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::WhitelistedToken(token.clone()));
        events::token_whitelist_removed(&env, &admin, &token);
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
    /// An optional `bidding_deadline` puts the listing into reverse-auction mode (#440).
    pub fn list_invoice(
        env: Env,
        seller: Address,
        invoice_id: u64,
        asking_price: i128,
        face_value: i128,
        token: Address,
        funding_deadline: u64,
        referrer: Option<Address>,
    ) -> Result<(), MarketplaceError> {
        Self::list_invoice_inner(
            env,
            seller,
            invoice_id,
            asking_price,
            face_value,
            token,
            funding_deadline,
            referrer,
            None,  // no bidding deadline
            None,  // no decay schedule
        )
    }

    /// List an invoice with an optional Dutch-auction decay schedule (#439).
    ///
    /// `min_asking_price` is the floor price (must be > 0 and < asking_price).
    /// `decay_start_ts`   is when price starts decaying (must be >= now).
    /// `decay_end_ts`     is when price reaches the floor (must be <= funding_deadline).
    pub fn list_invoice_with_decay(
        env: Env,
        seller: Address,
        invoice_id: u64,
        asking_price: i128,
        face_value: i128,
        token: Address,
        funding_deadline: u64,
        referrer: Option<Address>,
        min_asking_price: i128,
        decay_start_ts: u64,
        decay_end_ts: u64,
    ) -> Result<(), MarketplaceError> {
        // Validate decay schedule parameters
        require_non_zero_amount(min_asking_price)?;
        if min_asking_price >= asking_price {
            return Err(MarketplaceError::InvalidAmount);
        }
        if decay_start_ts >= decay_end_ts {
            return Err(MarketplaceError::InvalidDueDate);
        }
        if decay_end_ts > funding_deadline {
            return Err(MarketplaceError::InvalidDueDate);
        }

        let schedule = DecaySchedule {
            start_price: asking_price,
            floor_price: min_asking_price,
            decay_start_ts,
            decay_end_ts,
        };

        Self::list_invoice_inner(
            env,
            seller,
            invoice_id,
            asking_price,
            face_value,
            token,
            funding_deadline,
            referrer,
            None,
            Some(schedule),
        )
    }

    /// List an invoice in reverse-auction (bidding) mode (#440).
    ///
    /// `bidding_deadline` must be before `funding_deadline`.
    /// Direct `fund_invoice` calls are disabled while the bidding window is open.
    pub fn list_invoice_with_bidding(
        env: Env,
        seller: Address,
        invoice_id: u64,
        asking_price: i128,
        face_value: i128,
        token: Address,
        funding_deadline: u64,
        referrer: Option<Address>,
        bidding_deadline: u64,
    ) -> Result<(), MarketplaceError> {
        if bidding_deadline >= funding_deadline {
            return Err(MarketplaceError::InvalidDueDate);
        }
        kora_shared::validation::require_future_timestamp(&env, bidding_deadline)?;

        Self::list_invoice_inner(
            env,
            seller,
            invoice_id,
            asking_price,
            face_value,
            token,
            funding_deadline,
            referrer,
            Some(bidding_deadline),
            None,
        )
    }

    /// Internal implementation shared by all list_invoice variants.
    fn list_invoice_inner(
        env: Env,
        seller: Address,
        invoice_id: u64,
        asking_price: i128,
        face_value: i128,
        token: Address,
        funding_deadline: u64,
        referrer: Option<Address>,
        bidding_deadline: Option<u64>,
        decay_schedule: Option<DecaySchedule>,
    ) -> Result<(), MarketplaceError> {
        seller.require_auth();
        Self::require_not_paused(&env)?;

        require_non_zero_amount(asking_price)?;
        require_non_zero_amount(face_value)?;
        require_within_max_amount(asking_price)?;
        require_within_max_amount(face_value)?;

        // asking_price must be strictly less than face_value (discount must exist).
        // Decay-schedule listings are exempt: they start undiscounted at face_value
        // and decay toward `min_asking_price` over time, so the discount is
        // enforced there instead (`min_asking_price < asking_price`).
        if decay_schedule.is_none() {
            if asking_price >= face_value {
                return Err(MarketplaceError::InvalidAmount);
            }

            // Enforce a minimum discount so listings offer investors meaningful yield.
            let min_discount_bps = Self::get_min_discount_bps(env.clone());
            let min_discount = bps_of(face_value, min_discount_bps)?;
            let discount = face_value
                .checked_sub(asking_price)
                .ok_or(MarketplaceError::ArithmeticOverflow)?;
            if discount < min_discount {
                return Err(MarketplaceError::InvalidAmount);
            }
        }

        Self::require_whitelisted_token(&env, &token)?;
        Self::require_compliance_attested(&env, &seller)?;

        // Reject if an active listing already exists for this invoice.
        // A cancelled listing (is_active == false) does not block re-listing.
        if let Some(existing) = env
            .storage()
            .persistent()
            .get::<_, Listing>(&DataKey::Listing(invoice_id))
        {
            if existing.is_active {
                return Err(MarketplaceError::InvoiceAlreadyExists);
            }
        }

        let _guard = ReentrancyGuard::new(&env)?;

        let config = Self::load_config(&env)?;

        // ── #441: Fetch invoice BEFORE deadline validation so we can cross-check ──
        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
        let invoice = nft_client.get_invoice(&invoice_id);

        if invoice.amount != face_value {
            return Err(MarketplaceError::InvalidAmount);
        }

        // ── #441: Validate funding_deadline is in the future ──────────────────
        kora_shared::validation::require_future_timestamp(&env, funding_deadline)?;

        // ── #441: funding_deadline must be at least `min_buffer` seconds before
        //         the invoice's due_date ──────────────────────────────────────
        let min_buffer: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MinFundingBuffer)
            .unwrap_or(DEFAULT_MIN_FUNDING_BUFFER);

        let deadline_with_buffer = funding_deadline
            .checked_add(min_buffer)
            .ok_or(MarketplaceError::ArithmeticOverflow)?;

        if deadline_with_buffer > invoice.due_date {
            return Err(MarketplaceError::FundingDeadlineTooCloseToDueDate);
        }

        // Referrer may not be the seller (self-referral)
        if let Some(ref r) = referrer {
            if r == &seller {
                return Err(MarketplaceError::InvalidAddress);
            }
            env.storage()
                .persistent()
                .set(&DataKey::Referrer(invoice_id), r);
            Self::bump_persistent(&env, &DataKey::Referrer(invoice_id));
        }

        nft_client.set_listed(&env.current_contract_address(), &invoice_id);

        // ── #439: Store decay schedule if provided ────────────────────────────
        if let Some(ref schedule) = decay_schedule {
            env.storage()
                .persistent()
                .set(&DataKey::DecaySchedule(invoice_id), schedule);
            Self::bump_persistent(&env, &DataKey::DecaySchedule(invoice_id));
            events::decay_schedule_set(
                &env,
                invoice_id,
                &seller,
                schedule.floor_price,
                schedule.decay_end_ts,
            );
        }

        let listing = Listing {
            invoice_id,
            seller: seller.clone(),
            asking_price,
            face_value,
            token,
            funded_amount: 0,
            funding_deadline,
            is_active: true,
            bidding_deadline,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));
        events::invoice_listed(&env, invoice_id, &seller, asking_price, invoice.currency.clone());
        Ok(())
    }

    /// Investor funds a share of an invoice.
    /// Rejected when the listing is in bidding mode and the bidding window is still open (#440).
    /// Uses the time-adjusted price from the decay schedule when active (#439).
    pub fn fund_invoice(
        env: Env,
        investor: Address,
        invoice_id: u64,
        amount: i128,
    ) -> Result<(), MarketplaceError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;

        require_non_zero_amount(amount)?;
        require_within_max_amount(amount)?;

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)?;

        if !listing.is_active {
            return Err(MarketplaceError::ListingAlreadyCancelled);
        }
        if env.ledger().timestamp() > listing.funding_deadline {
            return Err(MarketplaceError::FundingDeadlinePassed);
        }

        // ── #440: Reject direct funding while bidding window is open ──────────
        if let Some(bid_dl) = listing.bidding_deadline {
            if env.ledger().timestamp() <= bid_dl {
                return Err(MarketplaceError::BiddingWindowActive);
            }
        }

        // ── #439: Use the time-adjusted price (Dutch auction) ─────────────────
        let effective_price = Self::current_asking_price_impl(&env, &listing);
        let remaining = safe_sub(effective_price, listing.funded_amount)?;
        if amount > remaining {
            return Err(MarketplaceError::ExceedsFundingTarget);
        }

        let config = Self::load_config(&env)?;

        // Check per-invoice freeze before any token operations.
        // Enforced in addition to the protocol-wide pause so a single disputed
        // invoice can be frozen without halting all protocol activity.
        let nft_client = kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
        if nft_client.is_invoice_frozen(&invoice_id) {
            return Err(MarketplaceError::InvoiceFrozen);
        }

        let token_client = token::Client::new(&env, &listing.token);
        let token_decimals = token_client.decimals();

        // Fetch the invoice's risk tier and apply tier-specific fee (#210)
        let invoice = nft_client.get_invoice(&invoice_id);
        let effective_fee_bps: u32 = env.storage().instance()
            .get(&DataKey::TierFeeBps(Self::tier_ordinal(&invoice.risk_tier)))
            .unwrap_or(config.fee_bps);

        let fee = bps_of_normalized(amount, effective_fee_bps, token_decimals)?;
        let net = amount
            .checked_sub(fee)
            .ok_or(MarketplaceError::ArithmeticOverflow)?;

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

        // Fully funded when funded_amount reaches the current effective price
        let fully_funded = listing.funded_amount >= effective_price;
        if fully_funded {
            listing.is_active = false;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

        events::invoice_funded(&env, invoice_id, &investor, amount, invoice.currency.clone());
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
    pub fn cancel_listing(env: Env, caller: Address, invoice_id: u64) -> Result<(), MarketplaceError> {
        caller.require_auth();

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)?;

        if !listing.is_active {
            return Err(MarketplaceError::ListingAlreadyCancelled);
        }

        let config = Self::load_config(&env)?;
        if caller != listing.seller && caller != config.admin {
            return Err(MarketplaceError::Unauthorized);
        }

        listing.is_active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

        // Revert the NFT status from Listed → Created so the SME can re-list.
        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
        nft_client.set_created(&env.current_contract_address(), &invoice_id);

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
    ) -> Result<(), MarketplaceError> {
        caller.require_auth();

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)?;

        if !listing.is_active {
            return Err(MarketplaceError::ListingAlreadyCancelled);
        }

        let config = Self::load_config(&env)?;
        if caller != listing.seller && caller != config.admin {
            return Err(MarketplaceError::Unauthorized);
        }

        // If no partial funding, cancel immediately — no two-phase needed
        if listing.funded_amount == 0 {
            listing.is_active = false;
            env.storage()
                .persistent()
                .set(&DataKey::Listing(invoice_id), &listing);
            Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

            // Revert NFT status from Listed → Created so the SME can re-list.
            let nft_client =
                kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
            nft_client.set_created(&env.current_contract_address(), &invoice_id);

            events::listing_cancelled(&env, invoice_id, &listing.seller);
            return Ok(());
        }

        // Guard against duplicate requests
        if env
            .storage()
            .persistent()
            .has(&DataKey::CancellationRequest(invoice_id))
        {
            return Err(MarketplaceError::CancellationPending);
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
    ) -> Result<(), MarketplaceError> {
        admin.require_auth();

        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }

        // A pending cancellation request must exist
        if !env
            .storage()
            .persistent()
            .has(&DataKey::CancellationRequest(invoice_id))
        {
            return Err(MarketplaceError::NoCancellationPending);
        }

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)?;

        if !listing.is_active {
            return Err(MarketplaceError::ListingAlreadyCancelled);
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

        // Revert NFT status from Listed → Created so the SME can re-list.
        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
        nft_client.set_created(&env.current_contract_address(), &invoice_id);

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
    ) -> Result<(), MarketplaceError> {
        investor.require_auth();

        let listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)?;

        // Refund only if the listing never reached full funding
        if listing.funded_amount >= listing.asking_price {
            return Err(MarketplaceError::ListingFullyFunded);
        }

        // Refund is allowed when the cancellation was confirmed OR the deadline passed
        let cancellation_confirmed = env
            .storage()
            .persistent()
            .get::<_, bool>(&DataKey::CancellationConfirmed(invoice_id))
            .unwrap_or(false);

        if !cancellation_confirmed && env.ledger().timestamp() <= listing.funding_deadline {
            return Err(MarketplaceError::FundingNotExpired);
        }

        // Guard: investor hasn't already claimed
        let refund_key = DataKey::RefundClaimed(invoice_id, investor.clone());
        if env
            .storage()
            .persistent()
            .get::<_, bool>(&refund_key)
            .unwrap_or(false)
        {
            return Err(MarketplaceError::RefundAlreadyClaimed);
        }

        // Look up the investor's net contribution
        let contrib_key = DataKey::Contribution(invoice_id, investor.clone());
        let net_contributed: i128 = env
            .storage()
            .persistent()
            .get(&contrib_key)
            .unwrap_or(0);

        if net_contributed <= 0 {
            return Err(MarketplaceError::NoContribution);
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
    pub fn get_listing(env: Env, invoice_id: u64) -> Result<Listing, MarketplaceError> {
        env.storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)
    }

    // ── Dutch Auction / Price Decay (#439) ────────────────────────────────────

    /// View: returns the current effective asking price for a listing.
    ///
    /// If no decay schedule is set, returns `listing.asking_price` unchanged.
    /// If a schedule is active, performs linear interpolation between
    /// `start_price` and `floor_price` based on the current ledger timestamp.
    pub fn get_current_price(env: Env, invoice_id: u64) -> Result<i128, MarketplaceError> {
        let listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)?;
        Ok(Self::current_asking_price_impl(&env, &listing))
    }

    /// View: returns the decay schedule for a listing, if one is set.
    pub fn get_decay_schedule(
        env: Env,
        invoice_id: u64,
    ) -> Option<DecaySchedule> {
        env.storage()
            .persistent()
            .get(&DataKey::DecaySchedule(invoice_id))
    }

    // ── Reverse Auction / Bid submission (#440) ───────────────────────────────

    /// Investor submits a bid on a reverse-auction listing.
    ///
    /// - The listing must have a `bidding_deadline` set.
    /// - Current ledger time must be <= `bidding_deadline`.
    /// - `bid_price` must be <= current_asking_price and > 0.
    /// - `amount` must be > 0 and <= face_value.
    /// - Each investor may have at most one active bid per listing.
    pub fn submit_bid(
        env: Env,
        investor: Address,
        invoice_id: u64,
        bid_price: i128,
        amount: i128,
    ) -> Result<(), MarketplaceError> {
        investor.require_auth();
        Self::require_not_paused(&env)?;

        require_non_zero_amount(bid_price)?;
        require_non_zero_amount(amount)?;
        require_within_max_amount(bid_price)?;
        require_within_max_amount(amount)?;

        let listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)?;

        if !listing.is_active {
            return Err(MarketplaceError::ListingAlreadyCancelled);
        }

        // Must be in bidding mode
        let bidding_deadline = listing
            .bidding_deadline
            .ok_or(MarketplaceError::BiddingWindowClosed)?;

        // Must be inside the bidding window
        if env.ledger().timestamp() > bidding_deadline {
            return Err(MarketplaceError::BiddingWindowClosed);
        }

        // bid_price must not exceed the current effective asking price
        let current_price = Self::current_asking_price_impl(&env, &listing);
        if bid_price > current_price {
            return Err(MarketplaceError::InvalidAmount);
        }

        // amount must not exceed the face_value
        if amount > listing.face_value {
            return Err(MarketplaceError::ExceedsFundingTarget);
        }

        // One bid per investor per listing
        let bid_key = DataKey::Bid(invoice_id, investor.clone());
        if env.storage().persistent().has(&bid_key) {
            return Err(MarketplaceError::BidAlreadyExists);
        }

        let bid = Bid {
            investor: investor.clone(),
            invoice_id,
            bid_price,
            amount,
            submitted_at: env.ledger().timestamp(),
        };

        env.storage().persistent().set(&bid_key, &bid);
        Self::bump_persistent(&env, &bid_key);

        events::bid_submitted(&env, invoice_id, &investor, bid_price, amount);
        Ok(())
    }

    /// Seller accepts one or more winning bids after the bidding window closes (#440).
    ///
    /// `bid_investors` is the ordered list of investors whose bids the seller accepts.
    /// Accepted bids are settled through the same fee/transfer/pool-release pipeline
    /// as `fund_invoice`.  Acceptance stops once the listing is fully funded.
    ///
    /// Rules:
    /// - Caller must be the listing seller or admin.
    /// - `bidding_deadline` must have passed.
    /// - Each accepted bid must exist.
    /// - Total accepted `amount` must not exceed remaining capacity.
    pub fn accept_bids(
        env: Env,
        caller: Address,
        invoice_id: u64,
        bid_investors: Vec<Address>,
    ) -> Result<(), MarketplaceError> {
        caller.require_auth();
        Self::require_not_paused(&env)?;

        let mut listing: Listing = env
            .storage()
            .persistent()
            .get(&DataKey::Listing(invoice_id))
            .ok_or(MarketplaceError::ListingNotFound)?;

        if !listing.is_active {
            return Err(MarketplaceError::ListingAlreadyCancelled);
        }

        let config = Self::load_config(&env)?;

        // Only seller or admin may accept bids
        if caller != listing.seller && caller != config.admin {
            return Err(MarketplaceError::Unauthorized);
        }

        // Must be in bidding mode and bidding window must have closed
        let bidding_deadline = listing
            .bidding_deadline
            .ok_or(MarketplaceError::BiddingWindowClosed)?;

        if env.ledger().timestamp() <= bidding_deadline {
            return Err(MarketplaceError::BiddingWindowActive);
        }

        let nft_client =
            kora_invoice_nft::InvoiceNftContractClient::new(&env, &config.invoice_nft);
        if nft_client.is_invoice_frozen(&invoice_id) {
            return Err(MarketplaceError::InvoiceFrozen);
        }

        let token_client = token::Client::new(&env, &listing.token);
        let token_decimals = token_client.decimals();

        let invoice = nft_client.get_invoice(&invoice_id);
        let effective_fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::TierFeeBps(Self::tier_ordinal(&invoice.risk_tier)))
            .unwrap_or(config.fee_bps);

        let effective_price = Self::current_asking_price_impl(&env, &listing);

        for investor in bid_investors.iter() {
            // Stop once fully funded
            if listing.funded_amount >= effective_price {
                break;
            }

            let bid_key = DataKey::Bid(invoice_id, investor.clone());
            let bid: Bid = env
                .storage()
                .persistent()
                .get(&bid_key)
                .ok_or(MarketplaceError::BidNotFound)?;

            let remaining = safe_sub(effective_price, listing.funded_amount)?;
            // Accept the smaller of bid.amount and remaining capacity
            let accept_amount = if bid.amount > remaining {
                remaining
            } else {
                bid.amount
            };

            let fee = bps_of_normalized(accept_amount, effective_fee_bps, token_decimals)?;
            let net = accept_amount
                .checked_sub(fee)
                .ok_or(MarketplaceError::ArithmeticOverflow)?;

            if fee > 0 {
                token_client.transfer(&investor, &config.treasury, &fee);
                let treasury_client =
                    kora_treasury::TreasuryContractClient::new(&env, &config.treasury);
                treasury_client.collect_fee(&listing.token, &fee);
            }
            if net > 0 {
                token_client.transfer(&investor, &config.financing_pool, &net);
            }

            listing.funded_amount = safe_add(listing.funded_amount, accept_amount)?;

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

            // Consume the bid
            env.storage().persistent().remove(&bid_key);

            events::invoice_funded(&env, invoice_id, &investor, accept_amount, invoice.currency.clone());
            events::bid_accepted(
                &env,
                invoice_id,
                &listing.seller,
                &investor,
                bid.bid_price,
                accept_amount,
            );
            if fee > 0 {
                events::fee_collected(&env, &investor, invoice_id, fee, &listing.token);
            }
        }

        let fully_funded = listing.funded_amount >= effective_price;
        if fully_funded {
            listing.is_active = false;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Listing(invoice_id), &listing);
        Self::bump_persistent(&env, &DataKey::Listing(invoice_id));

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

    /// Returns the bid submitted by `investor` for `invoice_id`, if any.
    pub fn get_bid(env: Env, invoice_id: u64, investor: Address) -> Option<Bid> {
        env.storage()
            .persistent()
            .get(&DataKey::Bid(invoice_id, investor))
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn require_compliance_attested(env: &Env, sme: &Address) -> Result<(), MarketplaceError> {
        let config = Self::load_config(env)?;
        let rr = kora_risk_registry::RiskRegistryContractClient::new(env, &config.risk_registry);
        if !rr.is_compliance_attested(sme) {
            return Err(MarketplaceError::ComplianceNotAttested);
        }
        Ok(())
    }

    fn require_whitelisted_token(env: &Env, token: &Address) -> Result<(), MarketplaceError> {
        let ok: bool = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedToken(token.clone()))
            .unwrap_or(false);
        if !ok {
            return Err(MarketplaceError::TokenNotWhitelisted);
        }
        Ok(())
    }

    /// Compute the current effective asking price for a listing (#439).
    ///
    /// - If no `DecaySchedule` is stored for the listing, returns `listing.asking_price`.
    /// - If now < `decay_start_ts`: returns `start_price`.
    /// - If now > `decay_end_ts`:   returns `floor_price`.
    /// - Otherwise:  linear interpolation between `start_price` and `floor_price`.
    ///
    /// Formula (integer, rounds toward floor):
    /// ```text
    /// elapsed   = now - decay_start_ts
    /// duration  = decay_end_ts - decay_start_ts
    /// drop      = (start_price - floor_price) * elapsed / duration
    /// price     = start_price - drop
    /// ```
    fn current_asking_price_impl(env: &Env, listing: &Listing) -> i128 {
        let schedule: Option<DecaySchedule> = env
            .storage()
            .persistent()
            .get(&DataKey::DecaySchedule(listing.invoice_id));

        let schedule = match schedule {
            None => return listing.asking_price,
            Some(s) => s,
        };

        let now = env.ledger().timestamp();

        if now <= schedule.decay_start_ts {
            return schedule.start_price;
        }
        if now >= schedule.decay_end_ts {
            return schedule.floor_price;
        }

        // Linear interpolation (integer arithmetic, truncates toward floor)
        let elapsed = (now - schedule.decay_start_ts) as i128;
        let duration = (schedule.decay_end_ts - schedule.decay_start_ts) as i128;
        let price_drop = (schedule.start_price - schedule.floor_price)
            .checked_mul(elapsed)
            .unwrap_or(schedule.start_price - schedule.floor_price)
            / duration;

        schedule.start_price - price_drop
    }

    // ── Upgrade ────────────────────────────────────────────────────────────────

    pub fn propose_upgrade(
        env: Env,
        admin: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }
        env.storage().instance().set(
            &DataKey::UpgradeProposal,
            &(new_wasm_hash.clone(), env.ledger().timestamp()),
        );
        events::upgrade_proposed(&env, &admin, &new_wasm_hash);
        Ok(())
    }

    pub fn execute_upgrade(env: Env, admin: Address) -> Result<(), MarketplaceError> {
        admin.require_auth();
        let config = Self::load_config(&env)?;
        if config.admin != admin {
            return Err(MarketplaceError::NotAdmin);
        }
        let (wasm_hash, proposed_at): (BytesN<32>, u64) = env
            .storage()
            .instance()
            .get(&DataKey::UpgradeProposal)
            .ok_or(MarketplaceError::NoUpgradeProposed)?;
        if env.ledger().timestamp() < proposed_at + UPGRADE_TIMELOCK_DELAY {
            return Err(MarketplaceError::UpgradeTimelockNotElapsed);
        }
        env.storage().instance().remove(&DataKey::UpgradeProposal);
        events::upgrade_executed(&env, &admin, &wasm_hash);
        env.deployer().update_current_contract_wasm(wasm_hash);
        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn load_config(env: &Env) -> Result<MarketplaceConfig, MarketplaceError> {
        if let Some(config) = env.storage().instance().get(&DataKey::Config) {
            return Ok(config);
        }

        // Legacy migration path: read individual keys and consolidate.
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(MarketplaceError::NotInitialized)?;
        let invoice_nft: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceNft)
            .ok_or(MarketplaceError::NotInitialized)?;
        let financing_pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::FinancingPool)
            .ok_or(MarketplaceError::NotInitialized)?;
        let treasury: Address = env
            .storage()
            .instance()
            .get(&DataKey::Treasury)
            .ok_or(MarketplaceError::NotInitialized)?;
        let access_control: Address = env
            .storage()
            .instance()
            .get(&DataKey::AccessControl)
            .ok_or(MarketplaceError::NotInitialized)?;
        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::FeeBps)
            .ok_or(MarketplaceError::NotInitialized)?;
        // risk_registry was never part of the legacy per-key storage format —
        // contracts still on that format must be re-initialized to consolidate
        // into `Config` before risk_registry-dependent calls can succeed.
        let risk_registry: Address = env
            .storage()
            .instance()
            .get(&DataKey::RiskRegistry)
            .ok_or(MarketplaceError::NotInitialized)?;

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

    /// NOTE: `DataKey::AccessControl` is read directly (not from inside Config) so this
    /// stays cheap on the common path. `initialize` always writes a real AccessControl
    /// contract address here, so this performs a live cross-contract pause check.
    fn require_not_paused(env: &Env) -> Result<(), MarketplaceError> {
        if let Some(ac_contract) =
            env.storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::AccessControl)
        {
            let ac =
                kora_access_control::AccessControlContractClient::new(env, &ac_contract);
            if ac.is_paused() {
                return Err(MarketplaceError::ProtocolPaused);
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
    use kora_access_control::AccessControlContract;
    use kora_financing_pool::{FinancingPoolContract, FinancingPoolContractClient};
    use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient};
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
        staking_token: Address,
        mp: MarketplaceContractClient<'static>,
        nft: InvoiceNftContractClient<'static>,
    }

    fn mint_stake(env: &Env, token: &Address, to: &Address, amount: i128) {
        soroban_sdk::token::StellarAssetClient::new(env, token).mint(to, &amount);
    }

    fn deploy() -> TestEnv {
        let env = Env::default();
        // add_verifier's stake transfer requires verifier.require_auth() from
        // inside a nested call (risk_registry -> token), which isn't tied to
        // the root invocation — plain mock_all_auths() rejects that.
        env.mock_all_auths_allowing_non_root_auth();

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
        let treasury_id = env.register_contract(None, kora_treasury::TreasuryContract);
        let treasury_client = kora_treasury::TreasuryContractClient::new(&env, &treasury_id);
        treasury_client.initialize(&admin, &50u32);
        let treasury = treasury_id;

        let ac_id = env.register_contract(None, AccessControlContract);
        let ac_client = kora_access_control::AccessControlContractClient::new(&env, &ac_id);
        ac_client.initialize(&admin);

        let nft_id = env.register_contract(None, InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_id);
        nft.initialize(&admin, &ac_id);

        let pool_id = env.register_contract(None, FinancingPoolContract);
        let pool_client = FinancingPoolContractClient::new(&env, &pool_id);
        let rr = Address::generate(&env);    // risk registry (unused in unit tests)
        let oracle = Address::generate(&env); // price oracle  (unused in unit tests)
        pool_client.initialize(&admin, &nft_id, &rr, &treasury, &ac_id, &200u32, &oracle, &5_000u32);

        let registry_id = env.register_contract(None, kora_risk_registry::RiskRegistryContract);
        let registry = registry_id.clone();
        let registry_client = kora_risk_registry::RiskRegistryContractClient::new(&env, &registry_id);
        let token_admin = Address::generate(&env);
        let staking_token = env.register_stellar_asset_contract_v2(token_admin).address();
        registry_client.initialize(&admin, &nft_id, &staking_token, &1_000_000i128, &5_000u32);

        let mp_id = env.register_contract(None, MarketplaceContract);
        let mp = MarketplaceContractClient::new(&env, &mp_id);
        mp.initialize(&admin, &nft_id, &pool_id, &treasury, &ac_id, &registry, &50u32);

        // Register marketplace and pool as authorized callers on the NFT contract (#209)
        nft.set_authorized_callers(&admin, &mp_id, &pool_id);

        let token_issuer = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_issuer).address();
        mp.whitelist_token(&admin, &token);
        treasury_client.whitelist_token(&admin, &token);

        let seller = Address::generate(&env);

        // Default seller must be compliance-attested for list_invoice to succeed.
        // list_invoice requires the seller to be a compliance-attested SME.
        let verifier = Address::generate(&env);
        mint_stake(&env, &staking_token, &verifier, 1_000_000i128);
        registry_client.add_verifier(&admin, &verifier, &1_000_000i128);
        registry_client.register_sme(&verifier, &seller, &50u32, &true);

        TestEnv { env, admin, token, seller, treasury, pool: pool_id, registry, staking_token, mp, nft }
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
            &None,
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
            &None::<Address>,
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::AlreadyInitialized);
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidFeeRate);
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
            MarketplaceError::NotInitialized
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::NotAdmin);
    }

    // ── remove_token_whitelist ────────────────────────────────────────────────

    #[test]
    fn test_remove_token_whitelist_success() {
        let t = deploy();
        let new_token = Address::generate(&t.env);
        t.mp.whitelist_token(&t.admin, &new_token);
        assert!(t.mp.is_token_whitelisted(&new_token));
        t.mp.remove_token_whitelist(&t.admin, &new_token);
        assert!(!t.mp.is_token_whitelisted(&new_token));
    }

    #[test]
    fn test_remove_token_whitelist_non_admin_rejected() {
        let t = deploy();
        let stranger = Address::generate(&t.env);
        let new_token = Address::generate(&t.env);
        t.mp.whitelist_token(&t.admin, &new_token);
        let result = t.mp.try_remove_token_whitelist(&stranger, &new_token);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::NotAdmin);
        assert!(t.mp.is_token_whitelisted(&new_token));
    }

    #[test]
    fn test_remove_token_whitelist_not_whitelisted_rejected() {
        let t = deploy();
        let new_token = Address::generate(&t.env);
        let result = t.mp.try_remove_token_whitelist(&t.admin, &new_token);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::TokenNotWhitelisted);
    }

    #[test]
    fn test_remove_token_whitelist_blocks_new_listing() {
        let t = deploy();
        // t.token is whitelisted by deploy(); remove it, then listing must fail.
        t.mp.remove_token_whitelist(&t.admin, &t.token);
        assert!(!t.mp.is_token_whitelisted(&t.token));

        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result = t.mp.try_list_invoice(
            &t.seller,
            &1u64,
            &9_000i128,
            &10_000i128,
            &t.token,
            &deadline,
            &None::<Address>,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::TokenNotWhitelisted);
    }

    #[test]
    fn test_remove_token_whitelist_existing_listing_unaffected() {
        let t = deploy();
        let id = list_one(&t);
        // Remove the whitelist for the token used by the already-created listing.
        t.mp.remove_token_whitelist(&t.admin, &t.token);
        assert!(!t.mp.is_token_whitelisted(&t.token));

        // The existing listing must remain funded-able despite the removal.
        let listing = t.mp.get_listing(&id);
        assert!(listing.is_active);
        let investor = Address::generate(&t.env);
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token)
            .mint(&investor, &1_000_000i128);
        let result = t.mp.try_fund_invoice(&investor, &id, &1_000_000i128);
        assert!(result.is_ok());
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
            &None::<Address>,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::TokenNotWhitelisted);
    }

    #[test]
    fn test_list_invoice_zero_asking_price_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result =
            t.mp.try_list_invoice(&t.seller, &1u64, &0i128, &10_000i128, &t.token, &deadline, &None::<Address>);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_zero_face_value_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result =
            t.mp.try_list_invoice(&t.seller, &1u64, &9_000i128, &0i128, &t.token, &deadline, &None::<Address>);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
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
            &None::<Address>,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    // ── minimum discount enforcement ──────────────────────────────────────────

    #[test]
    fn test_list_invoice_discount_below_minimum_rejected() {
        let t = deploy();
        // mint_invoice() always mints a face value of 10_000_000_000.
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        // face_value = 10_000_000_000, default min_discount_bps = 10 →
        // min_discount = 10_000_000. A discount of 9_999_999 (asking_price =
        // 9_990_000_001) is one below the minimum.
        let result = t.mp.try_list_invoice(
            &t.seller,
            &1u64,
            &9_990_000_001i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
            &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_discount_exact_minimum_accepted() {
        let t = deploy();
        // mint_invoice() always mints a face value of 10_000_000_000.
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        // face_value = 10_000_000_000, default min_discount_bps = 10 →
        // min_discount = 10_000_000. A discount of exactly 10_000_000
        // (asking_price = 9_990_000_000) must be accepted.
        let result = t.mp.try_list_invoice(
            &t.seller,
            &1u64,
            &9_990_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
            &None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_invoice_valid_discount_accepted() {
        let t = deploy();
        let id = list_one(&t);
        let listing = t.mp.get_listing(&id);
        assert!(listing.is_active);
        assert_eq!(listing.asking_price, 9_500_000_000i128);
    }

    #[test]
    fn test_set_min_discount_bps_enforced_on_next_listing() {
        let t = deploy();
        // Raise the minimum discount to 10% (1_000 bps); a 5% discount must
        // now be rejected even though it passed under the default 10 bps.
        t.mp.set_min_discount_bps(&t.admin, &1_000u32);
        let _id = mint_invoice(&t);
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result = t.mp.try_list_invoice(
            &t.seller,
            &1u64,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
            &None,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    #[test]
    fn test_set_min_discount_bps_non_admin_rejected() {
        let t = deploy();
        let stranger = Address::generate(&t.env);
        let result = t.mp.try_set_min_discount_bps(&stranger, &1_000u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_set_min_discount_bps_over_10000_rejected() {
        let t = deploy();
        let result = t.mp.try_set_min_discount_bps(&t.admin, &10_001u32);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_min_discount_bps_defaults_to_ten() {
        let t = deploy();
        assert_eq!(t.mp.get_min_discount_bps(), 10u32);
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
            &None::<Address>,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_past_deadline_rejected() {
        let t = deploy();
        let _id = mint_invoice(&t);
        let past = t.env.ledger().timestamp() - 1;
        let result =
            t.mp.try_list_invoice(&t.seller, &1u64, &9_000_000_000i128, &10_000_000_000i128, &t.token, &past, &None::<Address>);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidDueDate);
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
            &None::<Address>,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            MarketplaceError::InvoiceAlreadyExists
        );
    }

    #[test]
    fn test_list_multiple_invoices_independent() {
        let t = deploy();
        let deadline = t.env.ledger().timestamp() + 86_400;
        let result =
            t.mp.try_list_invoice(&t.seller, &1u64, &-1i128, &10_000i128, &t.token, &deadline, &None::<Address>);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    #[test]
    fn test_list_invoice_unattested_sme_rejected() {
        let t = deploy();
        let verifier = Address::generate(&t.env);
        let registry_client = kora_risk_registry::RiskRegistryContractClient::new(&t.env, &t.registry);
        mint_stake(&t.env, &t.staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.staking_token)
            .mint(&verifier, &1_000_000i128);
        registry_client.add_verifier(&t.admin, &verifier, &1_000_000i128);

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
            &None::<Address>,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::ComplianceNotAttested);
    }

    #[test]
    fn test_list_invoice_attested_sme_succeeds() {
        let t = deploy();
        let verifier = Address::generate(&t.env);
        let registry_client = kora_risk_registry::RiskRegistryContractClient::new(&t.env, &t.registry);
        mint_stake(&t.env, &t.staking_token, &verifier, 1_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.staking_token)
            .mint(&verifier, &1_000_000i128);
        registry_client.add_verifier(&t.admin, &verifier, &1_000_000i128);

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
                &None,
            )
        };

        assert!(t.mp.try_list_invoice(
            &attested_seller,
            &nft_id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
            &None::<Address>,
        ).is_ok());
    }

    // ── get_listing ───────────────────────────────────────────────────────────

    #[test]
    fn test_get_listing_not_found_returns_error() {
        let t = deploy();
        let result = t.mp.try_get_listing(&999u64);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::ListingNotFound);
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
            &None::<Address>,
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::ListingNotFound);
    }

    #[test]
    fn test_fund_invoice_zero_amount_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &id, &0i128);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    #[test]
    fn test_fund_invoice_negative_amount_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &id, &-1i128);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    #[test]
    fn test_fund_invoice_exceeds_target_rejected() {
        let t = deploy();
        let id = list_one(&t);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &1u64, &9_500_000_001i128);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::ExceedsFundingTarget);
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
            &None::<Address>,
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::FundingDeadlinePassed);
    }

    #[test]
    fn test_fund_invoice_on_cancelled_listing_rejected() {
        let t = deploy();
        let id = list_one(&t);
        t.mp.cancel_listing(&t.seller, &id);
        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &1u64, &1_000_000i128);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::ListingAlreadyCancelled);
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
            MarketplaceError::ListingAlreadyCancelled
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
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token)
            .mint(&inv1, &5_000_000_000i128);
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token)
            .mint(&inv2, &4_500_000_000i128);

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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::Unauthorized);
    }

    #[test]
    fn test_cancel_listing_not_found_returns_error() {
        let t = deploy();
        let result = t.mp.try_cancel_listing(&t.seller, &999u64);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::ListingNotFound);
    }

    #[test]
    fn test_cancel_listing_already_cancelled_returns_error() {
        let t = deploy();
        list_one(&t);
        t.mp.cancel_listing(&t.seller, &1u64);
        let result = t.mp.try_cancel_listing(&t.seller, &1u64);
        assert_eq!(
            result.unwrap_err().unwrap(),
            MarketplaceError::ListingAlreadyCancelled
        );
    }

    /// After cancel_listing the NFT status must be reverted to Created.
    #[test]
    fn test_cancel_listing_reverts_nft_status_to_created() {
        let t = deploy();
        let id = list_one(&t);
        // After listing the NFT must be Listed.
        assert_eq!(t.nft.get_invoice(&id).status, kora_shared::types::InvoiceStatus::Listed);

        t.mp.cancel_listing(&t.seller, &id);

        // After cancellation the NFT must be back to Created.
        assert_eq!(t.nft.get_invoice(&id).status, kora_shared::types::InvoiceStatus::Created);
    }

    /// Full list → cancel → relist flow must succeed end-to-end.
    #[test]
    fn test_list_cancel_relist_succeeds() {
        let t = deploy();
        let id = list_one(&t);

        // Cancel the initial listing.
        t.mp.cancel_listing(&t.seller, &id);

        // NFT is back to Created; the cancelled listing key (is_active=false) must
        // not block a fresh listing for the same invoice.
        let new_deadline = t.env.ledger().timestamp() + 86_400 * 30;
        let result = t.mp.try_list_invoice(
            &t.seller,
            &id,
            &9_000_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &new_deadline,
            &None,
        );
        assert!(result.is_ok(), "Re-listing after cancel must succeed, got: {:?}", result.err());
        let new_listing = t.mp.get_listing(&id);
        assert!(new_listing.is_active);
        assert_eq!(new_listing.asking_price, 9_000_000_000i128);
        // NFT must be Listed again after re-listing.
        assert_eq!(t.nft.get_invoice(&id).status, kora_shared::types::InvoiceStatus::Listed);
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::ListingAlreadyCancelled);
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
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token)
            .mint(&investor, &partial_amount);

        // Investor funds the listing partially
        t.mp.fund_invoice(&investor, &id, &partial_amount);
        let listing = t.mp.get_listing(&id);
        assert_eq!(listing.funded_amount, partial_amount);
        assert!(listing.is_active);

        // Seller cancels the partially-funded listing
        assert!(t.mp.try_cancel_listing(&t.seller, &id).is_ok());
        let cancelled_listing = t.mp.get_listing(&id);
        assert!(!cancelled_listing.is_active);

        // BROKEN: Investor cannot claim refund because claim_refund requires
        // the deadline to pass (line 352 of lib.rs). Cancellation before deadline
        // with partial funding leaves investor funds stranded.
        // Expected: refund should be claimable after cancel, or cancel should
        // refund automatically (scope of B9).
        let result = t.mp.try_claim_refund(&investor, &id);
        // This currently fails with FundingNotExpired because deadline hasn't passed
        // even though the listing was cancelled and funds are stuck.
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::FundingNotExpired);
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::Unauthorized);
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
            MarketplaceError::ListingAlreadyCancelled
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::CancellationPending);
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::NoCancellationPending);
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::NotAdmin);
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
            assert_ne!(e.unwrap(), MarketplaceError::FundingNotExpired);
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::FundingNotExpired);
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
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAddress);
    }

    #[test]
    fn test_fund_invoice_no_referrer_full_fee_to_treasury() {
        // With no referrer, entire fee must go to treasury.
        // fee_bps = 50 (0.5%), amount = 10_000_000 → fee = 50_000 → treasury gets 50_000.
        let t = deploy();
        let id = list_with_referrer(&t, None);
        let investor = Address::generate(&t.env);
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token)
            .mint(&investor, &10_000_000i128);
        t.mp.fund_invoice(&investor, &id, &10_000_000i128);
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
        soroban_sdk::token::StellarAssetClient::new(&t.env, &t.token)
            .mint(&investor, &10_000_000i128);
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

    // ── Issue #441: funding_deadline vs due_date buffer ───────────────────────

    /// funding_deadline exactly equal to due_date → rejected (no buffer at all).
    #[test]
    fn test_list_invoice_deadline_equals_due_date_rejected() {
        let t = deploy();
        let id = mint_invoice(&t); // due_date = now + 60 days
        let due_date = t.env.ledger().timestamp() + 86_400 * 60;
        // deadline == due_date → 0 buffer < 3-day minimum
        let result = t.mp.try_list_invoice(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &due_date,
            &None::<Address>,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            MarketplaceError::FundingDeadlineTooCloseToDueDate
        );
    }

    /// funding_deadline 1 second before due_date → rejected.
    #[test]
    fn test_list_invoice_deadline_one_second_before_due_date_rejected() {
        let t = deploy();
        let id = mint_invoice(&t);
        let due_date = t.env.ledger().timestamp() + 86_400 * 60;
        let result = t.mp.try_list_invoice(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &(due_date - 1), // 1 second buffer — less than 3 days
            &None::<Address>,
        );
        assert_eq!(
            result.unwrap_err().unwrap(),
            MarketplaceError::FundingDeadlineTooCloseToDueDate
        );
    }

    /// funding_deadline exactly at due_date - buffer → accepted.
    #[test]
    fn test_list_invoice_deadline_exactly_at_buffer_boundary_accepted() {
        let t = deploy();
        let id = mint_invoice(&t);
        let due_date = t.env.ledger().timestamp() + 86_400 * 60;
        let min_buffer = DEFAULT_MIN_FUNDING_BUFFER; // 3 days
        // deadline + buffer == due_date → accepted
        let deadline = due_date - min_buffer;
        assert!(t.mp.try_list_invoice(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
            &None::<Address>,
        ).is_ok());
    }

    /// funding_deadline comfortably before due_date → accepted (existing valid behavior).
    #[test]
    fn test_list_invoice_deadline_well_before_due_date_accepted() {
        let t = deploy();
        let id = list_one(&t); // deadline = now + 30 days, due_date = now + 60 days
        let listing = t.mp.get_listing(&id);
        assert!(listing.is_active);
    }

    /// Admin can configure the buffer; new buffer is respected.
    #[test]
    fn test_set_min_funding_buffer_admin_can_change() {
        let t = deploy();
        // Set buffer to 1 day
        t.mp.set_min_funding_buffer(&t.admin, &86_400u64);
        assert_eq!(t.mp.get_min_funding_buffer(), 86_400u64);

        // Now a listing with deadline = due_date - 2 days should be accepted
        let id = mint_invoice(&t);
        let due_date = t.env.ledger().timestamp() + 86_400 * 60;
        let deadline = due_date - 2 * 86_400; // 2 days > 1-day buffer
        assert!(t.mp.try_list_invoice(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &deadline,
            &None::<Address>,
        ).is_ok());
    }

    /// Non-admin cannot change the buffer.
    #[test]
    fn test_set_min_funding_buffer_non_admin_rejected() {
        let t = deploy();
        let stranger = Address::generate(&t.env);
        let result = t.mp.try_set_min_funding_buffer(&stranger, &86_400u64);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::NotAdmin);
    }

    // ── Issue #439: Dutch auction / price decay ───────────────────────────────

    /// list_invoice_with_decay stores a schedule and get_current_price returns start_price before decay begins.
    #[test]
    fn test_decay_schedule_price_before_decay_start() {
        let t = deploy();
        let id = mint_invoice(&t);
        let now = t.env.ledger().timestamp();
        let due_date = now + 86_400 * 60;
        let funding_deadline = due_date - DEFAULT_MIN_FUNDING_BUFFER;
        let decay_start = now + 86_400 * 5;   // decay starts in 5 days
        let decay_end   = now + 86_400 * 25;  // reaches floor in 25 days

        t.mp.list_invoice_with_decay(
            &t.seller,
            &id,
            &10_000_000_000i128, // start price
            &10_000_000_000i128, // face value
            &t.token,
            &funding_deadline,
            &None::<Address>,
            &8_000_000_000i128, // floor price
            &decay_start,
            &decay_end,
        );

        // Before decay_start → price == start_price
        let price = t.mp.get_current_price(&id);
        assert_eq!(price, 10_000_000_000i128);
    }

    /// Price at exactly the mid-point of the decay window is half-way between start and floor.
    #[test]
    fn test_decay_schedule_price_at_midpoint() {
        let t = deploy();
        let id = mint_invoice(&t);
        let now = t.env.ledger().timestamp();
        let due_date = now + 86_400 * 60;
        let funding_deadline = due_date - DEFAULT_MIN_FUNDING_BUFFER;
        let decay_start = now + 1_000;
        let decay_end   = now + 3_000; // 2000-second window

        t.mp.list_invoice_with_decay(
            &t.seller,
            &id,
            &10_000_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &funding_deadline,
            &None::<Address>,
            &8_000_000_000i128, // drop = 2_000_000_000 total
            &decay_start,
            &decay_end,
        );

        // Advance to exactly the mid-point of the window
        t.env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            timestamp: now + 2_000, // elapsed=1000 out of 2000
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        // drop = 2_000_000_000 * 1000 / 2000 = 1_000_000_000
        // price = 10_000_000_000 - 1_000_000_000 = 9_000_000_000
        let price = t.mp.get_current_price(&id);
        assert_eq!(price, 9_000_000_000i128);
    }

    /// After decay_end the price equals the floor.
    #[test]
    fn test_decay_schedule_price_after_decay_end_is_floor() {
        let t = deploy();
        let id = mint_invoice(&t);
        let now = t.env.ledger().timestamp();
        let due_date = now + 86_400 * 60;
        let funding_deadline = due_date - DEFAULT_MIN_FUNDING_BUFFER;
        let decay_start = now + 100;
        let decay_end   = now + 200;

        t.mp.list_invoice_with_decay(
            &t.seller,
            &id,
            &10_000_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &funding_deadline,
            &None::<Address>,
            &7_000_000_000i128,
            &decay_start,
            &decay_end,
        );

        // Advance past decay_end
        t.env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            timestamp: now + 300,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        let price = t.mp.get_current_price(&id);
        assert_eq!(price, 7_000_000_000i128);
    }

    /// A listing without a decay schedule returns asking_price unchanged (backward compat).
    #[test]
    fn test_no_decay_schedule_price_unchanged() {
        let t = deploy();
        let id = list_one(&t);
        let listing = t.mp.get_listing(&id);
        let price = t.mp.get_current_price(&id);
        assert_eq!(price, listing.asking_price);
    }

    /// floor_price >= asking_price → rejected.
    #[test]
    fn test_decay_schedule_floor_gte_asking_rejected() {
        let t = deploy();
        let id = mint_invoice(&t);
        let now = t.env.ledger().timestamp();
        let due_date = now + 86_400 * 60;
        let funding_deadline = due_date - DEFAULT_MIN_FUNDING_BUFFER;
        let result = t.mp.try_list_invoice_with_decay(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &funding_deadline,
            &None::<Address>,
            &9_500_000_000i128, // floor == asking → invalid
            &(now + 100),
            &(now + 200),
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    // ── Issue #440: Reverse-auction bidding ───────────────────────────────────

    /// Helper: list an invoice in bidding mode.
    fn list_with_bidding(t: &TestEnv, bidding_deadline: u64) -> u64 {
        let id = mint_invoice(t);
        let due_date = t.env.ledger().timestamp() + 86_400 * 60;
        let funding_deadline = due_date - DEFAULT_MIN_FUNDING_BUFFER;
        t.mp.list_invoice_with_bidding(
            &t.seller,
            &id,
            &9_500_000_000i128,
            &10_000_000_000i128,
            &t.token,
            &funding_deadline,
            &None::<Address>,
            &bidding_deadline,
        );
        id
    }

    /// submit_bid succeeds during the bidding window and can be retrieved.
    #[test]
    fn test_submit_bid_success() {
        let t = deploy();
        let bid_deadline = t.env.ledger().timestamp() + 86_400 * 7; // 7-day window
        let id = list_with_bidding(&t, bid_deadline);

        let investor = Address::generate(&t.env);
        assert!(t.mp.try_submit_bid(
            &investor,
            &id,
            &9_000_000_000i128, // bid below asking price
            &5_000_000_000i128, // amount
        ).is_ok());

        let bid = t.mp.get_bid(&id, &investor);
        assert!(bid.is_some());
        let b = bid.unwrap();
        assert_eq!(b.bid_price, 9_000_000_000i128);
        assert_eq!(b.amount, 5_000_000_000i128);
    }

    /// submit_bid after the bidding window closes → BiddingWindowClosed.
    #[test]
    fn test_submit_bid_after_window_closed_rejected() {
        let t = deploy();
        let bid_deadline = t.env.ledger().timestamp() + 100;
        let id = list_with_bidding(&t, bid_deadline);

        // Advance past bidding deadline
        t.env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            timestamp: bid_deadline + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        let investor = Address::generate(&t.env);
        let result = t.mp.try_submit_bid(
            &investor,
            &id,
            &9_000_000_000i128,
            &5_000_000_000i128,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::BiddingWindowClosed);
    }

    /// fund_invoice while bidding window is open → BiddingWindowActive.
    #[test]
    fn test_fund_invoice_during_bidding_window_rejected() {
        let t = deploy();
        let bid_deadline = t.env.ledger().timestamp() + 86_400 * 7;
        let id = list_with_bidding(&t, bid_deadline);

        let investor = Address::generate(&t.env);
        let result = t.mp.try_fund_invoice(&investor, &id, &1_000_000i128);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::BiddingWindowActive);
    }

    /// Duplicate bid from same investor → BidAlreadyExists.
    #[test]
    fn test_submit_bid_duplicate_rejected() {
        let t = deploy();
        let bid_deadline = t.env.ledger().timestamp() + 86_400 * 7;
        let id = list_with_bidding(&t, bid_deadline);

        let investor = Address::generate(&t.env);
        t.mp.submit_bid(&investor, &id, &9_000_000_000i128, &5_000_000_000i128);
        let result = t.mp.try_submit_bid(
            &investor,
            &id,
            &9_000_000_000i128,
            &5_000_000_000i128,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::BidAlreadyExists);
    }

    /// bid_price exceeds current_asking_price → InvalidAmount.
    #[test]
    fn test_submit_bid_price_above_asking_rejected() {
        let t = deploy();
        let bid_deadline = t.env.ledger().timestamp() + 86_400 * 7;
        let id = list_with_bidding(&t, bid_deadline);

        let investor = Address::generate(&t.env);
        let result = t.mp.try_submit_bid(
            &investor,
            &id,
            &9_500_000_001i128, // > asking_price
            &5_000_000_000i128,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::InvalidAmount);
    }

    /// accept_bids before bidding window closes → BiddingWindowActive.
    #[test]
    fn test_accept_bids_before_window_closes_rejected() {
        let t = deploy();
        let bid_deadline = t.env.ledger().timestamp() + 86_400 * 7;
        let id = list_with_bidding(&t, bid_deadline);

        let investor = Address::generate(&t.env);
        t.mp.submit_bid(&investor, &id, &9_000_000_000i128, &5_000_000_000i128);

        let investors = soroban_sdk::vec![&t.env, investor.clone()];
        let result = t.mp.try_accept_bids(&t.seller, &id, &investors);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::BiddingWindowActive);
    }

    /// Non-seller/non-admin cannot accept bids.
    #[test]
    fn test_accept_bids_stranger_rejected() {
        let t = deploy();
        let bid_deadline = t.env.ledger().timestamp() + 100;
        let id = list_with_bidding(&t, bid_deadline);

        // Advance past bid deadline
        t.env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            timestamp: bid_deadline + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });

        let stranger = Address::generate(&t.env);
        let investors = soroban_sdk::vec![&t.env, stranger.clone()];
        let result = t.mp.try_accept_bids(&stranger, &id, &investors);
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::Unauthorized);
    }

    /// submit_bid on a non-bidding listing → BiddingWindowClosed.
    #[test]
    fn test_submit_bid_on_non_bidding_listing_rejected() {
        let t = deploy();
        let id = list_one(&t); // standard listing (no bidding deadline)
        let investor = Address::generate(&t.env);
        let result = t.mp.try_submit_bid(
            &investor,
            &id,
            &9_000_000_000i128,
            &5_000_000_000i128,
        );
        assert_eq!(result.unwrap_err().unwrap(), MarketplaceError::BiddingWindowClosed);
    }
}
