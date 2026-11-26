// tests/council_election_grace.rs
//
// #1103 — Failed-election term-extension grace path coverage.
//
// `docs/access-control.md` §7 ("Council Rotation & Term-Limit Enforcement")
// promises that when a council rotation election fails to reach quorum, the
// expiring seat keeps its authority and its term is extended by the configured
// grace period, so pause/upgrade powers are never left without a quorum.
//
// Semantics pinned by this suite (see `conclude_council_rotation` and
// `is_council_seat_term_expired` in `contracts/access_control/src/lib.rs`):
//
//   • Quorum is `votes_for >= MultisigConfig.threshold`. The initiator's vote
//     counts as the first "for" vote. Exactly `threshold` votes confirms; one
//     fewer fails.
//   • Failed election: the seat stays in the signer set and its term start is
//     moved to `old_start + grace` (old start defaults to "now" when no term has
//     been recorded). Repeated failures compound the extension.
//   • Default grace when `configure_council_terms` was never called: 259_200 s
//     (3 days).
//   • Expiry is strict: expired iff `now > term_start + term_length`.
//   • Confirmed election: the seat is swapped, the successor's term starts
//     "now" (no grace), and the outgoing seat loses signer authority.
//   • Actions outside the rules are rejected: non-signers cannot vote or
//     conclude, elections cannot be re-concluded or voted on after conclusion,
//     double votes are rejected, and a confirmation that would breach minimum
//     quorum (`threshold < 2`) reverts without concluding.
//
// The final module covers the governance_voting quorum boundary that the same
// docs cross-link: `total_votes == quorum_required` executes, one below fails.

