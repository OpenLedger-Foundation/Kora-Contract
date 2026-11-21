import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  buildRoundPlans,
  requiredAccountCount,
  runHarness,
  writeReportArtifact,
  type HarnessOptions,
} from "../src/harness";
import { MockChain } from "../src/mockChain";
import type { HarnessReport } from "../src/types";
import { testThresholds } from "./helpers";

function options(overrides: Partial<HarnessOptions> = {}): HarnessOptions {
  return {
    rounds: 3,
    accountsPerRound: 3,
    baseFee: 100,
    feeStep: 500,
    thresholds: testThresholds(),
    outputDir: null,
    ...overrides,
  };
}

function tempDir(): string {
  return mkdtempSync(join(tmpdir(), "mev-ordering-"));
}

function deviationCodes(report: HarnessReport): string[] {
  return report.deviations.map((deviation) => deviation.code);
}

describe("buildRoundPlans", () => {
  it("cycles competition kinds and reports the account requirement", () => {
    const plans = buildRoundPlans(options());
    expect(plans.map((plan) => plan.kind)).toEqual([
      "fee_ladder",
      "equal_fee",
      "same_account_burst",
    ]);
    expect(plans.map((plan) => plan.txs.length)).toEqual([3, 3, 2]);
    expect(requiredAccountCount(plans)).toBe(3);
  });

  it("builds a strictly increasing fee ladder with distinct accounts", () => {
    const [plan] = buildRoundPlans(options({ rounds: 1, accountsPerRound: 4 }));
    expect(plan.txs.map((tx) => tx.fee)).toEqual([100, 600, 1100, 1600]);
    expect(new Set(plan.txs.map((tx) => tx.sourceIndex)).size).toBe(4);
  });

  it("reuses one source account for the same-account burst", () => {
    const plans = buildRoundPlans(options({ rounds: 3 }));
    const burst = plans[2];
    expect(burst.txs).toHaveLength(2);
    expect(burst.txs[0].sourceIndex).toBe(burst.txs[1].sourceIndex);
  });
});

describe("runHarness against the mock chain", () => {
  it("records ordering samples and passes when policy holds", async () => {
    const report = await runHarness(new MockChain(), options());

    expect(report.schemaVersion).toBe(1);
    expect(report.mode).toBe("mock");
    expect(report.summary.passed).toBe(true);
    expect(report.summary.txsPlanned).toBe(8);
    expect(report.summary.txsSubmitted).toBe(8);
    expect(report.summary.txsObserved).toBe(8);
    expect(report.summary.submissionFailures).toBe(0);
    expect(report.deviations).toEqual([]);
    expect(report.orderingSamples.length).toBeGreaterThan(0);
  });

  it("includes an ordering sample with the fee ladder executed by fee priority", async () => {
    const report = await runHarness(new MockChain(), options());
    const ladder = report.rounds[0];
    expect(ladder.kind).toBe("fee_ladder");
    expect(ladder.samples).toHaveLength(1);
    // highest fee (last submitted) executes first
    expect(ladder.samples[0].observedSequence).toEqual([
      "fee_ladder-2",
      "fee_ladder-1",
      "fee_ladder-0",
    ]);
  });

  it("serializes the same-account burst across two ledgers", async () => {
    const report = await runHarness(new MockChain(), options());
    const burst = report.rounds[2];
    expect(burst.kind).toBe("same_account_burst");
    expect(burst.samples).toHaveLength(2);
    expect(burst.samples.every((sample) => sample.entries.length === 1)).toBe(true);
  });

  it("flags a fee-priority regression as a hard failure once surge pricing is asserted", async () => {
    const report = await runHarness(
      new MockChain({ disableFeePriority: true }),
      options({ thresholds: testThresholds({ expectFeePriority: true }) }),
    );

    expect(deviationCodes(report)).toContain("FEE_PRIORITY_VIOLATION");
    expect(report.summary.hardDeviations).toBeGreaterThan(0);
    expect(report.summary.passed).toBe(false);
  });

  it("reports a fee-priority regression as advisory while surge pricing is unasserted", async () => {
    const report = await runHarness(new MockChain({ disableFeePriority: true }), options());

    expect(deviationCodes(report)).toContain("FEE_PRIORITY_VIOLATION");
    expect(report.summary.hardDeviations).toBe(0);
    expect(report.summary.passed).toBe(true);
  });

  it("survives a total submission failure and reports it as advisory", async () => {
    const report = await runHarness(new MockChain({ submitFailureRate: 1 }), options());

    expect(report.summary.submissionFailures).toBe(8);
    expect(report.summary.txsObserved).toBe(0);
    expect(report.summary.advisoryDeviations).toBeGreaterThan(0);
    expect(report.summary.passed).toBe(true);
    expect(deviationCodes(report)).toContain("PARTIAL_SUBMISSION_FAILURE");
  });

  it("reports RPC lag as advisory missing ordering data without failing the run", async () => {
    const report = await runHarness(
      new MockChain({ rpcLagLedgers: 1 }),
      options({ thresholds: testThresholds({ observeTimeoutMs: 0 }) }),
    );

    expect(deviationCodes(report)).toContain("MISSING_ORDERING_DATA");
    expect(report.summary.hardDeviations).toBe(0);
    expect(report.summary.passed).toBe(true);
  });
});

describe("artifact output", () => {
  it("writes a timestamped and a latest JSON artifact containing the ordering sample", async () => {
    const outputDir = tempDir();
    const report = await runHarness(new MockChain(), options({ outputDir }));

    const latestPath = join(outputDir, "mev-ordering-latest.json");
    expect(existsSync(latestPath)).toBe(true);

    const parsed = JSON.parse(readFileSync(latestPath, "utf8")) as HarnessReport;
    expect(parsed.summary).toEqual(report.summary);
    expect(parsed.orderingSamples.length).toBe(report.orderingSamples.length);
    expect(parsed.orderingSamples[0].observedSequence.length).toBeGreaterThan(0);
  });

  it("writeReportArtifact emits unique timestamped files", () => {
    const outputDir = tempDir();
    const base: HarnessReport = {
      schemaVersion: 1,
      generatedAt: "2026-11-22T10:00:00.000Z",
      mode: "mock",
      network: { rpcUrl: "mock://in-process", networkPassphrase: "Mock" },
      thresholds: testThresholds(),
      rounds: [],
      deviations: [],
      orderingSamples: [],
      summary: {
        rounds: 0,
        txsPlanned: 0,
        txsSubmitted: 0,
        txsObserved: 0,
        submissionFailures: 0,
        hardDeviations: 0,
        advisoryDeviations: 0,
        orderingSamples: 0,
        passed: true,
      },
    };

    const paths = writeReportArtifact(base, outputDir);
    expect(existsSync(paths.timestampedPath)).toBe(true);
    expect(existsSync(paths.latestPath)).toBe(true);
    expect(paths.timestampedPath).toContain("mev-ordering-2026-11-22T10-00-00-000Z.json");
  });
});
