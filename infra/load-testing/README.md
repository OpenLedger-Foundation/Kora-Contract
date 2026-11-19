# Kora Protocol — Load Testing Harness

Realistic traffic-pattern load tests for the API gateway and indexer, using [k6](https://k6.io).

## Scenarios

| Scenario | File | Pattern | Purpose |
|----------|------|---------|---------|
| Marketplace browsing burst | `scenarios/browsing-burst.js` | Spike: 0→300 VUs in 30s, hold 2m, ramp down | Simulates a popular listing driving traffic |
| Funding round spike | `scenarios/funding-spike.js` | Ramp: 0→500 VUs over 1m, sustained 3m | Simulates a funding rush when a high-yield invoice goes live |
| Subscription fan-out | `scenarios/subscription-fanout.js` | Steady: 200 VUs, 10m | Simulates many clients holding open SSE connections |
| Combined realistic | `scenarios/combined.js` | All three shapes simultaneously | Pre-release gate scenario |

## Pass/Fail Thresholds

Defined in each scenario file and enforced as k6 `thresholds`. A run **fails** if any threshold is breached:

| Metric | Threshold |
|--------|-----------|
| `http_req_duration` p95 | ≤ 500 ms |
| `http_req_duration` p99 | ≤ 1500 ms |
| `http_req_failed` | < 1% |
| `ws_session_duration` p95 (SSE) | ≥ 60s (connections must stay alive) |

## Quick Start

```bash
# Install k6
brew install k6   # macOS
# or: https://k6.io/docs/getting-started/installation/

# Run a single scenario against staging
K6_BASE_URL=https://staging-api.kora.finance k6 run scenarios/browsing-burst.js

# Run full combined scenario
K6_BASE_URL=https://staging-api.kora.finance k6 run scenarios/combined.js --out json=results/combined-$(date +%Y%m%d).json

# Run with HTML report
K6_BASE_URL=https://staging-api.kora.finance k6 run scenarios/combined.js \
  --out json=results/run.json && node scripts/report.js results/run.json
```

## CI / Scheduled Runs

`.github/workflows/load-test.yml` runs the combined scenario against staging:
- On a weekly schedule (Sundays 02:00 UTC)
- Manually via `workflow_dispatch`
- Automatically before any `infra/**` change merges to main

**Never runs against production.** The `K6_BASE_URL` must contain `staging` or the run aborts.

## Interpreting Results

After a run, `results/` contains:
- `<scenario>-<date>.json` — raw k6 JSON output
- `report.html` — generated HTML report (run `node scripts/report.js`)

When a threshold is breached, the report identifies the **bottleneck component**:
- High `http_req_duration` on `/listings` → API gateway or DB query
- High failure rate on `/fund` → financing pool or token contract latency
- Dropped SSE connections → subscription service or nginx buffer config

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `K6_BASE_URL` | Yes | — | Base URL of the target environment |
| `K6_API_KEY` | No | — | Bearer token for authenticated endpoints |
| `K6_INDEXER_URL` | No | same as BASE_URL | Separate indexer URL if different |
| `K6_SCENARIO` | No | `combined` | Which scenario to run |

## Keeper and indexer query scenarios

| Scenario | File | Purpose |
|---|---|---|
| Keeper metrics scrape | `scenarios/keeper-metrics-scrape.js` | Bounded concurrent GET load against keeper `/metrics`; verifies Prometheus output and p95/error thresholds. |
| Indexer query storm | `scenarios/indexer-query-storm.js` | Exercises bounded status filters and pagination against listings without unbounded query cardinality. |

Run locally against staging only:

```bash
K6_BASE_URL=http://localhost:3000 K6_KEEPER_URL=http://localhost:8080 k6 run scenarios/keeper-metrics-scrape.js
K6_BASE_URL=http://localhost:3000 k6 run scenarios/indexer-query-storm.js --out json=results/indexer-query-storm.json
node scripts/report.js results/indexer-query-storm.json
```

Both scenarios inherit the production guard from `lib/common.js`. They must use local or staging endpoints, never production URLs.
