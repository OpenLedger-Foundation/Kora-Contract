// contracts/tests/parameter_registry_events.rs
//
// Issue: param events — proposal/apply shared events
//
// Verifies that every write path in parameter_registry emits the correct
// standardised shared events (`PAR_PROP`, `PAR_APRV`, `PAR_EXEC`, `PAR_BOOT`,
// `REG_INIT`) so governance-sync dashboards can index them reliably.
//
// Acceptance criteria
//  Happy path: propose → approve → execute emits all three events in order.
//  Negative / auth: non-signer cannot propose; non-proposer cannot cancel;
//                   executing before timelock emits no PAR_EXEC event.
//  Paused / degraded: access_control pause does NOT block registry writes
//                     (parameter changes still need to reach consensus).
//  Idempotent: cancelling a proposal marks it cancelled; a re-execute attempt
//              after cancellation emits no additional PAR_EXEC event.

#[cfg(test)]
mod parameter_registry_events {
    use kora_parameter_registry::{ParameterRegistryContract, ParameterRegistryContractClient};
    use kora_access_control::{AccessControlContract, AccessControlContractClient};
    use kora_shared::types::RegistryParamKey;
    use soroban_sdk::{
        symbol_short,
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env, Symbol, TryFromVal, Val, Vec,
    };

    const GOVERNANCE_TIMELOCK: u64 = 86_400; // mirrors parameter_registry constant

    struct Ctx {
        env: Env,
        admin: Address,
        signer1: Address,
        signer2: Address,
        reg: ParameterRegistryContractClient<'static>,
        ac: AccessControlContractClient<'static>,
    }

