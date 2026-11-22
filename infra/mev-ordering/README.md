# Kora MEV Transaction-Ordering Observation Harness

Observes transaction-ordering behaviour on a Stellar/Soroban network and flags
regressions against the policy recorded in
[`docs/MEV_RESISTANCE_NOTES.md`](../../docs/MEV_RESISTANCE_NOTES.md) §1 and §4.1.

`MEV_RESISTANCE_NOTES.md` asserts that Stellar's ordering guarantees (fee
priority under surge, arrival order for equal fees, one transaction per source
account per ledger) make Kora's flows MEV-resistant. Those guarantees are
load-bearing for `fund_invoice`, `buy_position`, oracle conversion and
`PriorityWindow`, but nothing currently watches them. This harness submits
controlled competing transactions, records what the network actually did with
them, and fails when the observed ordering no longer matches the documented
policy.

> **This harness must never run against mainnet.**
> `assertSafeTarget()` (enforced by the CLI and by the Jest global setup)
> refuses `MEV_ENV=production`, the public RPC URL and the public passphrase.
> The default target is an in-process mock chain.

---

## How it works

Each **round** submits a controlled set of competing transactions — one per
source account, each with a known fee and a known submission order — and then
records the ledger number and the RPC-reported `applicationOrder` for every one
of them. `applicationOrder` is the intrinsic execution index inside a ledger,
which is exactly the ordering sample the policy is checked against.

| Round kind | Competition | Policy under test |
|---|---|---|
| `fee_ladder` | N accounts, strictly increasing fees (byte-identical operations) | Higher fee executes earlier (surge priority) |
| `equal_fee` | N accounts, identical fees, known submission order | Equal fees execute in arrival order |
| `same_account_burst` | 2 transactions from **one** account, submitted one at a time (Stellar rejects future sequence numbers) | At most one transaction per source account per ledger |

Kinds cycle, so `MEV_ROUNDS=3` (the default) exercises all three.

Two interchangeable clients implement the same interface, selected by
`MEV_MODE`:

- **`mock`** — an in-process model of Stellar's ordering rules. Deterministic,
  no network, no funded accounts. Used by the unit tests and by default.
- **`testnet`** — the live Soroban RPC, using the same submission lifecycle as
  the repository's existing RPC helpers
  ([`sdk/src/base.ts`](../../sdk/src/base.ts),
  [`services/keeper/src/dispatcher.ts`](../../services/keeper/src/dispatcher.ts)):
  `getAccount` → build → sign → `sendTransaction` → poll `getTransaction`.

The competing operation is a self `manageData` write: no destination, no value
transferred, valid on any funded account. The harness observes ordering without
mutating protocol state.

---

## Quick start

```bash
cd infra/mev-ordering
npm install

# Mock chain (default) — deterministic, offline
npm run harness:mock

# Live testnet — creates + friendbot-funds ephemeral accounts as needed
npm run harness:testnet

# Unit + integration tests
npm test
```

Example mock run:

```
MEV ordering observation — mode=mock result=PASS
rounds=3 txs=10 submitted=10 observed=10 samples=4 hard=0 advisory=0

round 0 (fee_ladder) ledger 1000: fee_ladder-3 -> fee_ladder-2 -> fee_ladder-1 -> fee_ladder-0
round 1 (equal_fee) ledger 1002: equal_fee-0 -> equal_fee-1 -> equal_fee-2 -> equal_fee-3
round 2 (same_account_burst) ledger 1004: same_account_burst-0 | ledger 1006: same_account_burst-1
```

Exit codes: `0` = no hard deviation beyond the threshold, `1` = ordering policy
regressed, `2` = the harness itself failed (RPC unreachable, bad config, …).

---

## Interpreting the output

Every run writes a JSON artifact to `MEV_OUTPUT_DIR` (default `results/`):

- `mev-ordering-latest.json` — always the most recent run.
- `mev-ordering-<timestamp>.json` — one file per run, for trend comparison.

