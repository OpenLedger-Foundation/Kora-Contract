# Keeper Service

**Issue #765 — Background Job Queue for Deadline-Based Contract Triggers**

A resilient background keeper that watches on-chain deadlines (funding expiry, invoice due dates, grace-period end) and automatically submits the appropriate **permissionless** trigger transactions so users do not have to monitor the chain manually.

---

## Architecture

```
Indexer (deadline export: /api/v1/deadlines?horizon_sec=…)
        │  IndexerDeadlineClient.fetchDeadlines() every DEADLINE_POLL_INTERVAL_MS
        ▼
  Scheduler.enqueueDeadlines()
        │
        ▼
  JobStore (SQLite, dedup_key UNIQUE)
        │  promoteExpired() on every tick
        ▼
  Dispatcher.dispatch()
        │  due_date / grace_period_end → simulateTransaction precheck (#1187)
        │     ├─ simulation error, no-op code → already_triggered (no tx sent)
        │     ├─ simulation error, other      → failed / permanent_failure (no tx sent)
        │     └─ RPC unavailable              → failed (precheck_unavailable; never submits blind)
        │
        ├─ submitted      → job: in_flight
        ├─ already_triggered (no-op) → job: done
        ├─ failed (transient) → retry with exponential back-off
        └─ permanent_failure → job: dead (dead-letter queue)
```

### Key design decisions

| Concern | Decision |
|---|---|
| Deduplication across restarts | `INSERT OR IGNORE` on `dedup_key` (SQLite UNIQUE) — a restart never re-enqueues |
| Already-triggered detection | Known no-op error codes (`AlreadyDefaulted`, `AlreadyRefunded`, etc.) are treated as `done`, not `failed` |
| Default-mark precheck | `due_date` / `grace_period_end` jobs are simulated before submit; reverting or already-defaulted invoices never reach the network, and skipped jobs do not consume the rate-limit budget. Counters via `Dispatcher.getPrecheckMetrics()` (`simulations`, `skippedIneligible`, `simulationErrors`) |
| Rate limiting | Token-bucket: max 10 tx/min on the keeper account |
| Fee bumping | Base 100 stroops × 1.1^attempts, capped at 10 000 stroops |
| Retry back-off | Exponential: `backoffBaseMs × 2^(attempts-1)` |
| Dead-letter | After `maxAttempts` failures, job is moved to `dead` for manual inspection |

---

## Configuration

| Env var | Default | Description |
|---|---|---|
| `STELLAR_RPC_URL` | `https://soroban-testnet.stellar.org` | Soroban RPC endpoint |
| `KEEPER_SECRET` | _(required)_ | Ed25519 secret key of keeper account |
| `KEEPER_DB_PATH` | `./keeper-jobs.db` | SQLite file path |
| `KEEPER_POLL_INTERVAL_MS` | `30000` | Tick interval (ms) |
| `KEEPER_MAX_ATTEMPTS` | `5` | Attempts before dead-letter |
| `KEEPER_RETRY_BACKOFF_BASE_MS` | `60000` | Back-off base (ms) |
| `KEEPER_HTTP_PORT` | `8080` | Observability HTTP port |
| `STELLAR_NETWORK_PASSPHRASE` | testnet | Network passphrase |
| `MARKETPLACE_CONTRACT_ID` | _(required)_ | Marketplace contract containing the listing entries |
| `KEEPER_ACTIVE_LISTINGS_URL` | _(required)_ | Paginated indexer endpoint for active listings |
| `KEEPER_INDEXER_API_KEY` | _(empty)_ | Optional Bearer token for the active-listings endpoint |
| `KEEPER_TTL_THRESHOLD_LEDGERS` | `518400` | Bump listings below this remaining TTL |
| `KEEPER_DRY_RUN` | _(off)_ | When `true`/`1`, run a read-only TTL rehearsal once, print the JSON report, and exit without starting the scheduler or submitting any transaction |

The active-listings endpoint receives `isActive=true`, `limit=100`, and an
optional `cursor` query parameter. It must return pages shaped as
`{"data":[{"invoiceId":"123","isActive":true}],"nextCursor":null}`;
`invoiceId` is the marketplace listing's u64 key. The keeper checks each key's
`liveUntilLedgerSeq` using Soroban RPC and enqueues a deduplicated bump job when
fewer than the configured number of ledgers remain. A job key includes the
observed expiry ledger, so a later TTL cycle can be scheduled after a successful
bump. Keep the endpoint backed by the same indexer/network as `STELLAR_RPC_URL`.

In deployed environments, inject `KEEPER_SECRET` and any RPC credentials from
the environment-specific secret in AWS Secrets Manager using the workload's
read-only role. Do not add credentials to deployment files or `.env` files.
For local work, use disposable testnet credentials in a mode-`0600` file under
`$HOME/.config/kora/` and source it into the process environment; never use
production credentials locally. See [the secrets runbook](../../infra/secrets/README.md).

---

## Observability endpoints

| Path | Description |
|---|---|
| `GET /health` | Liveness check |
| `GET /status` | Job counts by status |
| `GET /metrics` | Prometheus keeper job gauges |
| `GET /history` | Last 200 job records |
| `GET /dry-run` | Read-only TTL-bump rehearsal report (instance vs persistent keys + needs-bump); never enqueues or dispatches |