#[cfg(test)]
mod council_election_grace {
    use kora_access_control::{AccessControlContract, AccessControlContractClient, AccessControlError};
    use kora_shared::types::AdminAction;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        Address, Env, Vec,
    };

    const BASE_TIMESTAMP: u64 = 1_700_000_000;
    const TERM_LENGTH: u64 = 30 * 86_400;
    const GRACE_PERIOD: u64 = 3 * 86_400;
    const DEFAULT_GRACE_PERIOD: u64 = 259_200;

    struct Council {
        env: Env,
        admin: Address,
        signers: Vec<Address>,
        client: AccessControlContractClient<'static>,
    }

    impl Council {
        fn signer(&self, i: u32) -> Address {
            self.signers.get(i).unwrap()
        }

        fn set_time(&self, ts: u64) {
            self.env.ledger().with_mut(|li| li.timestamp = ts);
        }

        fn current_signers(&self) -> Vec<Address> {
            self.client.get_multisig_config().signers
        }
    }

    /// Deploy access_control with an N-of-M council. When `configure_terms` is
    /// set, term length and grace period are configured explicitly.
    fn setup(n_signers: u32, threshold: u32, configure_terms: bool) -> Council {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| li.timestamp = BASE_TIMESTAMP);

        let admin = Address::generate(&env);
        let id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &id);
        client.initialize(&admin);

        let mut signers = Vec::new(&env);
        for _ in 0..n_signers {
            signers.push_back(Address::generate(&env));
        }
        client.configure_multisig(&admin, &signers, &threshold);
        if configure_terms {
            client.configure_council_terms(&admin, &TERM_LENGTH, &GRACE_PERIOD);
        }

        Council { env, admin, signers, client }
    }

    fn contains(list: &Vec<Address>, who: &Address) -> bool {
        list.iter().any(|a| &a == who)
    }

    // ── Failed quorum activates grace ────────────────────────────────────────

    #[test]
    fn failed_quorum_activates_grace_and_keeps_seat() {
        let c = setup(3, 2, true);
        let seat = c.signer(2);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        c.client.vote_council_rotation(&c.signer(1), &id, &false);
        c.client.conclude_council_rotation(&c.signer(0), &id);

        let election = c.client.get_council_election(&id).unwrap();
        assert!(election.concluded);
        assert!(!election.confirmed, "1 of 2 required votes must not confirm");

        // The expiring seat keeps its authority: still a signer, candidate is not.
        let signers = c.current_signers();
        assert!(contains(&signers, &seat));
        assert!(!contains(&signers, &candidate));
        assert_eq!(signers.len(), 3);

        // The retained seat can still exercise council powers (e.g. propose a pause).
        let _ = c.client.propose_action(&seat, &AdminAction::Pause);

        // Grace boundary: extended term start = BASE + GRACE (no prior start recorded).
        let expiry = BASE_TIMESTAMP + GRACE_PERIOD + TERM_LENGTH;
        c.set_time(expiry);
        assert!(!c.client.is_council_seat_term_expired(&seat), "expiry is strict (>)");
        c.set_time(expiry + 1);
        assert!(c.client.is_council_seat_term_expired(&seat));
    }

    #[test]
    fn failed_election_without_votes_activates_grace() {
        // 3-of-4: initiator + one supporter = threshold - 1.
        let c = setup(4, 3, true);
        let seat = c.signer(3);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        c.client.vote_council_rotation(&c.signer(1), &id, &true);
        c.client.conclude_council_rotation(&c.signer(2), &id);

        let election = c.client.get_council_election(&id).unwrap();
        assert_eq!(election.votes_for, 2);
        assert!(election.concluded && !election.confirmed);
        assert!(contains(&c.current_signers(), &seat));

        c.set_time(BASE_TIMESTAMP + GRACE_PERIOD + TERM_LENGTH + 1);
        assert!(c.client.is_council_seat_term_expired(&seat));
    }

    #[test]
    fn repeated_failed_elections_compound_grace() {
        let c = setup(3, 2, true);
        let seat = c.signer(2);
        let candidate = Address::generate(&c.env);

        let first = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        c.client.conclude_council_rotation(&c.signer(0), &first);

        // A new election can be opened for the same seat after the first concluded.
        let second = c.client.initiate_council_rotation(&c.signer(1), &seat, &candidate);
        assert_ne!(first, second);
        c.client.conclude_council_rotation(&c.signer(1), &second);

        let expiry = BASE_TIMESTAMP + 2 * GRACE_PERIOD + TERM_LENGTH;
        c.set_time(expiry);
        assert!(!c.client.is_council_seat_term_expired(&seat));
        c.set_time(expiry + 1);
        assert!(c.client.is_council_seat_term_expired(&seat));
    }

    #[test]
    fn default_grace_applies_when_terms_unconfigured() {
        let c = setup(3, 2, false);
        let seat = c.signer(2);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        c.client.conclude_council_rotation(&c.signer(0), &id);

        // No term length yet: a seat can never be reported expired.
        c.set_time(BASE_TIMESTAMP + 10 * TERM_LENGTH);
        assert!(!c.client.is_council_seat_term_expired(&seat));

        // Configuring terms later uses the already-recorded extension (default grace).
        c.client.configure_council_terms(&c.admin, &TERM_LENGTH, &GRACE_PERIOD);
        let expiry = BASE_TIMESTAMP + DEFAULT_GRACE_PERIOD + TERM_LENGTH;
        c.set_time(expiry);
        assert!(!c.client.is_council_seat_term_expired(&seat));
        c.set_time(expiry + 1);
        assert!(c.client.is_council_seat_term_expired(&seat));
    }

    #[test]
    fn grace_extension_starts_from_election_conclusion_time() {
        let c = setup(3, 2, true);
        let seat = c.signer(2);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        let concluded_at = BASE_TIMESTAMP + 86_400;
        c.set_time(concluded_at);
        c.client.conclude_council_rotation(&c.signer(0), &id);

        let expiry = concluded_at + GRACE_PERIOD + TERM_LENGTH;
        c.set_time(expiry);
        assert!(!c.client.is_council_seat_term_expired(&seat));
        c.set_time(expiry + 1);
        assert!(c.client.is_council_seat_term_expired(&seat));
    }

    // ── Exact quorum boundary ────────────────────────────────────────────────

    #[test]
    fn exact_quorum_confirms_rotation_without_grace() {
        let c = setup(3, 2, true);
        let seat = c.signer(2);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        c.client.vote_council_rotation(&c.signer(1), &id, &true);
        c.client.conclude_council_rotation(&c.signer(0), &id);

        let election = c.client.get_council_election(&id).unwrap();
        assert_eq!(election.votes_for, 2, "votes_for == threshold");
        assert!(election.concluded && election.confirmed);

        let signers = c.current_signers();
        assert!(!contains(&signers, &seat));
        assert!(contains(&signers, &candidate));
        assert_eq!(signers.len(), 3, "quorum size preserved across the swap");

        // Successor's term starts at conclusion time — no grace is added.
        let expiry = BASE_TIMESTAMP + TERM_LENGTH;
        c.set_time(expiry);
        assert!(!c.client.is_council_seat_term_expired(&candidate));
        c.set_time(expiry + 1);
        assert!(c.client.is_council_seat_term_expired(&candidate));

        // Outgoing seat has lost council authority.
        assert_eq!(
            c.client.try_initiate_council_rotation(&seat, &candidate, &seat),
            Err(Ok(AccessControlError::SignerNotFound))
        );
    }

    #[test]
    fn against_votes_do_not_block_exact_quorum() {
        let c = setup(4, 2, true);
        let seat = c.signer(3);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        c.client.vote_council_rotation(&c.signer(1), &id, &false);
        c.client.vote_council_rotation(&c.signer(2), &id, &true);
        c.client.conclude_council_rotation(&c.signer(0), &id);

        let election = c.client.get_council_election(&id).unwrap();
        assert_eq!(election.votes_for, 2);
        assert_eq!(election.votes_against, 1);
        assert!(election.confirmed);
    }

    #[test]
    fn confirmation_below_minimum_quorum_reverts_without_concluding() {
        // threshold = 1: the initiator alone "confirms", but the quorum-safety
        // guard (threshold >= 2) rejects the swap and the call reverts.
        let c = setup(2, 1, true);
        let seat = c.signer(1);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        assert_eq!(
            c.client.try_conclude_council_rotation(&c.signer(0), &id),
            Err(Ok(AccessControlError::QuorumSafetyViolation))
        );

        let election = c.client.get_council_election(&id).unwrap();
        assert!(!election.concluded, "reverted conclusion leaves election open");
        assert!(contains(&c.current_signers(), &seat));
        assert!(!contains(&c.current_signers(), &candidate));
    }

    // ── Actions outside the grace rules are rejected ─────────────────────────

    #[test]
    fn non_signer_cannot_force_failed_election() {
        let c = setup(3, 2, true);
        let seat = c.signer(2);
        let candidate = Address::generate(&c.env);
        let outsider = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        assert_eq!(
            c.client.try_conclude_council_rotation(&outsider, &id),
            Err(Ok(AccessControlError::SignerNotFound))
        );
        assert_eq!(
            c.client.try_vote_council_rotation(&outsider, &id, &false),
            Err(Ok(AccessControlError::SignerNotFound))
        );

        // Election still open and no grace was applied.
        let election = c.client.get_council_election(&id).unwrap();
        assert!(!election.concluded);
        assert_eq!(election.voter_records.len(), 1);
    }

    #[test]
    fn concluded_election_rejects_further_votes_and_conclusions() {
        let c = setup(3, 2, true);
        let seat = c.signer(2);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        c.client.conclude_council_rotation(&c.signer(0), &id);

        // A late "for" vote cannot flip a failed election into a confirmation.
        assert_eq!(
            c.client.try_vote_council_rotation(&c.signer(1), &id, &true),
            Err(Ok(AccessControlError::RotationElectionAlreadyConcluded))
        );
        // Re-concluding cannot stack a second grace extension.
        assert_eq!(
            c.client.try_conclude_council_rotation(&c.signer(1), &id),
            Err(Ok(AccessControlError::RotationElectionAlreadyConcluded))
        );

        let expiry = BASE_TIMESTAMP + GRACE_PERIOD + TERM_LENGTH;
        c.set_time(expiry);
        assert!(!c.client.is_council_seat_term_expired(&seat));
        c.set_time(expiry + 1);
        assert!(c.client.is_council_seat_term_expired(&seat));
    }

    #[test]
    fn double_vote_and_unknown_election_rejected() {
        let c = setup(3, 2, true);
        let seat = c.signer(2);
        let candidate = Address::generate(&c.env);

        let id = c.client.initiate_council_rotation(&c.signer(0), &seat, &candidate);
        assert_eq!(
            c.client.try_vote_council_rotation(&c.signer(0), &id, &true),
            Err(Ok(AccessControlError::AlreadyVoted)),
            "initiator's implicit vote counts once"
        );
        assert_eq!(
            c.client.try_vote_council_rotation(&c.signer(1), &(id + 99), &true),
            Err(Ok(AccessControlError::RotationElectionNotFound))
        );
        assert_eq!(
            c.client.try_conclude_council_rotation(&c.signer(1), &(id + 99)),
            Err(Ok(AccessControlError::RotationElectionNotFound))
        );
    }

    #[test]
    fn grace_configuration_is_admin_gated_and_validated() {
        let c = setup(3, 2, false);
        let outsider = Address::generate(&c.env);
        assert_eq!(
            c.client.try_configure_council_terms(&outsider, &TERM_LENGTH, &GRACE_PERIOD),
            Err(Ok(AccessControlError::NotAdmin))
        );
        assert_eq!(
            c.client.try_configure_council_terms(&c.admin, &0u64, &GRACE_PERIOD),
            Err(Ok(AccessControlError::InvalidCouncilTermLength))
        );
    }

    #[test]
    fn rotation_requires_configured_council() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let id = env.register_contract(None, AccessControlContract);
        let client = AccessControlContractClient::new(&env, &id);
        client.initialize(&admin);

        let a = Address::generate(&env);
        let b = Address::generate(&env);
        assert_eq!(
            client.try_initiate_council_rotation(&a, &a, &b),
            Err(Ok(AccessControlError::MultisigNotConfigured))
        );
    }
}

