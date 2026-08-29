// tests/timelock_bypass_regression.rs
//
// #613 — Timelock Bypass Regression Tests
//
// Proves that no code path can execute an upgrade or governance-parameter change
// before its required timelock has elapsed.  Tests are written at exact boundary
// offsets: delay - 1, delay (exact), and delay + 1 seconds.
//
// Timelocks under test:
//   • price_oracle::execute_upgrade      — UPGRADE_TIMELOCK_DELAY  = 86_400 s (24 h)
//   • access_control::execute_parameter_change — GOVERNANCE_TIMELOCK_DELAY = 86_400 s (24 h)
//
// Ledger-timestamp assumptions:
//   The Soroban test environment's env.ledger().set(LedgerInfo { timestamp, .. })
//   allows arbitrary forward (and backward) manipulation of the ledger clock.
//   Tests here advance the clock by exactly the required offset between propose
//   and execute.  We document the assumption that the test environment does NOT
//   enforce monotonicity on timestamp — this is consistent with Soroban's test
//   harness design but differs from mainnet sequencing rules.

#[cfg(test)]
mod timelock_bypass_regression {
    use kora_access_control::{AccessControlContract, AccessControlContractClient};
    use kora_price_oracle::{PriceOracleContract, PriceOracleContractClient};
    use kora_shared::types::{AdminAction, ParameterKey};
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, BytesN, Env, Vec,
    };

    // Mirrors the constants in their respective contracts.
    const UPGRADE_TIMELOCK_DELAY: u64 = 86_400;
    const GOVERNANCE_TIMELOCK_DELAY: u64 = 86_400;

    const BASE_TIMESTAMP: u64 = 1_700_000_000;

    fn ledger_at(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1000,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 100_000,
        });
    }

    // ── price_oracle upgrade timelock ─────────────────────────────────────────

    fn setup_oracle() -> (Env, Address, PriceOracleContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        ledger_at(&env, BASE_TIMESTAMP);

        let admin = Address::generate(&env);
        let ac = Address::generate(&env);

        let oracle_id = env.register_contract(None, PriceOracleContract);
        let client = PriceOracleContractClient::new(&env, &oracle_id);
        client.initialize(&admin, &ac);

        (env, admin, client)
    }

    /// Propose an upgrade and return a dummy wasm hash.
    fn propose_oracle_upgrade(
        env: &Env,
        admin: &Address,
        client: &PriceOracleContractClient,
    ) -> BytesN<32> {
        let wasm_hash = BytesN::<32>::from_array(env, &[0xABu8; 32]);
        client.propose_upgrade(admin, &wasm_hash);
        wasm_hash
    }

    /// #613-A: execute_upgrade at delay - 1 must be rejected.
    #[test]
    fn oracle_upgrade_rejected_at_delay_minus_one() {
        let (env, admin, client) = setup_oracle();
        propose_oracle_upgrade(&env, &admin, &client);

        // Advance to exactly one second before the timelock expires.
        ledger_at(&env, BASE_TIMESTAMP + UPGRADE_TIMELOCK_DELAY - 1);

        let result = client.try_execute_upgrade(&admin);
        assert!(
            result.is_err(),
            "#613-A: execute_upgrade must be rejected at delay - 1 (timelock not elapsed)"
        );
    }

    /// #613-B: execute_upgrade at exactly delay must succeed.
    ///
    /// The implementation checks: `timestamp < proposed_at + UPGRADE_TIMELOCK_DELAY`
    /// so the exact boundary (proposed_at + delay) is the first timestamp that passes.
    #[test]
    fn oracle_upgrade_accepted_at_exact_delay() {
        let (env, admin, client) = setup_oracle();
        propose_oracle_upgrade(&env, &admin, &client);

        // Advance to exactly the timelock boundary.
        ledger_at(&env, BASE_TIMESTAMP + UPGRADE_TIMELOCK_DELAY);

        let result = client.try_execute_upgrade(&admin);
        assert!(
            result.is_ok(),
            "#613-B: execute_upgrade must succeed at delay (exact boundary)"
        );
    }

    /// #613-C: execute_upgrade at delay + 1 must succeed.
    #[test]
    fn oracle_upgrade_accepted_at_delay_plus_one() {
        let (env, admin, client) = setup_oracle();
        propose_oracle_upgrade(&env, &admin, &client);

        ledger_at(&env, BASE_TIMESTAMP + UPGRADE_TIMELOCK_DELAY + 1);

        let result = client.try_execute_upgrade(&admin);
        assert!(
            result.is_ok(),
            "#613-C: execute_upgrade must succeed at delay + 1"
        );
    }

    /// #613-D: execute_upgrade without any prior proposal must be rejected
    /// regardless of timestamp.
    #[test]
    fn oracle_upgrade_no_proposal_rejected() {
        let (env, admin, client) = setup_oracle();
        // Far future — still no proposal.
        ledger_at(&env, BASE_TIMESTAMP + UPGRADE_TIMELOCK_DELAY * 10);

        let result = client.try_execute_upgrade(&admin);
        assert!(
            result.is_err(),
            "#613-D: execute_upgrade without a proposal must always be rejected"
        );
    }

    /// #613-E: a second execute_upgrade call after successful execution must fail.
    /// Guards against proposal-replay (proposal is cleared on execute).
    #[test]
    fn oracle_upgrade_proposal_consumed_on_execute() {
        let (env, admin, client) = setup_oracle();
        propose_oracle_upgrade(&env, &admin, &client);
        ledger_at(&env, BASE_TIMESTAMP + UPGRADE_TIMELOCK_DELAY);

        // First execute — should succeed.
        let first = client.try_execute_upgrade(&admin);
        assert!(first.is_ok(), "#613-E: first execute must succeed");

        // Second execute — proposal must be gone.
        let second = client.try_execute_upgrade(&admin);
        assert!(
            second.is_err(),
            "#613-E: second execute must fail (proposal consumed)"
        );
    }

    // ── access_control governance parameter-change timelock ───────────────────

    fn setup_access_control_with_multisig(
        n_signers: usize,
        threshold: u32,
    ) -> (Env, Address, Vec<Address>, AccessControlContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        ledger_at(&env, BASE_TIMESTAMP);

        let admin = Address::generate(&env);
        let ac_id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &ac_id);
        client.initialize(&admin);

        let mut signers = Vec::new(&env);
        for _ in 0..n_signers {
            signers.push_back(Address::generate(&env));
        }
        client.configure_multisig(&admin, &signers, &threshold);

        (env, admin, signers, client)
    }

    /// Propose a FeeBps parameter change, vote to reach threshold, return proposal_id.
    fn propose_and_vote_fee_change(
        client: &AccessControlContractClient,
        signers: &Vec<Address>,
        threshold: u32,
        new_value: u32,
    ) -> u64 {
        let proposer = signers.get(0).unwrap();
        let proposal_id = client.propose_parameter_change(&proposer, &ParameterKey::FeeBps, &new_value);

        // Cast remaining votes to meet threshold.
        for i in 1..threshold {
            let voter = signers.get(i).unwrap();
            client.vote_parameter_change(&voter, &proposal_id);
        }

        proposal_id
    }

    /// #613-F: execute_parameter_change at delay - 1 must be rejected.
    #[test]
    fn governance_parameter_change_rejected_at_delay_minus_one() {
        let (env, _admin, signers, client) = setup_access_control_with_multisig(3, 2);

        let proposal_id = propose_and_vote_fee_change(&client, &signers, 2, 75u32);

        // Advance to one second before the governance timelock expires.
        ledger_at(&env, BASE_TIMESTAMP + GOVERNANCE_TIMELOCK_DELAY - 1);

        let executor = signers.get(0).unwrap();
        let result = client.try_execute_parameter_change(&executor, &proposal_id);
        assert!(
            result.is_err(),
            "#613-F: execute_parameter_change must be rejected at delay - 1"
        );
    }

    /// #613-G: execute_parameter_change at exactly delay must succeed.
    ///
    /// Assumption: the check is `timestamp < created_at + GOVERNANCE_TIMELOCK_DELAY`,
    /// so the exact boundary timestamp passes.
    #[test]
    fn governance_parameter_change_accepted_at_exact_delay() {
        let (env, _admin, signers, client) = setup_access_control_with_multisig(3, 2);

        let proposal_id = propose_and_vote_fee_change(&client, &signers, 2, 75u32);

        ledger_at(&env, BASE_TIMESTAMP + GOVERNANCE_TIMELOCK_DELAY);

        let executor = signers.get(0).unwrap();
        let result = client.try_execute_parameter_change(&executor, &proposal_id);
        assert!(
            result.is_ok(),
            "#613-G: execute_parameter_change must succeed at exact delay boundary"
        );
    }

    /// #613-H: execute_parameter_change at delay + 1 must succeed.
    #[test]
    fn governance_parameter_change_accepted_at_delay_plus_one() {
        let (env, _admin, signers, client) = setup_access_control_with_multisig(3, 2);

        let proposal_id = propose_and_vote_fee_change(&client, &signers, 2, 75u32);

        ledger_at(&env, BASE_TIMESTAMP + GOVERNANCE_TIMELOCK_DELAY + 1);

        let executor = signers.get(0).unwrap();
        let result = client.try_execute_parameter_change(&executor, &proposal_id);
        assert!(
            result.is_ok(),
            "#613-H: execute_parameter_change must succeed at delay + 1"
        );
    }

    /// #613-I: execute_parameter_change before threshold is met must be rejected
    /// (guards against the threshold-check being bypassed by a timelock race).
    #[test]
    fn governance_parameter_change_rejected_below_threshold() {
        let (env, _admin, signers, client) = setup_access_control_with_multisig(3, 2);

        // Only proposer votes — threshold=2 not met with 1 approval.
        let proposer = signers.get(0).unwrap();
        let proposal_id =
            client.propose_parameter_change(&proposer, &ParameterKey::FeeBps, &75u32);

        // Advance past the timelock — but threshold not met.
        ledger_at(&env, BASE_TIMESTAMP + GOVERNANCE_TIMELOCK_DELAY + 1);

        let result = client.try_execute_parameter_change(&proposer, &proposal_id);
        assert!(
            result.is_err(),
            "#613-I: execution must fail when threshold is not met, even after timelock"
        );
    }

    /// #613-J: once executed, a parameter proposal cannot be replayed.
    #[test]
    fn governance_parameter_change_proposal_consumed_on_execute() {
        let (env, _admin, signers, client) = setup_access_control_with_multisig(3, 2);

        let proposal_id = propose_and_vote_fee_change(&client, &signers, 2, 75u32);

        ledger_at(&env, BASE_TIMESTAMP + GOVERNANCE_TIMELOCK_DELAY);

        let executor = signers.get(0).unwrap();
        let first = client.try_execute_parameter_change(&executor, &proposal_id);
        assert!(first.is_ok(), "#613-J: first execute must succeed");

        let second = client.try_execute_parameter_change(&executor, &proposal_id);
        assert!(
            second.is_err(),
            "#613-J: second execute must fail (proposal consumed)"
        );
    }

    /// #613-K: non-signer cannot execute a governance change even after timelock.
    #[test]
    fn governance_parameter_change_rejected_for_non_signer() {
        let (env, _admin, signers, client) = setup_access_control_with_multisig(3, 2);

        let proposal_id = propose_and_vote_fee_change(&client, &signers, 2, 75u32);

        ledger_at(&env, BASE_TIMESTAMP + GOVERNANCE_TIMELOCK_DELAY + 1);

        let outsider = Address::generate(&env);
        let result = client.try_execute_parameter_change(&outsider, &proposal_id);
        assert!(
            result.is_err(),
            "#613-K: non-signer must not be able to execute governance change"
        );
    }
}
