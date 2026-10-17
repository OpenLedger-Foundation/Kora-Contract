# Governance-Forum Bridge Service

Syncs on-chain governance proposals from the `governance_voting` contract with an off-chain discussion forum, creating linked threads and updating vote tallies in real-time.

## Features

- **Automatic Thread Creation**: New proposals automatically get forum threads
- **Live Vote Tallies**: Threads display current vote counts from chain
- **Idempotent Sync**: Keyed by proposal ID to prevent duplicate threads
- **Retry with Backoff**: Handles forum API downtime gracefully
- **Read-Only**: One-way sync (chain → forum), no forum-triggered actions

## Weight Formula

Vote weight for each proposal is computed as the beneficiary's **vested token
balance** at the proposal's `created_at` snapshot timestamp:

```
weight = vesting_contract.get_vested_balance_at(voter, proposal.created_at)
```

- If the Vesting contract has not been set on the governance contract, the
  vote weight falls back to the manual `VotingWeight` storage (backward
  compatible). Once the vesting contract is set, manual weights are ignored
  and only vested balances are used.
- For delegated voting, the delegate's total weight is the sum of their own
  vested balance plus the vested balance of each delegator (computed at the
  current ledger timestamp, not the proposal snapshot).

This formula ensures that only economically locked tokens contribute to
governance influence. The off-chain sync service should read the `snapshot_at`
field from `voteCast` events to reconcile tallies against vesting state.

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

# Alternatively, read contracts.governance_voting.address from the deploy output
KORA_DEPLOYMENT_MANIFEST=../../deployments/testnet.json

# Forum API credentials
FORUM_API_URL=https://forum.example.com/api/v1
FORUM_API_KEY=your_api_key_here

# Polling interval (milliseconds)
POLL_INTERVAL_MS=60000

# Max new proposals scanned per sync cycle (bounds the on-chain scan)
MAX_NEW_PROPOSALS_PER_CYCLE=50

# Retry configuration (applies to RPC reads and forum API calls)
MAX_RETRIES=5
RETRY_BACKOFF_MS=5000

# Database path
DB_PATH=./governance-sync.db

# Logging level
LOG_LEVEL=info
```

`GOVERNANCE_CONTRACT_ID` takes precedence. If it is unset, the bridge reads
`contracts.governance_voting.address` from `KORA_DEPLOYMENT_MANIFEST`, which can
point at the network manifest written by `scripts/deploy.sh`.
The address must identify the contract with the `Proposal` schema
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

- **RPC read failure** (network, simulation error): Retried with bounded exponential
  backoff (`MAX_RETRIES`, `RETRY_BACKOFF_MS`). Non-retryable contract errors are logged
  and the proposal is skipped for that cycle.
- **Proposal absent/cancelled on-chain** (`get_proposal` returns `None`): Logged as a
  warning, no thread created or updated.
- **Forum API 429/5xx**: Retry with exponential backoff
- **Forum API 4xx**: Log error, skip proposal (non-retryable)
- **Database errors**: Fatal, service exits

**Startup validation:** `GOVERNANCE_CONTRACT_ID` (or `contracts.governance_voting.address`
from `KORA_DEPLOYMENT_MANIFEST`) must be set and be a valid Stellar address; otherwise
the service fails fast at startup.

**Scan bounds:** Each sync cycle refreshes tallies for already-synced proposals, then
scans up to `MAX_NEW_PROPOSALS_PER_CYCLE` new proposal IDs sequentially. Proposal IDs are
contiguous from 0, so the first on-chain `NotFound` marks the end of the known set and
scanning resumes at the next cycle.

## Monitoring

Key metrics to monitor:

- Proposals synced vs. on-chain total
- Forum API error rate
- Sync lag (time between proposal creation and thread creation)
- Database size growth

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
