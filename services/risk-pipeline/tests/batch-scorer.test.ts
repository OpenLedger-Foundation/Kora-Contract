/**
 * Batch scoring idempotency tests — issue #755
 *
 * Covers:
 *  - First submission scores and persists the job
 *  - Double submit returns identical result, fromCache=true, no re-scoring
 *  - duplicatesBlocked counter increments on each duplicate
 *  - Different keys for different invoices do not collide
 *  - Key collision across invoices: same key → first invoice's result returned
 *  - getJob retrieves the stored job correctly
 *  - totalJobs does not grow on duplicate submissions
 *  - Formula version is forwarded to the pipeline
 *  - Empty input (sparse data) is handled without throwing
 */

import { BatchScorer, InMemoryBatchScoringStore } from "../src/batch-scorer";
import { RiskPipeline } from "../src/scorer";
import { ScoringInput } from "../src/types";

// ── Helpers ───────────────────────────────────────────────────────────────────

function makeScorer(): { scorer: BatchScorer; store: InMemoryBatchScoringStore } {
  const store = new InMemoryBatchScoringStore();
  const scorer = new BatchScorer(new RiskPipeline(), store);
  return { scorer, store };
}

const FULL_INPUT: ScoringInput = {
  sme: {
    repayments: [{ paidOnTime: true }, { paidOnTime: true }, { paidOnTime: false }],
    totalInvoices: 15,
    invoicesLast90d: 4,
    defaults: 0,
  },
  debtors: [{ debtorHash: "ab".repeat(32), exposureShareBps: 10_000, avgDebtorScore: 40 }],
};

// ── First submission ──────────────────────────────────────────────────────────

describe("BatchScorer — first submission", () => {
  it("scores the input and returns fromCache=false", async () => {
    const { scorer } = makeScorer();
    const { score, fromCache } = await scorer.submit("inv:001:v1:1000", FULL_INPUT);

    expect(fromCache).toBe(false);
    expect(score.score).toBeGreaterThanOrEqual(0);
    expect(score.score).toBeLessThanOrEqual(100);
    expect(score.formulaVersion).toBeDefined();
  });

  it("persists the job so getJob returns it", async () => {
    const { scorer } = makeScorer();
    const key = "inv:002:v1:1000";
    const { score } = await scorer.submit(key, FULL_INPUT);

    const job = await scorer.getJob(key);
    expect(job).toBeDefined();
    expect(job!.idempotencyKey).toBe(key);
    expect(job!.result.score).toBe(score.score);
    expect(new Date(job!.scoredAt).toISOString()).toBe(job!.scoredAt);
  });

  it("stores the input alongside the result", async () => {
    const { scorer } = makeScorer();
    const key = "inv:003:v1:1000";
    await scorer.submit(key, FULL_INPUT);

    const job = await scorer.getJob(key);
    expect(job!.input).toEqual(FULL_INPUT);
  });

  it("duplicatesBlocked is 0 after a fresh submission", async () => {
    const { scorer } = makeScorer();
    await scorer.submit("inv:004:v1:1000", FULL_INPUT);

    const m = await scorer.metrics();
    expect(m.duplicatesBlocked).toBe(0);
    expect(m.totalJobs).toBe(1);
  });
});

// ── Double submit ─────────────────────────────────────────────────────────────

describe("BatchScorer — double submit", () => {
  it("second call returns fromCache=true with an identical result", async () => {
    const { scorer } = makeScorer();
    const key = "inv:005:v1:1000";

    const first  = await scorer.submit(key, FULL_INPUT);
    const second = await scorer.submit(key, FULL_INPUT);

    expect(second.fromCache).toBe(true);
    expect(second.score.score).toBe(first.score.score);
    expect(second.score.formulaVersion).toBe(first.score.formulaVersion);
  });

  it("double submit does not grow totalJobs", async () => {
    const { scorer } = makeScorer();
    const key = "inv:006:v1:1000";

    await scorer.submit(key, FULL_INPUT);
    await scorer.submit(key, FULL_INPUT);

    const m = await scorer.metrics();
    expect(m.totalJobs).toBe(1);
  });

  it("each duplicate increments duplicatesBlocked by 1", async () => {
    const { scorer } = makeScorer();
    const key = "inv:007:v1:1000";

    await scorer.submit(key, FULL_INPUT);
    await scorer.submit(key, FULL_INPUT); // +1
    await scorer.submit(key, FULL_INPUT); // +2
    await scorer.submit(key, FULL_INPUT); // +3

    const m = await scorer.metrics();
    expect(m.duplicatesBlocked).toBe(3);
  });

  it("duplicate returns exactly the original stored result (not a re-score)", async () => {
    // Inject a pipeline that counts invocations so we can verify it's called once.
    let callCount = 0;
    const countingPipeline = new RiskPipeline();
    const origScore = countingPipeline.score.bind(countingPipeline);
    countingPipeline.score = (...args) => { callCount++; return origScore(...args); };

    const store = new InMemoryBatchScoringStore();
    const scorer = new BatchScorer(countingPipeline, store);
    const key = "inv:008:v1:1000";

    await scorer.submit(key, FULL_INPUT);
    await scorer.submit(key, FULL_INPUT);
    await scorer.submit(key, FULL_INPUT);

    expect(callCount).toBe(1); // pipeline invoked exactly once
  });
});

