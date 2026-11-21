import {
  buildLedgerOrderingSamples,
  evaluateRound,
  severityOf,
  summarize,
} from "../src/policy";
import type { RoundResult, TxObservation } from "../src/types";
import { observation, testThresholds } from "./helpers";

function evaluate(observations: TxObservation[], thresholds = testThresholds()) {
  const samples = buildLedgerOrderingSamples(observations);
  return evaluateRound(0, observations, samples, thresholds);
}

function codes(deviations: ReturnType<typeof evaluate>): string[] {
  return deviations.map((deviation) => deviation.code).sort();
}

describe("buildLedgerOrderingSamples", () => {
  it("groups by ledger and sorts by application order", () => {
    const samples = buildLedgerOrderingSamples([
      observation({ id: "b", ledger: 12, applicationOrder: 2, fee: 300 }),
      observation({ id: "a", ledger: 12, applicationOrder: 1, fee: 100 }),
      observation({ id: "c", ledger: 13, applicationOrder: 1, fee: 100 }),
    ]);

    expect(samples.map((sample) => sample.ledger)).toEqual([12, 13]);
    expect(samples[0].observedSequence).toEqual(["a", "b"]);
    expect(samples[0].entries[0].applicationOrder).toBe(1);
  });

  it("ignores transactions that never settled", () => {
    const samples = buildLedgerOrderingSamples([
      observation({ id: "a", outcome: "PENDING", ledger: null, applicationOrder: null }),
    ]);
    expect(samples).toEqual([]);
  });
});

describe("fee priority policy", () => {
  it("accepts higher fees executing first", () => {
    const deviations = evaluate([
      observation({ id: "rich", sourceIndex: 1, fee: 600, applicationOrder: 1, submitOrder: 1 }),
      observation({ id: "cheap", sourceIndex: 0, fee: 100, applicationOrder: 2, submitOrder: 0 }),
    ]);
    expect(codes(deviations)).toEqual([]);
  });

  it("flags a lower-fee transaction executing before a higher-fee one as advisory by default", () => {
    const deviations = evaluate([
      observation({ id: "cheap", sourceIndex: 0, fee: 100, applicationOrder: 1, submitOrder: 0 }),
      observation({ id: "rich", sourceIndex: 1, fee: 600, applicationOrder: 2, submitOrder: 1 }),
    ]);
    expect(codes(deviations)).toEqual(["FEE_PRIORITY_VIOLATION"]);
    expect(deviations[0].severity).toBe("advisory");
    expect(deviations[0].txIds).toEqual(["cheap", "rich"]);
  });

  it("becomes hard once surge pricing is asserted", () => {
    const deviations = evaluate(
      [
        observation({ id: "cheap", sourceIndex: 0, fee: 100, applicationOrder: 1, submitOrder: 0 }),
        observation({ id: "rich", sourceIndex: 1, fee: 600, applicationOrder: 2, submitOrder: 1 }),
      ],
      testThresholds({ expectFeePriority: true }),
    );
    expect(deviations[0].severity).toBe("hard");
  });
});

describe("arrival order policy", () => {
  const outOfOrder = [
    observation({ id: "second", sourceIndex: 1, fee: 100, submitOrder: 1, applicationOrder: 1 }),
    observation({ id: "first", sourceIndex: 0, fee: 100, submitOrder: 0, applicationOrder: 2 }),
  ];

  it("is advisory by default", () => {
    const deviations = evaluate(outOfOrder);
    expect(codes(deviations)).toEqual(["ARRIVAL_ORDER_VIOLATION"]);
    expect(deviations[0].severity).toBe("advisory");
  });

  it("becomes hard when MEV_STRICT_ARRIVAL_ORDER is enabled", () => {
    const deviations = evaluate(outOfOrder, testThresholds({ strictArrivalOrder: true }));
    expect(deviations[0].severity).toBe("hard");
  });

  it("accepts equal-fee transactions in submission order", () => {
    const deviations = evaluate([
      observation({ id: "first", sourceIndex: 0, fee: 100, submitOrder: 0, applicationOrder: 1 }),
      observation({ id: "second", sourceIndex: 1, fee: 100, submitOrder: 1, applicationOrder: 2 }),
    ]);
    expect(codes(deviations)).toEqual([]);
  });
});

