/**
 * FX Ingestion Service — Test Suite
 *
 * Covers:
 *  - Source divergence rejection (sources disagree beyond threshold)
 *  - Missing-data skip (all sources unavailable — nothing published)
 *  - Single-source skip (cannot cross-check with one source)
 *  - Consensus rate computation (mean of passing sources)
 *  - Staleness-guard interaction (oracle relay refuses stale rates)
 *  - Rate-limit enforcement (no double-publish within same cycle)
 *  - Full engine cycle: pairs processed, failures isolated
 *
 * Issue #766 — Minimum 90% coverage.
 */

import { CrossChecker } from "../src/crosscheck";
import { OracleRelay, PublishResult } from "../src/relay";
import { IngestionEngine } from "../src/engine";
import { FxSource } from "../src/sources";
import { FxMetrics } from "../src/metrics";
import { CurrencyCode, FxQuote, FxIngestionConfig } from "../src/types";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeQuote(
  source: string,
  base: CurrencyCode,
  quote: CurrencyCode,
  rate: number,
  fetchedAt?: number
): FxQuote {
  return {
    source,
    base,
    quote,
    rateScaled: BigInt(Math.round(rate * 10_000_000)),
    fetchedAt: fetchedAt ?? Math.floor(Date.now() / 1000),
  };
}

/** A test FxSource that returns a pre-configured response. */
class StubSource implements FxSource {
  constructor(
    readonly name: string,
    private readonly response: FxQuote | null
  ) {}

  async fetchRate(_base: CurrencyCode, _quote: CurrencyCode): Promise<FxQuote | null> {
    return this.response;
  }
}

/** A test OracleRelay subclass that captures publish calls. */
class MockRelay extends OracleRelay {
  published: Array<{
    base: CurrencyCode;
    quote: CurrencyCode;
    rateScaled: bigint;
    fetchedAt: number;
  }> = [];

  protected override async _submitToOracle(
    base: CurrencyCode,
    quote: CurrencyCode,
    rateScaled: bigint,
    fetchedAt: number
  ): Promise<string> {
    this.published.push({ base, quote, rateScaled, fetchedAt });
    return "0xmocktxhash";
  }
}

/** A relay whose publish always skips without a reason (defensive branch). */
class NoReasonSkipRelay extends OracleRelay {
  override async publish(
    base: CurrencyCode,
    quote: CurrencyCode,
  ): Promise<PublishResult> {
    return { base, quote, skipped: true };
  }
}

/** A relay whose submission throws a non-Error value (defensive branch). */
class StringThrowRelay extends OracleRelay {
  protected override async _submitToOracle(): Promise<string> {
    throw "string error";
  }
}

const BASE_CONFIG: FxIngestionConfig = {
  currencies: ["NGN", "KES"],
  quoteCurrency: "USDC",
  maxDivergenceBps: 200,
  pollIntervalMs: 300_000,
  oracleContractAddress: "CORACLE",
  relaySecret: "SRELAY",
  rpcUrl: "https://test",
  networkPassphrase: "Test SDF Network ; September 2015",
  maxStalenessSeconds: 3600,
};

// ---------------------------------------------------------------------------
// CrossChecker tests
// ---------------------------------------------------------------------------

describe("CrossChecker", () => {
  test("throws if fewer than 2 sources are provided", () => {
    const src = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    expect(() => new CrossChecker([src], 200)).toThrow("at least 2");
  });

  test("passes when sources agree within threshold", async () => {
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.000652));
    const checker = new CrossChecker([src1, src2], 200);
    const result = await checker.check("NGN", "USDC");
    expect(result.passed).toBe(true);
    expect(result.consensusRate).toBeDefined();
  });

  test("rejects when sources diverge beyond threshold", async () => {
    // 0.00065 vs 0.0009 — large divergence
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.0009));
    const checker = new CrossChecker([src1, src2], 200);
    const result = await checker.check("NGN", "USDC");
    expect(result.passed).toBe(false);
    expect(result.reason).toMatch(/divergence/);
  });

  test("skips publication when all sources are unavailable", async () => {
    const src1 = new StubSource("a", null);
    const src2 = new StubSource("b", null);
    const checker = new CrossChecker([src1, src2], 200);
    const result = await checker.check("NGN", "USDC");
    expect(result.passed).toBe(false);
    expect(result.reason).toBe("no_sources_available");
    expect(result.consensusRate).toBeUndefined();
  });

  test("skips publication when only one source is available (cannot cross-check)", async () => {
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    const src2 = new StubSource("b", null); // unavailable
    const checker = new CrossChecker([src1, src2], 200);
    const result = await checker.check("NGN", "USDC");
    expect(result.passed).toBe(false);
    expect(result.reason).toBe("insufficient_sources_for_cross_check");
  });

  test("consensus rate is the arithmetic mean of two sources", async () => {
    const rate1 = 6500n; // scaled by 1e7 * 1e-7 = 0.0006500
    const rate2 = 6600n;
    const q1: FxQuote = { source: "a", base: "NGN", quote: "USDC", rateScaled: rate1, fetchedAt: 0 };
    const q2: FxQuote = { source: "b", base: "NGN", quote: "USDC", rateScaled: rate2, fetchedAt: 0 };
    // Use tiny rates to keep divergence manageable
    const src1: FxSource = { name: "a", fetchRate: async () => q1 };
    const src2: FxSource = { name: "b", fetchRate: async () => q2 };
    const checker = new CrossChecker([src1, src2], 5000); // wide threshold
    const result = await checker.check("NGN", "USDC");
    expect(result.passed).toBe(true);
    // mean of 6500 and 6600 = 6550
    expect(result.consensusRate).toBe(6550n);
  });

  test("computes consensus regardless of source rate ordering", async () => {
    // Descending order exercises the other branch of each reduce ternary.
    const rate1 = 6600n;
    const rate2 = 6500n;
    const q1: FxQuote = { source: "a", base: "NGN", quote: "USDC", rateScaled: rate1, fetchedAt: 0 };
    const q2: FxQuote = { source: "b", base: "NGN", quote: "USDC", rateScaled: rate2, fetchedAt: 0 };
    const src1: FxSource = { name: "a", fetchRate: async () => q1 };
    const src2: FxSource = { name: "b", fetchRate: async () => q2 };
    const checker = new CrossChecker([src1, src2], 5000);
    const result = await checker.check("NGN", "USDC");
    expect(result.passed).toBe(true);
    expect(result.consensusRate).toBe(6550n);
  });
});

