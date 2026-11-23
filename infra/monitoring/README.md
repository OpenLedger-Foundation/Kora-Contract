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

## FX ingestion `/metrics`

The FX ingestion service (`services/fx-ingestion`) exposes Prometheus 0.0.4
text at `GET /metrics` on configurable `FX_METRICS_PORT` (default `8081`):

- `kora_fx_rate_age_seconds{base,quote}`: gauge = seconds since the last
  successful FX rate publication for a pair. Emits a comment instead of a bogus
  zero when no sample exists yet (so a missing series, not `0`, drives alerts).
- `kora_fx_last_ingestion_timestamp_seconds{base,quote}`: gauge = unix
  timestamp of the last successful publication per pair.
- `kora_fx_ingestion_cycles_total`: counter incremented once per completed
  ingestion cycle.
- `kora_fx_publication_skipped_total{reason}`: counter of skipped publications
  by reason (e.g. `no_sources_available`, `insufficient_sources_for_cross_check`,
  `source_divergence_*`, `stale_rate`, `rate_limited`).

Scrape target (already in `prometheus.yml` job `kora-fx-ingestion`):

```yaml
- job_name: kora-fx-ingestion
  metrics_path: /metrics
  static_configs:
    - targets: ["fx-ingestion:8081"]
```

### FX staleness alerts

| Alert | Expression | For | Tier |
|---|---|---|---|
| `KoraFxRateStale` | `max by (base,quote) (kora_fx_rate_age_seconds) > 1800` | 5m | Sev-2 warning |
| `KoraFxRateNeverPublished` | `absent(kora_fx_rate_age_seconds)` | 15m | Sev-2 warning |
| `KoraFxIngestionDown` | `up{job="kora-fx-ingestion"} == 0` | 3m | Sev-3 warning |

The staleness threshold (1800s) is half the oracle's 3600s `maxStalenessSeconds`
budget so operators are paged before the on-chain staleness guard begins
rejecting rates. Tune via a recording rule or threshold override per
environment.

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
| FX rate stale beyond 30 min for any pair | Sev-2 / warning | Operations channel | Multi-currency invoice displays and funding math depend on fresh rates; inspect source availability and the fx-ingestion `/metrics` endpoint |
| FX ingestion scrape unavailable for 3 minutes | Sev-3 / warning | Operations channel | Degradation; FX staleness may be going undetected |

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