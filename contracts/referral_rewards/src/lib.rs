#![no_std]

//! Single-level, first-invoice referral rewards backed by an isolated token balance.
use kora_invoice_nft::InvoiceNftContractClient;
use kora_shared::types::InvoiceStatus;
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ReferralError {
    AlreadyInitialized = 1,
    InvalidConfiguration = 2,
    NotInitialized = 3,
    NotAdmin = 4,
    InvalidAmount = 5,
    AllocationExceeded = 6,
    DepthExceeded = 7,
}

#[contracttype]
enum Key {
    Admin,
    InvoiceNft,
    Token,
    Bonus,
    Allocation,
    Funded,
    Paid,
    MaxDepth,
    Depth(Address),
    Claimed(Address),
}

#[contract]
pub struct ReferralRewards;

#[contractimpl]
impl ReferralRewards {
    /// The treasury admin withdraws an approved allocation and deposits it here.
    /// Allocation and fixed bonus are immutable; no claim can spend beyond them.
    pub fn initialize(
        env: Env, admin: Address, invoice_nft: Address, token: Address,
        bonus: i128, allocation: i128,
    ) -> Result<(), ReferralError> {
        if env.storage().instance().has(&Key::Admin) {
            return Err(ReferralError::AlreadyInitialized);
        }
        if bonus <= 0 || allocation <= 0 || bonus > allocation
            || admin == env.current_contract_address()
            || invoice_nft == env.current_contract_address()
            || token == env.current_contract_address()
            || admin == invoice_nft || admin == token || invoice_nft == token
        {
            return Err(ReferralError::InvalidConfiguration);
        }
        env.storage().instance().set(&Key::Admin, &admin);
        env.storage().instance().set(&Key::InvoiceNft, &invoice_nft);
        env.storage().instance().set(&Key::Token, &token);
        env.storage().instance().set(&Key::Bonus, &bonus);
        env.storage().instance().set(&Key::Allocation, &allocation);
        env.storage().instance().set(&Key::Funded, &0i128);
        env.storage().instance().set(&Key::Paid, &0i128);
        // Default anti-sybil graph depth limit; admin may tighten via set_max_depth.
        env.storage().instance().set(&Key::MaxDepth, &1u32);
        Ok(())
    }

    /// Admin sets the maximum referral graph depth (>= 1). Fails closed on bad input.
    pub fn set_max_depth(env: Env, admin: Address, max_depth: u32) -> Result<(), ReferralError> {
        admin.require_auth();
        let configured: Address = env.storage().instance().get(&Key::Admin)
            .ok_or(ReferralError::NotInitialized)?;
        if admin != configured { return Err(ReferralError::NotAdmin); }
        if max_depth == 0 { return Err(ReferralError::InvalidConfiguration); }
        env.storage().instance().set(&Key::MaxDepth, &max_depth);
        Ok(())
    }

    pub fn max_depth(env: Env) -> u32 {
        env.storage().instance().get(&Key::MaxDepth).unwrap_or(0)
    }

    /// Fund the dedicated reward balance, up to the configured total allocation.
    pub fn deposit(env: Env, admin: Address, amount: i128) -> Result<(), ReferralError> {
        admin.require_auth();
        let configured: Address = env.storage().instance().get(&Key::Admin)
            .ok_or(ReferralError::NotInitialized)?;
        if admin != configured { return Err(ReferralError::NotAdmin); }
        if amount <= 0 { return Err(ReferralError::InvalidAmount); }
        let funded: i128 = env.storage().instance().get(&Key::Funded).unwrap_or(0);
        let allocation: i128 = env.storage().instance().get(&Key::Allocation).unwrap();
        if funded.checked_add(amount).map_or(true, |n| n > allocation) {
            return Err(ReferralError::AllocationExceeded);
        }
        let token_addr: Address = env.storage().instance().get(&Key::Token).unwrap();
        token::Client::new(&env, &token_addr).transfer(&admin, &env.current_contract_address(), &amount);
        env.storage().instance().set(&Key::Funded, &(funded + amount));
        Ok(())
    }