// ---------------------------------------------------------------------------
// OracleRelay tests
// ---------------------------------------------------------------------------

describe("OracleRelay", () => {
  function makeRelay(overrides: Partial<FxIngestionConfig> = {}): MockRelay {
    return new MockRelay({ ...BASE_CONFIG, ...overrides });
  }

  test("base OracleRelay reports a publish_error when _submitToOracle throws", async () => {
    // Use the real OracleRelay (not the mock) so _submitToOracle throws its
    // "Stellar RPC not configured" default, exercising the publish catch path.
    const relay = new OracleRelay(BASE_CONFIG);
    const now = Math.floor(Date.now() / 1000);
    const result = await relay.publish("NGN", "USDC", 6500n, now);
    expect(result.skipped).toBe(true);
    expect(result.reason).toMatch(/publish_error/);
  });

  test("publish_error reason stringifies a non-Error throw", async () => {
    const relay = new StringThrowRelay(BASE_CONFIG);
    const now = Math.floor(Date.now() / 1000);
    const result = await relay.publish("NGN", "USDC", 6500n, now);
    expect(result.skipped).toBe(true);
    expect(result.reason).toContain("string error");
  });

  test("publishes a fresh rate successfully", async () => {
    const relay = makeRelay();
    const result = await relay.publish("NGN", "USDC", 6500n, Math.floor(Date.now() / 1000));
    expect(result.skipped).toBe(false);
    expect(result.txHash).toBe("0xmocktxhash");
    expect(relay.published).toHaveLength(1);
  });

  test("staleness-guard: skips rate that is too old", async () => {
    const relay = makeRelay({ maxStalenessSeconds: 60 });
    const staleTs = Math.floor(Date.now() / 1000) - 120; // 2 minutes old
    const result = await relay.publish("NGN", "USDC", 6500n, staleTs);
    expect(result.skipped).toBe(true);
    expect(result.reason).toBe("stale_rate");
    expect(relay.published).toHaveLength(0);
  });

  test("rate-limit: double-publish in same cycle is skipped", async () => {
    const relay = makeRelay({ pollIntervalMs: 300_000 }); // 5-min cycle
    const now = Math.floor(Date.now() / 1000);
    // First publish succeeds
    await relay.publish("NGN", "USDC", 6500n, now);
    // Second publish within same cycle is rate-limited
    const result = await relay.publish("NGN", "USDC", 6500n, now);
    expect(result.skipped).toBe(true);
    expect(result.reason).toBe("rate_limited");
    expect(relay.published).toHaveLength(1);
  });

  test("rate-limit: a second publish after the window elapses is allowed", async () => {
    const relay = makeRelay({ pollIntervalMs: 1_000 }); // 1s window
    let now = 1_000_000_000;
    const dateSpy = jest
      .spyOn(Date, "now")
      .mockImplementation(() => now * 1000);
    await relay.publish("NGN", "USDC", 6500n, now); // last = 1_000_000_000
    now += 2; // 2s later — beyond the 1s window
    const result = await relay.publish("NGN", "USDC", 6500n, now);
    expect(result.skipped).toBe(false);
    expect(relay.published).toHaveLength(2);
    dateSpy.mockRestore();
  });

  test("different pairs are not rate-limited against each other", async () => {
    const relay = makeRelay();
    const now = Math.floor(Date.now() / 1000);
    await relay.publish("NGN", "USDC", 6500n, now);
    const result = await relay.publish("KES", "USDC", 131n, now);
    expect(result.skipped).toBe(false);
    expect(relay.published).toHaveLength(2);
  });
});

