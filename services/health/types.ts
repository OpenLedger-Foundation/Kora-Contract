/**
 * Health Check Service — shared types
 *
 * Defines the standard /health contract that every upstream Kora service
 * must expose, plus the aggregated status shape returned by the health
 * aggregator endpoint.
 *
 * Issue: #771
 */

// ── Per-service status ────────────────────────────────────────────────────────

export type ServiceStatus = "ok" | "degraded" | "down";

/**
 * Standard health response shape every upstream service exposes at GET /health.
 *
 * Services MUST return HTTP 200 for both "ok" and "degraded" — only "down"
 * maps to a non-2xx response (or a timeout/network error).
 */
export interface ServiceHealthReport {
  status: ServiceStatus;
  /** ISO-8601 timestamp of when the check was taken. */
  checkedAt: string;
  /** Arbitrary per-service metrics (indexer lag, queue depth, etc.). */
  metrics: Record<string, number | string | boolean>;
  /** Human-readable message when status !== "ok". */
  message?: string;
}

// ── SLA thresholds ────────────────────────────────────────────────────────────

/**
 * SLA thresholds for a single service.  The aggregator applies these to the
 * raw metrics to compute the final status.
 */
export interface ServiceSlaConfig {
  /** Maximum allowed value for a numeric metric before status → "degraded". */
  degradedThresholds?: Record<string, number>;
  /** Maximum allowed value before status → "down". */
  downThresholds?: Record<string, number>;
  /** HTTP timeout (ms) when polling the upstream /health endpoint. */
  timeoutMs: number;
}

// ── Aggregated result ─────────────────────────────────────────────────────────

export interface ServiceAggregateEntry {
  name: string;
  status: ServiceStatus;
  metrics: Record<string, number | string | boolean>;
  message?: string;
  /** True when the upstream endpoint could not be reached within the timeout. */
  unreachable: boolean;
}

export interface AggregatedHealthReport {
  /** Aggregated schema version (bump on contract change). */
  schemaVersion: string;
  /** Overall protocol health — "ok" only when ALL services are "ok". */
  overall: ServiceStatus;
  /** ISO-8601 timestamp of the aggregation run. */
  aggregatedAt: string;
  services: ServiceAggregateEntry[];
}