    /// Anyone can settle an eligible first-invoice referral after `set_repaid`.
    /// Returns false for ineligible, duplicate, unfunded, or exhausted claims.
    /// Enforces the anti-sybil referral graph depth limit; a referrer already at
    /// the max depth cannot accrue further referrals (fails closed).
    pub fn claim_referral_reward(env: Env, sme: Address) -> Result<bool, ReferralError> {
        let nft_addr: Address = env.storage().instance().get(&Key::InvoiceNft)
            .ok_or(ReferralError::NotInitialized)?;
        let max_depth: u32 = env.storage().instance().get(&Key::MaxDepth)
            .ok_or(ReferralError::NotInitialized)?;
        if max_depth == 0 { return Err(ReferralError::InvalidConfiguration); }
        let key = Key::Claimed(sme.clone());
        if env.storage().persistent().has(&key) { return Ok(false); }
        let nft = InvoiceNftContractClient::new(&env, &nft_addr);
        let referrer = match nft.get_sme_referrer(&sme) {
            Some(address) if address != sme => address,
            _ => return Ok(false),
        };
        let first = match nft.get_first_mint(&sme) {
            Some(id) => id,
            None => return Ok(false),
        };
        match nft.try_get_invoice(&first) {
            Ok(Ok(invoice)) if invoice.sme == sme && invoice.status == InvoiceStatus::Repaid => {},
            _ => return Ok(false),
        }
        // Anti-sybil: the referrer's own depth must be below the configured limit.
        let referrer_depth: u32 = env.storage().persistent()
            .get(&Key::Depth(referrer.clone())).unwrap_or(0);
        if referrer_depth >= max_depth { return Ok(false); }
        let bonus: i128 = env.storage().instance().get(&Key::Bonus).unwrap();
        let funded: i128 = env.storage().instance().get(&Key::Funded).unwrap();
        let paid: i128 = env.storage().instance().get(&Key::Paid).unwrap();
        if funded.saturating_sub(paid) < bonus { return Ok(false); }
        let token_addr: Address = env.storage().instance().get(&Key::Token).unwrap();
        let token_client = token::Client::new(&env, &token_addr);
        if token_client.balance(&env.current_contract_address()) < bonus { return Ok(false); }
        // Claim is recorded before token transfer; a failed transfer rolls back both writes.
        env.storage().persistent().set(&key, &true);
        env.storage().persistent().extend_ttl(&key, 518_400, 518_400);
        // Record the sme's depth in the referral graph (referrer depth + 1).
        let sme_depth = referrer_depth.saturating_add(1);
        let depth_key = Key::Depth(sme.clone());
        env.storage().persistent().set(&depth_key, &sme_depth);
        env.storage().persistent().extend_ttl(&depth_key, 518_400, 518_400);
        env.storage().instance().set(&Key::Paid, &(paid + bonus));
        token_client.transfer(&env.current_contract_address(), &referrer, &bonus);
        env.events().publish((symbol_short!("referral"), sme), (referrer, first, bonus));
        Ok(true)
    }

    pub fn has_claimed(env: Env, sme: Address) -> bool {
        env.storage().persistent().has(&Key::Claimed(sme))
    }

    pub fn depth_of(env: Env, sme: Address) -> u32 {
        env.storage().persistent().get(&Key::Depth(sme)).unwrap_or(0)
    }

    pub fn paid_total(env: Env) -> i128 {
        env.storage().instance().get(&Key::Paid).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient, InvoiceNftError};
    use soroban_sdk::{testutils::{Address as _, Ledger, LedgerInfo}, Bytes, String, Symbol};

