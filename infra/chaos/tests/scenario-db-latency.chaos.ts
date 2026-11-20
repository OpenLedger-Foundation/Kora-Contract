/**
 * Chaos Scenario 2 — Database Latency Spike
 *
 * Resilience claim being tested (from the API/indexer reconciliation issue):
 *   "The API and indexer MUST remain available and return correct data even
 *    when database round-trip latency exceeds 2 seconds; read requests MUST
 *    not return stale data silently — they MUST either return current data
 *    or a 503/timeout error, never a cached-but-wrong 200."
 *
 * What this scenario does:
 *   1. Establishes a baseline: a known API read endpoint returns 200 with
 *      a consistent response.
 *   2. Injects artificial TCP latency on the DB container's network interface
 *      via tc-netem.
 *   3. Asserts that API read requests either:
 *      a. Still succeed (200) within a reasonable client timeout, OR
 *      b. Return 503/504 with a clear error (not a silent stale result).
 *   4. Removes the latency injection and asserts 200 returns within the
 *      recovery timeout.
 *
 * Gap surfaced: if the API has an overly-aggressive DB connection pool
 * without timeout configuration, it may hang indefinitely under high
 * latency rather than returning a 503 — this surfaces as a test timeout,
 * which is filed as a follow-up.
 */

import {
  apiIsHealthy,
  apiDbRead,
  apiDbReadStatus,
  waitUntil,
  sleep,
} from "../src/probes";
import { injectDbLatency, type Cleanup } from "../src/injectors";
import { config } from "../src/config";
import { logger } from "../src/logger";

// A stable read-only endpoint that exercises the DB path.
// Adjust to match the staging API's actual route structure.
const DB_READ_ENDPOINT = "/api/v1/pools";

describe("Chaos: Database Latency Spike", () => {
  let cleanup: Cleanup | null = null;

  afterEach(async () => {
    if (cleanup) {
      await cleanup();
      cleanup = null;
    }
  });

  it(
    "API returns correct status under DB latency and recovers cleanly",
    async () => {
      // ── 1. Baseline ──────────────────────────────────────────────────────
      const baselineStatus = await apiDbReadStatus(DB_READ_ENDPOINT);
      logger.info({ status: baselineStatus }, "chaos[db-latency]: baseline DB read status");

      // The API must be healthy before we start.
      expect(baselineStatus).toBe(200);

      // ── 2. Inject latency ─────────────────────────────────────────────────
      cleanup = await injectDbLatency(config.dbLatencyMs);
      logger.info(
        { latencyMs: config.dbLatencyMs, durationMs: config.failureDurationMs },
        "chaos[db-latency]: DB latency injected",
      );

      // Give the latency time to affect in-flight queries.
      await sleep(2_000);

      // ── 3. Assert behaviour under latency ─────────────────────────────────
      // Poll the read endpoint several times during the latency window.
      const responsesUnderLatency: Array<{ status: number; data: unknown; retryAfter?: string }> = [];
      const pollCount = 5;
      const pollGapMs = Math.floor(config.failureDurationMs / (pollCount + 1));

      for (let i = 0; i < pollCount; i++) {
        const response = await apiDbRead(DB_READ_ENDPOINT);
        responsesUnderLatency.push(response);
        logger.info(
          { attempt: i + 1, status: response.status },
          "chaos[db-latency]: probe under latency",
        );
        await sleep(pollGapMs);
      }

      // Every response must be either a successful 200 (if the query completed
      // within the API's timeout) or a structured service error. A client-side
      // timeout (0) is a failure: the DB pool should cancel the statement and
      // map SQLSTATE 57014 to HTTP 503 with code DB_TIMEOUT.
      for (const response of responsesUnderLatency) {
        if (response.status === 0) {
          logger.error({ statementTimeoutMs: config.dbStatementTimeoutMs }, "DB_TIMEOUT gap: API client timed out before the database statement was cancelled");
        }
        expect(response.status === 200 || response.status >= 400).toBe(true);
        if (response.status === 503) {
          expect((response.data as { code?: string })?.code).toBe("DB_TIMEOUT");
          expect(response.retryAfter).toBeDefined();
        }
      }

      // ── 4. Remove latency and verify recovery ─────────────────────────────
      await cleanup();
      cleanup = null;

      const recovered = await waitUntil(
        async () => (await apiDbReadStatus(DB_READ_ENDPOINT)) === 200,
        config.recoveryTimeoutMs,
      );

      logger.info({ recovered }, "chaos[db-latency]: post-cleanup recovery status");
      expect(recovered).toBe(true);
    },
    config.failureDurationMs + config.recoveryTimeoutMs + 30_000,
  );
});
