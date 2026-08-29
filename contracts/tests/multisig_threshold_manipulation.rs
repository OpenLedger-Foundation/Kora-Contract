// tests/multisig_threshold_manipulation.rs
//
// #614 — Multisig Threshold Manipulation Test Suite
//
// Tests scenarios where the signer set or threshold changes while a proposal is
// mid-flight, ensuring no stale-signer-count exploit is possible.
//
// Intended semantics (documented here because they are not yet explicit in the
// contract's documentation):
//
//   • Votes are recorded as signer addresses.  At execution time, execute_action()
//     reloads the CURRENT MultisigConfig and re-validates the executor is still a
//     signer.  The approval count is compared against the CURRENT threshold.
//
//   • A signer who voted but was later removed from the config: their vote still
//     counts (it is stored in the proposal's `approvals` Vec).  The contract does
//     NOT re-validate each historical voter against the current signer set at
//     execution time.  This is a known design choice documented in this file.
//
//   • Raising the threshold after votes are collected: if the new threshold
//     exceeds the current approval count, execution must be rejected.
//
//   • Lowering the threshold after votes are collected: execution may succeed if
//     the approval count now meets or exceeds the lower threshold.
//
//   • Replacing the entire signer set: a proposal approved by the old set can be
//     executed by any member of the new set, but only if the new threshold is met
//     by the stored approval count.
//
// Each scenario maps to a distinct test.  Where the current implementation allows
// a potentially undesirable outcome, it is documented as a known design choice
// for future hardening.