// ── Different keys — no cross-collision ──────────────────────────────────────

describe("BatchScorer — different keys", () => {
  it("two different invoices with different keys are independent", async () => {
    const { scorer } = makeScorer();

    const r1 = await scorer.submit("inv:A:v1:1000", FULL_INPUT);
    const r2 = await scorer.submit("inv:B:v1:1000", { ...FULL_INPUT, macro: { stressIndex: 90 } });

    // Both are fresh scores.
    expect(r1.fromCache).toBe(false);
    expect(r2.fromCache).toBe(false);

    const m = await scorer.metrics();
    expect(m.totalJobs).toBe(2);
    expect(m.duplicatesBlocked).toBe(0);
  });

  it("re-submitting one key does not affect the other", async () => {
    const { scorer } = makeScorer();

    await scorer.submit("inv:C:v1:1000", FULL_INPUT);
    await scorer.submit("inv:D:v1:1000", FULL_INPUT);
    const dup = await scorer.submit("inv:C:v1:1000", FULL_INPUT); // duplicate of C only

    expect(dup.fromCache).toBe(true);
    const m = await scorer.metrics();
    expect(m.totalJobs).toBe(2);        // C and D
    expect(m.duplicatesBlocked).toBe(1); // only C's duplicate
  });
});

// ── Key-collision hazard across invoices ──────────────────────────────────────

describe("BatchScorer — key collision across invoices", () => {
  it("same key for two different invoices returns the first invoice's result", async () => {
    const { scorer } = makeScorer();

    // Two distinct inputs intentionally sharing the same (bad) key.
    const sharedKey = "COLLISION_KEY";
    const inputA: ScoringInput = {
      sme: { repayments: [{ paidOnTime: true }], totalInvoices: 5, invoicesLast90d: 1, defaults: 0 },
      debtors: [{ debtorHash: "aaa", exposureShareBps: 10_000 }],
    };
    const inputB: ScoringInput = {
      sme: { repayments: [{ paidOnTime: false }, { paidOnTime: false }], totalInvoices: 5, invoicesLast90d: 2, defaults: 2 },
      debtors: [{ debtorHash: "bbb", exposureShareBps: 10_000 }],
    };

    const { score: scoreA } = await scorer.submit(sharedKey, inputA);
    const { score: scoreB, fromCache } = await scorer.submit(sharedKey, inputB);

    // The collision returns A's result, not B's.
    expect(fromCache).toBe(true);
    expect(scoreB.score).toBe(scoreA.score);
    // The stored job still carries inputA.
    const job = await scorer.getJob(sharedKey);
    expect(job!.input).toEqual(inputA);
  });
});

// ── Formula version forwarding ────────────────────────────────────────────────

describe("BatchScorer — formula version", () => {
  it("forwards the formula version to the pipeline", async () => {
    const { scorer } = makeScorer();
    const { score } = await scorer.submit("inv:E:v2:1000", FULL_INPUT, { formulaVersion: "v2" });
    expect(score.formulaVersion).toBe("v2");
  });

  it("different formula versions produce separate cached entries when keys differ", async () => {
    const { scorer } = makeScorer();

    const r1 = await scorer.submit("inv:F:v1:1000", FULL_INPUT, { formulaVersion: "v1" });
    const r2 = await scorer.submit("inv:F:v2:1000", FULL_INPUT, { formulaVersion: "v2" });

    expect(r1.fromCache).toBe(false);
    expect(r2.fromCache).toBe(false);
    expect(r1.score.formulaVersion).toBe("v1");
    expect(r2.score.formulaVersion).toBe("v2");
  });
});

// ── Sparse / empty input ──────────────────────────────────────────────────────

describe("BatchScorer — sparse input", () => {
  it("handles empty input without throwing", async () => {
    const { scorer } = makeScorer();
    const { score, fromCache } = await scorer.submit("inv:G:v1:1000", {});

    expect(fromCache).toBe(false);
    expect(score.warnings).toContain("SPARSE_DATA");
    expect(score.score).toBe(50); // neutral fallback
  });

  it("empty-input duplicate is also idempotent", async () => {
    const { scorer } = makeScorer();
    const key = "inv:H:v1:1000";

    const first  = await scorer.submit(key, {});
    const second = await scorer.submit(key, {});

    expect(second.fromCache).toBe(true);
    expect(second.score.score).toBe(first.score.score);
  });
});

// ── Metrics counter integrity ─────────────────────────────────────────────────

describe("BatchScorer — metrics", () => {
  it("totalJobs counts unique keys only", async () => {
    const { scorer } = makeScorer();

    for (const key of ["k1", "k2", "k3"]) {
      await scorer.submit(key, FULL_INPUT);
    }
    await scorer.submit("k1", FULL_INPUT); // duplicate
    await scorer.submit("k2", FULL_INPUT); // duplicate

    const m = await scorer.metrics();
    expect(m.totalJobs).toBe(3);
    expect(m.duplicatesBlocked).toBe(2);
  });

  it("metrics are isolated per store instance", async () => {
    const { scorer: s1 } = makeScorer();
    const { scorer: s2 } = makeScorer();

    await s1.submit("key", FULL_INPUT);
    await s1.submit("key", FULL_INPUT);

    expect((await s1.metrics()).duplicatesBlocked).toBe(1);
    expect((await s2.metrics()).duplicatesBlocked).toBe(0);
  });
});
