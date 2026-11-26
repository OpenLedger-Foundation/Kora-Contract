# Soroban Storage Rent & TTL Cost Model

This guide explains Soroban's underlying rent economics, which storage keys are most expensive to keep alive, the current TTL constants used in each Kora contract, and worked cost projections at protocol scale.

> **Audience:** keeper-bot operators (B5), archival engineers (B19), and anyone estimating ongoing infrastructure costs.

---

## Background: How Soroban Rent Works

Every piece of data written to a Soroban contract occupies ledger space. Stellar charges **rent** to keep that data alive, expressed as a required minimum TTL (time-to-live) measured in ledgers. When a key's TTL reaches zero, the entry is **archived** — the data still exists in a historical archive but the contract can no longer read it without a restoration transaction.

Rent is paid at write time and at explicit `extend_ttl` calls. There is **no periodic charge** — you simply must extend TTL before it expires or risk archival.

**Ledger timing:** one ledger closes approximately every 5 seconds on Stellar mainnet.

```
1 day   ≈  17 280 ledgers
1 week  ≈ 120 960 ledgers
30 days ≈ 518 400 ledgers
31 days ≈ 535 680 ledgers
```

---

## Three Storage Tiers

| Tier | Soroban API | Lifetime | Typical use in Kora |
|------|-------------|----------|---------------------|
| **Persistent** | `env.storage().persistent()` | Managed manually via `extend_ttl` | Per-entity data: invoices, listings, pools, profiles |
| **Instance** | `env.storage().instance()` | Tied to the contract instance TTL | Contract-level config: admin, fee_bps, flags |
| **Temporary** | `env.storage().temporary()` | Expires automatically (max TTL capped) | Not used in Kora v1 |

### Persistent storage

Most expensive to maintain at scale — each key has its own TTL that must be extended independently. Missing an extension archives the key and requires a restore transaction before the contract can read it.

**Kora usage:** invoices, listings, pools, positions, SME profiles, verifier data, debtor scores, token whitelists, fee balances.

### Instance storage

Shares one TTL with the entire contract instance. Extending the instance TTL extends all instance-storage keys simultaneously — much cheaper per-key at scale.

**Kora usage:** admin address, fee_bps, paused flag, reentrancy lock, rate-limit counters, upgrade/cap proposals.

### Temporary storage

Expires automatically at a ledger-level cap (currently 1 day on mainnet). No manual extension needed, but data is not recoverable after expiry. Kora v1 does not use this tier.

---

## TTL Constants Per Contract

All constants assume ~5 s per ledger.

| Contract | Constant name | Value (ledgers) | ~Duration | Applied to |
|----------|--------------|-----------------|-----------|------------|
| `treasury` | `PERSISTENT_BUMP_AMOUNT` | 535 680 | 31 days | Admin, FeeBps, Collected, WhitelistedToken |
| `treasury` | `PERSISTENT_LIFETIME_THRESHOLD` | 267 840 | 15.5 days | Trigger threshold for bump |
| `risk_registry` | `PERSISTENT_TTL_BUMP` | 518 400 | 30 days | Verifier, VerifierStake, VerifierReputation, SmeProfile, DebtorScoreAttestation, DebtorAttestors |
| `risk_registry` | `PERSISTENT_TTL_THRESHOLD` | 518 400 | 30 days | Trigger threshold |
| `invoice_nft` | (see source) | ≥ 518 400 | ≥ 30 days | Invoice entries |
| `marketplace` | (see source) | ≥ 518 400 | ≥ 30 days | Listing entries |
| `financing_pool` | (see source) | ≥ 518 400 | ≥ 30 days | Pool, Position entries |
| `access_control` | (see source) | ≥ 518 400 | ≥ 30 days | Role entries |

**Threshold semantics:** `extend_ttl(key, threshold, bump)` only performs the extension if the remaining TTL is below `threshold`, avoiding redundant writes on hot keys.

---

## What Keys Cost the Most

Cost scales with two factors: **entry size** (bytes stored) and **extension frequency** (how often TTL must be bumped).

### Highest cost at scale

1. **`Pool(u64)` + `Positions(u64)`** in `financing_pool` — one pool entry and one positions map per invoice. The positions map grows with the number of investors per invoice. A pool with 100 investors has a large serialized `Map<Address, Position>` that costs proportionally more per bump.

