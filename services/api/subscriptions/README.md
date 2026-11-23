# API Subscriptions (SSE)

**Issue #769 — Authorized SSE fan-out for Kora invoice events**

Server-Sent Events fan-out for invoice events emitted by the indexer. Every
stream is authorized against a verified wallet/session **before** any event is
delivered, so counterparty commercial data (repayments, investor positions) is
never leaked to an unauthenticated or cross-investor subscriber.

## Architecture

```
client ──HTTP/SSE──► sse-handler ──authz──► EventBus ──► indexer-adapter (on-chain events)
                       │  rate-limit
                       └─ mid-stream session expiry evicts the connection
```

- `sse-handler.ts` — transport entry point. Authorizes the session, enforces
  the per-wallet rate limit, subscribes the authorized wallet to the EventBus,
  and evicts the stream on mid-stream session expiry or indexer degradation.
- `authz.ts` — session/wallet authorization gate (HMAC-signed session tokens)
  and the topic-authorization policy (permitted topics + per-wallet scoping).
- `rateLimiter.ts` — per-wallet token-bucket rate limiter for new connections.
- `event-bus.ts` — in-process fan-out with per-user scoping for
  `position-status-change` and cursor-based reconnect replay.
- `indexer-adapter.ts` — translates raw on-chain Soroban events into typed
  `KoraSubscriptionEvent`s (see `docs/EVENTS.md`).

## Authorization model

Subscriptions authenticate with a **session token**, not a raw wallet signature
on every connection. The login flow (out of scope for this service) verifies the
wallet signature and issues an HMAC-signed session bound to the wallet address.

| Step | Check | Failure |
|---|---|---|
| 1 | Valid, non-expired session token (`Authorization: Bearer <token>` or `?session=`) | HTTP 401 |
| 2 | Topic is permitted and, for `position-status-change`, scoped to the authenticated wallet | HTTP 403 |
| 3 | Per-wallet connection rate limit not exceeded | HTTP 429 + `Retry-After` |
| 4 | Mid-stream: session must not expire while the stream is open | `event: session_expired` + close |

The handler never trusts a caller-supplied address; the authenticated wallet
address from the session is passed to the EventBus as the subscriber identity.

### Permitted topics

An authenticated session may subscribe to any of:

| Topic | Scoping |
|---|---|
| `listing-funding-progress` | Public funding progress for a listing/invoice |
| `repayment` | Repayment events for a listing/invoice |
| `position-status-change` | Scoped to the authenticated investor only |

A subscriber may not request `position-status-change` for an
`investorAddress` other than its own; the server auto-scopes to the
authenticated wallet.

### Mid-stream session expiry

Long-lived SSE connections are re-checked on each keep-alive ping (15 s). When the
session expires mid-stream, the handler emits a `session_expired` event and closes
the connection so the client re-authenticates and reconnects with a fresh cursor.

## Configuration

| Env var | Default | Description |
|---|---|---|
| `SESSION_SECRET` | _(required)_ | HMAC secret used to validate session tokens. Must be ≥16 chars. Fail-closed: unset → service refuses to start |
| `SUBSCRIPTION_RATE_LIMIT_PER_MIN` | `30` | Max new subscriptions per wallet per minute. Must be a positive integer |

In deployed environments, inject `SESSION_SECRET` from the environment-specific
secret in AWS Secrets Manager using the workload's read-only role. Do not place
secrets in `.env` files or deployment manifests. See
[the secrets runbook](../../../infra/secrets/README.md).

## Usage

```ts
import {
  handleSseConnection,
  createHmacSessionVerifier,
  SubscriptionAuthz,
  SubscriptionRateLimiter,
  readSessionSecret,
  readSubscriptionRateLimit,
} from "@kora/api-subscriptions";

const authz = new SubscriptionAuthz(
  createHmacSessionVerifier(readSessionSecret()),
);
const rateLimiter = new SubscriptionRateLimiter({
  maxRequestsPerWindow: readSubscriptionRateLimit(),
});

app.get("/subscriptions/sse", (req, res) =>
  handleSseConnection(req, res, { authz, rateLimiter, health: () => indexerHealthy }),
);
```

## Testing

```bash
npm install
npm test   # 90% line/branch/function/statement coverage enforced
```

Covers authorization fail-closed paths, permitted-topic scoping, rate-limit
enforcement, mid-stream session expiry, and indexer-degradation eviction.
