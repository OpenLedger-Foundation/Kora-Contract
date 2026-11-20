/**
 * Probe helpers that query the staging services to verify resilience claims.
 *
 * These are deliberately thin HTTP wrappers.  Each probe represents a
 * specific, documented invariant that the relevant service issue claims
 * to uphold.  When a probe assertion fails, the chaos scenario reports
 * the specific resilience claim that was violated.
 */

import axios, { type AxiosInstance } from "axios";
import { config } from "./config";
import { logger } from "./logger";

// ── HTTP clients ────────────────────────────────────────────────────────

function client(baseURL: string): AxiosInstance {
  return axios.create({
    baseURL,
    timeout: 10_000,
    validateStatus: () => true, // don't throw on 4xx/5xx — we inspect status ourselves
  });
}

const api = client(config.apiBaseUrl);
const indexerAdmin = client(config.indexerAdminUrl);
const keeperAdmin = client(config.keeperAdminUrl);

// ── Poll helpers ────────────────────────────────────────────────────────

/** Wait until `predicate` returns true or `timeoutMs` elapses. */
export async function waitUntil(
  predicate: () => Promise<boolean>,
  timeoutMs: number,
  intervalMs = config.healthPollIntervalMs,
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return true;
    await sleep(intervalMs);
  }
  return false;
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// ── Indexer probes ───────────────────────────────────────────────────────

/** GET /health on the indexer admin port. Returns true when healthy. */
export async function indexerIsHealthy(): Promise<boolean> {
  try {
    const res = await indexerAdmin.get("/health");
    return res.status === 200 && res.data?.status === "ok";
  } catch {
    return false;
  }
}

/**
 * Returns the last checkpoint ledger sequence number persisted by the indexer.
 *
 * Resilience claim: after an RPC outage the indexer resumes from this value
 * rather than from ledger 0, so no events are double-processed.
 */
export async function indexerLastCheckpoint(): Promise<number | null> {
  try {
    const res = await indexerAdmin.get("/checkpoint");
    if (res.status !== 200) return null;
    const seq = Number(res.data?.lastLedger ?? res.data?.checkpoint);
    return Number.isFinite(seq) ? seq : null;
  } catch {
    return null;
  }
}

/**
 * Returns the total number of events the indexer has processed since startup.
 *
 * Used to detect duplicate processing: record the count before an outage,
 * advance the ledger during the outage, then assert that the post-recovery
 * count equals pre-outage + new events (not pre-outage + new + re-processed).
 */
export async function indexerEventCount(): Promise<number | null> {
  try {
    const res = await indexerAdmin.get("/stats");
    const count = Number(res.data?.eventsProcessed ?? res.data?.totalEvents);
    return Number.isFinite(count) ? count : null;
  } catch {
    return null;
  }
}

/**
 * Advance the mock Stellar RPC ledger by `count` synthetic ledgers.
 *
 * Called during RPC-outage recovery to generate new events that the indexer
 * must process exactly once — confirming the no-duplicate claim is upheld
 * against real new data, not just a cold start with no ledgers.
 *
 * Returns the new ledger sequence, or null if the mock does not support this.
 */
export async function advanceMockLedger(count = 3): Promise<number | null> {
  try {
    const res = await indexerAdmin.post("/mock/advance", { count });
    if (res.status !== 200) return null;
    return Number(res.data?.ledger ?? res.data?.sequence) || null;
  } catch {
    return null;
  }
}

/**
 * Returns the current reconciliation status as reported by the reconciler service.
 *
 * Resilience claim (reconciliation alerting issue): the reconciler MUST
 * detect any ledger gap (missing events) within its scan window and emit
 * an alert within `alert_lag_max_ms`.
 */
export async function reconcilerStatus(): Promise<{
  lastScannedLedger: number;
  gapsDetected: number;
  alertsFired: number;
  alertQueueDepth: number;
} | null> {
  try {
    const res = await indexerAdmin.get("/reconciler/status");
    if (res.status !== 200) return null;
    return {
      lastScannedLedger: Number(res.data?.lastScannedLedger ?? 0),
      gapsDetected: Number(res.data?.gapsDetected ?? 0),
      alertsFired: Number(res.data?.alertsFired ?? 0),
      alertQueueDepth: Number(res.data?.alertQueueDepth ?? 0),
    };
  } catch {
    return null;
  }
}

