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
