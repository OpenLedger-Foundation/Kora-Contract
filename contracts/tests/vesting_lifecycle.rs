// tests/vesting_lifecycle.rs
//! Integration tests for the vesting grant lifecycle.
//!
//! This test module covers the full grant lifecycle:
//!   - Happy path: creation, cliff progression, partial release, full release
//!   - Admin-only revoke that preserves vested balance
//!   - Negative tests: duplicate grant, zero-amount grant, invalid schedule,
//!     double release, over-release, release before cliff
//!   - Edge cases: release exactly at cliff boundary, max-duration grants,
//!     revoked grant blocks further release
//!   - View helpers: get_vested_balance, get_unvested_balance, get_grant

#[cfg(test)]
mod vesting_lifecycle {
    use kora_vesting::{VestingContractClient, VestingContract, VestingError, VestingGrant};
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env,
    };

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Shared test fixture.  Returns an `Env` (with mock auths enabled), an
    /// `admin`, a `beneficiary`, the registered vesting `client`, and a token
    /// client pre-minted with sufficient balance.
    struct VestingFixture<'a> {
        env: Env,
        admin: Address,
        beneficiary: Address,
        other_user: Address,
        contract_address: Address,
        client: VestingContractClient<'a>,
        token: TokenFixture<'a>,
    }

    struct TokenFixture<'a> {
        admin: Address,
        address: Address,
        client: soroban_sdk::token::StellarAssetClient<'a>,
    }

    fn create_token<'a>(env: &Env) -> TokenFixture<'a> {
        let admin = Address::generate(env);
        let client = soroban_sdk::token::StellarAssetClient::new(
            env,
            &env.register_stellar_asset_contract_v2(admin.clone()).address(),
        );
        TokenFixture { admin, address: client.address.clone(), client }
    }

    fn token_balance(env: &Env, token: &TokenFixture, account: &Address) -> i128 {
        soroban_sdk::token::Client::new(env, &token.address).balance(account)
    }

    fn setup<'a>() -> VestingFixture<'a> {
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
        let beneficiary = Address::generate(&env);
        let other_user = Address::generate(&env);

        let contract_id = env.register_contract(None, VestingContract);
        let contract_address = contract_id.clone();
        let client = VestingContractClient::new(&env, &contract_id);

        let token = create_token(&env);

        // Initialize the vesting contract
        client.initialize(&admin, &token.address);

        // Mint tokens to the vesting contract
        token.client.mint(&contract_id, &10_000_000);

        VestingFixture { env, admin, beneficiary, other_user, contract_address, client, token }
    }

    /// Advance ledger timestamp to a specific value.
    fn advance_to(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 21,
            sequence_number: env.ledger().sequence() + 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
    }

    // ── Happy Path ───────────────────────────────────────────────────────────

    #[test]
    fn test_happy_path_create_release_full() {
        let f = setup();

        let total: i128 = 10_000;
        let cliff = 100u64;
        let duration = 1000u64;
        let start = f.env.ledger().timestamp();

        // 1. Create grant
        f.client.create_grant(&f.admin, &f.beneficiary, &total, &start, &cliff, &duration);

        // 2. Before cliff – nothing vested
        assert_eq!(f.client.get_vested_balance(&f.beneficiary), 0);
        assert_eq!(f.client.get_unvested_balance(&f.beneficiary), total);

        // 3. Exactly at cliff boundary – first vest
        advance_to(&f.env, start + cliff);
        let vested_at_cliff = f.client.get_vested_balance(&f.beneficiary);
        assert!(vested_at_cliff > 0, "should have some vested at cliff boundary");

        // 4. Half way – proportional vesting
        advance_to(&f.env, start + 500);
        assert_eq!(f.client.get_vested_balance(&f.beneficiary), 5_000);
        assert_eq!(f.client.get_unvested_balance(&f.beneficiary), 5_000);

        // 5. Release partial
        let balance_before = token_balance(&f.env, &f.token, &f.beneficiary);
        f.client.release(&f.beneficiary);
        let balance_after = token_balance(&f.env, &f.token, &f.beneficiary);
        assert_eq!(balance_after - balance_before, 5_000);
        let grant = f.client.get_grant(&f.beneficiary);
        assert_eq!(grant.unwrap().released_amount, 5_000);

        // 6. After full duration – everything vested
        advance_to(&f.env, start + duration);
        assert_eq!(f.client.get_vested_balance(&f.beneficiary), total);
        assert_eq!(f.client.get_unvested_balance(&f.beneficiary), 0);

        // 7. Release the rest
        let balance_before2 = token_balance(&f.env, &f.token, &f.beneficiary);
        f.client.release(&f.beneficiary);
        let balance_after2 = token_balance(&f.env, &f.token, &f.beneficiary);
        assert_eq!(balance_after2 - balance_before2, 5_000);
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), total);
    }

    #[test]
    fn test_partial_releases_never_exceed_total() {
        let f = setup();

        let total: i128 = 10_000;
        let cliff = 100u64;
        let duration = 1000u64;
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &total, &start, &cliff, &duration);

        // Release at several points; sum of released must never exceed total
        let mut cumulative: i128 = 0;

        // 30 % vested
        advance_to(&f.env, start + 300);
        f.client.release(&f.beneficiary);
        cumulative += 3_000;
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), cumulative);

        // 70 % vested (releases incremental 4_000)
        advance_to(&f.env, start + 700);
        f.client.release(&f.beneficiary);
        cumulative += 4_000;
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), cumulative);

        // 100 % vested (releases remaining 3_000)
        advance_to(&f.env, start + duration);
        f.client.release(&f.beneficiary);
        cumulative += 3_000;
        assert_eq!(cumulative, total);
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), total);

        // Double-release should now be rejected
        let result = f.client.try_release(&f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::InsufficientVestedBalance);
    }

    // ── Revocation ───────────────────────────────────────────────────────────

    #[test]
    fn test_admin_revoke_preserves_vested_and_blocks_further() {
        let f = setup();

        let total: i128 = 10_000;
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &total, &start, &100, &1000);

        // Advance to 50 % vested
        advance_to(&f.env, start + 500);
        assert_eq!(f.client.get_vested_balance(&f.beneficiary), 5_000);

        // Revoke
        f.client.revoke_grant(&f.admin, &f.beneficiary);

        // Vested balance should be frozen at 5_000 (not increasing)
        let vested_after_revoke = f.client.get_vested_balance(&f.beneficiary);
        assert_eq!(vested_after_revoke, 5_000);

        // Get grant details to verify revoked flag
        let grant = f.client.get_grant(&f.beneficiary).unwrap();
        assert_eq!(grant.revoked, true);
        assert!(grant.revoked_at.is_some());

        // Advance further – still 5_000
        advance_to(&f.env, start + 900);
        assert_eq!(f.client.get_vested_balance(&f.beneficiary), 5_000);

        // Can still release the vested amount
        f.client.release(&f.beneficiary);
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), 5_000);

        // Further release should be rejected (nothing left to release)
        let result = f.client.try_release(&f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::InsufficientVestedBalance);
    }

    #[test]
    fn test_revoke_twice_fails() {
        let f = setup();

        let total: i128 = 10_000;
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &total, &start, &100, &1000);
        advance_to(&f.env, start + 200);
        f.client.revoke_grant(&f.admin, &f.beneficiary);

        // Second revoke must fail
        let result = f.client.try_revoke_grant(&f.admin, &f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::GrantAlreadyRevoked);
    }

    #[test]
    fn test_non_admin_cannot_revoke() {
        let f = setup();

        let total: i128 = 10_000;
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &total, &start, &100, &1000);

        // Non-admin tries to revoke
        let result = f.client.try_revoke_grant(&f.other_user, &f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::NotAdmin);
    }

    // ── View Helpers ─────────────────────────────────────────────────────────

    #[test]
    fn test_view_helpers_no_grant() {
        let f = setup();

        // Nobody has a grant yet
        assert_eq!(f.client.get_vested_balance(&f.beneficiary), 0);
        assert_eq!(f.client.get_unvested_balance(&f.beneficiary), 0);
        assert!(f.client.get_grant(&f.beneficiary).is_none());
    }

    #[test]
    fn test_view_helpers_after_partial_release() {
        let f = setup();

        let total: i128 = 10_000;
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &total, &start, &100, &1000);

        // 50 % vested
        advance_to(&f.env, start + 500);
        assert_eq!(f.client.get_vested_balance(&f.beneficiary), 5_000);
        assert_eq!(f.client.get_unvested_balance(&f.beneficiary), 5_000);

        // Release 5_000
        f.client.release(&f.beneficiary);
        let grant = f.client.get_grant(&f.beneficiary).unwrap();
        assert_eq!(grant.released_amount, 5_000);
        // Vested hasn't changed, unvested hasn't changed
        assert_eq!(f.client.get_vested_balance(&f.beneficiary), 5_000);
        assert_eq!(f.client.get_unvested_balance(&f.beneficiary), 5_000);
    }

    // ── Negative Tests ───────────────────────────────────────────────────────

    #[test]
    fn test_zero_total_rejected() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        let result = f.client.try_create_grant(&f.admin, &f.beneficiary, &0, &start, &100, &1000);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::InvalidAmount);
    }

    #[test]
    fn test_negative_total_rejected() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        let result = f.client.try_create_grant(&f.admin, &f.beneficiary, &-1, &start, &100, &1000);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::InvalidAmount);
    }

    #[test]
    fn test_invalid_schedule_rejected() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        // cliff == vesting_duration
        let result = f.client.try_create_grant(&f.admin, &f.beneficiary, &10_000, &start, &500, &500);
        assert_eq!(
            result.unwrap_err().unwrap(),
            VestingError::InvalidVestingSchedule,
        );

        // cliff > vesting_duration
        let result2 = f.client.try_create_grant(
            &f.admin, &f.beneficiary, &10_000, &start, &600, &500,
        );
        assert_eq!(
            result2.unwrap_err().unwrap(),
            VestingError::InvalidVestingSchedule,
        );
    }

    #[test]
    fn test_duplicate_grant_rejected() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &10_000, &start, &100, &1000);

        let result = f.client.try_create_grant(&f.admin, &f.beneficiary, &5_000, &start, &100, &1000);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::GrantAlreadyExists);
    }

    #[test]
    fn test_release_before_cliff_returns_insufficient() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &10_000, &start, &100, &1000);

        // Before cliff
        let result = f.client.try_release(&f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::InsufficientVestedBalance);
    }

    #[test]
    fn test_double_release_fails_on_second_call() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &10_000, &start, &100, &1000);

        // Release everything
        advance_to(&f.env, start + 1000);
        f.client.release(&f.beneficiary);

        // Second release must fail
        let result = f.client.try_release(&f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::InsufficientVestedBalance);
    }

    #[test]
    fn test_over_release_rejected() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &10_000, &start, &100, &1000);

        // Release 5_000 (all that's vested at 500s)
        advance_to(&f.env, start + 500);
        f.client.release(&f.beneficiary);

        // Try to release again immediately – no more vested yet
        let result = f.client.try_release(&f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::InsufficientVestedBalance);

        // Advance to 600s – only 1_000 more vested, should release that
        advance_to(&f.env, start + 600);
        f.client.release(&f.beneficiary);
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), 6_000);
    }

    // ── Edge Cases ───────────────────────────────────────────────────────────

    #[test]
    fn test_release_at_cliff_boundary() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &10_000, &start, &100, &1000);

        // At exactly cliff boundary (start + 100) – should have some vested
        advance_to(&f.env, start + 100);
        let vested = f.client.get_vested_balance(&f.beneficiary);
        // Linear: 10_000 * 100 / 1000 = 1_000
        assert_eq!(vested, 1_000);

        // Release at boundary must succeed
        f.client.release(&f.beneficiary);
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), 1_000);
    }

    #[test]
    fn test_max_duration_grant() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        // Very long vesting (effectively never fully vests in test time).
        // NOTE: u64::MAX would vest dust (integer division yields 0), so use a
        // large-but-representable duration that still vests a nonzero amount.
        let max_duration: u64 = 1_000_000;
        let cliff: u64 = 1_000;
        f.client.create_grant(&f.admin, &f.beneficiary, &10_000, &start, &cliff, &max_duration);

        // Well past cliff
        advance_to(&f.env, start + cliff + 5_000);
        let vested = f.client.get_vested_balance(&f.beneficiary);
        // Should have some vested (proportional)
        assert!(vested > 0, "should have some vested after cliff even with max duration");
        assert!(vested < 10_000, "should not be fully vested yet because max_duration is enormous");

        // Release should work
        f.client.release(&f.beneficiary);
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), vested);
    }

    #[test]
    fn test_release_on_revoked_grant_fails() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &10_000, &start, &100, &1000);

        // Revoke before any vesting
        advance_to(&f.env, start + 50);
        f.client.revoke_grant(&f.admin, &f.beneficiary);

        // Release on a revoked grant with 0 vested
        let result = f.client.try_release(&f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::InsufficientVestedBalance);
    }

    #[test]
    fn test_grant_not_found_returns_proper_error() {
        let f = setup();

        // No grant created yet
        let result = f.client.try_release(&f.beneficiary);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::GrantNotFound);

        let result2 = f.client.try_revoke_grant(&f.admin, &f.beneficiary);
        assert_eq!(result2.unwrap_err().unwrap(), VestingError::GrantNotFound);
    }

    #[test]
    fn test_non_admin_cannot_create_grant() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        let result = f.client.try_create_grant(&f.other_user, &f.beneficiary, &10_000, &start, &100, &1000);
        assert_eq!(result.unwrap_err().unwrap(), VestingError::NotAdmin);
    }

    #[test]
    fn test_beneficiary_calls_release_successfully() {
        let f = setup();
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &10_000, &start, &100, &1000);

        // Beneficiary calls release directly (anyone can call release)
        advance_to(&f.env, start + 500);
        f.client.release(&f.beneficiary);
        assert_eq!(token_balance(&f.env, &f.token, &f.beneficiary), 5_000);
    }

    #[test]
    fn test_get_admin_and_token() {
        let f = setup();

        let admin = f.client.get_admin();
        assert_eq!(admin, f.admin);

        let token = f.client.get_token();
        assert_eq!(token, f.token.client.address);
    }

    // ── Accounting Invariant ─────────────────────────────────────────────────

    #[test]
    fn test_released_amount_never_exceeds_grant_total() {
        let f = setup();
        let total: i128 = 10_000;
        let start = f.env.ledger().timestamp();

        f.client.create_grant(&f.admin, &f.beneficiary, &total, &start, &100, &2000);

        // Many small releases (strided: a step per ledger would exhaust the
        // test host budget long before covering the schedule).
        let mut cumulative: i128 = 0;
        for step in (200u64..=2000u64).step_by(20) {
            advance_to(&f.env, start + step);
            let result = f.client.try_release(&f.beneficiary);
            if result.is_ok() {
                let new_balance = token_balance(&f.env, &f.token, &f.beneficiary);
                let released_this_step = new_balance - cumulative;
                cumulative = new_balance;
                assert!(released_this_step >= 0, "released amount must be non-negative");
                assert!(
                    cumulative <= total,
                    "cumulative released {} must never exceed grant total {}",
                    cumulative,
                    total,
                );
            }
        }

        assert_eq!(cumulative, total, "all tokens should eventually be released");
    }

    #[test]
    fn test_get_grant_returns_correct_fields() {
        let f = setup();
        let total: i128 = 42_000;
        let start = f.env.ledger().timestamp();
        let cliff = 200u64;
        let duration = 2000u64;

        f.client.create_grant(&f.admin, &f.beneficiary, &total, &start, &cliff, &duration);

        let grant = f.client.get_grant(&f.beneficiary).unwrap();
        assert_eq!(grant.beneficiary, f.beneficiary);
        assert_eq!(grant.total_amount, total);
        assert_eq!(grant.start_time, start);
        assert_eq!(grant.cliff_duration, cliff);
        assert_eq!(grant.vesting_duration, duration);
        assert_eq!(grant.released_amount, 0);
        assert_eq!(grant.revoked, false);
        assert!(grant.revoked_at.is_none());
    }
}