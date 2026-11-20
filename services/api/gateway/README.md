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

## Database timeout policy

API database adapters must run `configureDbSession()` after acquiring a pooled PostgreSQL connection. The helper issues `SET statement_timeout = ...` using `DB_STATEMENT_TIMEOUT_MS`, which defaults to 5000 ms and rejects zero, negative, or non-integer values at startup. PostgreSQL SQLSTATE `57014` is mapped by `mapDbError()` to HTTP 503 with this body:

```json
{"code":"DB_TIMEOUT","message":"Database operation timed out; retry the request","retryable":true}
```

The HTTP adapter should also send `Retry-After` using the returned `retryAfterSeconds` value. Reporting routes that intentionally run longer must use a separately configured pool or an explicit role with a documented timeout exemption; the API pool should never be globally relaxed for exports.
