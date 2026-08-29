//! Deploys the full Kora protocol into a fresh Soroban test env and exposes it
//! to the fuzz targets, mirroring the wiring in `contracts/tests/src/lib.rs`.
//!
//! Every target builds a `Protocol` per fuzz input, so each input starts from
//! clean state. The address pool ([`Protocol::actor`]) holds the known protocol
//! actors and every deployed contract id; targets select a caller with a byte,
//! so auth-gated entry points have a real chance of hitting the authorised path
//! instead of always failing the `require_auth` check.

use soroban_sdk::testutils::{Address as _, EnvTestConfig, Ledger, LedgerInfo};
use soroban_sdk::{token::StellarAssetClient, Address, Env};

use kora_access_control::{AccessControlContract, AccessControlContractClient};
use kora_financing_pool::{FinancingPoolContract, FinancingPoolContractClient};
use kora_invoice_nft::{InvoiceNftContract, InvoiceNftContractClient};
use kora_marketplace::{MarketplaceContract, MarketplaceContractClient};
use kora_price_oracle::{PriceOracleContract, PriceOracleContractClient};
use kora_risk_registry::{RiskRegistryContract, RiskRegistryContractClient};
use kora_treasury::{TreasuryContract, TreasuryContractClient};

pub struct Protocol<'a> {
    pub env: Env,
    pub access_control: AccessControlContractClient<'a>,
    pub invoice_nft: InvoiceNftContractClient<'a>,
    pub marketplace: MarketplaceContractClient<'a>,
    pub pool: FinancingPoolContractClient<'a>,
    pub treasury: TreasuryContractClient<'a>,
    pub risk_registry: RiskRegistryContractClient<'a>,
    pub price_oracle: PriceOracleContractClient<'a>,
    pub token: Address,
    actors: std::vec::Vec<Address>,
}

impl Protocol<'static> {
    /// Deploy and wire every contract. Best-effort: the individual `initialize`
    /// calls use `try_*` so a config combination that a future contract change
    /// rejects does not poison the whole target.
    pub fn deploy() -> Protocol<'static> {
        let mut env = Env::default();
        // Fuzzing deploys a fresh Env per input; without this every iteration
        // writes a test-snapshot JSON to disk on drop.
        env.set_config(EnvTestConfig {
            capture_snapshot_at_drop: false,
        });
        // risk_registry::add_verifier does a nested token transfer that needs
        // non-root auth, same as the integration harness.
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
        let sme = Address::generate(&env);
        let investor = Address::generate(&env);
        let investor2 = Address::generate(&env);
        let outsider = Address::generate(&env);

        let ac_id = env.register_contract(None, AccessControlContract);
        let nft_id = env.register_contract(None, InvoiceNftContract);
        let mp_id = env.register_contract(None, MarketplaceContract);
        let pool_id = env.register_contract(None, FinancingPoolContract);
        let treasury_id = env.register_contract(None, TreasuryContract);
        let rr_id = env.register_contract(None, RiskRegistryContract);
        let oracle_id = env.register_contract(None, PriceOracleContract);

        let access_control = AccessControlContractClient::new(&env, &ac_id);
        let invoice_nft = InvoiceNftContractClient::new(&env, &nft_id);
        let marketplace = MarketplaceContractClient::new(&env, &mp_id);
        let pool = FinancingPoolContractClient::new(&env, &pool_id);
        let treasury = TreasuryContractClient::new(&env, &treasury_id);
        let risk_registry = RiskRegistryContractClient::new(&env, &rr_id);
        let price_oracle = PriceOracleContractClient::new(&env, &oracle_id);

        let token_admin = Address::generate(&env);
        let token = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        let sac = StellarAssetClient::new(&env, &token);
        for a in [&admin, &sme, &investor, &investor2, &outsider] {
            sac.mint(a, &1_000_000_000_000i128);
        }

        let _ = access_control.try_initialize(&admin);
        let _ = invoice_nft.try_initialize(&admin, &ac_id);
        let _ = invoice_nft.try_set_risk_registry(&admin, &rr_id);
        let _ = price_oracle.try_initialize(&admin);
        let _ = treasury.try_initialize(&admin, &50u32);
        let _ = treasury.try_whitelist_token(&admin, &token);
        let _ = risk_registry.try_initialize(&admin, &nft_id, &token, &1_000_000i128, &5_000u32);
        let _ = pool.try_initialize(
            &admin, &nft_id, &rr_id, &treasury_id, &ac_id, &200u32, &oracle_id, &10_000u32,
        );
        let _ = marketplace.try_initialize(
            &admin, &nft_id, &pool_id, &treasury_id, &ac_id, &rr_id, &50u32,
        );
        let _ = marketplace.try_whitelist_token(&admin, &token);
        let _ = invoice_nft.try_set_authorized_callers(&admin, &mp_id, &pool_id);
        let _ = price_oracle.try_set_price(
            &admin,
            &soroban_sdk::Symbol::new(&env, "USDC"),
            &soroban_sdk::Symbol::new(&env, "USDC"),
            &10_000_000i128,
        );

        let actors = std::vec![
            admin,
            sme,
            investor,
            investor2,
            outsider,
            token.clone(),
            ac_id,
            nft_id,
            mp_id,
            pool_id,
            treasury_id,
            rr_id,
            oracle_id,
        ];

        return Protocol {
            env,
            access_control,
            invoice_nft,
            marketplace,
            pool,
            treasury,
            risk_registry,
            price_oracle,
            token,
            actors,
        };
    }
}

impl<'a> Protocol<'a> {
    /// Resolve an actor-selector byte to one of the known addresses.
    pub fn actor(&self, sel: u8) -> Address {
        return self.actors[sel as usize % self.actors.len()].clone();
    }

    /// Mint an invoice owned by `sme` and return its id, so listing/funding
    /// targets have something to act on. Best-effort; returns `None` on any
    /// contract error.
    pub fn seed_invoice(&self, sme: &Address) -> Option<u64> {
        let env = &self.env;
        let verifier = self.actor(0);
        let _ = self
            .risk_registry
            .try_add_verifier(&self.actor(0), &verifier, &1_000_000i128);
        let _ = self
            .risk_registry
            .try_register_sme(&verifier, sme, &30u32, &true);
        let res = self.invoice_nft.try_mint_invoice(
            sme,
            &crate::gen::bytes32(env, 0xAB),
            &10_000_000_000i128,
            &soroban_sdk::Symbol::new(env, "USDC"),
            &(env.ledger().timestamp() + 86_400 * 60),
            &crate::gen::text(env, 1),
            &30u32,
            &None,
        );
        return res.ok().and_then(|r| r.ok());
    }
}
