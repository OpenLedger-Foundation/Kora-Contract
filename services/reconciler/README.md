# Reconciliation alert service

The reconciler compares the indexer checkpoint with the observed RPC head and persists every detected ledger-gap alert before attempting delivery. SQLite is used as the durable queue so a process restart or alert-sink outage cannot discard an operator signal.

## Endpoints

- `GET /health` — liveness.
- `GET /reconciler/status` — checkpoint, RPC head, detected gaps, delivered alerts, and queue depth.
- `GET /reconciler/pending-alerts` — pending alerts with monotonic IDs.
- `GET /reconciler/alerts/pending` — compatibility alias used by the chaos probes.
- `DELETE /reconciler/alerts/pending` — acknowledge and remove pending alerts during controlled cleanup.

## Configuration

| Variable | Default | Purpose |
|---|---:|---|
| `RECONCILER_PORT` | `4000` | Admin HTTP port |
| `RECONCILER_DB_PATH` | `./reconciler.db` | SQLite queue file |
| `RECONCILER_RPC_HEAD_URL` | empty | JSON endpoint returning `{ "ledger": 123 }` |
| `RECONCILER_INDEXER_CHECKPOINT_URL` | empty | JSON endpoint returning `{ "checkpoint": 120 }` |
| `RECONCILER_ALERT_SINK_URL` | empty | Webhook accepting a JSON alert |
| `CHAOS_RECONCILER_ALERT_LAG_MS` | `15000` | Scan cadence and alert budget |

Alerts are retried with exponential backoff and remain pending until the sink acknowledges with a 2xx response. Payloads contain ledger metadata and IDs only; SQL text and sensitive request data are never logged.