The artifact carries the full evidence, not just a verdict:

```jsonc
{
  "schemaVersion": 1,
  "mode": "mock",
  "network": { "rpcUrl": "mock://in-process", "networkPassphrase": "…" },
  "thresholds": { "strictArrivalOrder": false, "maxHardDeviations": 0, "…": "…" },
  "rounds": [ /* per-round observations, samples and deviations */ ],
  "orderingSamples": [
    {
      "ledger": 1000,
      "entries": [ /* id, sourceIndex, fee, submitOrder, applicationOrder */ ],
      "observedSequence": ["fee_ladder-3", "fee_ladder-2", "fee_ladder-1", "fee_ladder-0"]
    }
  ],
  "summary": {
    "rounds": 3, "txsPlanned": 10, "txsSubmitted": 10, "txsObserved": 10,
    "submissionFailures": 0, "hardDeviations": 0, "advisoryDeviations": 0,
    "orderingSamples": 4, "passed": true
  }
}
```

Use `orderingSamples` to answer "what order did the network actually use?", and
`deviations` to answer "does that match the policy?".

### Deviation codes

| Code | Meaning | Default severity |
|---|---|---|
| `FEE_PRIORITY_VIOLATION` | A lower-fee transaction executed before a higher-fee one in the same ledger — surge priority regressed | advisory (`hard` with `MEV_EXPECT_FEE_PRIORITY=true`) |
| `PER_ACCOUNT_SERIALIZATION_VIOLATION` | One source account produced two transactions in the same ledger — the 1-tx-per-account-per-ledger guarantee regressed | **hard** |
| `ARRIVAL_ORDER_VIOLATION` | Equal-fee transactions did not execute in submission order | advisory (`hard` with `MEV_STRICT_ARRIVAL_ORDER=true`) |
| `MISSING_ORDERING_DATA` | A submitted transaction had no ledger/`applicationOrder` before the observation deadline (RPC lag, delayed inclusion) | advisory |
| `PARTIAL_SUBMISSION_FAILURE` | A transaction was rejected by the RPC or failed on-chain | advisory |

### Thresholds

| Threshold | Env var | Default | Effect |
|---|---|---|---|
| Max hard deviations | `MEV_MAX_HARD_DEVIATIONS` | `0` | `passed = hardDeviations <= MEV_MAX_HARD_DEVIATIONS`; raise temporarily only to triage a known regression |
| Expect fee priority | `MEV_EXPECT_FEE_PRIORITY` | `false` | Assert the target ledger is congested so fee priority is guaranteed; promotes `FEE_PRIORITY_VIOLATION` from advisory to hard |
| Strict arrival order | `MEV_STRICT_ARRIVAL_ORDER` | `false` | Promotes `ARRIVAL_ORDER_VIOLATION` from advisory to hard |
| Observation deadline | `MEV_OBSERVE_TIMEOUT_MS` | `30000` | Bounded wait for a submitted transaction to settle |
| Poll interval | `MEV_POLL_INTERVAL_MS` | `1000` | Delay between `getTransaction` polls in `testnet` mode |

**Why fee priority is advisory by default:** §1.1 of
[`docs/MEV_RESISTANCE_NOTES.md`](../../docs/MEV_RESISTANCE_NOTES.md) scopes fee
priority to *surge pricing* — when a ledger is congested, higher fees win. A
probe-sized burst cannot exceed a public testnet ledger's capacity, so on an idle
ledger the network falls back to arrival order and a fee-order mismatch is
expected, not a regression. Set `MEV_EXPECT_FEE_PRIORITY=true` only when you are
deliberately observing a congested ledger; then the mismatch becomes a hard
failure. Per-account serialization stays hard because it is an unconditional
consensus property (one transaction per source account per ledger).

**Why arrival order is advisory by default:** client-side submission order is
not the network's arrival order — the RPC may fan out to different validators
and reconnect at any time. The harness records the submission order it used and
reports mismatches so they can be trended, but only promotes them to a failure
when the operator opts in.

