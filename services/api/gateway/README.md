# API Gateway

**Issue #753 — REST/GraphQL API Gateway for Kora Protocol Data**

Public-facing, read-only gateway over the indexer database: invoices,
listings, positions, and risk scores. Backs the frontend and third-party
integrators so they never hit the chain per read.

Out of scope: write operations (all writes remain on-chain, signed by the
user's wallet).

## Endpoints (query layer)

| Query | Filters | Sort | Pagination |
|---|---|---|---|
| `queryInvoices` | sme, status, riskTier, riskScore, currency | any field | cursor, limit 1–100 |
| `queryListings` | seller, token, isActive | any field | cursor, limit 1–100 |
| `queryPositions` | investor, invoiceId | any field | cursor, limit 1–100 |
| `queryRiskScores` | kind, verifier, score | any field | cursor, limit 1–100 |

Every page returns `{ data, nextCursor, lastIndexedLedger }`.
`lastIndexedLedger` exposes indexer lag transparently.

## Rate limiting

Per-API-key token bucket with access tiers:

| Tier | Budget |
|---|---|
| `free` | 60 req/min |
| `standard` | 600 req/min |
| `enterprise` | 6000 req/min |

Unknown keys default to `free`. Exceeding the budget throws
`RateLimitedError(resetAfterMs)`; budgets reset automatically per window or via
`RateLimiter.reset(key)`.

## Usage

```ts
import { ApiGateway, createMemoryStore } from "@kora/api-gateway";

const gateway = new ApiGateway(indexerDb /* IndexedStore */, rateLimiter);
const page = gateway.queryInvoices(
  { status: { eq: "Listed" } },
  { limit: 20 },
  { apiKey: "partner-key", sortBy: "createdAt", sortDir: "desc" },
);
console.log(page.data, page.nextCursor, page.lastIndexedLedger);
```

## OpenAPI / GraphQL

The `ApiGateway` class is transport-agnostic: mount each `query*` method as a
REST `GET` resource (`/invoices`, `/listings`, `/positions`, `/risk-scores`
with `filter[…]`, `sort`, `cursor`, `limit` params) and/or as GraphQL root
fields sharing the same `Filter`/`Page` shapes in `src/types.ts`.

## Testing

From `services/api/gateway`:

```bash
npm install
npm test   # 90% line/branch/function/statement coverage enforced
```

Covers pagination correctness, filter combinations, rate-limit enforcement and
reset.

## Database timeout policy (Closes #1095 — Chaos GAP-2)

### How it works

Pool adapters call `configureDbSession(connection)` immediately after acquiring
a connection. This issues `SET statement_timeout = <ms>` so every statement in
the request lifecycle is bounded, regardless of query complexity or network
conditions.

PostgreSQL SQLSTATE `57014` (and equivalent driver message patterns) is detected
by `isDbTimeoutError()` and converted to `DbTimeoutError` by the gateway's
catch blocks. Transport adapters catch this distinct error class to emit the
correct HTTP response without swallowing the structured error code.

### Environment variable

| Variable | Default | Validation |
|---|---:|---|
| `DB_STATEMENT_TIMEOUT_MS` | `5000` | Must be a positive integer; zero, negative, and non-integer values throw at startup |

### HTTP response contract

A timed-out DB statement produces:

```
HTTP/1.1 503 Service Unavailable
Retry-After: 5
Content-Type: application/json

{"code":"DB_TIMEOUT","message":"Database operation timed out; retry the request","retryable":true}
```

`Retry-After` is `ceil(DB_STATEMENT_TIMEOUT_MS / 1000)`, minimum 1 second.

### Error class

```ts
import { DbTimeoutError, DB_TIMEOUT_CODE } from "@kora/api-gateway";

try {
  const page = gateway.queryInvoices(filter, pageArgs, opts);
} catch (err) {
  if (err instanceof DbTimeoutError) {
    res.setHeader("Retry-After", String(err.retryAfterSeconds));
    res.status(err.statusCode).json({
      code: err.code,         // "DB_TIMEOUT"
      message: err.message,
      retryable: err.retryable, // true
    });
    return;
  }
  throw err;
}
```

### Pool configuration example

```ts
import { configureDbSession, readDbStatementTimeout } from "@kora/api-gateway";

// Validate at startup — throws if DB_STATEMENT_TIMEOUT_MS is misconfigured.
const timeoutMs = readDbStatementTimeout();

pool.on("connect", async (client) => {
  await configureDbSession(client, timeoutMs);
});
```

### Reporting / long-running queries

Reporting routes that run queries longer than the API timeout must use a
separately configured pool or database role with an explicit, reviewed timeout
value. The main API pool must never be globally relaxed for export workloads.

## Stale cache fail-closed (Closes #<issue_number> — Chaos GAP-1)

### Problem

During a DB network partition the read-through cache kept serving HTTP 200
with stale data for up to ~8 s, violating the resilience contract: callers
assumed live data but received outdated funding availability figures.

### Fix

Every cache entry is now timestamped.  `beginRead` applies three rules in
priority order:

| Rule | Condition | Outcome |
|------|-----------|---------|
| 1 — fail-closed | `store.health.available === false` | Evict all matching entries; throw `StaleCacheError` if an entry existed, `ApiUnavailableError` if no entry |
| 2 — cache disabled | `CACHE_FRESHNESS_SLA_MS === 0` | Always miss (read-only / no-cache mode) |
| 3 — expired entry | Entry age > `CACHE_FRESHNESS_SLA_MS` | Evict; fall through to live DB |
| 4 — fresh hit | None of the above | Return cached page |

### StaleCacheError

A new error class distinct from `ApiUnavailableError` and `DbTimeoutError` so
transport adapters and clients can apply targeted retry logic (wait for the
partition to heal rather than immediately retrying a timed-out statement).

```ts
import { StaleCacheError } from "@kora/api-gateway";

try {
  const page = gateway.queryListings(filter, pageArgs, opts);
} catch (err) {
  if (err instanceof StaleCacheError) {
    // err.code          → "STALE_CACHE"
    // err.statusCode    → 503
    // err.retryable     → true
    // err.ageMs         → how old the evicted entry was
    // err.retryAfterSeconds → how long to wait before retrying
    res.setHeader("Retry-After", String(err.retryAfterSeconds));
    res.status(503).json({ code: err.code, message: err.message, retryable: true });
    return;
  }
}
```

### Environment variable

| Variable | Default | Description |
|---|---:|---|
| `CACHE_FRESHNESS_SLA_MS` | `8000` | Maximum cache entry age in ms. Set to `0` to disable caching entirely (read-only/no-cache mode). Must be a non-negative integer. |

### Chaos partition scenario

```
Baseline:    queryListings() → 200 (live data, entry cached at T)
Partition:   store.health.available = false
  T + 1 s:  queryListings() → StaleCacheError 503  (entry evicted, age reported)
  T + 2 s:  queryListings() → StaleCacheError 503  (no entry, ApiUnavailableError)
Recovery:   store.health.available = true
  T + 10 s: queryListings() → 200 (live data, fresh entry cached)
```

No silent 200 is ever served while the store is degraded.

### Read-through vs read-only cache modes

| Mode | Config | Behaviour |
|------|--------|-----------|
| Read-through (default) | `CACHE_FRESHNESS_SLA_MS=8000` | Cache populated on first DB hit; evicted on DB failure or expiry |
| No-cache | `CACHE_FRESHNESS_SLA_MS=0` | Every request hits the DB; `StaleCacheError` is impossible |
| Extended cache | `CACHE_FRESHNESS_SLA_MS=30000` | Relaxed for high-read-low-write workloads; set alongside a tighter DB health probe |