// ---------------------------------------------------------------------------
// IngestionEngine integration tests
// ---------------------------------------------------------------------------

describe("IngestionEngine", () => {
  test("full cycle: passes cross-check → publishes to oracle", async () => {
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.000651));
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, {
      ...BASE_CONFIG,
      currencies: ["NGN"],
    });

    await engine.runCycle();
    expect(relay.published).toHaveLength(1);
    expect(relay.published[0].base).toBe("NGN");
  });

  test("failed cross-check: nothing published to oracle", async () => {
    // Both sources unavailable
    const src1 = new StubSource("a", null);
    const src2 = new StubSource("b", null);
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, {
      ...BASE_CONFIG,
      currencies: ["NGN"],
    });

    await engine.runCycle();
    expect(relay.published).toHaveLength(0);
  });

  test("one pair failing does not block others", async () => {
    // NGN fails, KES passes
    const ngnFail: FxSource = { name: "a", fetchRate: async (_b, q) => q === "USDC" ? null : null };
    const kesSrc1 = new StubSource("a", makeQuote("a", "KES", "USDC", 0.0077));
    const kesSrc2 = new StubSource("b", makeQuote("b", "KES", "USDC", 0.00771));

    // We need separate checkers per currency — the engine uses one shared checker,
    // but we verify the engine's allSettled behaviour isolates failures.
    const allAvailSrc1: FxSource = {
      name: "src1",
      fetchRate: async (base, _quote) => {
        if (base === "KES") return makeQuote("src1", "KES", "USDC", 0.0077);
        return null; // NGN unavailable
      },
    };
    const allAvailSrc2: FxSource = {
      name: "src2",
      fetchRate: async (base, _quote) => {
        if (base === "KES") return makeQuote("src2", "KES", "USDC", 0.00771);
        return null; // NGN unavailable
      },
    };

    const checker = new CrossChecker([allAvailSrc1, allAvailSrc2], 200);
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, {
      ...BASE_CONFIG,
      currencies: ["NGN", "KES"],
    });

    await engine.runCycle();
    // Only KES should have been published
    expect(relay.published.some((p) => p.base === "KES")).toBe(true);
    expect(relay.published.some((p) => p.base === "NGN")).toBe(false);
  });

  test("start() is idempotent and stop() clears the polling timer", () => {
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.000651));
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, {
      ...BASE_CONFIG,
      pollIntervalMs: 60_000,
    });

    engine.start();
    engine.start(); // idempotent — no double timer
    engine.stop();
    engine.stop(); // idempotent — safe to stop when not running
  });

  test("skips a pair where base equals quote", async () => {
    const src1 = new StubSource("a", makeQuote("a", "USDC", "USDC", 1));
    const src2 = new StubSource("b", makeQuote("b", "USDC", "USDC", 1));
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, {
      ...BASE_CONFIG,
      currencies: ["USDC"],
      quoteCurrency: "USDC",
    });

    await engine.runCycle();
    expect(relay.published).toHaveLength(0);
  });

  test("records a skip when cross-check passes but consensusRate is missing", async () => {
    // A stub checker that reports passed=true with no consensusRate, exercising
    // the engine's `consensusRate == null` branch.
    const stubChecker = {
      check: async () => ({
        base: "NGN",
        quote: "USDC",
        passed: true,
        divergenceBps: 0,
        quotes: [],
      }),
    } as unknown as CrossChecker;
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(stubChecker, relay, BASE_CONFIG);

    await engine.runCycle();
    expect(relay.published).toHaveLength(0);
  });

  test("records a skip when the relay rejects a stale rate", async () => {
    // Quotes with fetchedAt=0 are stale relative to now → the relay skips with
    // stale_rate, exercising the engine's published.skipped branch.
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065, 0));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.000651, 0));
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay({ ...BASE_CONFIG, maxStalenessSeconds: 60 });
    const engine = new IngestionEngine(checker, relay, BASE_CONFIG);

    await engine.runCycle();
    expect(relay.published).toHaveLength(0);
  });

  test("records a generic skip reason when the relay skips without one", async () => {
    // A relay that returns skipped:true with no reason exercises the engine's
    // `published.reason ?? "publish_skipped"` fallback.
    let now = 1_000_000_000;
    const metrics = new FxMetrics({ now: () => now });
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.000651));
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new NoReasonSkipRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, {
      ...BASE_CONFIG,
      currencies: ["NGN"],
    }, metrics);

    await engine.runCycle();
    expect(metrics.render()).toContain(
      'kora_fx_publication_skipped_total{reason="publish_skipped"} 1',
    );
  });
});
