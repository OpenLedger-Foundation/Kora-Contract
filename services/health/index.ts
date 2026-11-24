/**
 * Health aggregation service — process entrypoint.
 *
 * Loads upstream /health URLs from the environment, constructs the
 * {@link HealthAggregator}, and serves GET /health (AggregatedHealthReport
 * JSON) plus GET /metrics (Prometheus text) via `createHealthHttpServer`.
 *
 * Env vars (see README.md):
 *  - HEALTH_PORT (default 8081)
 *  - KEEPER_HEALTH_URL, INDEXER_HEALTH_URL, API_HEALTH_URL,
 *    FEATURE_FLAGS_HEALTH_URL, REPORTING_HEALTH_URL
 *  - HEALTH_SERVICES: comma-separated subset to poll (default: all)
 *  - HEALTH_POLL_TIMEOUT_MS (default 2000)
 *
 * Issue #1024
 */

import * as http from "http";
import { HealthAggregator, HealthFetcher, UpstreamService } from "./health-aggregator";
import { createHealthHttpServer } from "./http-server";
import { ServiceHealthReport } from "./types";

function env(name: string, fallback: string): string {
  const value = process.env[name];
  return value === undefined || value === "" ? fallback : value;
}

/** Default `fetch`-based poller: timeouts classify as unreachable, never throw out. */
export function defaultHealthFetcher(): HealthFetcher {
  return async (url: string, timeoutMs: number): Promise<ServiceHealthReport> => {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const res = await fetch(url, {
        headers: { Accept: "application/json" },
        signal: controller.signal,
      });
      if (!res.ok) {
        return {
          status: "down",
          checkedAt: new Date().toISOString(),
          metrics: {},
          message: `upstream HTTP ${res.status}`,
        };
      }
      const body = (await res.json()) as Partial<ServiceHealthReport>;
      const status = body.status;
      return {
        status: status === "ok" || status === "degraded" || status === "down" ? status : "down",
        checkedAt: typeof body.checkedAt === "string" ? body.checkedAt : new Date().toISOString(),
        metrics:
          body.metrics !== undefined && typeof body.metrics === "object" && body.metrics !== null
            ? (body.metrics as Record<string, number | string | boolean>)
            : {},
        message: typeof body.message === "string" ? body.message : undefined,
      };
    } finally {
      clearTimeout(timer);
    }
  };
}

/** Build the upstream list from env, matching the deploy-services/nginx set. */
export function loadUpstreams(timeoutMs: number): UpstreamService[] {
  const all = [
    { name: "keeper", url: env("KEEPER_HEALTH_URL", "http://keeper:8080/health") },
    { name: "indexer", url: env("INDEXER_HEALTH_URL", "http://indexer:8080/health") },
    { name: "api", url: env("API_HEALTH_URL", "http://api:8080/health") },
    {
      name: "feature-flags",
      url: env("FEATURE_FLAGS_HEALTH_URL", "http://feature-flags:8080/health"),
    },
    { name: "reporting", url: env("REPORTING_HEALTH_URL", "http://reporting:8080/health") },
  ];
  const wanted = env("HEALTH_SERVICES", all.map((svc) => svc.name).join(","))
    .split(",")
    .map((name) => name.trim())
    .filter(Boolean);
  const unknown = wanted.filter((name) => !all.some((svc) => svc.name === name));
  if (wanted.length === 0 || unknown.length > 0) {
    throw new Error(`HEALTH_SERVICES is invalid: ${unknown.join(", ") || "no services listed"}`);
  }
  return all
    .filter((svc) => wanted.includes(svc.name))
    .map((svc) => ({ ...svc, sla: { timeoutMs } }));
}

function main(): void {
  const timeoutMs = Number(process.env.HEALTH_POLL_TIMEOUT_MS ?? "2000");
  const aggregator = new HealthAggregator(loadUpstreams(timeoutMs), defaultHealthFetcher());
  const server: http.Server = createHealthHttpServer(aggregator);

  const port = Number(process.env.HEALTH_PORT ?? "8081");
  server.listen(port, () => {
    // eslint-disable-next-line no-console
    console.log(`health-aggregator listening on :${port}`);
  });

  const shutdown = (): void => {
    server.close(() => process.exit(0));
  };
  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
}

// Only auto-boot when run as a process, not when imported as a library.
if (require.main === module) {
  main();
}

export { main as startHealthService };
