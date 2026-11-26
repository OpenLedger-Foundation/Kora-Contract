/**
 * SSE handler — Server-Sent Events transport for Kora subscriptions.
 *
 * This handler AUTHORIZES the wallet/session before streaming any invoice event.
 * A missing, invalid, or expired session token is rejected with HTTP 401; an
 * attempt to subscribe to a topic the session is not permitted to (e.g. another
 * investor's position stream) is rejected with HTTP 403; exceeding the per-wallet
 * connection rate limit returns HTTP 429 with Retry-After. The handler also evicts
 * a stream mid-flight when the session expires, emitting a `session_expired` SSE
 * event so the client can re-authenticate and reconnect.
 *
 * The caller constructs a {@link SseHandlerOptions} once at process start (see the
 * service README) and passes it to every connection; the handler does NOT trust a
 * caller-supplied address.
 *
 * Usage (Express):
 *   const opts = createSseHandlerOptions(); // builds authz + rate limiter from env
 *   app.get("/subscriptions/sse", (req, res) => handleSseConnection(req, res, opts));
 *
 * Issue: #769
 */

import { IncomingMessage, ServerResponse } from "http";
import { EventBus } from "./event-bus";
import { EventEnvelope, SubscriptionFilter, SubscriptionTopic } from "./types";
import {
  ForbiddenError,
  SessionClaims,
  SubscriptionAuthz,
  UnauthorizedError,
} from "./authz";
import { SubscriptionRateLimiter } from "./rateLimiter";

/** Keep-alive ping interval (ms). */
const KEEPALIVE_MS = 15_000;

export interface SseHandlerOptions {
  /** Session/wallet authorization gate. */
  authz: SubscriptionAuthz;
  /** Per-wallet subscription rate limiter. */
  rateLimiter: SubscriptionRateLimiter;
  /** Optional indexer-health probe; when false the stream degrades and closes. */
  health?: () => boolean;
  /** Injectable clock (ms). Defaults to Date.now. */
  now?: () => number;
}

/** Extract the bearer session token from the request. */
export function extractSessionToken(
  req: IncomingMessage,
  url: URL,
): string | undefined {
  const header = req.headers["authorization"];
  if (typeof header === "string") {
    const m = header.match(/^Bearer\s+(.+)$/i);
    if (m) return m[1].trim();
  }
  const q = url.searchParams.get("session");
  return q ?? undefined;
}

export function handleSseConnection(
  req: IncomingMessage,
  res: ServerResponse,
  options: SseHandlerOptions,
): void {
  const now = options.now ?? Date.now;
  const url = new URL(req.url ?? "/", "http://localhost");
  const topic = (url.searchParams.get("topic") ?? "") as SubscriptionTopic;
  const invoiceIdParam = url.searchParams.get("invoiceId");
  const investorAddressParam =
    url.searchParams.get("investorAddress") ?? undefined;
  const afterCursor = url.searchParams.get("after") ?? undefined;
  const token = extractSessionToken(req, url);

  // 1. Authorize wallet/session (fail closed).
  let claims: SessionClaims;
  try {
    claims = options.authz.authorize(token, Math.floor(now() / 1000));
  } catch (err) {
    const e = err as UnauthorizedError;
    res.writeHead(e.statusCode, { "Content-Type": "application/json" });
    res.end(
      JSON.stringify({ code: e.code, message: e.message }),
    );
    return;
  }

  // 2. Topic authorization (permitted topics + per-wallet scoping).
  const filter: SubscriptionFilter = {};
  if (invoiceIdParam) filter.invoiceId = BigInt(invoiceIdParam);
  if (investorAddressParam) filter.investorAddress = investorAddressParam;
  try {
    options.authz.canSubscribe(claims, topic, filter);
  } catch (err) {
    const e = err as ForbiddenError;
    res.writeHead(e.statusCode, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ code: e.code, message: e.message }));
    return;
  }

  // 3. Per-wallet rate limit on new subscriptions.
  const rl = options.rateLimiter.consume(claims.address);
  if (!rl.allowed) {
    const retryAfter = Math.max(1, Math.ceil(rl.resetAfterMs / 1000));
    res.writeHead(429, {
      "Content-Type": "application/json",
      "Retry-After": String(retryAfter),
    });
    res.end(
      JSON.stringify({
        code: "RATE_LIMITED",
        message: "Subscription rate limit exceeded",
        retryAfterSeconds: retryAfter,
      }),
    );
    return;
  }

  res.setHeader("Content-Type", "text/event-stream");
  res.setHeader("Cache-Control", "no-cache");
  res.setHeader("Connection", "keep-alive");
  res.setHeader("X-Accel-Buffering", "no");
  res.flushHeaders?.();

  const send = (env: EventEnvelope): void => {
    res.write(`id: ${env.cursor}\n`);
    res.write(
      `data: ${JSON.stringify(env.event, (_k, v) =>
        typeof v === "bigint" ? v.toString() : v,
      )}\n\n`,
    );
  };

  let subscriptionId: string;
  try {
    subscriptionId = EventBus.instance.subscribe(
      topic,
      filter,
      claims.address,
      send,
      afterCursor,
    );
  } catch (err: unknown) {
    const message =
      err instanceof Error ? err.message : "Subscription error";
    res.writeHead(403, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ code: "FORBIDDEN", message }));
    return;
  }

  // Keep-alive ping so proxies don't time out idle connections. Also evicts the
  // stream when the session expires mid-flight or the indexer degrades.
  const ping = setInterval(() => {
    if (options.authz.isExpired(claims, Math.floor(now() / 1000))) {
      res.write(
        `event: session_expired\ndata: ${JSON.stringify({
          message: "Session expired; please re-authenticate",
        })}\n\n`,
      );
      clearInterval(ping);
      EventBus.instance.unsubscribe(subscriptionId);
      res.end();
      return;
    }
    if (options.health && !options.health()) {
      // Do not leave clients attached to a stream backed by an unavailable
      // indexer. A reconnect lets the caller obtain a fresh cursor/state.
      res.write(
        `event: degraded\ndata: ${JSON.stringify({ retryAfterSeconds: 5 })}\n\n`,
      );
      clearInterval(ping);
      EventBus.instance.unsubscribe(subscriptionId);
      res.end();
      return;
    }
    res.write(": ping\n\n");
  }, KEEPALIVE_MS);

  req.on("close", () => {
    clearInterval(ping);
    EventBus.instance.unsubscribe(subscriptionId);
  });
}