describe("per-account serialization policy", () => {
  it("flags two transactions from one account in the same ledger as hard", () => {
    const deviations = evaluate([
      observation({ id: "one", sourceIndex: 0, fee: 600, applicationOrder: 1 }),
      observation({ id: "two", sourceIndex: 0, fee: 100, applicationOrder: 2 }),
    ]);
    expect(codes(deviations)).toEqual(["PER_ACCOUNT_SERIALIZATION_VIOLATION"]);
    expect(deviations[0].severity).toBe("hard");
  });

  it("accepts the same account appearing across separate ledgers", () => {
    const deviations = evaluate([
      observation({ id: "one", sourceIndex: 0, ledger: 100, applicationOrder: 1 }),
      observation({ id: "two", sourceIndex: 0, ledger: 101, applicationOrder: 1 }),
    ]);
    expect(codes(deviations)).toEqual([]);
  });
});

describe("edge cases", () => {
  it("records partial submission failures as advisory", () => {
    const deviations = evaluate([
      observation({ id: "ok", sourceIndex: 0 }),
      observation({ id: "dead", sourceIndex: 1, outcome: "SUBMIT_ERROR", hash: null, ledger: null, applicationOrder: null, error: "rejected" }),
    ]);
    expect(codes(deviations)).toEqual(["PARTIAL_SUBMISSION_FAILURE"]);
    expect(deviations[0].severity).toBe("advisory");
  });

  it("records unresolved transactions (RPC lag) as advisory missing data", () => {
    const deviations = evaluate([
      observation({ id: "lagged", sourceIndex: 0, outcome: "PENDING", ledger: null, applicationOrder: null }),
    ]);
    expect(codes(deviations)).toEqual(["MISSING_ORDERING_DATA"]);
    expect(deviations[0].detail).toMatch(/RPC lag/);
  });
});

describe("severityOf", () => {
  it("maps deviation codes to the configured severity", () => {
    expect(severityOf("FEE_PRIORITY_VIOLATION", testThresholds())).toBe("advisory");
    expect(severityOf("FEE_PRIORITY_VIOLATION", testThresholds({ expectFeePriority: true }))).toBe("hard");
    expect(severityOf("PER_ACCOUNT_SERIALIZATION_VIOLATION", testThresholds())).toBe("hard");
    expect(severityOf("MISSING_ORDERING_DATA", testThresholds())).toBe("advisory");
    expect(severityOf("PARTIAL_SUBMISSION_FAILURE", testThresholds())).toBe("advisory");
    expect(severityOf("ARRIVAL_ORDER_VIOLATION", testThresholds())).toBe("advisory");
    expect(severityOf("ARRIVAL_ORDER_VIOLATION", testThresholds({ strictArrivalOrder: true }))).toBe("hard");
  });
});

describe("summarize", () => {
  function round(index: number, observations: TxObservation[], deviations: RoundResult["deviations"]): RoundResult {
    return {
      index,
      kind: "fee_ladder",
      observations,
      samples: buildLedgerOrderingSamples(observations),
      deviations,
    };
  }

  it("passes when only advisory deviations are present", () => {
    const observations = [observation({ id: "a" }), observation({ id: "b", sourceIndex: 1 })];
    const deviations = evaluate(observations);
    const summary = summarize([round(0, observations, deviations)], 2, testThresholds());
    expect(summary.passed).toBe(true);
    expect(summary.txsObserved).toBe(2);
    expect(summary.orderingSamples).toBe(1);
  });

  it("fails on a hard deviation and counts it", () => {
    const observations = [
      observation({ id: "cheap", sourceIndex: 0, fee: 100, applicationOrder: 1 }),
      observation({ id: "rich", sourceIndex: 1, fee: 600, applicationOrder: 2 }),
    ];
    const thresholds = testThresholds({ expectFeePriority: true });
    const deviations = evaluate(observations, thresholds);
    const summary = summarize([round(0, observations, deviations)], 2, thresholds);
    expect(summary.passed).toBe(false);
    expect(summary.hardDeviations).toBe(1);
  });

  it("honours the maxHardDeviations threshold", () => {
    const observations = [
      observation({ id: "cheap", sourceIndex: 0, fee: 100, applicationOrder: 1 }),
      observation({ id: "rich", sourceIndex: 1, fee: 600, applicationOrder: 2 }),
    ];
    const thresholds = testThresholds({ expectFeePriority: true, maxHardDeviations: 1 });
    const deviations = evaluate(observations, thresholds);
    const summary = summarize([round(0, observations, deviations)], 2, thresholds);
    expect(summary.passed).toBe(true);
  });
});
