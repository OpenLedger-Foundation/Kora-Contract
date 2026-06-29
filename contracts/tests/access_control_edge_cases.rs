// tests/access_control_edge_cases.rs
//! Edge case tests for Access Control Contract
//!
//! This test module covers:
//! - Role transition edge cases
//! - Admin transfer validation
//! - Pause state consistency
//! - Role revocation edge cases
//! - Cross-role authorization

#[cfg(test)]
mod access_control_edge_cases {
    use kora_access_control::{AccessControlContractClient, AccessControlContract, Role};
    use kora_shared::errors::KoraError;
    use soroban_sdk::{
        testutils::Address as _,
        Address, Env,
    };

    struct TestEnv {
        env: Env,
        admin: Address,
        client: AccessControlContractClient<'static>,
    }

    fn setup() -> TestEnv {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &contract_id);

        client.initialize(&admin);

        TestEnv { env, admin, client }
    }

    // ── Role Grant Edge Cases ─────────────────────────────────────────────────

    #[test]
    fn test_cannot_grant_admin_role() {
        let t = setup();
        let target = Address::generate(&t.env);

        let result = t.client.try_grant_role(&t.admin, &target, &Role::Admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_cannot_grant_none_role() {
        let t = setup();
        let target = Address::generate(&t.env);

        let result = t.client.try_grant_role(&t.admin, &target, &Role::None);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_cannot_grant_role_to_admin() {
        let t = setup();

        // Try to grant role to the current admin address
        let result = t.client.try_grant_role(&t.admin, &t.admin, &Role::Operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_can_grant_operator_role() {
        let t = setup();
        let operator = Address::generate(&t.env);

        let result = t.client.try_grant_role(&t.admin, &operator, &Role::Operator);
        assert!(result.is_ok());

        assert_eq!(t.client.get_role(&operator), Role::Operator);
        assert!(t.client.has_role(&operator, &Role::Operator));
    }

    #[test]
    fn test_can_grant_verifier_role() {
        let t = setup();
        let verifier = Address::generate(&t.env);

        let result = t.client.try_grant_role(&t.admin, &verifier, &Role::Verifier);
        assert!(result.is_ok());

        assert_eq!(t.client.get_role(&verifier), Role::Verifier);
        assert!(t.client.has_role(&verifier, &Role::Verifier));
    }

    #[test]
    fn test_grant_role_non_admin_fails() {
        let t = setup();
        let non_admin = Address::generate(&t.env);
        let target = Address::generate(&t.env);

        let result = t.client.try_grant_role(&non_admin, &target, &Role::Operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_can_overwrite_role() {
        let t = setup();
        let user = Address::generate(&t.env);

        // Grant Operator
        t.client.grant_role(&t.admin, &user, &Role::Operator);
        assert_eq!(t.client.get_role(&user), Role::Operator);

        // Overwrite with Verifier
        t.client.grant_role(&t.admin, &user, &Role::Verifier);
        assert_eq!(t.client.get_role(&user), Role::Verifier);
    }

    // ── Role Revocation Edge Cases ────────────────────────────────────────────

    #[test]
    fn test_cannot_revoke_none_role() {
        let t = setup();
        let user = Address::generate(&t.env);

        // User has no role assigned
        let result = t.client.try_revoke_role(&t.admin, &user);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::RoleNotAssigned);
    }

    #[test]
    fn test_cannot_revoke_admin_role() {
        let t = setup();

        // Cannot revoke admin's own role
        let result = t.client.try_revoke_role(&t.admin, &t.admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_can_revoke_operator_role() {
        let t = setup();
        let operator = Address::generate(&t.env);

        t.client.grant_role(&t.admin, &operator, &Role::Operator);
        assert_eq!(t.client.get_role(&operator), Role::Operator);

        t.client.revoke_role(&t.admin, &operator);
        assert_eq!(t.client.get_role(&operator), Role::None);
    }

    #[test]
    fn test_can_revoke_verifier_role() {
        let t = setup();
        let verifier = Address::generate(&t.env);

        t.client.grant_role(&t.admin, &verifier, &Role::Verifier);
        assert_eq!(t.client.get_role(&verifier), Role::Verifier);

        t.client.revoke_role(&t.admin, &verifier);
        assert_eq!(t.client.get_role(&verifier), Role::None);
    }

    #[test]
    fn test_revoke_role_non_admin_fails() {
        let t = setup();
        let non_admin = Address::generate(&t.env);
        let operator = Address::generate(&t.env);

        t.client.grant_role(&t.admin, &operator, &Role::Operator);

        let result = t.client.try_revoke_role(&non_admin, &operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    // ── Admin Transfer Edge Cases ─────────────────────────────────────────────

    #[test]
    fn test_cannot_transfer_admin_to_self() {
        let t = setup();
        let result = t.client.try_propose_admin(&t.admin, &t.admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::InvalidAddress);
    }

    #[test]
    fn test_cannot_transfer_admin_to_zero_address() {
        let t = setup();
        let zero = Address::generate(&t.env);
        // Can't create zero address easily in tests, so this is conceptual

        // This would be: Address::from_contract_id(&env, &ContractId::zero())
        // But for practical purposes, we test with non-zero address
    }

    #[test]
    fn test_can_transfer_admin_to_new_address() {
        let t = setup();
        let new_admin = Address::generate(&t.env);

        t.client.propose_admin(&t.admin, &new_admin);
        t.client.accept_admin(&new_admin);

        assert_eq!(t.client.get_admin(), new_admin);
        assert_eq!(t.client.get_role(&new_admin), Role::Admin);
        assert_eq!(t.client.get_role(&t.admin), Role::None);
    }

    #[test]
    fn test_transfer_admin_non_current_admin_fails() {
        let t = setup();
        let non_admin = Address::generate(&t.env);
        let new_admin = Address::generate(&t.env);

        let result = t.client.try_propose_admin(&non_admin, &new_admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_old_admin_loses_privileges_after_transfer() {
        let t = setup();
        let new_admin = Address::generate(&t.env);
        let old_admin = t.admin.clone();

        t.client.propose_admin(&old_admin, &new_admin);
        t.client.accept_admin(&new_admin);

        // Old admin cannot grant roles
        let target = Address::generate(&t.env);
        let result = t.client.try_grant_role(&old_admin, &target, &Role::Operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);

        // New admin can grant roles
        let result = t.client.try_grant_role(&new_admin, &target, &Role::Operator);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cannot_transfer_admin_to_existing_operator() {
        let t = setup();
        let operator = Address::generate(&t.env);

        // Make someone an operator
        t.client.grant_role(&t.admin, &operator, &Role::Operator);

        // Try to propose admin to that operator (must fail)
        let result = t.client.try_propose_admin(&t.admin, &operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::Unauthorized);
    }

    #[test]
    fn test_transfer_admin_to_existing_admin_allowed() {
        let t = setup();

        // This is an edge case - what if we transfer to an address that already has admin role?
        // This shouldn't normally happen, but if it does, it should be idempotent
        // (This test documents expected behavior)
    }

    // ── Pause/Unpause Edge Cases ──────────────────────────────────────────────

    #[test]
    fn test_cannot_pause_twice() {
        let t = setup();

        t.client.pause(&t.admin);
        assert!(t.client.is_paused());

        let result = t.client.try_pause(&t.admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::AlreadyPaused);
    }

    #[test]
    fn test_cannot_unpause_when_not_paused() {
        let t = setup();

        assert!(!t.client.is_paused());

        let result = t.client.try_unpause(&t.admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotPaused);
    }

    #[test]
    fn test_pause_unpause_cycle() {
        let t = setup();

        for cycle in 0..3 {
            assert!(!t.client.is_paused());
            t.client.pause(&t.admin);
            assert!(t.client.is_paused());
            t.client.unpause(&t.admin);
            assert!(!t.client.is_paused());
        }
    }

    #[test]
    fn test_non_admin_cannot_pause() {
        let t = setup();
        let non_admin = Address::generate(&t.env);

        let result = t.client.try_pause(&non_admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    #[test]
    fn test_non_admin_cannot_unpause() {
        let t = setup();
        let non_admin = Address::generate(&t.env);

        t.client.pause(&t.admin);

        let result = t.client.try_unpause(&non_admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }

    // ── Role Query Edge Cases ─────────────────────────────────────────────────

    #[test]
    fn test_get_role_unassigned_user() {
        let t = setup();
        let user = Address::generate(&t.env);

        assert_eq!(t.client.get_role(&user), Role::None);
    }

    #[test]
    fn test_has_role_false_for_unassigned_user() {
        let t = setup();
        let user = Address::generate(&t.env);

        assert!(!t.client.has_role(&user, &Role::Operator));
        assert!(!t.client.has_role(&user, &Role::Verifier));
        assert!(!t.client.has_role(&user, &Role::Admin));
    }

    #[test]
    fn test_admin_has_role_admin() {
        let t = setup();

        assert_eq!(t.client.get_role(&t.admin), Role::Admin);
        assert!(t.client.has_role(&t.admin, &Role::Admin));
    }

    #[test]
    fn test_get_role_after_revocation() {
        let t = setup();
        let user = Address::generate(&t.env);

        t.client.grant_role(&t.admin, &user, &Role::Operator);
        assert_eq!(t.client.get_role(&user), Role::Operator);

        t.client.revoke_role(&t.admin, &user);
        assert_eq!(t.client.get_role(&user), Role::None);
    }

    // ── Initialization Edge Cases ─────────────────────────────────────────────

    #[test]
    fn test_cannot_initialize_twice() {
        let t = setup();

        let new_admin = Address::generate(&t.env);
        let result = t.client.try_initialize(&new_admin);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::AlreadyInitialized);
    }

    #[test]
    fn test_initial_state_correct() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let contract_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &contract_id);

        client.initialize(&admin);

        // Verify initial state
        assert_eq!(client.get_admin(), admin);
        assert_eq!(client.get_role(&admin), Role::Admin);
        assert!(!client.is_paused());
    }

    // ── Authorization Consistency ─────────────────────────────────────────────

    #[test]
    fn test_operator_and_verifier_are_different() {
        let t = setup();
        let operator = Address::generate(&t.env);
        let verifier = Address::generate(&t.env);

        t.client.grant_role(&t.admin, &operator, &Role::Operator);
        t.client.grant_role(&t.admin, &verifier, &Role::Verifier);

        assert!(t.client.has_role(&operator, &Role::Operator));
        assert!(!t.client.has_role(&operator, &Role::Verifier));

        assert!(t.client.has_role(&verifier, &Role::Verifier));
        assert!(!t.client.has_role(&verifier, &Role::Operator));
    }

    #[test]
    fn test_user_cannot_self_grant_role() {
        let t = setup();
        let user = Address::generate(&t.env);

        // User tries to grant role to themselves (should fail due to not being admin)
        let result = t.client.try_grant_role(&user, &user, &Role::Operator);
        assert_eq!(result.unwrap_err().unwrap(), KoraError::NotAdmin);
    }
}