### Edge cases

- **RPC lag** — transactions that never settle before `MEV_OBSERVE_TIMEOUT_MS`
  are recorded as `MISSING_ORDERING_DATA` (advisory) with a `summary` count, so
  a slow RPC produces a noisy-but-green run instead of a false ordering
  regression. Increase `MEV_OBSERVE_TIMEOUT_MS` on a congested network.
- **Partial submission failure** — a rejected or failed transaction is recorded
  with its error and reported as `PARTIAL_SUBMISSION_FAILURE` (advisory). The
  remaining transactions in the round are still observed, so one bad account
  does not invalidate the whole ordering sample.

---

## Environment variables

Copy `.env.mev.example` to `.env.mev` (git-ignored) or export variables
directly. All values have mock-safe defaults.

| Variable | Default | Purpose |
|---|---|---|
| `MEV_MODE` | `mock` | `mock` or `testnet` |
| `MEV_ENV` | `staging` | Environment tag; `production` is refused |
| `MEV_RPC_URL` | `https://soroban-testnet.stellar.org` | Soroban RPC endpoint (`testnet` mode) |
| `MEV_NETWORK_PASSPHRASE` | `Test SDF Network ; September 2015` | Passphrase matching `MEV_RPC_URL` |
| `MEV_SOURCE_SECRETS` | *(empty)* | Comma-separated source-account secret keys; missing slots are filled with ephemeral friendbot-funded accounts |
| `MEV_AUTOFUND_ACCOUNTS` | `true` | Fund missing/ephemeral accounts through friendbot |
| `MEV_FRIENDBOT_URL` | *(empty)* | Explicit friendbot URL (blank = SDK discovery) |
| `MEV_ACCOUNTS_PER_ROUND` | `4` | Competing transactions / accounts per round |
| `MEV_ROUNDS` | `3` | Number of rounds |
| `MEV_BASE_FEE` | `100` | Fee of the cheapest transaction, in stroops |
| `MEV_FEE_STEP` | `500` | Fee increment between tiers, in stroops |
| `MEV_SUBMIT_STAGGER_MS` | `0` | Delay between competing submissions; `0` fires them all at once |
| `MEV_OBSERVE_TIMEOUT_MS` | `30000` | Bounded wait for settlement |
| `MEV_POLL_INTERVAL_MS` | `1000` | Poll delay in `testnet` mode |
| `MEV_EXPECT_FEE_PRIORITY` | `false` | Assert the ledger is congested; treat fee-priority mismatches as hard failures |
| `MEV_STRICT_ARRIVAL_ORDER` | `false` | Treat arrival-order mismatches as hard failures |
| `MEV_MAX_HARD_DEVIATIONS` | `0` | Tolerated hard deviations |
| `MEV_OUTPUT_DIR` | `results/` | Artifact directory |
| `LOG_LEVEL` | `info` | pino log level (`silent` for CI-friendly output) |

CLI flags `--mode`, `--rounds`, `--accounts`, `--output-dir` override the
corresponding variables for a single run (`npm run harness -- --help`).

---

## Running nightly

`.github/workflows/mev-ordering-nightly.yml` runs the harness against testnet
every night at 03:00 UTC and on `workflow_dispatch`, then uploads the JSON
artifact.

| Setting | Value |
|---|---|
| Schedule | `0 3 * * *` (nightly, 03:00 UTC) |
| Manual run | `workflow_dispatch` with optional `rounds` / `accounts` inputs |
| Artifact | `infra/mev-ordering/results/mev-ordering-*.json`, retained 30 days |
| Failure behaviour | Non-zero exit when hard deviations exceed the threshold; the artifact is uploaded either way |

Nightly runs use `MEV_MODE=testnet` (no secrets required — ephemeral accounts
are friendbot-funded) with `MEV_ROUNDS=6` for a slightly larger sample. Compare
consecutive `mev-ordering-latest.json` artifacts to spot ordering trends before
they become failures.
