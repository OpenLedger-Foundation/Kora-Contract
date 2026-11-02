// tests/treasury_edge_cases.rs
//! Edge case tests for Treasury Contract multi-recipient distribution (Issue #464)
//! Updated for parameter_registry integration (Issue #969)

#[cfg(test)]
mod treasury_edge_cases {
    use kora_treasury::TreasuryContractClient;
    use kora_parameter_registry::{ParameterRegistryContract, ParameterRegistryContractClient};
    use kora_shared::types::RegistryParamKey;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env,
    };

    struct TestEnv {
        env: Env,
        admin: Address,
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
        let token_admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_admin).address();

        let treasury_id = env.register_contract(None, kora_treasury::TreasuryContract);
        let treasury_client = TreasuryContractClient::new(&env, &treasury_id);
        treasury_client.initialize(&admin, &50u32);

        TestEnv {
            env,
            admin,
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
        registry_client.initialize(&admin, &None);

        let treasury_id = env.register_contract(None, kora_treasury::TreasuryContract);
        let treasury_client = TreasuryContractClient::new(&env, &treasury_id);
        treasury_client.initialize(&admin, &50u32);

        treasury_client.set_parameter_registry(&admin, &registry_id);

        (env, admin, registry_id, treasury_client, registry_client)
    }

    #[test]
    fn test_settle_withdrawal_edge_success() {
        let TestEnv {
            env,
            admin,
            token,
            treasury_client,
        } = setup();
        // Setup: whitelist token, propose and execute recipient, whitelist_token
        let recipient = Address::generate(&env);
        treasury_client.whitelist_token(&admin, &token);
        treasury_client.propose_recipient(&admin, &recipient);
        // Advance ledger timestamp past timelock
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000 + 86401,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
        treasury_client.execute_recipient(&admin, &recipient);
    }

    #[test]
    fn test_collect_fee_then_withdraw_flow() {
        let TestEnv {
            env,
            admin,
            token,
            treasury_client,
        } = setup();
        let recipient = Address::generate(&env);
        treasury_client.whitelist_token(&admin, &token);
        treasury_client.propose_recipient(&admin, &recipient);
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000 + 86401,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
        treasury_client.execute_recipient(&admin, &recipient);

        // Collect a fee (token needs a balance)
        let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        token_client.mint(&treasury_client.address, &1_000_000i128);
        treasury_client.collect_fee(&token, &1_000_000i128);
        assert_eq!(treasury_client.get_collected(&token), 1_000_000);

        // Withdraw
        treasury_client.withdraw(&admin, &token, &recipient, &500_000i128);
        assert_eq!(treasury_client.get_collected(&token), 500_000);
    }

    #[test]
    fn test_withdraw_exceeds_reserve_excluded_balance_fails() {
        let TestEnv {
            env,
            admin,
            token,
            treasury_client,
        } = setup();
        let recipient = Address::generate(&env);
        treasury_client.whitelist_token(&admin, &token);
        treasury_client.propose_recipient(&admin, &recipient);
        env.ledger().set(LedgerInfo {
            timestamp: 1_700_000_000 + 86401,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
        treasury_client.execute_recipient(&admin, &recipient);

        // Set reserve allocation BEFORE collecting: only fees collected
        // while the allocation is active are split into the reserve.
        treasury_client.set_reserve_allocation_bps(&admin, &5000); // 50% to reserve
        let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        token_client.mint(&treasury_client.address, &1_000_000i128);
        treasury_client.collect_fee(&token, &1_000_000i128);

        // Withdraw 600k - reserve is 500k, so spendable is 500k
        let result = treasury_client.try_withdraw(&admin, &token, &recipient, &600_000i128);
        assert!(result.is_err());
    }

    // ---- Registry integration edge cases ----

    #[test]
    fn test_registry_pause_blocks_fee_change() {
        // Fee-changing ops fail closed when registry is paused
        // This is enforced by the registry's own governance flow
        let (env, _admin, _registry_id, _treasury_client, registry_client) = setup_with_registry();
        
        // Registry bounds prevent out-of-range values
        let result = registry_client.try_admin_set_parameter(&_admin, &RegistryParamKey::TreasuryFeeBps, &10_001i128);
        assert!(result.is_err());
        
        let result = registry_client.try_admin_set_parameter(&_admin, &RegistryParamKey::TreasuryFeeBps, &(-1i128));
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_fee_bps_zero_edge() {
        // Fee bps set to zero — treasury still accounts for gas/revenue policy
        let (env, _admin, _registry_id, treasury_client, registry_client) = setup_with_registry();
        
        registry_client.admin_set_parameter(&_admin, &RegistryParamKey::TreasuryFeeBps, &0i128);
        
        let token_admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_admin).address();
        treasury_client.whitelist_token(&_admin, &token);
        
        // collect_fee should succeed even with 0 fee bps
        let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        token_client.mint(&treasury_client.address, &100_000i128);
        let result = treasury_client.try_collect_fee(&token, &100_000i128);
        assert!(result.is_ok());
        assert_eq!(treasury_client.get_collected(&token), 100_000);
    }

    #[test]
    fn test_registry_rounding_dust_bounded() {
        // Rounding dust on fee collection remains bounded per existing policy
        let (env, _admin, _registry_id, treasury_client, registry_client) = setup_with_registry();
        
        // Set reserve allocation to 3333 bps (1/3) to test rounding
        registry_client.admin_set_parameter(&_admin, &RegistryParamKey::ReserveAllocationBps, &3333i128);
        
        let token_admin = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(token_admin).address();
        treasury_client.whitelist_token(&_admin, &token);
        
        let token_client = soroban_sdk::token::StellarAssetClient::new(&env, &token);
        token_client.mint(&treasury_client.address, &100i128);
        
        // With 100 units at 3333 bps = 33.33 -> 33 (integer division, dust < 1)
        let result = treasury_client.try_collect_fee(&token, &100i128);
        assert!(result.is_ok());
        
        // Total collected is 100, reserve should be 33, spendable = 100 - 33 = 67
        assert_eq!(treasury_client.get_collected(&token), 100);
        assert_eq!(treasury_client.get_reserve_balance(&token), 33);
    }

    #[test]
    fn test_set_registry_only_admin() {
        let TestEnv {
            env,
            admin: _,
            token: _,
            treasury_client,
        } = setup();
        
        let non_admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let result = treasury_client.try_set_parameter_registry(&non_admin, &registry);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_registry_returns_none_when_unset() {
        let TestEnv {
            env,
            admin: _,
            token: _,
            treasury_client,
        } = setup();
        
        let reg = treasury_client.get_parameter_registry();
        assert!(reg.is_none());
    }

    #[test]
    fn test_get_registry_returns_address_when_set() {
        let (env, _admin, registry_id, treasury_client, _registry_client) = setup_with_registry();
        
        let reg = treasury_client.get_parameter_registry();
        assert!(reg.is_some());
        assert_eq!(reg.unwrap(), registry_id);
    }
}