/**
 * Health aggregator — polls all upstream Kora service /health endpoints
 * in parallel and rolls up results into a single AggregatedHealthReport.
 *
 * Key properties:
 *  - Per-service timeouts: one unreachable service never blocks the others.
 *  - Status classification: SLA thresholds determine "ok" / "degraded" / "down".
 *  - Parallel fetch: all upstream services polled concurrently.
 *
 * Issue: #771
 */

import {
  AggregatedHealthReport,
  ServiceAggregateEntry,
  ServiceHealthReport,
  ServiceSlaConfig,
} from "./types";
import { classifyPinningStatus, classifyServiceStatus, rollupStatus, unreachableEntry } from "./sla-classifier";

// ── Upstream service descriptor ───────────────────────────────────────────────

export interface UpstreamService {
  /** Display name used in the aggregated report. */
  name: string;
  /** URL of the upstream /health endpoint. */
  url: string;
  sla: ServiceSlaConfig;
}

// ── HTTP fetcher abstraction ──────────────────────────────────────────────────

/**
 * Injectable HTTP fetch abstraction so the aggregator can be unit-tested
 * without real network calls.
 */
export type HealthFetcher = (
  url: string,
  timeoutMs: number
) => Promise<ServiceHealthReport>;

// ── Aggregator ────────────────────────────────────────────────────────────────

export class HealthAggregator {
  constructor(
    private readonly services: UpstreamService[],
    private readonly fetcher: HealthFetcher
  ) {}

  /**
   * Polls all upstream services in parallel and returns the aggregated report.
   * A service that times out or throws is recorded as "down / unreachable"
   * without blocking the results from other services.
   */
  async aggregate(): Promise<AggregatedHealthReport> {
    const results = await Promise.allSettled(
      this.services.map((svc) => this._pollOne(svc))
    );

    const entries: ServiceAggregateEntry[] = results.map((result, i) => {
      if (result.status === "fulfilled") return result.value;
      // Should never happen because _pollOne catches internally, but guard anyway.
      return unreachableEntry(this.services[i].name);
    });

    const overall = rollupStatus(entries.map((e) => e.status));

    return {
      schemaVersion: HEALTH_SCHEMA_VERSION,
      overall,
      aggregatedAt: new Date().toISOString(),
      services: entries,
    };
  }

  // ── Private helpers ────────────────────────────────────────────────────────

  private async _pollOne(svc: UpstreamService): Promise<ServiceAggregateEntry> {
    try {
      const report = await this.fetcher(svc.url, svc.sla.timeoutMs);
      const status =
        svc.name === "ipfs-pinning" || svc.name === "pinning"
          ? classifyPinningStatus(report, svc.sla)
          : classifyServiceStatus(report, svc.sla);
      return {
        name: svc.name,
        status,
        metrics: report.metrics,
        message: status !== "ok" ? (report.message ?? `SLA threshold exceeded`) : undefined,
        unreachable: false,
      };
    } catch {
      return unreachableEntry(svc.name);
    }
  }
}

// ── Default SLA configs per Kora service ─────────────────────────────────────

/**
 * Default SLA thresholds per service type.
 * Operators may override these via environment / config injection.
 */
export const DEFAULT_SLAS: Record<string, ServiceSlaConfig> = {
  indexer: {
    degradedThresholds: { lagSeconds: 10 },
    downThresholds: { lagSeconds: 60 },
    timeoutMs: 3_000,
  },
  api: {
    degradedThresholds: { p99LatencyMs: 500 },
    downThresholds: { p99LatencyMs: 5_000 },
    timeoutMs: 3_000,
  },
  keeper: {
    degradedThresholds: { backlogDepth: 50 },
    downThresholds: { backlogDepth: 500 },
    timeoutMs: 3_000,
  },
  notification: {
    degradedThresholds: { deliveryFailureRate: 5 },   // percent
    downThresholds: { deliveryFailureRate: 20 },
    timeoutMs: 3_000,
  },
  reconciliation: {
    degradedThresholds: { pendingReconciliations: 10 },
    downThresholds: { pendingReconciliations: 100 },
    timeoutMs: 3_000,
  },
  pinning: {
    degradedThresholds: { unpinnedCids: 20 },
    downThresholds: { unpinnedCids: 200 },
    timeoutMs: 3_000,
  },
  reporting: {
    degradedThresholds: { exportQueueDepth: 100 },
    downThresholds: { exportQueueDepth: 1_000 },
    timeoutMs: 3_000,
  },
  "ipfs-pinning": {
    degradedThresholds: { failedProviders: 1 },
    downThresholds: { failedProviders: 999_999 },
    timeoutMs: 3_000,
  },
};

export const HEALTH_SCHEMA_VERSION = "1.1.0";

/** Env-configurable probe URLs (IPv6-safe: set to bracketed host if needed). */
export function reportingProbeUrl(): string {
  return process.env.REPORTING_URL ?? "http://reporting:8080/health";
}

export function ipfsPinningProbeUrl(): string {
  return process.env.IPFS_PINNING_URL ?? "http://ipfs-pinning:8080/health";
}

/** Default upstream descriptors including reporting + ipfs-pinning probes. */
export function buildDefaultServices(): UpstreamService[] {
  return [
    { name: "reporting", url: reportingProbeUrl(), sla: DEFAULT_SLAS.reporting },
    { name: "ipfs-pinning", url: ipfsPinningProbeUrl(), sla: DEFAULT_SLAS["ipfs-pinning"] },
  ];
}