#[cfg(test)]
mod governance_voting_quorum_boundary {
    use kora_governance_voting::{
        GovernanceError, GovernanceVotingContract, GovernanceVotingContractClient, ProposalType,
        VoteChoice, VotingMode,
    };
    use soroban_sdk::{testutils::Address as _, Address, Env, String};

    fn proposal_with_weight(weight: i128, quorum: i128) -> (GovernanceVotingContractClient<'static>, Address, u64) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, GovernanceVotingContract);
        let client = GovernanceVotingContractClient::new(&env, &id);

        let admin = Address::generate(&env);
        let access_control = Address::generate(&env);
        let proposer = Address::generate(&env);
        let voter = Address::generate(&env);

        client.initialize(&admin, &access_control);
        client.set_voting_weight(&admin, &voter, &weight);
        let proposal_id = client.create_proposal(
            &proposer,
            &String::from_str(&env, "Rotate council"),
            &String::from_str(&env, "Quorum boundary"),
            &ProposalType::Financial,
            &VotingMode::Standard,
            &quorum,
            &0,
        );
        client.vote(&voter, &proposal_id, &VoteChoice::For, &0);
        (client, admin, proposal_id)
    }

    #[test]
    fn exact_quorum_executes() {
        let (client, admin, proposal_id) = proposal_with_weight(100, 100);
        client.execute_proposal(&admin, &proposal_id);
        assert!(client.get_proposal(&proposal_id).unwrap().executed);
    }

    #[test]
    fn one_below_quorum_fails() {
        let (client, admin, proposal_id) = proposal_with_weight(99, 100);
        assert_eq!(
            client.try_execute_proposal(&admin, &proposal_id),
            Err(Ok(GovernanceError::InsufficientVotingWeight))
        );
        assert!(!client.get_proposal(&proposal_id).unwrap().executed);
    }
}
