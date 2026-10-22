/**
 * SLA classifier — applies configured thresholds to raw service metrics and
 * determines whether a service is "ok", "degraded", or "down".
 *
 * Issue: #771
 */

import { ServiceAggregateEntry, ServiceHealthReport, ServiceSlaConfig, ServiceStatus } from "./types";

/**
 * Applies SLA thresholds to a successfully fetched health report.
 *
 * Rules (applied in descending severity):
 *  1. Any metric ≥ `downThresholds[metric]`   → "down"
 *  2. Any metric ≥ `degradedThresholds[metric]` → "degraded"
 *  3. Otherwise keep the service-reported status (capped at "degraded" if
 *     the service itself reports "degraded").
 */
export function classifyServiceStatus(
  report: ServiceHealthReport,
  sla: ServiceSlaConfig
): ServiceStatus {
  for (const [key, threshold] of Object.entries(sla.downThresholds ?? {})) {
    const value = report.metrics[key];
    if (typeof value === "number" && value >= threshold) return "down";
  }

  for (const [key, threshold] of Object.entries(sla.degradedThresholds ?? {})) {
    const value = report.metrics[key];
    if (typeof value === "number" && value >= threshold) return "degraded";
  }

  return report.status;
}

/**
 * Pinning provider check: degraded when all configured providers fail.
 * Expects metrics `providersReachable` (number) and `providerCount` (number).
 * Falls back to generic SLA classification when those bits are absent.
 */
export function classifyPinningStatus(
  report: ServiceHealthReport,
  sla: ServiceSlaConfig
): ServiceStatus {
  const reachable = report.metrics["providersReachable"];
  const total = report.metrics["providerCount"];
  const failed = report.metrics["failedProviders"];
  if (typeof reachable === "number" && typeof total === "number" && total > 0) {
    if (reachable <= 0) return "down";
    if (reachable < total) return "degraded";
  }
  if (typeof failed === "number" && typeof total === "number" && total > 0) {
    if (failed >= total) return "down";
  }
  return classifyServiceStatus(report, sla);
}

/**
 * Builds a `ServiceAggregateEntry` for a service that could not be reached
 * (timeout or network error).
 */
export function unreachableEntry(name: string): ServiceAggregateEntry {
  return {
    name,
    status: "down",
    metrics: {},
    message: "Service unreachable — timeout or network error",
    unreachable: true,
  };
}

/**
 * Rolls up a list of per-service statuses into an overall protocol status.
 *
 * - Any "down" → overall "down"
 * - Any "degraded" (no "down") → overall "degraded"
 * - All "ok" → overall "ok"
 */
export function rollupStatus(statuses: ServiceStatus[]): ServiceStatus {
  if (statuses.some((s) => s === "down")) return "down";
  if (statuses.some((s) => s === "degraded")) return "degraded";
  return "ok";
}
