# Protocol monitoring and alerting

This stack uses Prometheus for scraping/rules, Grafana for dashboards, and
Alertmanager to route notifications to the operations on-call router. Deploy
these checked-in configs with the team's existing container platform; do not
expose Prometheus, Grafana, Alertmanager, or the webhook receiver publicly.
Restrict dashboard access to operators and configure persistent storage and
backups for dashboard state as required by the deployment.

## Configuration

- Load `prometheus.yml` as the Prometheus configuration and `alerts.yml` as its
  rule file.
- Provision `grafana-dashboard.json` in Grafana with a Prometheus datasource
  whose UID is `prometheus`.
- Load `alertmanager.yml` into Alertmanager. The internal `oncall-router`
  receiver must deliver `/critical` to the paging rotation and `/warnings` to
  the operations channel. Keep paging credentials in the router's secret store,
  not in this repository.
- Set the configured targets to the internal service names or deployment DNS
  names. Scrape over the private network and apply network policy so only the
  monitoring namespace can reach `/metrics`.

The keeper now exposes `/metrics` with `kora_keeper_jobs{status=...}`. The
health aggregator and indexer targets must expose Prometheus text metrics. The
indexer metrics contract used by the dashboard/rules is:

- `kora_indexer_lag_seconds`: current processed-ledger lag.
- `kora_onchain_events_total{event_type="default|circuit_breaker_trip|treasury_sweep"}`:
  monotonically increasing counters sourced from finalized indexed events.

This workspace contains the health aggregation library, an HTTP adapter in
`services/health/http-server.ts`, a runnable bootstrap in
`services/health/index.ts` (see `services/health/README.md` for env vars),
and the keeper. The hosting application must construct a
`HealthAggregator`, pass it to `createHealthHttpServer`, and listen on the
configured internal port. The health adapter exports `kora_service_health`
gauges and numeric per-service metrics.

## Indexer `/metrics`

The Rust indexer (`services/indexer`) exposes Prometheus 0.0.4 text via its
metrics registry (`IndexerStore::metrics_text`, served by the host process on
configurable `INDEXER_METRICS_PORT` at `GET /metrics`):

- `kora_indexer_lag_seconds`: gauge computed as `(head - checkpoint) * 5`.
  Emits a stale/head-unavailable comment instead of a bogus zero when the RPC
  head is unknown; lag grows while the checkpoint stalls.
- `kora_onchain_events_total{event_type="..."}`: counters incremented only on
  successful `insert` (duplicate/reorg replays do not double-count). Only the
  low-cardinality labels `default`, `circuit_breaker_trip`, `treasury_sweep`
  are exported (unknown types fold into `default`); never use `invoice_id`.

Scrape target (already in `prometheus.yml` job `kora-indexer`):

```yaml
- job_name: kora-indexer
  metrics_path: /metrics
  static_configs:
    - targets: ["indexer:9090"]
```

## Severity and routing

Severity follows [the incident response runbook](../../docs/INCIDENT_RESPONSE.md):

| Signal | Alert tier | Route | Rationale |
|---|---|---|---|
| Aggregated health service unavailable for 2 minutes | Sev-1 / critical | On-call page | Removes central protocol health visibility |
| A service remains down in aggregated health for 2 minutes | Sev-3 / warning | Operations channel | Degradation; investigate and escalate if it affects funds or a critical workflow |
| Circuit-breaker trip sustained in indexed events | Sev-1 / critical | On-call page | Indicates protective intervention requiring immediate review |
| Keeper/indexer scrape unavailable for 3 minutes | Sev-3 / warning | Operations channel | Degradation; does not by itself establish risk to funds |
| Keeper dead-letter jobs for 5 minutes | Sev-3 / warning | Operations channel | Operator follow-up required, no automatic page |
| Default or treasury-sweep events | Sev-2 / warning or Sev-3 / warning | Operations channel | Events may be expected; inspect transaction, amount, and authorization before escalation |

Rules use `for` windows; Alertmanager groups by alert and service, delays
initial notifications, and applies repeat intervals to reduce flapping and
duplicate pages. Circuit-breaker events page only after a counter increase is
observed and remains active through the rule's one-minute debounce. Expected
defaults and treasury sweeps never page automatically.

## Validation and drill

Validate `alerts.yml` with `promtool check rules` and run
`promtool test rules infra/monitoring/tests/alerts.test.yml` before rollout.
The test fixture injects a circuit-breaker counter increase and a default event:
the former is a critical alert after debounce, while the latter remains a
warning. In a staging deployment, send a synthetic circuit-breaker metric
increase to the test indexer exporter, confirm the `/critical` route receives
one resolved notification, then remove the synthetic series and confirm
resolution. Never inject test events into production contracts or mainnet.

Keep alert-rule changes reviewed by the security/on-call owner, and exercise
the routing path during the regular incident-response tabletop.

---

## Timeseries downsampling retention policy

The timeseries service maintains four granularity tiers.  Raw points feed the
finest tier; coarser tiers survive longer so long-range APR and marketplace
charts remain queryable after raw data has been pruned.