    fn setup() -> (Env, InvoiceNftContractClient<'static>, ReferralRewardsClient<'static>,
        Address, Address, Address, Address, Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000, protocol_version: 21, sequence_number: 1,
            network_id: Default::default(), base_reserve: 10,
            min_temp_entry_ttl: 1000, min_persistent_entry_ttl: 1000,
            max_entry_ttl: 600_000,
        });
        let admin = Address::generate(&env);
        let ac = env.register_contract(None, kora_access_control::AccessControlContract);
        let market = Address::generate(&env);
        let pool = Address::generate(&env);
        let sme = Address::generate(&env);
        let referrer = Address::generate(&env);
        let nft_addr = env.register_contract(None, InvoiceNftContract);
        let nft = InvoiceNftContractClient::new(&env, &nft_addr);
        nft.initialize(&admin, &ac);
        nft.set_authorized_callers(&admin, &market, &pool);
        let token_admin = Address::generate(&env);
        let token_addr = env.register_stellar_asset_contract_v2(token_admin).address();
        soroban_sdk::token::StellarAssetClient::new(&env, &token_addr).mint(&admin, &100);
        let rewards_addr = env.register_contract(None, ReferralRewards);
        let rewards = ReferralRewardsClient::new(&env, &rewards_addr);
        rewards.initialize(&admin, &nft_addr, &token_addr, &10, &20);
        (env, nft, rewards, admin, market, pool, sme, referrer)
    }

    fn mint(env: &Env, nft: &InvoiceNftContractClient, sme: &Address,
        referrer: Option<Address>) -> u64 {
        let hash = Bytes::from_slice(env, &[1; 32]);
        let cid = String::from_str(env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
        nft.mint_invoice_with_referrer(sme, &hash, &1_000_000,
            &Symbol::new(env, "USDC"), &(env.ledger().timestamp() + 86_400),
            &cid, &30, &None, &referrer)
    }

    #[test]
    fn rejects_self_referral_and_locks_first_mint_attribution() {
        let (env, nft, _, _, _, _, sme, referrer) = setup();
        let bad = nft.try_mint_invoice_with_referrer(&sme,
            &Bytes::from_slice(&env, &[1; 32]), &1_000_000,
            &Symbol::new(&env, "USDC"), &(env.ledger().timestamp() + 86_400),
            &String::from_str(&env, "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
            &30, &None, &Some(sme.clone()));
        assert_eq!(bad, Err(Ok(InvoiceNftError::SelfReferral)));
        let _ = referrer;
    }

    #[test]
    fn default_max_depth_is_one() {
        let (_env, _nft, rewards, _, _, _, _, _) = setup();
        assert_eq!(rewards.max_depth(), 1);
    }

    #[test]
    fn set_max_depth_rejects_zero_and_requires_admin() {
        let (env, _nft, rewards, admin, _, _, _, _) = setup();
        assert_eq!(rewards.try_set_max_depth(&admin, &0), Err(Ok(ReferralError::InvalidConfiguration)));
        let stranger = Address::generate(&env);
        assert_eq!(rewards.try_set_max_depth(&stranger, &3), Err(Ok(ReferralError::NotAdmin)));
        rewards.set_max_depth(&admin, &3);
        assert_eq!(rewards.max_depth(), 3);
    }

    #[test]
    fn depth_limit_blocks_deep_sybil_chain() {
        let (env, nft, rewards, admin, _, _, sme, referrer) = setup();
        rewards.deposit(&admin, &20);
        // referrer (depth 0) refers sme -> sme depth becomes 1.
        let id = mint(&env, &nft, &sme, Some(referrer.clone()));
        nft.set_repaid(&admin, &id);
        assert_eq!(rewards.claim_referral_reward(&sme), Ok(true));
        assert_eq!(rewards.depth_of(&sme), 1);
        // sme is now at max depth (1); a downstream referral must be blocked.
        let leaf = Address::generate(&env);
        let leaf_id = mint(&env, &nft, &leaf, Some(sme.clone()));
        nft.set_repaid(&admin, &leaf_id);
        assert_eq!(rewards.claim_referral_reward(&leaf), Ok(false));
        assert!(!rewards.has_claimed(&leaf));
    }

    #[test]
    fn claim_fails_closed_when_uninitialized() {
        let env = Env::default();
        env.mock_all_auths();
        let rewards_addr = env.register_contract(None, ReferralRewards);
        let rewards = ReferralRewardsClient::new(&env, &rewards_addr);
        let sme = Address::generate(&env);
        assert_eq!(rewards.try_claim_referral_reward(&sme), Err(Ok(ReferralError::NotInitialized)));
    }
}