#[cfg(test)]
mod multisig_threshold_manipulation {
    use kora_access_control::{AccessControlContract, AccessControlContractClient};
    use kora_shared::types::AdminAction;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env, Vec,
    };

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

    /// Deploy access_control, configure an N-of-M multisig, return (env, admin, signers, client).
    fn setup(
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

    /// Propose a Pause action via `proposer` and return the proposal_id.
    fn propose_pause(
        client: &AccessControlContractClient,
        proposer: &Address,
    ) -> u64 {
        client.propose_action(proposer, &AdminAction::Pause)
    }

    // ── Scenario 1: Signer removed after voting, before execution ────────────
    //
    // A 2-of-3 multisig.  Signer A proposes (vote 1), Signer B approves (vote 2).
    // Admin then reconfigures to a new 2-of-3 that excludes B (but keeps A).
    // The proposal already has 2 approvals stored.
    //
    // Intended behaviour: execution by a member of the NEW set succeeds because
    // the approval count (2) still meets the new threshold (2).  The reconfigured
    // set does not retroactively invalidate stored votes.
    //
    // This is documented as a known design choice; a stricter implementation would
    // re-validate each stored voter against the current set at execution time.
    #[test]
    fn signer_removed_after_voting_proposal_still_executes() {
        let (env, admin, signers, client) = setup(3, 2);

        let signer_a = signers.get(0).unwrap();
        let signer_b = signers.get(1).unwrap();
        let signer_c = signers.get(2).unwrap();

        // Signer A proposes (auto-votes), Signer B approves.
        let proposal_id = propose_pause(&client, &signer_a);
        client.approve_action(&signer_b, &proposal_id);

        // Admin reconfigures: replace B with a new signer D, keeping A and C.
        let signer_d = Address::generate(&env);
        let mut new_signers = Vec::new(&env);
        new_signers.push_back(signer_a.clone());
        new_signers.push_back(signer_c.clone());
        new_signers.push_back(signer_d.clone());
        client.configure_multisig(&admin, &new_signers, &2u32);

        // Signer A (still in new set) executes. Proposal has 2 approvals, threshold is 2.
        let result = client.try_execute_action(&signer_a, &proposal_id);
        assert!(
            result.is_ok(),
            "#614-1: execution must succeed — approval count (2) meets new threshold (2) \
             even though signer B was removed after voting (known design: stored votes not re-validated)"
        );
    }

    // ── Scenario 2: Threshold raised after votes collected ───────────────────
    //
    // A 2-of-3 multisig.  Two signers vote (meeting the original threshold of 2).
    // Admin raises the threshold to 3.  Execution must now fail.
    #[test]
    fn threshold_raised_after_votes_collected_blocks_execution() {
        let (env, admin, signers, client) = setup(3, 2);

        let signer_a = signers.get(0).unwrap();
        let signer_b = signers.get(1).unwrap();

        let proposal_id = propose_pause(&client, &signer_a);
        client.approve_action(&signer_b, &proposal_id);
        // Approval count = 2, which meets the original threshold of 2.

        // Admin raises threshold to 3 (unanimous).
        client.configure_multisig(&admin, &signers, &3u32);

        // Execution must now fail: 2 approvals < new threshold of 3.
        let result = client.try_execute_action(&signer_a, &proposal_id);
        assert!(
            result.is_err(),
            "#614-2: execution must fail when threshold was raised above current approval count"
        );
    }

    // ── Scenario 3: Threshold lowered after votes collected ──────────────────
    //
    // A 3-of-3 multisig.  Only 2 signers vote (below original threshold of 3).
    // Admin lowers the threshold to 2.  Execution must now succeed.
    //
    // This documents that lowering the threshold retroactively allows execution
    // of proposals that previously lacked quorum — a significant power the admin
    // holds over in-flight proposals.
    #[test]
    fn threshold_lowered_after_votes_allows_execution() {
        let (env, admin, signers, client) = setup(3, 3);

        let signer_a = signers.get(0).unwrap();
        let signer_b = signers.get(1).unwrap();

        let proposal_id = propose_pause(&client, &signer_a);
        client.approve_action(&signer_b, &proposal_id);
        // Approval count = 2, below original threshold of 3.

        // Execution must fail at original threshold of 3.
        let before_reconfig = client.try_execute_action(&signer_a, &proposal_id);
        assert!(
            before_reconfig.is_err(),
            "#614-3: execution must fail before threshold is lowered"
        );

        // Admin lowers threshold to 2.
        client.configure_multisig(&admin, &signers, &2u32);

        // Execution must now succeed: 2 approvals >= new threshold of 2.
        let after_reconfig = client.try_execute_action(&signer_a, &proposal_id);
        assert!(
            after_reconfig.is_ok(),
            "#614-3: execution must succeed after threshold was lowered to match approval count \
             (documented design: threshold change applies retroactively to in-flight proposals)"
        );
    }

    // ── Scenario 4: Entire signer set replaced mid-proposal ──────────────────
    //
    // A 2-of-3 multisig collects 2 votes.  Admin replaces ALL signers with a
    // completely new set (new threshold = 2).
    // A member of the new set executes.
    //
    // Intended behaviour: the executor must be a member of the CURRENT (new) set —
    // that check is enforced by require_signer() against the live config.
    // However, the stored approval count (2) still meets the new threshold (2),
    // so execution succeeds.
    #[test]
    fn signer_set_replaced_mid_proposal_new_signer_can_execute() {
        let (env, admin, old_signers, client) = setup(3, 2);

        let signer_a = old_signers.get(0).unwrap();
        let signer_b = old_signers.get(1).unwrap();

        let proposal_id = propose_pause(&client, &signer_a);
        client.approve_action(&signer_b, &proposal_id);
        // 2 approvals stored (from old set).

        // Admin replaces entire signer set.
        let new_signer_x = Address::generate(&env);
        let new_signer_y = Address::generate(&env);
        let new_signer_z = Address::generate(&env);
        let mut new_signers = Vec::new(&env);
        new_signers.push_back(new_signer_x.clone());
        new_signers.push_back(new_signer_y.clone());
        new_signers.push_back(new_signer_z.clone());
        client.configure_multisig(&admin, &new_signers, &2u32);

        // Member of new set can execute — executor auth is checked against new config.
        let result = client.try_execute_action(&new_signer_x, &proposal_id);
        assert!(
            result.is_ok(),
            "#614-4: new-set member must be able to execute a proposal with sufficient stored approvals \
             (documented behaviour: vote count re-validated against new threshold at execution)"
        );
    }

    // ── Scenario 5: Old-set member cannot execute after being replaced ────────
    //
    // After the signer set is replaced (Scenario 4 setup), a member of the OLD
    // set must no longer be able to call execute_action.
    #[test]
    fn evicted_signer_cannot_execute_after_set_replaced() {
        let (env, admin, old_signers, client) = setup(3, 2);

        let signer_a = old_signers.get(0).unwrap();
        let signer_b = old_signers.get(1).unwrap();

        let proposal_id = propose_pause(&client, &signer_a);
        client.approve_action(&signer_b, &proposal_id);

        // Replace entire signer set.
        let new_signer_x = Address::generate(&env);
        let new_signer_y = Address::generate(&env);
        let mut new_signers = Vec::new(&env);
        new_signers.push_back(new_signer_x);
        new_signers.push_back(new_signer_y);
        client.configure_multisig(&admin, &new_signers, &2u32);

        // Old signer A tries to execute — must fail.
        let result = client.try_execute_action(&signer_a, &proposal_id);
        assert!(
            result.is_err(),
            "#614-5: evicted signer must not be able to execute after signer set replaced"
        );
    }

    // ── Scenario 6: Non-signer cannot approve or execute ─────────────────────
    //
    // Baseline sanity: an address that was never configured as a signer must be
    // rejected by both approve_action and execute_action.
    #[test]
    fn non_signer_cannot_approve_or_execute() {
        let (env, _admin, signers, client) = setup(3, 2);

        let signer_a = signers.get(0).unwrap();
        let proposal_id = propose_pause(&client, &signer_a);

        let outsider = Address::generate(&env);

        let approve_result = client.try_approve_action(&outsider, &proposal_id);
        assert!(
            approve_result.is_err(),
            "#614-6: non-signer must not be able to approve a proposal"
        );

        let execute_result = client.try_execute_action(&outsider, &proposal_id);
        assert!(
            execute_result.is_err(),
            "#614-6: non-signer must not be able to execute a proposal"
        );
    }

    // ── Scenario 7: Double-voting by the same signer is rejected ─────────────
    //
    // A signer who already voted (or is the proposer) must not be able to cast a
    // second vote to artificially inflate the approval count.
    #[test]
    fn same_signer_cannot_vote_twice() {
        let (_env, _admin, signers, client) = setup(3, 2);

        let signer_a = signers.get(0).unwrap();
        let proposal_id = propose_pause(&client, &signer_a);
        // Signer A already voted as proposer.

        let second_vote = client.try_approve_action(&signer_a, &proposal_id);
        assert!(
            second_vote.is_err(),
            "#614-7: same signer must not be able to vote twice on the same proposal"
        );
    }

    // ── Scenario 8: Executed proposal cannot be re-executed ──────────────────
    //
    // After successful execution, a repeat call must fail regardless of signer
    // set composition.
    #[test]
    fn executed_proposal_cannot_be_replayed() {
        let (_env, _admin, signers, client) = setup(3, 2);

        let signer_a = signers.get(0).unwrap();
        let signer_b = signers.get(1).unwrap();

        let proposal_id = propose_pause(&client, &signer_a);
        client.approve_action(&signer_b, &proposal_id);

        let first = client.try_execute_action(&signer_a, &proposal_id);
        assert!(first.is_ok(), "#614-8: first execute must succeed");

        let second = client.try_execute_action(&signer_a, &proposal_id);
        assert!(
            second.is_err(),
            "#614-8: executed proposal must not be re-executable"
        );
    }

    // ── Scenario 9: Threshold of 1-of-1 single signer ────────────────────────
    //
    // Edge case: minimum valid configuration.  A 1-of-1 multisig must allow
    // the single signer to propose and immediately execute.
    #[test]
    fn single_signer_1_of_1_can_propose_and_execute() {
        let (_env, _admin, signers, client) = setup(1, 1);
        let signer = signers.get(0).unwrap();

        let proposal_id = propose_pause(&client, &signer);
        // No additional approvals needed.
        let result = client.try_execute_action(&signer, &proposal_id);
        assert!(
            result.is_ok(),
            "#614-9: 1-of-1 signer must be able to propose and immediately execute"
        );
    }

    // ── Scenario 10: configure_multisig with invalid threshold rejected ───────
    //
    // Threshold = 0 or threshold > n_signers must be rejected to prevent a
    // configuration that can never be met (or trivially met with zero votes).
    #[test]
    fn configure_multisig_rejects_zero_threshold() {
        let (env, admin, signers, client) = setup(3, 2);

        let result = client.try_configure_multisig(&admin, &signers, &0u32);
        assert!(
            result.is_err(),
            "#614-10: configure_multisig with threshold=0 must be rejected"
        );
    }

    #[test]
    fn configure_multisig_rejects_threshold_above_signer_count() {
        let (env, admin, signers, client) = setup(3, 2);

        let result = client.try_configure_multisig(&admin, &signers, &4u32);
        assert!(
            result.is_err(),
            "#614-10: configure_multisig with threshold > signers must be rejected"
        );
    }
}
