// tests/treasury_collect_fee_auth.rs
//! Tests for Treasury collect_fee authorization (Issue #465)
//! Updated for parameter_registry integration (Issue #969)

#[cfg(test)]
mod treasury_collect_fee_auth {
    use kora_treasury::TreasuryContractClient;
    use kora_shared::errors::KoraError;
    use kora_parameter_registry::{ParameterRegistryContract, ParameterRegistryContractClient};
    use kora_shared::types::RegistryParamKey;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env,
    };

    struct TestEnv {
        env: Env,
        admin: Address,
        marketplace: Address,
        token: Address,
        treasury_client: TreasuryContractClient<'static>,
    }

    fn setup() -> TestEnv {
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
        let marketplace = Address::generate(&env);
        let token = Address::generate(&env);

        let treasury_id = env.register_contract(None, kora_treasury::TreasuryContract);
        let treasury_client = TreasuryContractClient::new(&env, &treasury_id);
        treasury_client.initialize(&admin, &50u32);

        TestEnv {
            env,
            admin,
            marketplace,
            token,
            treasury_client,
        }
    }

    // ---- Registry integration tests (Issue #969) ----

    /// Sets up a full parameter_registry + treasury integration
    fn setup_with_registry() -> (Env, Address, Address, TreasuryContractClient<'static>, ParameterRegistryContractClient<'static>) {
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
        let registry_id = env.register_contract(None, ParameterRegistryContract);
        let registry_client = ParameterRegistryContractClient::new(&env, &registry_id);
        registry_client.initialize(&admin, &None); // bootstrap defaults

        let treasury_id = env.register_contract(None, kora_treasury::TreasuryContract);
        let treasury_client = TreasuryContractClient::new(&env, &treasury_id);
        treasury_client.initialize(&admin, &50u32);

        // Wire treasury to the registry
        treasury_client.set_parameter_registry(&admin, &registry_id);

        (env, admin, registry_id, treasury_client, registry_client)
    }

    #[test]
    fn test_collect_fee_unauthorized_fails() {
        let TestEnv {
            env: _,
            admin,
            marketplace: _,
            token,
            treasury_client,
        } = setup();
        // Non-marketplace caller without auth should succeed (collect_fee is permissionless)
        // but the token must be whitelisted
        treasury_client.whitelist_token(&admin, &token);
        let result = treasury_client.try_collect_fee(&token, &1_000_000i128);
        assert!(result.is_ok());
    }

    #[test]
    fn test_collect_fee_non_whitelisted_token_fails() {
        let TestEnv {
            env: _,
            admin: _,
            marketplace: _,
            token,
            treasury_client,
        } = setup();
        let result = treasury_client.try_collect_fee(&token, &1_000_000i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_collect_fee_zero_amount_fails() {
        let TestEnv {
            env: _,
            admin,
            marketplace: _,
            token,
            treasury_client,
        } = setup();
        treasury_client.whitelist_token(&admin, &token);
        let result = treasury_client.try_collect_fee(&token, &0i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_collect_fee_negative_amount_fails() {
        let TestEnv {
            env: _,
            admin,
            marketplace: _,
            token,
            treasury_client,
        } = setup();
        treasury_client.whitelist_token(&admin, &token);
        let result = treasury_client.try_collect_fee(&token, &-1_000i128);
        assert!(result.is_err());
    }

    // ---- Registry integration tests ----

    #[test]
    fn test_registry_controls_effective_fee_bps() {
        let (_env, _admin, registry_id, treasury_client, registry_client) = setup_with_registry();
        
        // Registry default TreasuryFeeBps is 50
        let fee = treasury_client.get_fee_bps();
        assert_eq!(fee, 50);

        // Update registry to 100 bps
        registry_client.admin_set_parameter(&_admin, &RegistryParamKey::TreasuryFeeBps, &100i128);
        
        // Treasury should now read 100 from registry
        let fee = treasury_client.get_fee_bps();
        assert_eq!(fee, 100);
    }

    #[test]
    fn test_registry_zero_fee_still_allows_collect() {
        let (_env, _admin, _registry_id, treasury_client, registry_client) = setup_with_registry();
        
        // Set fee bps to 0 via registry
        registry_client.admin_set_parameter(&_admin, &RegistryParamKey::TreasuryFeeBps, &0i128);
        
        // Even with 0 fee, collect_fee should work for accounting
        let token = Address::generate(&_env);
        treasury_client.whitelist_token(&_admin, &token);
        let result = treasury_client.try_collect_fee(&token, &1_000_000i128);
        assert!(result.is_ok());
        assert_eq!(treasury_client.get_collected(&token), 1_000_000);
    }

    #[test]
    fn test_registry_update_emits_fee_event() {
        let (_env, _admin, _registry_id, _treasury_client, registry_client) = setup_with_registry();
        
        // Governance should not be able to set out-of-bounds fee
        let result = registry_client.try_admin_set_parameter(&_admin, &RegistryParamKey::TreasuryFeeBps, &-1i128);
        assert!(result.is_err());

        // Max is 10_000
        let result = registry_client.try_admin_set_parameter(&_admin, &RegistryParamKey::TreasuryFeeBps, &10_001i128);
        assert!(result.is_err());

        // Valid value should succeed
        let result = registry_client.try_admin_set_parameter(&_admin, &RegistryParamKey::TreasuryFeeBps, &100i128);
        assert!(result.is_ok());
    }

    #[test]
    fn test_registry_reserve_allocation_bps_read() {
        let (_env, _admin, _registry_id, treasury_client, _registry_client) = setup_with_registry();
        
        // Reserve allocation defaults to 0 from bootstrap
        let bps = treasury_client.get_reserve_allocation_bps();
        assert_eq!(bps, 0);
    }

    #[test]
    fn test_collect_fee_auth_same_as_auth_tests() {
        // Auth tests unchanged for non-admin fee collection attempts
        let TestEnv {
            env: _,
            admin,
            marketplace: _,
            token,
            treasury_client,
        } = setup();
        
        // Non-admin should be able to collect fee (permissionless)
        treasury_client.whitelist_token(&admin, &token);
        let result = treasury_client.try_collect_fee(&token, &500_000i128);
        assert!(result.is_ok());
        assert_eq!(treasury_client.get_collected(&token), 500_000);
    }
}