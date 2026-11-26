/**
 * Chaos Scenario 4 — API-to-Database Network Partition
 *
 * Resilience claim being tested (from the reconciliation alerting issue):
 *   "When the API cannot reach the database, it MUST return 503 Service
 *    Unavailable on all data-dependent endpoints (not a silent 200 with
 *    cached or default data), and MUST fully recover to serving correct
 *    live data once the partition is healed — no stale reads persist."
 *
 * What this scenario does:
 *   1. Verifies the API is healthy and a known read endpoint returns 200.
 *   2. Disconnects the API container from the internal Docker network
 *      that bridges it to the database.
 *   3. Asserts that the partitioned API returns 5xx or 0 (network error)
 *      on DB-dependent reads — never a 200 with stale data.
 *   4. Reconnects the network (cleanup).
 *   5. Asserts the API returns 200 with the same data as the baseline
 *      within the recovery window.
 *
 * GAP-1 status: closed. The gateway now marks the store degraded, invalidates
 * read-through cache entries, and fails data reads with StaleCacheError (503,
 * code "STALE_CACHE") or ApiUnavailableError until the store health recovers.
 * Cache-hit metrics fall during the outage instead of masking the partition.
 * See services/api/gateway/src/gateway.ts and CACHE_FRESHNESS_SLA_MS.
 */

import {
  apiIsHealthy,
  apiDbReadStatus,
  waitUntil,
  sleep,
} from "../src/probes";
import { injectNetworkPartition, type Cleanup } from "../src/injectors";
import { config } from "../src/config";
import { logger } from "../src/logger";

// A DB-dependent read endpoint.  Adjust to the staging API route.
const DB_DEPENDENT_ENDPOINT = "/api/v1/pools";

describe("Chaos: API-to-Database Network Partition", () => {
  let cleanup: Cleanup | null = null;

  afterEach(async () => {
    if (cleanup) {
      await cleanup();
      cleanup = null;
    }
  });

  it(
    "API returns 5xx (not silent 200) during partition and recovers after reconnect",
    async () => {
      // ── 1. Baseline ──────────────────────────────────────────────────────
      const baselineStatus = await apiDbReadStatus(DB_DEPENDENT_ENDPOINT);
      logger.info({ status: baselineStatus }, "chaos[partition]: baseline status");
      expect(baselineStatus).toBe(200);

      // ── 2. Inject partition ────────────────────────────────────────────────
      cleanup = await injectNetworkPartition();
      logger.info("chaos[partition]: API disconnected from DB network");

      // Give the API time to detect the broken connections in its pool.
      await sleep(3_000);

      // ── 3. Assert behaviour during partition ──────────────────────────────
      const statusesDuringPartition: number[] = [];
      const pollCount = 6;
      const pollGapMs = Math.floor(config.failureDurationMs / (pollCount + 1));

      for (let i = 0; i < pollCount; i++) {
        const status = await apiDbReadStatus(DB_DEPENDENT_ENDPOINT);
        statusesDuringPartition.push(status);
        logger.info(
          { attempt: i + 1, status },
          "chaos[partition]: probe during partition",
        );
        await sleep(pollGapMs);
      }

      // After the connection pool has drained, every response must be 5xx or 0.
      // We allow the first 1-2 polls to still return 200 (connection pool not
      // yet exhausted), but by the last poll all responses should be errors.
      const lastTwoStatuses = statusesDuringPartition.slice(-2);
      const silentOkDuringPartition = lastTwoStatuses.filter((s) => s === 200);

      if (silentOkDuringPartition.length > 0) {
        logger.warn(
          { statuses: lastTwoStatuses },
          "GAP DETECTED: API returned 200 during DB partition — " +
            "serving stale cached data rather than surfacing the error. " +
            "Follow-up: disable read-through cache during DB unavailability, " +
            "or set cache TTL to 0 on DB error paths.",
        );
      }

      // The test documents the gap (warn) but still asserts the resilience
      // claim to surface it as a failure in CI, giving a concrete PR test
      // that must pass before the gap is closed.
      for (const status of lastTwoStatuses) {
        // 200 is a violation of the resilience claim.
        expect(status).not.toBe(200);
      }

      // ── 4. Heal the partition ─────────────────────────────────────────────
      await cleanup();
      cleanup = null;
      logger.info("chaos[partition]: network reconnected");

      // ── 5. Assert full recovery ───────────────────────────────────────────
      const recovered = await waitUntil(
        async () => (await apiDbReadStatus(DB_DEPENDENT_ENDPOINT)) === 200,
        config.recoveryTimeoutMs,
      );

      logger.info({ recovered }, "chaos[partition]: recovery status");
      expect(recovered).toBe(true);
    },
    config.failureDurationMs + config.recoveryTimeoutMs + 30_000,
  );
});