2. **`Invoice(u64)`** in `invoice_nft` — one entry per minted invoice, held for the full invoice lifetime (potentially years for long-dated trade finance).

3. **`SmeProfile(Address)`** and **`DebtorScoreAttestation(Bytes, Address)`** in `risk_registry` — long-lived entries for every registered SME and debtor attestation.

### Lower cost

- **Instance storage** keys (admin, fee_bps, flags) — one TTL extension bumps all of them together.
- **`Listing(u64)`** entries — shorter-lived (active only until funded or expired).

---

## Worked Cost Projections at Scale

Assumptions:
- 5 s per ledger
- Rent cost = 0.0001 XLM per ledger per entry (approximate; see Stellar fee documentation for current rates)
- 30-day bump interval matches `PERSISTENT_TTL_BUMP ≈ 518 400`

### Scenario A — 1 000 active invoices

| Entry type | Count | Bumps/month | XLM/month (est.) |
|------------|-------|-------------|-----------------|
| Invoice entries | 1 000 | 1 000 | 0.1 |
| Listing entries | 1 000 | 1 000 | 0.1 |
| Pool + Positions (avg 10 investors) | 1 000 | 1 000 | ~0.2 (size premium) |
| SME profiles | 200 | 200 | 0.02 |
| **Total** | | | **~0.42 XLM/month** |

### Scenario B — 10 000 active invoices

Linear scaling from Scenario A: **~4.2 XLM/month** for persistent storage TTL extensions.

These are order-of-magnitude estimates. Actual fees depend on Stellar's current fee schedule and the serialized byte size of each entry.

---

## Keeper Bot Responsibilities (B5)

A keeper bot must periodically call `extend_ttl` on persistent keys before they drop below the threshold. Recommended approach:

1. **Enumerate live keys** — query indexed invoice IDs, listing IDs, pool IDs, and registered SME/verifier addresses from contract events or an off-chain index.

2. **Check remaining TTL** — use the Stellar RPC `getLedgerEntries` call to read the current `liveUntilLedger` for each key.

3. **Bump if below threshold** — submit a transaction with `extend_ttl` only if `liveUntilLedger - currentLedger < threshold`.

4. **Batch where possible** — multiple `extend_ttl` operations can be packed into a single transaction to reduce fee overhead.

### Dry-run rehearsal (#<keeper-dry-run>)

Before burning fees on a mass extend, operators can rehearse the decision
read-only using the keeper's dry-run mode (`services/keeper`):

- `KEEPER_DRY_RUN=1 npm start` runs once, prints a JSON report, and exits
  without starting the scheduler or submitting any transaction.
- `GET /dry-run` (on the running keeper) returns the same report on demand.

The dry run classifies each watched key as **instance** (the contract instance
entry, whose TTL covers all instance-storage keys at once) or **persistent**
(per-listing `Listing` entries), reports each key's `remainingLedgers` against
`KEEPER_TTL_THRESHOLD_LEDGERS`, and flags `needsBump` for keys below the
threshold. Missing (archived/expired) entries are flagged `needsBump + missing`
rather than silently skipped. Use it to size a planned extend and to confirm
the instance bump (cheap, covers all instance keys) separately from the more
numerous persistent listing bumps.

**Priority order for bumping:**
1. Pool + Positions (investor funds at risk if archived)
2. Invoice entries (canonical state machine)
3. SME profiles + Verifier entries
4. Listing entries (active funding rounds)

---

## Archival and Restoration (B19)

If a key expires, the entry is archived into Stellar's historical ledger. To restore it:

1. Locate the archived entry via the Stellar archive (Horizon or direct archive node).
2. Submit a `restoreFootprint` transaction referencing the entry's ledger key.
3. Once restored, immediately `extend_ttl` to prevent immediate re-archival.
4. Verify the contract can read the entry before resuming normal operations.

**Key risk:** an archived `Pool` or `Invoice` entry means `repay()` and `mark_default()` will fail until restoration. SME funds may be temporarily locked. Keeper bots must treat pool and invoice entries as highest priority.

---

## References

- [Stellar Soroban Storage documentation](https://developers.stellar.org/docs/build/smart-contracts/storage)
- [Stellar fee schedule](https://developers.stellar.org/docs/learn/fundamentals/fees-resource-limits-metering)
- `scripts/ttl_keeper.sh` — reference keeper script in this repo
- `docs/ARCHITECTURE.md § Storage Layout` — per-contract storage key reference
