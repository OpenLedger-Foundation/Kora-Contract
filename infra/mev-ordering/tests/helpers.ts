/**
 * Test helpers for the ordering-observation harness.
 *
 * Not a test file itself (`testMatch` only picks up `*.test.ts`).
 */

import type { HarnessThresholds, TxObservation } from "../src/types";

/** Thresholds that keep unit tests fast and deterministic. */
export function testThresholds(overrides: Partial<HarnessThresholds> = {}): HarnessThresholds {
  return {
    expectFeePriority: false,
    strictArrivalOrder: false,
    maxHardDeviations: 0,
    observeTimeoutMs: 5000,
    pollIntervalMs: 0,
    submitStaggerMs: 0,
    ...overrides,
  };
}

/** Build a settled observation with sensible defaults. */
export function observation(partial: Partial<TxObservation> & { id: string }): TxObservation {
  return {
    sourceIndex: 0,
    fee: 100,
    submitOrder: 0,
    hash: `hash-${partial.id}`,
    outcome: "SUCCESS",
    ledger: 100,
    applicationOrder: 1,
    submittedAtMs: 0,
    observedAtMs: 0,
    error: null,
    ...partial,
  };
}
