# Governance-Forum Bridge Service

Syncs on-chain governance proposals from the `governance_voting` contract with an off-chain discussion forum, creating linked threads and updating vote tallies in real-time.

## Features

- **Automatic Thread Creation**: New proposals automatically get forum threads
- **Live Vote Tallies**: Threads display current vote counts from chain
- **Idempotent Sync**: Keyed by proposal ID to prevent duplicate threads
- **Retry with Backoff**: Handles forum API downtime gracefully
- **Read-Only**: One-way sync (chain → forum), no forum-triggered actions

## Architecture

```
Soroban RPC → Governance Monitor → Forum API
                    ↓
              Sync Database (SQLite)
```

1. **Governance Monitor**: Polls governance contract for new/updated proposals
2. **Sync Database**: Tracks which proposals have been synced to forum
3. **Forum Client**: Creates/updates threads with retry logic

## Configuration

Environment variables:

```bash
# Soroban RPC endpoint
SOROBAN_RPC_URL=https://soroban-testnet.stellar.org

# Deployed governance_voting address (not governance or parameter_registry)
GOVERNANCE_CONTRACT_ID=CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX

# Forum API credentials
FORUM_API_URL=https://forum.example.com/api/v1
FORUM_API_KEY=your_api_key_here

# Polling interval (milliseconds)
POLL_INTERVAL_MS=60000

# Retry configuration
MAX_RETRIES=5
RETRY_BACKOFF_MS=5000

# Database path
DB_PATH=./governance-sync.db

# Logging level
LOG_LEVEL=info
```

`GOVERNANCE_CONTRACT_ID` must identify the contract with the `Proposal` schema
in [`kora-governance-voting`](../../contracts/governance_voting/src/lib.rs).
Its generated Rust typed client is `GovernanceVotingContractClient`; use
`get_proposal` to read the title, description, and three vote tallies consumed
by this bridge. See the [client example](../../contracts/governance_voting/README.md#integration-example).
The separate [`GovernanceContractClient`](../../contracts/governance/src/lib.rs)
uses upgrade proposals, and [`ParameterRegistryContractClient`](../../contracts/parameter_registry/src/lib.rs)
uses parameter proposals; their addresses and return types are not interchangeable.
The TypeScript [`KoraClient`](../../sdk/src/KoraClient.ts) currently exposes
access-control governance only, so this bridge does not have a generated
TypeScript governance-voting binding yet.

## Installation

```bash
npm install
```

## Usage

```bash
# Development
npm run dev

# Production
npm run build
npm start

# Tests
npm test
```

## Database Schema

```sql
CREATE TABLE sync_records (
  proposal_id INTEGER PRIMARY KEY,
  forum_thread_id TEXT NOT NULL,
  synced_at INTEGER NOT NULL,
  last_update INTEGER NOT NULL
);
```

## Forum API Requirements

The forum platform must provide:

- `POST /threads` - Create new thread
- `POST /threads/{id}/posts` - Add post to thread

Request format:

```json
{
  "title": "[Proposal #123] Example Proposal",
  "body": "Markdown-formatted proposal details...",
  "tags": ["governance", "financial"],
  "metadata": {
    "proposal_id": 123,
    "voting_mode": "Standard"
  }
}
```

## Error Handling

- **Forum API 429/5xx**: Retry with exponential backoff
- **Forum API 4xx**: Log error, skip proposal (non-retryable)
- **Contract read failure**: Log warning, continue polling
- **Database errors**: Fatal, service exits

## Monitoring & Operational Telemetry

The governance sync service exposes Prometheus metrics and health status on port `8082` (configurable via `METRICS_PORT` and `METRICS_HOST`).

### Endpoints

- `GET /metrics`: Prometheus text exposition format (version 0.0.4).
- `GET /health`: JSON service liveness and uptime status (`{"status":"ok","service":"governance-sync","uptime":...}`).

### Metrics Catalog

| Metric | Type | Description |
|---|---|---|
| `kora_governance_sync_rpc_errors_total` | Counter | Total number of Soroban RPC query errors. |
| `kora_governance_sync_errors_total` | Counter | Total governance sync loop failures. |
| `kora_governance_sync_proposals_total{status="synced\|failed"}` | Counter | Total proposals processed by sync status. |
| `kora_governance_sync_last_poll_timestamp_seconds` | Gauge | Unix timestamp of the most recent contract poll. |
| `kora_governance_sync_heartbeat_timestamp_seconds` | Gauge | Unix timestamp of the most recent service heartbeat. |

### Alerts & Incident Response Runbook

Alert definitions in `infra/monitoring/alerts.yml`:
- **`KoraGovernanceSyncRpcErrors`** (Sev-3 Warning): Triggers if Soroban RPC failures occur over a 5-minute window (`increase(kora_governance_sync_rpc_errors_total[5m]) > 0` for 2m).
  - *Triage*: Verify Stellar testnet RPC health (`SOROBAN_RPC_URL`), network connectivity, and rate-limiting responses.
- **`KoraGovernanceSyncHeartbeatMissing`** (Sev-3 Warning): Triggers if service heartbeat stops for >5 minutes.
  - *Triage*: Check container logs, SQLite database lock states, and restart the service if hung.
- **`KoraServiceUnavailable`** (Sev-3 Warning): Triggers when `up{job="kora-governance-sync"} == 0` for 3m.

For escalation protocol and severity classifications, consult [`docs/INCIDENT_RESPONSE.md`](../../docs/INCIDENT_RESPONSE.md).

## Testing

```bash
npm test
```

Tests cover:

- Idempotent thread creation
- Retry on forum downtime
- Tally freshness within bounds
- Database sync record management

## Security

- Forum API key stored in environment variable (never committed)
- Read-only access to governance contract
- No authentication bypass or privilege escalation paths

## License

MIT
