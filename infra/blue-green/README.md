# Blue-Green Deployment — API Gateway & Indexer

Zero-downtime deploys for the two highest-traffic user-facing services: the API gateway and the indexer.

## Strategy

```
                    Load Balancer (nginx)
                           │
              ┌────────────┴────────────┐
         [ACTIVE]                  [STANDBY]
        api-blue:3001           api-green:3002
              │                       │
    currently serving traffic    new version warming up
```

1. Start the new version (green) alongside the current (blue)
2. Health-check green until it passes
3. Update nginx upstream → graceful reload (SIGHUP — in-flight requests complete)
4. Post-cutover health check via nginx (if it fails → automated rollback to blue)
5. Drain blue for 30s (allows in-flight SSE subscriptions to reconnect gracefully)
6. Stop and remove blue

## Subscription Handling

GraphQL subscriptions and SSE flag-stream connections are long-lived. During the upstream switch:

- nginx sends a `SIGHUP` (graceful reload) — existing connections are served to completion before the upstream pointer changes
- SSE clients receive a `Connection: close` hint at the end of their current read cycle and reconnect automatically via `EventSource` built-in retry
- Reconnecting clients send their cursor; the server resumes from that position so no events are lost
- `proxy_read_timeout 600s` in nginx keeps connections alive for up to 10 minutes of inactivity

## Indexer Double-Processing Prevention

Before the new indexer starts, `deploy.sh` reads the outgoing instance's watermark cursor from `/internal/cursor`. The new instance is given `INDEXER_START_CURSOR` as an environment variable and starts processing from that ledger sequence. This prevents reprocessing events that were already indexed during the handover window.

## Automated Rollback

Health checks run both before (new instance must pass) and after (traffic shifted to new instance). If the post-cutover check fails, `deploy.sh` immediately:

1. Reverts the nginx upstream to the old colour
2. Reloads nginx (graceful — in-flight requests on green drain before blue resumes)
3. Removes the failed green instance

No manual intervention is needed for a failed deploy.

## Manual Rollback

```bash
./infra/blue-green/deploy.sh api --rollback
./infra/blue-green/deploy.sh indexer --rollback
./infra/blue-green/deploy.sh both --rollback
```

## Automated Cutover Checklist

Before running `deploy.sh`, use `cutover-checklist.sh` to verify all health and metrics gates.  The script exits `0` only when every gate passes; a failed gate prints rollback instructions and exits `1`.

```bash
# Verify gates for both services
./infra/blue-green/cutover-checklist.sh both

# Dry-run in CI — prints gates without live HTTP calls, always exits 0
./infra/blue-green/cutover-checklist.sh both --dry-run
```

### Gates checked

| # | Gate | Failure action |
|---|------|---------------|
| 1 | New instance `/health` responds 200 | Block cutover |
| 2 | `/metrics` endpoint is parseable | Block cutover |
| 3 | `kora_indexer_lag_seconds` ≤ `CHECKLIST_MAX_INDEXER_LAG_S` | Block cutover |
| 4 | `kora_keeper_jobs{status="dead"}` == 0 | Block cutover |
| 5 | No `circuit_breaker_trip` events observed | Block cutover |
| 6 | Active colour confirmed in state file | Warn on first deploy |
| 7 | No critical alerts active in Alertmanager | Block cutover |

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `CHECKLIST_API_BLUE_URL` | `http://localhost:3001` | api-blue base URL |
| `CHECKLIST_API_GREEN_URL` | `http://localhost:3002` | api-green base URL |
| `CHECKLIST_INDEXER_BLUE_URL` | `http://localhost:8081` | indexer-blue base URL |
| `CHECKLIST_INDEXER_GREEN_URL` | `http://localhost:8082` | indexer-green base URL |
| `CHECKLIST_ALERTMANAGER_URL` | `http://localhost:9093` | Alertmanager API |
| `CHECKLIST_MAX_INDEXER_LAG_S` | `60` | Max indexer lag before blocking |
| `CHECKLIST_HEALTH_TIMEOUT_S` | `5` | Per-request curl timeout |
| `CHECKLIST_RETRIES` | `3` | Retries per gate |
| `CHECKLIST_RETRY_INTERVAL_S` | `2` | Seconds between retries |

### Partial-service healthy scenario

If only one of the two services fails its gates:

1. Roll back only the failing service: `./deploy.sh <service> --rollback`
2. Leave the passing service on its new colour
3. Re-run the checklist for the failing service alone once the issue is resolved

---

## Usage

```bash
# Deploy API gateway (latest image)
DOCKER_IMAGE_API=ghcr.io/org/kora-api:v1.2.0 ./infra/blue-green/deploy.sh api

# Deploy both services
DOCKER_IMAGE_API=ghcr.io/org/kora-api:v1.2.0 \
DOCKER_IMAGE_INDEXER=ghcr.io/org/kora-indexer:v1.2.0 \
./infra/blue-green/deploy.sh both

# Roll back
./infra/blue-green/deploy.sh api --rollback
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DOCKER_IMAGE_API` | — | Required: full image ref for api service |
| `DOCKER_IMAGE_INDEXER` | — | Required: full image ref for indexer service |
| `HEALTH_CHECK_RETRIES` | 12 | Attempts before rollback (12 × 5s = 60s window) |
| `HEALTH_CHECK_INTERVAL` | 5 | Seconds between health check attempts |
| `DRAIN_WAIT_SECONDS` | 30 | Grace period before stopping old instance |
