# Health Aggregation Service

Runnable bootstrap over the health aggregation library (`health-aggregator.ts`)
and the HTTP adapter (`http-server.ts`).

## Running

```bash
npm install
npm run build
npm start
```

Listens on `HEALTH_PORT` (default `8081`):

- `GET /health` → `AggregatedHealthReport` JSON (503 when overall is `down`)
- `GET /metrics` → Prometheus text (`kora_service_health` gauges)

When imported as a library (tests), no HTTP server is started; only the
`node dist/index.js` / `ts-node` entrypoint boots the process. The process
exits cleanly on `SIGTERM`/`SIGINT` for container deployments.

## Environment

| Var | Default | Purpose |
|---|---|---|
| `HEALTH_PORT` | `8081` | Port for `/health` and `/metrics` |
| `KEEPER_HEALTH_URL` | `http://keeper:8080/health` | Keeper upstream |
| `INDEXER_HEALTH_URL` | `http://indexer:8080/health` | Indexer upstream |
| `API_HEALTH_URL` | `http://api:8080/health` | API upstream |
| `FEATURE_FLAGS_HEALTH_URL` | `http://feature-flags:8080/health` | Feature-flags upstream |
| `REPORTING_HEALTH_URL` | `http://reporting:8080/health` | Reporting upstream |
| `HEALTH_POLL_TIMEOUT_MS` | `2000` | Per-upstream probe timeout; slow upstreams record `down` without blocking others |

A misconfigured URL classifies that upstream as unreachable (`down`), never a
process crash. Upstream list changes require a restart.