### Deadline ingestion env vars (#1022)

| Var | Default | Purpose |
|---|---|---|
| `INDEXER_URL` | `KEEPER_ACTIVE_LISTINGS_URL` | Indexer deadline export endpoint |
| `DEADLINE_HORIZON_SEC` | `86400` | Only ingest deadlines within now + horizon (elapsed always ingested) |
| `DEADLINE_POLL_INTERVAL_MS` | `60000` | Ingestion cadence, separate from the dispatch tick |
| `KEEPER_INDEXER_API_KEY` | _(empty)_ | Bearer token for the indexer endpoint |

Indexer 503s/timeouts resolve to an empty batch (degraded health, logged
with jittered backoff) and never corrupt SQLite job state. Unset
`INDEXER_URL` keeps the manual seed path (`enqueueDeadlines`) for local dev.

---

## Lease Fencing (Multi-Replica Safety)

When multiple keeper replicas run against the same job database (e.g. in an active-passive HA setup), only the **lease holder** runs dispatch ticks.  Other replicas skip the tick silently and re-contest the lease on the next poll cycle.

### How it works

1. Each replica has a unique `holderId` (defaults to `$HOSTNAME-$PID`).
2. Before dispatching, the scheduler calls `LeaseStore.acquireLease("keeper:dispatch", holderId, ttlS)`.
3. If acquired, the replica dispatches normally and renews the lease after the tick.
4. If another non-expired replica holds the lease, the tick is skipped.
5. If the holder crashes without releasing, the lease expires after `KEEPER_LEASE_TTL_S` seconds and the next replica to tick takes over.

### Configuration

| Env var | Default | Description |
|---|---|---|
| `KEEPER_LEASE_DB_PATH` | `KEEPER_DB_PATH` | SQLite file for lease records (can share with job DB) |
| `KEEPER_LEASE_TTL_S` | `60` | Lease TTL in seconds |
| `KEEPER_HOLDER_ID` | `$HOSTNAME-$PID` | Unique replica identifier |

Pass `KEEPER_LEASE_DB_PATH` to a shared volume so all replicas see the same lease table.

### Clock skew

The lease acquisition logic includes a 2-second clock-skew tolerance (`clockSkewTolS`).  Replicas whose clocks differ by up to 2 seconds will still elect a single holder correctly.  For larger skews, set `clockSkewTolS` via constructor injection in the `LeaseStore`.

### Ops runbook

| Situation | Action |
|---|---|
| Replica with the lease is healthy | Do nothing — lease renews automatically |
| Replica with the lease is stuck / crashed | Wait up to `KEEPER_LEASE_TTL_S` seconds for the next replica to take over, or manually `DELETE FROM keeper_leases WHERE lease_name = 'keeper:dispatch'` and restart the target replica |
| All replicas show "lease refused" in logs | Check that all replicas point to the same `KEEPER_LEASE_DB_PATH` |
| Need to force-promote a specific replica | On the target replica: `DELETE FROM keeper_leases WHERE lease_name = 'keeper:dispatch'`; the next tick will acquire it |

---

## Running

```bash
npm install
npm run build
npm start
```

Start the service only after its environment has been populated through the
local testnet secret file or the deployment secret manager. Do not place secret
values directly in shell command history.

## Testing

```bash
npm test
```

Minimum 90% line/branch/function/statement coverage enforced.

TTL scan metrics are also exported at `/metrics`:
`kora_keeper_ttl_listings_scanned_total`,
`kora_keeper_ttl_listings_at_risk_total`, and
`kora_keeper_ttl_scan_errors_total`.

---

## TTL-bump dry-run (#<keeper-dry-run>)

Before paying for a mass `extend_ttl`, operators can rehearse the decision
read-only. The dry run scans the marketplace contract's persistent `Listing`
entries **and** its contract *instance* entry, classifies each as `instance` or
`persistent`, and reports which are below `KEEPER_TTL_THRESHOLD_LEDGERS` (i.e.
need a bump) — **without enqueuing, dispatching, or submitting any
transaction**.

```bash
# One-shot: print the report and exit (no scheduler, no writes)
KEEPER_DRY_RUN=1 npm start
```

The report is also available at `GET /dry-run` while the service is running:

```bash
curl -s http://localhost:8080/dry-run | jq
```

Report shape:

```json
{
  "contractAddress": "C…",
  "latestLedger": 12345,
  "thresholdLedgers": 518400,
  "entries": [
    { "key": "instance", "durability": "instance", "remainingLedgers": 400000, "needsBump": false, "missing": false },
    { "key": "listing:42", "durability": "persistent", "invoiceId": "42", "liveUntilLedgerSeq": 124000, "remainingLedgers": 555, "needsBump": true, "missing": false }
  ],
  "summary": { "total": 2, "instance": 1, "persistent": 1, "needsBump": 1, "healthy": 1, "missing": 0 }
}
```

Missing (archived/expired) entries are flagged `needsBump: true, missing: true`
rather than silently skipped, so they surface for restoration. The dry run
fails closed with a typed `DryRunConfigError` when `MARKETPLACE_CONTRACT_ID` is
missing or `KEEPER_TTL_THRESHOLD_LEDGERS` is non-positive.