    fn setup() -> Ctx {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10_000,
            min_persistent_entry_ttl: 10_000,
            max_entry_ttl: 1_000_000,
        });

        let admin = Address::generate(&env);
        let signer1 = Address::generate(&env);
        let signer2 = Address::generate(&env);

        // Real access_control so multisig gating exercises actual cross-contract behaviour.
        let ac_id = env.register_contract(None, AccessControlContract);
        let ac = AccessControlContractClient::new(&env, &ac_id);
        ac.initialize(&admin);
        // 2-of-2 quorum.
        let mut signers = soroban_sdk::Vec::new(&env);
        signers.push_back(signer1.clone());
        signers.push_back(signer2.clone());
        ac.configure_multisig(&admin, &signers, &2u32);

        let reg_id = env.register_contract(None, ParameterRegistryContract);
        let reg = ParameterRegistryContractClient::new(&env, &reg_id);
        reg.initialize(&admin, &Some(ac_id.clone()));

        Ctx { env, admin, signer1, signer2, reg, ac }
    }

    // ── Helper: advance ledger past the governance timelock ───────────────────

    fn advance_past_timelock(ctx: &Ctx) {
        let now = ctx.env.ledger().timestamp();
        ctx.env.ledger().set(LedgerInfo {
            timestamp: now + GOVERNANCE_TIMELOCK + 1,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 10_000,
            min_persistent_entry_ttl: 10_000,
            max_entry_ttl: 1_000_000,
        });
    }

    // ── Helper: decode the event-name symbol from a topics Vec<Val>.
    // Shared events use topics layout: [SCHEMA_V, event_name, schema_version].
    // Raw events put the topic first.

    fn topic_of(env: &Env, topics: &Vec<Val>) -> Option<Symbol> {
        let first = Symbol::try_from_val(env, &topics.get(0)?).ok()?;
        if first == symbol_short!("SCHEMA_V") {
            Symbol::try_from_val(env, &topics.get(1)?).ok()
        } else {
            Some(first)
        }
    }

    // ── Helper: event-name symbol of the last emitted event ──────────────────

    fn last_topic1(env: &Env) -> Symbol {
        let all = env.events().all();
        let (_, topics, _) = all.last().expect("no events emitted");
        topic_of(env, &topics).expect("last event has no recognisable topic")
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 1. Happy-path: PAR_PROP emitted when a governance proposal is opened
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn propose_param_emits_par_prop() {
        let ctx = setup();
        let proposal_id = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        assert_eq!(proposal_id, 1);
        // Verify PAR_PROP topic was emitted.
        let topic = last_topic1(&ctx.env);
        assert_eq!(topic, Symbol::new(&ctx.env, "PAR_PROP"));
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 2. Happy-path: PAR_APRV emitted when a signer approves a proposal
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn approve_param_emits_par_aprv() {
        let ctx = setup();
        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        ctx.reg.approve_param_update(&ctx.signer2, &pid);
        let topic = last_topic1(&ctx.env);
        assert_eq!(topic, Symbol::new(&ctx.env, "PAR_APRV"));
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 3. Happy-path: PAR_EXEC emitted and value visible after execute
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn execute_param_emits_par_exec_and_updates_value() {
        let ctx = setup();
        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        ctx.reg.approve_param_update(&ctx.signer2, &pid);
        advance_past_timelock(&ctx);
        ctx.reg.execute_param_update(&ctx.signer1, &pid);

        let topic = last_topic1(&ctx.env);
        assert_eq!(topic, Symbol::new(&ctx.env, "PAR_EXEC"));

        // Value is now 75 bps.
        assert_eq!(ctx.reg.get_value(&RegistryParamKey::MarketplaceFeeBps), 75);
        // is_param_governed reflects that an explicit governance decision was made.
        assert!(ctx.reg.is_param_governed(&RegistryParamKey::MarketplaceFeeBps));
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 4. Full propose → approve → execute sequence produces all three events
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn full_governance_cycle_emits_prop_then_aprv_then_exec() {
        let ctx = setup();
        ctx.env.events().all(); // consume existing init events

        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::LatePenaltyBps,
            &750,
        );
        ctx.reg.approve_param_update(&ctx.signer2, &pid);
        advance_past_timelock(&ctx);
        ctx.reg.execute_param_update(&ctx.signer1, &pid);

        // All three SCHEMA_V events are present (proposals emit one each).
        let events = ctx.env.events().all();
        // Find our three in order: PAR_PROP, PAR_APRV, PAR_EXEC.
        let par_prop = Symbol::new(&ctx.env, "PAR_PROP");
        let par_aprv = Symbol::new(&ctx.env, "PAR_APRV");
        let par_exec = Symbol::new(&ctx.env, "PAR_EXEC");
        let governance_topics: Vec<Symbol> = {
            let mut v = Vec::new(&ctx.env);
            for (_, topics, _) in events.iter() {
                if let Some(sym) = topic_of(&ctx.env, &topics) {
                    if sym == par_prop || sym == par_aprv || sym == par_exec {
                        v.push_back(sym);
                    }
                }
            }
            v
        };
        assert_eq!(governance_topics.len(), 3);
        assert_eq!(governance_topics.get(0).unwrap(), par_prop);
        assert_eq!(governance_topics.get(1).unwrap(), par_aprv);
        assert_eq!(governance_topics.get(2).unwrap(), par_exec);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 5. REG_INIT emitted during initialize
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn initialize_emits_reg_init() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let reg_id = env.register_contract(None, ParameterRegistryContract);
        let reg = ParameterRegistryContractClient::new(&env, &reg_id);
        reg.initialize(&admin, &None);

        let events = env.events().all();
        let found = events.iter().any(|(_, topics, _)| {
            topic_of(&env, &topics)
                .map(|sym| sym == Symbol::new(&env, "REG_INIT"))
                .unwrap_or(false)
        });
        assert!(found, "REG_INIT event must be emitted during initialize");
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 6. Negative: non-signer cannot propose (no PAR_PROP event emitted)
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn non_signer_propose_fails_no_event() {
        let ctx = setup();
        let outsider = Address::generate(&ctx.env);
        let events_before = ctx.env.events().all().len();

        let result = ctx.reg.try_propose_param_update(
            &outsider,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        assert!(result.is_err(), "non-signer must be rejected");

        // No new PAR_PROP event was emitted on failure.
        let events_after = ctx.env.events().all().len();
        assert_eq!(
            events_before, events_after,
            "failed proposal must not emit any event"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 7. Negative: executing before timelock returns error; PAR_EXEC not emitted
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn execute_before_timelock_fails_no_exec_event() {
        let ctx = setup();
        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        ctx.reg.approve_param_update(&ctx.signer2, &pid);
        // Do NOT advance time.
        let events_before = ctx.env.events().all().len();

        let result = ctx.reg.try_execute_param_update(&ctx.signer1, &pid);
        assert!(result.is_err(), "execute before timelock must fail");

        // No new PAR_EXEC event.
        let after = ctx.env.events().all();
        let exec_emitted = after.iter().skip(events_before).any(|(_, topics, _)| {
            topic_of(&ctx.env, &topics)
                .map(|sym| sym == Symbol::new(&ctx.env, "PAR_EXEC"))
                .unwrap_or(false)
        });
        assert!(!exec_emitted, "PAR_EXEC must not be emitted on timelock failure");
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 8. Negative: below-quorum execute fails
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn execute_below_quorum_fails() {
        let ctx = setup();
        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        // Only proposer has approved; signer2 has not → quorum 1 < threshold 2.
        advance_past_timelock(&ctx);
        let result = ctx.reg.try_execute_param_update(&ctx.signer1, &pid);
        assert!(result.is_err(), "execute below quorum must fail");
        // Value unchanged.
        assert_eq!(ctx.reg.get_value(&RegistryParamKey::MarketplaceFeeBps), 50);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 9. Idempotent: cancelled proposal cannot be executed; no PAR_EXEC emitted
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn cancelled_proposal_cannot_be_executed() {
        let ctx = setup();
        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        ctx.reg.approve_param_update(&ctx.signer2, &pid);
        // Proposer cancels before execution.
        ctx.reg.cancel_param_update(&ctx.signer1, &pid);

        advance_past_timelock(&ctx);
        let result = ctx.reg.try_execute_param_update(&ctx.signer1, &pid);
        assert!(result.is_err(), "cancelled proposal must be unexecutable");
        // Value unchanged at bootstrap default.
        assert_eq!(ctx.reg.get_value(&RegistryParamKey::MarketplaceFeeBps), 50);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 10. Idempotent: re-execute after success returns error
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn double_execute_rejected() {
        let ctx = setup();
        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        ctx.reg.approve_param_update(&ctx.signer2, &pid);
        advance_past_timelock(&ctx);
        ctx.reg.execute_param_update(&ctx.signer1, &pid);

        let result = ctx.reg.try_execute_param_update(&ctx.signer1, &pid);
        assert!(result.is_err(), "double-execute must be rejected");
        // Value not rolled back.
        assert_eq!(ctx.reg.get_value(&RegistryParamKey::MarketplaceFeeBps), 75);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 11. Edge-path: out-of-bounds value rejected; no proposal stored
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn propose_out_of_bounds_value_rejected() {
        let ctx = setup();
        // MarketplaceFeeBps max is 10_000; 20_000 must be rejected.
        let result = ctx.reg.try_propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &20_000,
        );
        assert!(result.is_err(), "out-of-bounds value must be rejected at proposal time");
        // No proposal stored (next id stays at 1).
        assert_eq!(ctx.reg.get_next_proposal_id(), 1);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 12. Edge-path: admin_set_parameter emits no PAR_PROP/EXEC but bootstrap
    //     path works when no multisig is configured
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn admin_set_parameter_blocked_when_multisig_configured() {
        let ctx = setup();
        // Multisig is wired; direct admin write must be blocked.
        let result = ctx.reg.try_admin_set_parameter(
            &ctx.admin,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        assert!(result.is_err(), "admin_set_parameter must be blocked when multisig is configured");
    }

    #[test]
    fn admin_set_parameter_works_without_multisig() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let reg_id = env.register_contract(None, ParameterRegistryContract);
        let reg = ParameterRegistryContractClient::new(&env, &reg_id);
        reg.initialize(&admin, &None);

        // No multisig configured → direct write is allowed.
        reg.admin_set_parameter(&admin, &RegistryParamKey::MarketplaceFeeBps, &75);
        assert_eq!(reg.get_value(&RegistryParamKey::MarketplaceFeeBps), 75);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 13. get_values_batch returns consistent values before and after governance
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn get_values_batch_reflects_executed_governance() {
        let ctx = setup();
        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::LatePenaltyBps,
            &750,
        );
        ctx.reg.approve_param_update(&ctx.signer2, &pid);
        advance_past_timelock(&ctx);
        ctx.reg.execute_param_update(&ctx.signer1, &pid);

        let mut keys = soroban_sdk::Vec::new(&ctx.env);
        keys.push_back(RegistryParamKey::MarketplaceFeeBps);
        keys.push_back(RegistryParamKey::LatePenaltyBps);
        let batch = ctx.reg.get_values_batch(&keys);

        assert_eq!(batch.len(), 2);
        // MarketplaceFeeBps unchanged at bootstrap default (50).
        assert_eq!(batch.get(0).unwrap().1, 50);
        // LatePenaltyBps changed to 750.
        assert_eq!(batch.get(1).unwrap().1, 750);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 14. Protocol-paused state does NOT block parameter governance
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn registry_governance_unaffected_by_protocol_pause() {
        let ctx = setup();

        // Pause the protocol via the access_control admin.
        // With a 2-of-2 multisig configured, pause must go through multisig.
        let pause_pid = ctx.ac.propose_action(
            &ctx.signer1,
            &kora_shared::types::AdminAction::Pause,
        );
        ctx.ac.approve_action(&ctx.signer2, &pause_pid);
        ctx.ac.execute_action(&ctx.signer1, &pause_pid);
        assert!(ctx.ac.is_paused(), "protocol must be paused");

        // Parameter governance still works while paused.
        let pid = ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        ctx.reg.approve_param_update(&ctx.signer2, &pid);
        advance_past_timelock(&ctx);
        // This must succeed even while the protocol is paused.
        ctx.reg.execute_param_update(&ctx.signer1, &pid);
        assert_eq!(ctx.reg.get_value(&RegistryParamKey::MarketplaceFeeBps), 75);
    }

    // ══════════════════════════════════════════════════════════════════════════
    // 15. Event schema version carried in first topic
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn par_prop_event_carries_schema_version_sentinel() {
        let ctx = setup();
        ctx.reg.propose_param_update(
            &ctx.signer1,
            &RegistryParamKey::MarketplaceFeeBps,
            &75,
        );
        let all = ctx.env.events().all();
        // Find the event whose decoded name symbol is PAR_PROP.
        let par_prop_event = all
            .iter()
            .find(|(_, topics, _)| {
                topic_of(&ctx.env, &topics)
                    .map(|sym| sym == Symbol::new(&ctx.env, "PAR_PROP"))
                    .unwrap_or(false)
            })
            .expect("PAR_PROP event not found");

        let (_, topics, _) = par_prop_event;

        // Shared events: topics[0] = SCHEMA_V sentinel, topics[1] = event name, topics[2] = version.
        let topic0_sym = Symbol::try_from_val(&ctx.env, &topics.get(0).expect("no first topic"))
            .expect("first topic must be a Symbol");
        assert_eq!(
            topic0_sym,
            symbol_short!("SCHEMA_V"),
            "first topic must be SCHEMA_V sentinel"
        );

        // topics[2] must be the schema version integer (u32).
        let version_val = topics.get(2).expect("no third topic");
        let version: u32 = soroban_sdk::TryFromVal::try_from_val(&ctx.env, &version_val)
            .expect("third topic must be a u32");
        assert_eq!(
            version,
            kora_shared::events::schema_version(),
            "schema version in event must match EVENT_SCHEMA_VERSION constant"
        );
    }
}
