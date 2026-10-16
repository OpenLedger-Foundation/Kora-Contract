# Risk Scoring Data Pipeline

**Issue #755 — Off-Chain Risk Scoring Data Pipeline**

Ingests external signals (SME transaction history, debtor payment history, macro
trade data) and computes a **suggested** risk score with an auditable breakdown
that verifiers review before submitting their own judgment to `risk_registry`.

Out of scope: verifier-facing UI (Frontend) and fully automated on-chain
submission — the pipeline never submits on-chain itself.

## Architecture

```
configured sources → ScoringInput → RiskPipeline.score(input, version)
                                        ├─ PaymentHistoryFactor
                                        ├─ InvoiceFrequencyFactor
                                        ├─ DebtorConcentrationFactor
                                        └─ MacroTradeFactor (v2+)
                                      → SuggestedScore {
                                          score, confidence,
                                          formulaVersion, breakdown[],
                                          warnings[], computedAt
                                        } → verifier review → risk_registry
```

- **Pluggable factors** implement `ScoringFactor` (`src/factors.ts`).
- **Versioned formulas** live in `src/formulas.ts` (`v1`, `v2`, immutable once
  registered via `FormulaRegistry`). Every output records `formulaVersion` so
  historical scores remain explainable.
- **Combination rule** (stable across versions): renormalize configured weights
  over non-missing factors, take the weighted mean; overall confidence is the
  weight-weighted mean of per-factor confidences.

## Sparse data

Missing factors return a neutral score with `confidence: 0` and `missing: true`.
Overall confidence degrades; `warnings` includes `SPARSE_DATA` and, below
`minConfidence` (default 0.4), `LOW_CONFIDENCE_NOT_RECOMMENDED`. An input with
no usable factors yields neutral `50` with `NO_USABLE_FACTORS`.

## Usage

```ts
import { RiskPipeline } from "@kora/risk-pipeline";

const pipeline = new RiskPipeline(); // default factors, latest formula
const suggestion = pipeline.score({
  sme: { repayments: [{ paidOnTime: true }], totalInvoices: 5, invoicesLast90d: 1, defaults: 0 },
  debtors: [{ debtorHash: "ab…", exposureShareBps: 10_000, avgDebtorScore: 30 }],
});
console.log(suggestion.score, suggestion.confidence, suggestion.breakdown);
```

## Testing

```bash
npm install
npm test   # 90% line/branch/function/statement coverage enforced
```

Covers sparse-data confidence degradation, versioned-formula backward
compatibility + immutability, and factor-breakdown correctness.

## Operational Telemetry & Monitoring

The risk pipeline exposes Prometheus metrics and service health endpoints via an internal HTTP server (default port `8081`, configurable via `METRICS_PORT` and `METRICS_HOST`).

### Endpoints

- `GET /metrics`: Prometheus text exposition format (version 0.0.4).
- `GET /health`: JSON service liveness and uptime status (`{"status":"ok","service":"risk-pipeline","uptime":...}`).

### Metrics Catalog

| Metric | Type | Description |
|---|---|---|
| `kora_risk_pipeline_job_failures_total` | Counter | Total number of scoring evaluation failures encountered. |
| `kora_risk_pipeline_executions_total{status="success\|failure"}` | Counter | Total risk score evaluations categorized by completion status. |
| `kora_risk_pipeline_stale_scores` | Gauge | Count of SME risk scores that exceed the freshness/staleness threshold. |
| `kora_risk_pipeline_last_run_timestamp_seconds` | Gauge | Unix timestamp of the most recent scoring run completion. |
| `kora_risk_pipeline_heartbeat_timestamp_seconds` | Gauge | Unix timestamp of the latest service heartbeat. |

### Alerts & Incident Response Runbook

Alert definitions in `infra/monitoring/alerts.yml`:
- **`KoraRiskPipelineJobFailures`** (Sev-3 Warning): Triggers if evaluation job failures occur over a 5-minute window (`increase(kora_risk_pipeline_job_failures_total[5m]) > 0` for 2m).
  - *Triage*: Inspect scoring input logs for upstream API formatting inconsistencies or invalid SME financial inputs.
- **`KoraRiskPipelineStaleScores`** (Sev-3 Warning): Triggers if unrefreshed SME scores remain above 0 for 5 minutes (`kora_risk_pipeline_stale_scores > 0` for 5m).
  - *Triage*: Check external data provider latency and queue processing delays.
- **`KoraRiskPipelineHeartbeatMissing`** (Sev-3 Warning): Triggers if no service heartbeat is registered for >5 minutes.
  - *Triage*: Verify container process status, memory limits, and restart daemon if unresponsive.

For escalation and severity matrix, consult [`docs/INCIDENT_RESPONSE.md`](../../docs/INCIDENT_RESPONSE.md).
