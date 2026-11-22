# Kora Chaos Engineering Suite

[![Staging chaos suite](https://github.com/OpenLedger-Foundation/Kora-Contract/actions/workflows/chaos-staging.yml/badge.svg)](https://github.com/OpenLedger-Foundation/Kora-Contract/actions/workflows/chaos-staging.yml)

Deliberately injects failures into the **staging** deployment to verify the
resilience behaviours claimed by other issues in this wave.

> **This suite must never run against production.**
> The `global-setup.ts` pre-flight guard enforces this via `CHAOS_ENV`.

---

## GitHub Actions CI

The workflow at [`.github/workflows/chaos-staging.yml`](../../.github/workflows/chaos-staging.yml)
runs the full chaos suite automatically and on demand.

### Triggers

| Trigger | When |
|---|---|
| `schedule` | Every Monday at 02:00 UTC |
| `workflow_dispatch` | Manual — from the [Actions tab](https://github.com/OpenLedger-Foundation/Kora-Contract/actions/workflows/chaos-staging.yml) |

### `workflow_dispatch` inputs

| Input | Default | Description |
|---|---|---|
| `scenario` | _(empty — full suite)_ | Run a single scenario by name: `rpc-outage`, `db-latency`, `keeper-crash`, `network-partition`, `indexer-checkpointing`, `reconciliation-alerting` |
| `alert_lag_ms` | `15000` | Override `CHAOS_RECONCILER_ALERT_LAG_MS` for slower environments |
| `dry_run` | `false` | Print the Jest command without executing — useful for validating inputs |

### Secrets and variables

Configure these in the **`staging`** GitHub environment (Settings → Environments):

| Name | Kind | Required | Description |
|---|---|---|---|
| `CHAOS_DATABASE_URL` | Secret | **Yes** | Postgres DSN for the staging database (`postgres://user:pass@host:5432/db`) |
| `CHAOS_STELLAR_RPC_URL` | Variable | No | Stellar RPC base URL (default: `http://localhost:8000`) |
| `CHAOS_RECONCILER_ALERT_LAG_MS` | Variable | No | Alert lag window override in ms (default: `15000`) |
| `CHAOS_REQUIRED` | Variable | No | Set to `false` **only** when triaging a known transient environment failure. Omit or set to `true` to enforce failures (default behaviour). |

#### Missing credentials behaviour

If `CHAOS_DATABASE_URL` is absent the job fails immediately with:

```
error: Missing secret — CHAOS_DATABASE_URL is not set.
Configure the secret in the 'staging' GitHub environment.
See infra/chaos/.env.chaos.example for the full variable reference.
```

No containers are started and no failure is injected.

### Artifacts

Each run uploads `infra/chaos/results/chaos-summary.txt` as an artifact named
`chaos-staging-<run-id>`.  The last 40 lines are also written to the
[job summary](https://docs.github.com/en/actions/using-workflows/workflow-commands-for-github-actions#adding-a-job-summary)
for quick inspection without downloading the artifact.

### Invariant enforcement

The job exits non-zero whenever any Jest test fails.  The `tee` pipeline in the
run step uses `${PIPESTATUS[0]}` to propagate the Jest exit code, ensuring that
a piped summary never masks a failure.  `continue-on-error` defaults to `false`;
it can only be relaxed by explicitly setting `CHAOS_REQUIRED=false` in the
repository/environment variables.

---

## Scenarios

| # | Scenario file | Target | Resilience Claim Tested |
|---|---|---|---|
| 1 | `scenario-rpc-outage` | Indexer | Resumes from checkpoint after RPC downtime; no duplicate event processing |
| 2 | `scenario-db-latency` | API + Indexer | Remains available or returns 5xx (not silent stale 200) under 2 s DB latency |
| 3 | `scenario-keeper-crash` | Keeper | No double-submitted transactions after SIGKILL; dedup_key uniqueness survives crash |
| 4 | `scenario-network-partition` | API | Returns 5xx during API↔DB partition; recovers cleanly after reconnect |
| 5 | `scenario-indexer-checkpointing` | Indexer | Persists checkpoint to durable storage; resumes from persisted ledger after SIGKILL, not from 0 |
| 6 | `scenario-reconciliation-alerting` | Reconciler | Fires alert within lag window after ledger gap; buffers alerts when sink is down and drains on recovery |

---

## Staging Run Results

| Scenario | Status | Notes |
|---|---|---|
| RPC outage | PASS | Checkpoint resumed at correct ledger; event count monotonically non-decreasing |
| DB latency | PASS | API returns 503 with `DB_TIMEOUT` under sustained latency; positive `DB_STATEMENT_TIMEOUT_MS` is enforced |
| Keeper crash | PASS | No duplicate dedup_keys in done set; in_flight drained to 0 after restart |
| Network partition | **FAIL / GAP-1** | API serves stale 200 from read-through cache for ~8 s after partition (see gap below) |
| Indexer checkpointing | PASS | Post-SIGKILL restart begins at persisted checkpoint ≥ pre-crash value; monotonicity holds across 3 rapid restart cycles |
| Reconciliation alerting — gap→alert | PASS | Alert fired within 15 s lag window; gaps-detected counter incremented; pending alert queue non-empty |
| Reconciliation alerting — buffering | PASS | SQLite-backed queue retains alerts during sink outages and drains after acknowledgement |

---

## Identified Gaps (to be filed as follow-up issues)

### GAP-1 — API read-through cache masks DB partition (network-partition scenario)

- **What happened:** The API's response-level cache returns stale 200 responses
  for up to 8 seconds after the DB network is disconnected.  The resilience
  claim requires 5xx on all DB-dependent reads once the pool is exhausted.
- **Impact:** Callers cannot distinguish "real 200" from "stale cached 200"
  during a partition, leading to silent data staleness in dashboards.
- **Proposed fix:** Set cache TTL to 0 on any `Error` thrown by the DB layer,
  or disable the cache layer entirely during detected DB unavailability.
- **Priority:** High — affects data integrity guarantees during incidents.

### GAP-2 — DB connection pool lacks `statement_timeout` (resolved)

- **What happened:** Under 2 s round-trip latency, some API requests hang for
  up to 30 s rather than returning a 503 with `Retry-After`.
- **Impact:** Client-facing timeouts surface as HTTP client errors rather than
  clean 503 responses with retry guidance.
- **Resolution:** `services/api/gateway/src/dbTimeout.ts` configures the session
  timeout and maps SQLSTATE `57014` to a structured 503 `DB_TIMEOUT` response.
  `readDbStatementTimeout()` validates `DB_STATEMENT_TIMEOUT_MS` at startup and
  rejects zero, negative, or non-integer values.  The gateway throws
  `DbTimeoutError` (distinct from the generic `ApiUnavailableError`) so
  transport adapters can emit `HTTP 503` + `Retry-After: <seconds>` with the
  exact body `{ "code": "DB_TIMEOUT", "retryable": true }`.
  Closes #1095.
- **Priority:** Medium (resolved).

### GAP-3 — Reconciler drops alerts when alert sink is unavailable (resolved)

- **What happened:** When the alert sink container is paused, the reconciler's
  alert queue depth stays at 0 — alerts created during the outage are silently
  discarded rather than queued for retry.  On sink recovery, no buffered alerts
  are delivered.
- **Impact:** Operators receive no notification of ledger gaps that occurred
  during an alerting channel outage, defeating the reconciler's purpose during
  the most critical failure window (when both a gap and a sink outage coincide).
- **Resolution:** `services/reconciler` persists `pending_alerts` in SQLite,
  retries with exponential back-off, and removes an alert only after a 2xx sink
  acknowledgement. The admin endpoints expose queue depth and pending records.
- **Priority:** High — this is a single point of failure in the observability stack.

---

## Running Against Staging

```bash
# 1. Ensure the staging stack is up:
docker compose -p kora-staging up -d

# 2. Configure environment (copy and edit):
cp .env.chaos.example .env.chaos
# Edit CHAOS_API_URL, CHAOS_INDEXER_ADMIN_URL, etc.

# 3. Install dependencies:
npm install

# 4. Run the full chaos suite:
npm test

# 5. Run a single scenario:
npm test -- --testPathPattern=scenario-rpc-outage

# 6. Run with verbose reconciler lag window:
CHAOS_RECONCILER_ALERT_LAG_MS=30000 npm test -- --testPathPattern=scenario-reconciliation-alerting
```

---

## Configuration

Copy `.env.chaos.example` to `.env.chaos` and populate:

```
CHAOS_ENV=staging
CHAOS_API_URL=http://localhost:3000
CHAOS_INDEXER_ADMIN_URL=http://localhost:4000
CHAOS_KEEPER_ADMIN_URL=http://localhost:8080
CHAOS_DOCKER_PROJECT=kora-staging
CHAOS_DB_CONTAINER=kora-postgres
CHAOS_INDEXER_CONTAINER=kora-indexer
CHAOS_KEEPER_CONTAINER=kora-keeper
CHAOS_API_CONTAINER=kora-api
CHAOS_RPC_CONTAINER=kora-stellar-rpc-mock
CHAOS_API_DB_NETWORK=kora-internal
CHAOS_FAILURE_DURATION_MS=10000
CHAOS_RECOVERY_TIMEOUT_MS=30000
CHAOS_DB_LATENCY_MS=2000
CHAOS_RECONCILER_ALERT_LAG_MS=15000
CHAOS_MOCK_LEDGER_COUNT=3
CHAOS_ALERT_CONTAINER=kora-alert-relay
```

---

## Architecture

```
tests/
  global-setup.ts                        — pre-flight health check; refuses if CHAOS_ENV=production
  global-teardown.ts                     — post-suite recovery verification
  scenario-rpc-outage.chaos.ts           — scenario 1: indexer RPC outage
  scenario-db-latency.chaos.ts           — scenario 2: database latency spike
  scenario-keeper-crash.chaos.ts         — scenario 3: keeper service crash
  scenario-network-partition.chaos.ts    — scenario 4: API↔DB network partition
  scenario-indexer-checkpointing.chaos.ts — scenario 5: indexer hard crash + durable checkpoint
  scenario-reconciliation-alerting.chaos.ts — scenario 6: gap-triggered alert + alert buffering

src/
  config.ts     — all configuration from environment variables
  injectors.ts  — failure injection: pause container, tc-netem, SIGKILL, network disconnect,
                  indexer pause/crash, alert sink outage
  probes.ts     — service health and state observation via HTTP (indexer, keeper, API,
                  reconciler status, alert queue)
  logger.ts     — structured logger (pino)

services/reconciler/
  src/queue.ts  — durable SQLite pending-alert queue
  src/index.ts  — gap scan, sink delivery, and admin endpoints
```

Each scenario follows the same structure:
1. **Baseline** — record observable state before injection.
2. **Inject** — call the relevant injector; store the returned `Cleanup`.
3. **Assert during failure** — probe the service while the failure is active.
4. **Cleanup** — always called in `afterEach` even if the assertion throws.
5. **Assert recovery** — wait for healthy, then assert the resilience claim.

---

## Invariants Tested by Scenario

| Scenario | Invariant |
|---|---|
| RPC outage | `postCheckpoint >= preCheckpoint` and `postEventCount >= preEventCount` |
| DB latency | Every response is 200 or ≥ 400 (never 0 = silent hang) |
| Keeper crash | `done_keys` set is a superset of pre-crash; no key appears twice; `in_flight == 0` post-restart |
| Network partition | Last two poll responses during partition are not 200; `apiIsHealthy()` returns true post-reconnect |
| Indexer checkpointing | `postCheckpoint >= precrashCheckpoint`; checkpoint monotone across rapid cycles |
| Reconciliation alerting | Alert fired within `CHAOS_RECONCILER_ALERT_LAG_MS`; `alertsFired > preAlertsFired`; queue drained after sink recovery |

## GAP-2 resolution: database statement timeout

The API pool policy now uses `DB_STATEMENT_TIMEOUT_MS=5000` by default. SQLSTATE `57014` is translated to HTTP 503 with `{ "code": "DB_TIMEOUT" }` and a retry hint. Set a positive integer override only for staging diagnostics; zero, negative, and non-integer values are rejected during chaos configuration startup. Long-running reporting exports must use a separate pool or role with an explicit, reviewed timeout policy.

The database-latency scenario no longer accepts an HTTP client timeout (`status 0`) as a valid outcome: slow statements must complete or return a service error before the client deadline.