/**
 * Returns all pending alerts from the alert queue (used in the reconciliation
 * alerting scenario to assert that alerts are fired and drained).
 *
 * Each alert entry has: { id, type, severity, ledger, createdAt }.
 */
export async function reconcilerPendingAlerts(): Promise<
  Array<{ id: string; type: string; severity: string; ledger: number; createdAt: number }>
> {
  try {
    const res = await indexerAdmin.get("/reconciler/alerts/pending");
    if (res.status !== 200) return [];
    return Array.isArray(res.data) ? res.data : [];
  } catch {
    return [];
  }
}

/**
 * Drain (acknowledge) all pending reconciliation alerts.
 * Used in cleanup to leave the alert queue empty after a scenario.
 * Returns the number of alerts drained.
 */
export async function drainReconcilerAlerts(): Promise<number> {
  try {
    const res = await indexerAdmin.delete("/reconciler/alerts/pending");
    return Number(res.data?.drained ?? 0);
  } catch {
    return 0;
  }
}

// ── Keeper probes ─────────────────────────────────────────────────────────

/** GET /health on the keeper admin port. Returns true when alive. */
export async function keeperIsHealthy(): Promise<boolean> {
  try {
    const res = await keeperAdmin.get("/health");
    return res.status === 200;
  } catch {
    return false;
  }
}

/**
 * Returns keeper job status counters: { pending, in_flight, done, dead }.
 *
 * Resilience claim: after a crash-and-restart no job that was already `done`
 * transitions back to `in_flight`, ensuring no double-submitted transactions.
 */
export async function keeperJobCounts(): Promise<{
  pending: number;
  in_flight: number;
  done: number;
  dead: number;
} | null> {
  try {
    const res = await keeperAdmin.get("/status");
    if (res.status !== 200) return null;
    return {
      pending: Number(res.data?.pending ?? 0),
      in_flight: Number(res.data?.in_flight ?? 0),
      done: Number(res.data?.done ?? 0),
      dead: Number(res.data?.dead ?? 0),
    };
  } catch {
    return null;
  }
}

/**
 * Returns the list of job dedup_keys that have been submitted at least once.
 *
 * The dedup_key uniqueness invariant in the keeper's SQLite store is the
 * primary mechanism that prevents double-submission across crashes.  This
 * probe surfaces those keys so tests can verify no key appears more than once
 * in the `done` set after a crash cycle.
 */
export async function keeperDoneJobKeys(): Promise<string[] | null> {
  try {
    const res = await keeperAdmin.get("/history");
    if (res.status !== 200) return null;
    const rows: Array<{ dedup_key: string; status: string }> = res.data ?? [];
    return rows
      .filter((r) => r.status === "done")
      .map((r) => r.dedup_key);
  } catch {
    return null;
  }
}

// ── API probes ────────────────────────────────────────────────────────────

/** GET /health on the API service. Returns true when healthy. */
export async function apiIsHealthy(): Promise<boolean> {
  try {
    const res = await api.get("/health");
    return res.status === 200;
  } catch {
    return false;
  }
}

/**
 * Probe an API read endpoint (e.g. GET /pools/:id) that reads from the DB.
 * Returns the HTTP status code.
 *
 * During a network partition the API cannot reach the DB; after the partition
 * is healed it must serve correct data again (no stale cache masking errors).
 */
export async function apiDbReadStatus(path: string): Promise<number> {
  return (await apiDbRead(path)).status;
}

/** Probe a DB-backed API read while retaining structured error details. */
export async function apiDbRead(path: string): Promise<{
  status: number;
  data: unknown;
  retryAfter?: string;
}> {
  try {
    const res = await api.get(path);
    return { status: res.status, data: res.data, retryAfter: res.headers["retry-after"] };
  } catch {
    return { status: 0, data: null };
  }
}

/** Returns the HTTP status of a write endpoint during a partition. */
export async function apiDbWriteStatus(
  path: string,
  body: Record<string, unknown>,
): Promise<number> {
  try {
    const res = await api.post(path, body);
    return res.status;
  } catch {
    return 0;
  }
}
