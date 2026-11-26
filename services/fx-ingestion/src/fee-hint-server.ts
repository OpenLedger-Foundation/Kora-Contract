/**
 * FX Ingestion Service — Fee Hint HTTP endpoint
 *
 *   GET /network/fee-hint  → advisory FeeHint JSON (always 200 while the process is up)
 *   GET /metrics           → Prometheus metrics incl. `kora_surge_pricing_active`
 *
 * Responses carry `Cache-Control: max-age=<poll interval>` and, when the hint
 * is served from last-known-good data, `X-Fee-Hint-Stale: true` plus `Age`.
 * A per-client fixed-window limiter returns 429 + Retry-After so the public
 * gateway cannot be used to hammer the upstream RPC (hints are cached; the
 * endpoint never calls RPC itself).
 *
 * Issue #1034
 */

import http from "http";
import pino from "pino";
import { SurgePricingMonitor, renderPrometheusMetrics } from "./fee-monitor";

const logger = pino({ name: "fx-ingestion:fee-hint-server" });

export interface FeeHintServerConfig {
  port: number;
  /** Requests allowed per client per window. Default 60. */
  rateLimitPerWindow: number;
  /** Window length (ms). Default 60_000. */
  rateLimitWindowMs: number;
  /** Cache-Control max-age (seconds). */
  maxAgeSeconds: number;
}

export function loadFeeHintServerConfig(env: NodeJS.ProcessEnv = process.env): FeeHintServerConfig {
  return {
    port: Number(env.FEE_HINT_PORT ?? "8089"),
    rateLimitPerWindow: Number(env.FEE_HINT_RATE_LIMIT ?? "60"),
    rateLimitWindowMs: Number(env.FEE_HINT_RATE_LIMIT_WINDOW_MS ?? "60000"),
    maxAgeSeconds: Math.max(1, Math.floor(Number(env.FEE_HINT_POLL_INTERVAL_MS ?? "15000") / 1000)),
  };
}

/** Fixed-window per-key limiter. */
export class FixedWindowLimiter {
  private readonly windows = new Map<string, { start: number; count: number }>();

  constructor(
    private readonly limit: number,
    private readonly windowMs: number,
    private readonly now: () => number = Date.now
  ) {}

  /** Returns 0 when allowed, otherwise ms until the window resets. */
  consume(key: string): number {
    const t = this.now();
    const w = this.windows.get(key);
    if (!w || t - w.start >= this.windowMs) {
      this.windows.set(key, { start: t, count: 1 });
      if (this.windows.size > 10_000) this.prune(t);
      return 0;
    }
    if (w.count >= this.limit) return this.windowMs - (t - w.start);
    w.count += 1;
    return 0;
  }

  private prune(t: number): void {
    for (const [k, w] of this.windows) {
      if (t - w.start >= this.windowMs) this.windows.delete(k);
    }
  }
}

export interface FeeHintResponse {
  status: number;
  headers: Record<string, string>;
  body: string;
}

/** Transport-agnostic request handler (exported for tests). */
export function handleFeeHintRequest(
  method: string | undefined,
  url: string | undefined,
  clientKey: string,
  monitor: SurgePricingMonitor,
  limiter: FixedWindowLimiter,
  config: Pick<FeeHintServerConfig, "maxAgeSeconds">,
  now: () => number = Date.now
): FeeHintResponse {
  const path = (url ?? "/").split("?")[0];
  if (method !== "GET") {
    return { status: 405, headers: { Allow: "GET", "Content-Type": "application/json" }, body: JSON.stringify({ error: "METHOD_NOT_ALLOWED" }) };
  }

  if (path === "/metrics") {
    return {
      status: 200,
      headers: { "Content-Type": "text/plain; version=0.0.4" },
      body: renderPrometheusMetrics(monitor.getMetrics()),
    };
  }

  if (path !== "/network/fee-hint") {
    return { status: 404, headers: { "Content-Type": "application/json" }, body: JSON.stringify({ error: "NOT_FOUND" }) };
  }

  const retryAfterMs = limiter.consume(clientKey);
  if (retryAfterMs > 0) {
    return {
      status: 429,
      headers: { "Content-Type": "application/json", "Retry-After": String(Math.ceil(retryAfterMs / 1000)) },
      body: JSON.stringify({ error: "RATE_LIMITED", retry_after_ms: retryAfterMs }),
    };
  }

  const hint = monitor.getHint();
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    "Cache-Control": `public, max-age=${config.maxAgeSeconds}`,
    "X-Fee-Hint-Stale": String(hint.stale),
    "X-Fee-Hint-Degraded": String(hint.degraded),
  };
  if (hint.observed_at) {
    headers["Age"] = String(Math.max(0, Math.floor((now() - Date.parse(hint.observed_at)) / 1000)));
  }
  return { status: 200, headers, body: JSON.stringify(hint) };
}

export function startFeeHintServer(
  monitor: SurgePricingMonitor,
  config: FeeHintServerConfig
): http.Server {
  const limiter = new FixedWindowLimiter(config.rateLimitPerWindow, config.rateLimitWindowMs);
  const server = http.createServer((req, res) => {
    const clientKey = req.socket.remoteAddress ?? "unknown";
    const out = handleFeeHintRequest(req.method, req.url, clientKey, monitor, limiter, config);
    res.writeHead(out.status, out.headers);
    res.end(out.body);
  });
  server.listen(config.port, () => logger.info({ port: config.port }, "Fee hint endpoint listening"));
  return server;
}
