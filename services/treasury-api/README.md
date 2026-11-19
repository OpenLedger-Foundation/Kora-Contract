# Treasury transparency API

This service implements the read-only routes consumed by `apps/web/treasury`. It returns public, PII-safe treasury data from the indexer and includes `lastIndexedLedger` freshness metadata on every indexed response. It never exposes debtor fields or administrative secrets.

## Routes

- `GET /treasury/balances`
- `GET /treasury/fee-sweeps?limit=25&offset=0`
- `GET /treasury/fee-sweeps/:txHash`
- `GET /treasury/grants?limit=25&offset=0&status=executed`
- `GET /treasury/grants/:id`
- `GET /treasury/governance?limit=25&offset=0`
- `GET /treasury/governance/:id`
- `GET /treasury/stats`

Set `INDEXER_URL`, `STELLAR_RPC_URL`, `TREASURY_CONTRACT_ID`, `TREASURY_TOKENS_JSON`, `STELLAR_EXPLORER_URL`, `MAX_INDEXER_LAG_LEDGERS`, and `PORT` through the deployment environment. The balance adapter uses Stellar RPC as a live fallback; history remains indexer-backed. If indexed data is stale or unavailable, endpoints return `503` rather than presenting stale data as current.