| Granularity | Retained for | Primary use |
|---|---|---|
| 5-minute | 7 days | Real-time dashboards, recent anomaly detection |
| Hourly | 90 days | Matches raw retention; covers quarterly views |
| Daily | 730 days (2 years) | Default resolution for APR/marketplace charts |
| Weekly | 1 825 days (5 years) | Long-range trend views and investor reports |

Raw points follow `RetentionPolicy.rawRetentionDays` (default 90 days).
After raw pruning, downsampled buckets remain queryable up to their tier
window.

The `DownsamplingJob` runs on a cron schedule (recommended: every 5 minutes)
and automatically calls `enforceRetention` at the end of each pass, so no
separate pruning job is needed.

### Env vars

| Variable | Default | Description |
|---|---|---|
| `TIMESERIES_URL` | `http://timeseries:8080/health` | Health probe URL for the timeseries service |

### Alert guidance

Add the following rule to `alerts.yml` to catch a stalled downsampling job:

```yaml
- alert: TimeseriesDownsamplingStalled
  expr: kora_service_timeseries_lagSeconds > 3600
  for: 5m
  labels:
    severity: critical
  annotations:
    summary: "Timeseries downsampling job has not run for >1 hour"
    description: >
      The DownsamplingJob lagSeconds metric exceeds the down threshold (3 600 s).
      Long-range APR and marketplace charts will show stale data.
      Check the timeseries service logs and cron scheduler.
```

---

## Governance-sandbox simulation snapshots

The `simulateAndSnapshot` function persists each simulation result under a
deterministic SHA-256 snapshot ID derived from the serialised (change,
protocolSnapshot) pair.  Voters can share the ID and retrieve an identical
summary at any point before the snapshot store is reset.

### Nondeterminism sources

The snapshot ID is stable only when all four sources of nondeterminism are
controlled.

1. **`capturedAt`** — always pass a canonical ledger sequence string (e.g.
   `"ledger-12345"`), never a wall-clock timestamp.  Two simulations over the
   same state but different `capturedAt` values produce different IDs.

2. **BigInt serialisation** — the store converts BigInt values to
   `{"__bigint__":"<n>"}` before hashing.  Do not bypass this step or the
   digest will differ across runtimes.

3. **Plugin registration order** — registering a new plugin for an existing
   change type between two runs changes the computed Impact and therefore the
   ID, even with identical inputs.  Treat plugin registration as an
   append-only, versioned operation.

4. **Object key ordering** — the store normalises keys alphabetically before
   hashing, so insertion-order differences in the input objects are safe.

### Payload size limit

The store rejects payloads larger than 1 MiB (1 048 576 bytes).  Proposals
with very large `listings` or `investors` arrays should paginate or summarise
the ProtocolSnapshot before passing it to `simulateAndSnapshot`.

### Env vars

None.  The snapshot store is in-process; production deployments should replace
`InMemorySnapshotStore` with a persistent backend and configure its connection
parameters separately.

---

## Risk-pipeline batch scoring idempotency

`BatchScorer` wraps `RiskPipeline.score()` with an idempotency layer so that
retry storms, duplicate webhook deliveries, and operator re-runs cannot produce
duplicate scores, tier flips, or spurious attestation triggers.

### Idempotency key scheme

Every submission requires a caller-supplied `idempotencyKey`.  The recommended
format is:

```
<smeAddress>:<invoiceId>:<formulaVersion>:<dataEpochSeconds>
```

Where `dataEpochSeconds` is the Unix second at which the input signals were
snapshotted.  Two runs over the same signals at the same epoch produce the same
key and therefore the same result.  A genuine re-score after new data arrives
uses a later epoch and gets a fresh key.

**Always include a stable invoice identifier.**  Two different invoices that
share the same key will collide: the second submission silently returns the
first invoice's result.  The store does not validate key contents.

### Duplicate metrics

`BatchScorer.metrics()` returns:

| Field | Prometheus metric | Description |
|---|---|---|
| `totalJobs` | `kora_risk_pipeline_total_jobs` | Unique idempotency keys stored |
| `duplicatesBlocked` | `kora_risk_pipeline_duplicates_blocked_total` | Submissions short-circuited by an existing key |

Wire `duplicatesBlocked` into the Prometheus exporter.  A sustained non-zero
rate indicates a retry loop or misconfigured caller; a sudden spike may indicate
a replay attack.

### Alert guidance

```yaml
- alert: RiskPipelineDuplicateRateHigh
  expr: rate(kora_risk_pipeline_duplicates_blocked_total[5m]) > 0.05
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "Risk pipeline duplicate submission rate above 5 %"
    description: >
      More than 5 % of batch scoring submissions are duplicates.
      Investigate the calling service for retry loops or misconfigured
      idempotency keys.  Persistent duplicates can mask genuine re-score
      requests and stale tier data.
```

### Env vars

| Variable | Default | Description |
|---|---|---|
| `RISK_PIPELINE_URL` | `http://risk-pipeline:8080/health` | Health probe URL for the risk-pipeline service |
| `GOVERNANCE_SANDBOX_URL` | `http://governance-sandbox:8080/health` | Health probe URL for the governance-sandbox service |
