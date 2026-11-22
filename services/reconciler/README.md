# Reconciliation alert service

Closes #1094.

The reconciler compares the indexer checkpoint with the observed RPC head and
persists every detected ledger-gap alert before attempting delivery. SQLite is
used as the durable queue so a process restart or alert-sink outage cannot
discard an operator signal.

## Architecture

```
src/
  queue.ts    — PersistentAlertQueue: SQLite-backed dedup, backoff, drain
  metrics.ts  — renderMetrics(): Prometheus 0.0.4 text-format exposition
  index.ts    — scan loop, delivery, admin HTTP server, metrics HTTP server

tests/
  queue.test.ts                   — unit tests for PersistentAlertQueue
  metrics.test.ts                 — unit tests for renderMetrics
  reconciler-integration.test.ts  — in-process integration: gap → enqueue → deliver
```

## Endpoints

### Admin server (`RECONCILER_PORT`, default `4000`)

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Liveness probe |
| `GET` | `/reconciler/status` | Checkpoint, RPC head, counters, queue depth |
| `GET` | `/reconciler/pending-alerts` | All pending alerts (admin/chaos probe alias) |
| `GET` | `/reconciler/alerts/pending` | All pending alerts (chaos probe canonical path) |
| `DELETE` | `/reconciler/alerts/pending` | Drain queue (chaos cleanup) |

### Metrics server (`RECONCILER_METRICS_PORT`, default `9091`)

| Method | Path | Description |
|---|---|---|
| `GET` | `/metrics` | Prometheus 0.0.4 text format |

## Metrics

| Metric | Type | Description |
|---|---|---|
| `kora_reconciler_gaps_detected_total` | counter | Ledger gaps enqueued since startup |
| `kora_reconciler_alerts_delivered_total` | counter | Alerts acknowledged by the sink since startup |
| `kora_reconciler_pending_alerts` | gauge | Current queue depth |
| `kora_reconciler_oldest_gap_age_seconds` | gauge | Age of the oldest undelivered alert; zero when queue is empty. Primary SLO signal. |

Prometheus scrape target (`infra/monitoring/prometheus.yml` job `kora-reconciler`):

```yaml
- job_name: kora-reconciler
  metrics_path: /metrics
  static_configs:
    - targets: ["reconciler:9091"]
```

## Alert rules (`infra/monitoring/alerts.yml`)

| Alert | Condition | Severity |
|---|---|---|
| `KoraLedgerGapDetected` | `increase(kora_reconciler_gaps_detected_total[5m]) > 0` for 1 m | sev-2 / warning |
| `KoraReconcilerAlertQueueStuck` | `kora_reconciler_oldest_gap_age_seconds > 300` for 2 m | sev-1 / critical |
| `KoraReconcilerDown` | `up{job="kora-reconciler"} == 0` for 3 m | sev-3 / warning |

## Configuration

| Variable | Default | Purpose |
|---|---:|---|
| `RECONCILER_PORT` | `4000` | Admin HTTP port |
| `RECONCILER_METRICS_PORT` | `9091` | Prometheus metrics port |
| `RECONCILER_DB_PATH` | `./reconciler.db` | SQLite queue file; use `:memory:` in tests |
| `RECONCILER_RPC_HEAD_URL` | _(empty)_ | JSON endpoint returning `{ "ledger": 123 }` |
| `RECONCILER_INDEXER_CHECKPOINT_URL` | _(empty)_ | JSON endpoint returning `{ "checkpoint": 120 }` |
| `RECONCILER_ALERT_SINK_URL` | _(empty)_ | Webhook accepting a JSON alert body; `Idempotency-Key` header is set per alert ID |
| `CHAOS_RECONCILER_ALERT_LAG_MS` | `15000` | Alert lag budget; scan interval defaults to `max(1 s, budget / 3)` |
| `RECONCILER_SCAN_INTERVAL_MS` | _(budget / 3)_ | Override scan cadence |

## Gap detection logic

Each scan cycle:

1. Fetch the indexer checkpoint (`RECONCILER_INDEXER_CHECKPOINT_URL`).
2. Fetch the RPC head ledger (`RECONCILER_RPC_HEAD_URL`).
3. If `head > checkpoint`, enqueue one alert per missing ledger up to 1 000
   per cycle (capped to prevent OOM on large gaps).
4. `INSERT OR IGNORE` with dedup key `ledger-gap:<ledger>` suppresses
   duplicate rows across repeated scans.
5. Drain due alerts via POST to the sink; acknowledge (delete) on 2xx.
6. On sink failure, reschedule with exponential backoff capped at 300 s.

## Running tests

```bash
cd services/reconciler
npm install
npm test
```

## Missing credentials / no URLs configured

If `RECONCILER_RPC_HEAD_URL` or `RECONCILER_INDEXER_CHECKPOINT_URL` are not
set, `readLedger` returns `null` and the scan cycle is a no-op.  The service
starts and serves health/metrics normally.  Set both variables before deploying
to staging.
