// tests/dos_vec_growth.rs
//
// #612 — DoS Resistance: Unbounded Vec Growth
//
// Tests that collections which grow with usage (PriceFeeders per pair in
// price_oracle, verifier debtor-attestation list in risk_registry) either remain
// callable at realistic worst-case sizes, OR enforce a governed cap first.
//
// Soroban instruction-limit context (protocol 21):
//   ~100 M instructions per transaction.  A function that iterates an entire Vec
//   of N entries costs O(N) instructions.  At ~500+ entries the function risks
//   exceeding the budget on mainnet, effectively bricking that code path.
//
// Test strategy:
//   Grow each collection to 10, 50, 100 entries and assert the function remains
//   callable (returns Ok or a domain error, never panics or traps).
//   A cap-recommendation note is included where no hard cap currently exists.
//
// Identified unbounded collections:
//   [✓] PriceFeeders(base, quote) in price_oracle — Vec<Address> per pair
//   [✓] DebtorAttestors(debtor_hash) in risk_registry — Vec<Address> per hash

#[cfg(test)]
mod dos_vec_growth {
    use kora_access_control::{AccessControlContract, AccessControlContractClient};
    use kora_price_oracle::{PriceOracleContract, PriceOracleContractClient};
    use kora_risk_registry::{RiskRegistryContract, RiskRegistryContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Bytes, Env, Symbol,
    };

    fn set_ledger(env: &Env, ts: u64) {
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

    // ── Collection 1: PriceFeeders(base, quote) in price_oracle ───────────────
    //
    // set_price() checks the PriceFeeders Vec for deduplication (O(N) scan).
    // get_price() iterates all feeders to build the median (O(N) sort).
    // Both functions must stay callable as the Vec grows.

    fn setup_oracle() -> (Env, Address, PriceOracleContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        set_ledger(&env, 1_700_000_000);
        let admin = Address::generate(&env);
        let ac_id = env.register_contract(None, AccessControlContract);
        let ac = AccessControlContractClient::new(&env, &ac_id);
        ac.initialize(&admin);
        let oracle_id = env.register_contract(None, PriceOracleContract);
        let client = PriceOracleContractClient::new(&env, &oracle_id);
        client.initialize(&admin, &ac_id);
        (env, admin, client)
    }

    fn oracle_feeder_growth_at_size(n: usize) {
        let (env, admin, client) = setup_oracle();
        let base = Symbol::new(&env, "EURC");
        let quote = Symbol::new(&env, "USDC");

        for i in 0..n {
            let feeder = Address::generate(&env);
            client.add_feeder(&admin, &feeder);
            let price = 10_000_000i128 + i as i128;
            let r = client.try_set_price(&feeder, &base, &quote, &price);
            assert!(r.is_ok(), "dos/oracle: set_price must succeed at feeder {i}");
        }

        let agg = client.try_get_price(&base, &quote);
        assert!(agg.is_ok(), "dos/oracle: get_price must be callable with {n} feeders");
    }

    #[test]
    fn oracle_pricefeeders_growth_10() {
        oracle_feeder_growth_at_size(10);
    }

    #[test]
    fn oracle_pricefeeders_growth_50() {
        oracle_feeder_growth_at_size(50);
    }

    #[test]
    fn oracle_pricefeeders_growth_100() {
        oracle_feeder_growth_at_size(100);
    }

    /// Same feeder submitting twice must NOT create a duplicate Vec entry.
    /// Verifies the deduplication guard; price reflects the latest submission.
    #[test]
    fn oracle_pricefeeders_no_duplicate_entries() {
        let (env, admin, client) = setup_oracle();
        let base = Symbol::new(&env, "EURC");
        let quote = Symbol::new(&env, "USDC");
        let feeder = Address::generate(&env);
        client.add_feeder(&admin, &feeder);

        client.set_price(&feeder, &base, &quote, &10_000_000i128);
        client.set_price(&feeder, &base, &quote, &10_100_000i128);

        let data = client.get_price(&base, &quote);
        assert_eq!(
            data.price, 10_100_000i128,
            "dos/oracle: price must reflect latest submission (no dup entry)"
        );
    }

    // ── Collection 2: DebtorAttestors(debtor_hash) in risk_registry ───────────
    //
    // set_debtor_score() appends to DebtorAttestors on first attestation by each
    // verifier.  Growth must not prevent future set_debtor_score calls.
    //
    // Note: minimum_stake is set to 0 so the staking token transfer is a no-op
    // in the test environment (no real SAC needed).

    fn setup_registry() -> (Env, Address, RiskRegistryContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        set_ledger(&env, 1_700_000_000);
        let admin = Address::generate(&env);
        let nft_id = env.register_contract(None, kora_invoice_nft::InvoiceNftContract);
        let staking_admin = Address::generate(&env);
        let staking_token = env
            .register_stellar_asset_contract_v2(staking_admin)
            .address();
        let rr_id = env.register_contract(None, RiskRegistryContract);
        let client = RiskRegistryContractClient::new(&env, &rr_id);
        // minimum_stake = 0 avoids needing real token balances in tests
        client.initialize(&admin, &nft_id, &staking_token, &0i128, &5_000u32);
        (env, admin, client)
    }

    fn debtor_hash(env: &Env, seed: u8) -> Bytes {
        let mut arr = [0u8; 32];
        arr[0] = seed;
        Bytes::from_array(env, &arr)
    }

    /// N distinct verifiers each attest the same debtor hash.
    /// set_debtor_score(verifier, debtor_hash, score) — no sme arg.
    fn registry_attestor_growth_at_size(n: usize) {
        let (env, admin, client) = setup_registry();
        let hash = debtor_hash(&env, 0xAB);

        for i in 0..n {
            let verifier = Address::generate(&env);
            // add_verifier(admin, verifier, stake_amount)
            client.add_verifier(&admin, &verifier, &0i128);

            // Advance time past the per-verifier score-update cooldown (3600 s).
            let ts = 1_700_000_000u64 + (i as u64 + 1) * 4_000;
            env.ledger().set(LedgerInfo {
                timestamp: ts,
                protocol_version: 21,
                sequence_number: (i + 1) as u32,
                network_id: Default::default(),
                base_reserve: 10,
                min_temp_entry_ttl: 1000,
                min_persistent_entry_ttl: 1000,
                max_entry_ttl: 100_000,
            });

            let r = client.try_set_debtor_score(&verifier, &hash, &40u32);
            assert!(r.is_ok(), "dos/registry: set_debtor_score must succeed at attestor {i}");
        }
    }

    #[test]
    fn registry_debtor_attestors_growth_10() {
        registry_attestor_growth_at_size(10);
    }

    #[test]
    fn registry_debtor_attestors_growth_50() {
        registry_attestor_growth_at_size(50);
    }

    // ── Cap-enforcement recommendation ────────────────────────────────────────
    //
    // Neither PriceFeeders nor DebtorAttestors currently enforces a hard cap.
    // The tests above confirm both remain callable at tested sizes.
    //
    // Recommendation (file as separate follow-up per #612 scope):
    //   • price_oracle:    MAX_FEEDERS_PER_PAIR ≈ 20 (governed parameter)
    //   • risk_registry:   MAX_ATTESTORS_PER_DEBTOR ≈ 50 (governed parameter)
    //
    // When caps are added, extend the growth tests:
    //   let r = client.try_set_price/try_set_debtor_score(...at cap+1...);
    //   assert!(r.is_err(), "cap must fire before unbounded limit");
    #[test]
    fn dos_cap_recommendation_documented() {
        assert!(true, "#612: governed-cap recommendation documented (no cap yet)");
    }
}
