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
  // ── New services (issue #771 expansion) ─────────────────────────────────────
  /**
   * timeseries — downsampling lag and ingestion queue depth.
   *   degraded: downsampling job has not run for >10 min (lagSeconds > 600)
   *             OR the raw ingest queue is backing up (ingestQueueDepth > 100)
   *   down:     job stalled for >60 min OR queue depth critical (> 1 000)
   */
  timeseries: {
    degradedThresholds: { lagSeconds: 600, ingestQueueDepth: 100 },
    downThresholds:     { lagSeconds: 3_600, ingestQueueDepth: 1_000 },
    timeoutMs: 3_000,
  },
  /**
   * risk-pipeline — batch scoring idempotency and duplicate rates.
   *   degraded: duplicate score submissions detected (duplicateRate > 0, < 5 %)
   *             OR scoring backlog building (scoringBacklogDepth > 20)
   *   down:     duplicate rate ≥ 5 % (may flip tiers) OR backlog critical (> 200)
   */
  "risk-pipeline": {
    degradedThresholds: { duplicateRate: 1, scoringBacklogDepth: 20 },
    downThresholds:     { duplicateRate: 5, scoringBacklogDepth: 200 },
    timeoutMs: 3_000,
  },
  /**
   * governance-sandbox — proposal simulation service.
   *   degraded: snapshot store approaching capacity (snapshotCount > 5 000)
   *             OR average simulation latency elevated (simLatencyMs > 2 000)
   *   down:     store full / unavailable (snapshotCount > 50 000)
   *             OR simulations timing out (simLatencyMs > 10 000)
   */
  "governance-sandbox": {
    degradedThresholds: { snapshotCount: 5_000, simLatencyMs: 2_000 },
    downThresholds:     { snapshotCount: 50_000, simLatencyMs: 10_000 },
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

export function timeseriesProbeUrl(): string {
  return process.env.TIMESERIES_URL ?? "http://timeseries:8080/health";
}

export function riskPipelineProbeUrl(): string {
  return process.env.RISK_PIPELINE_URL ?? "http://risk-pipeline:8080/health";
}

export function governanceSandboxProbeUrl(): string {
  return process.env.GOVERNANCE_SANDBOX_URL ?? "http://governance-sandbox:8080/health";
}

/** Default upstream descriptors including all known Kora service probes. */
export function buildDefaultServices(): UpstreamService[] {
  return [
    { name: "reporting",          url: reportingProbeUrl(),          sla: DEFAULT_SLAS.reporting },
    { name: "ipfs-pinning",       url: ipfsPinningProbeUrl(),        sla: DEFAULT_SLAS["ipfs-pinning"] },
    { name: "timeseries",         url: timeseriesProbeUrl(),         sla: DEFAULT_SLAS.timeseries },
    { name: "risk-pipeline",      url: riskPipelineProbeUrl(),       sla: DEFAULT_SLAS["risk-pipeline"] },
    { name: "governance-sandbox", url: governanceSandboxProbeUrl(),  sla: DEFAULT_SLAS["governance-sandbox"] },
  ];
}
