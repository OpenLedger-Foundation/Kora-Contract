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
