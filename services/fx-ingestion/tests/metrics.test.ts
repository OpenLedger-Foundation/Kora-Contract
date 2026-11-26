/**
 * FX staleness metrics tests — issue #<fx-staleness>.
 *
 * Covers:
 *  - kora_fx_rate_age_seconds moves with ingestion time (resets on publication)
 *  - no samples yet → comment, not a bogus zero
 *  - skip counter increments by reason; cycle counter increments
 *  - render() Prometheus text format (HELP/TYPE)
 *  - /metrics scrape endpoint returns the registry; /health and 404 paths
 *  - readFxMetricsPort env validation
 *  - IngestionEngine integration: gauges reflect publish / skip / cycle
 */

import http from "http";
import type { AddressInfo } from "net";
import { FxMetrics, readFxMetricsPort, startMetricsServer } from "../src/metrics";
import { CrossChecker } from "../src/crosscheck";
import { OracleRelay } from "../src/relay";
import { IngestionEngine } from "../src/engine";
import type { FxSource } from "../src/sources";
import type { CurrencyCode, FxQuote, FxIngestionConfig } from "../src/types";

// ── Test doubles (mirrors fx-ingestion.test.ts) ────────────────────────────────

function makeQuote(
  source: string,
  base: CurrencyCode,
  quote: CurrencyCode,
  rate: number,
  fetchedAt?: number,
): FxQuote {
  return {
    source,
    base,
    quote,
    rateScaled: BigInt(Math.round(rate * 10_000_000)),
    fetchedAt: fetchedAt ?? Math.floor(Date.now() / 1000),
  };
}

class StubSource implements FxSource {
  constructor(
    readonly name: string,
    private readonly response: FxQuote | null,
  ) {}
  async fetchRate(): Promise<FxQuote | null> {
    return this.response;
  }
}

class MockRelay extends OracleRelay {
  protected override async _submitToOracle(): Promise<string> {
    return "0xmocktxhash";
  }
}

const BASE_CONFIG: FxIngestionConfig = {
  currencies: ["NGN"],
  quoteCurrency: "USDC",
  maxDivergenceBps: 200,
  pollIntervalMs: 300_000,
  oracleContractAddress: "CORACLE",
  relaySecret: "SRELAY",
  rpcUrl: "https://test",
  networkPassphrase: "Test SDF Network ; September 2015",
  maxStalenessSeconds: 3600,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

function httpGet(port: number, path: string): Promise<{ status: number; body: string }> {
  return new Promise((resolve, reject) => {
    http.get(`http://127.0.0.1:${port}${path}`, (res) => {
      let body = "";
      res.on("data", (c) => (body += c));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body }));
    }).on("error", reject);
  });
}

// ── FxMetrics unit tests ──────────────────────────────────────────────────────

describe("FxMetrics", () => {
  it("omits the age gauge and emits a comment when no samples exist yet", () => {
    let now = 1_000_000_000;
    const metrics = new FxMetrics({ now: () => now });
    const out = metrics.render();
    expect(out).toContain(
      "# No FX rate samples ingested yet — staleness gauges omitted",
    );
    expect(out).not.toMatch(/kora_fx_rate_age_seconds\{.*\}\s+\d/);
  });

  it("reports age 0 immediately after publication, then grows with the clock", () => {
    let now = 1_000_000_000; // ms
    const metrics = new FxMetrics({ now: () => now });

    metrics.recordPublication("NGN", "USDC", Math.floor(now / 1000));
    expect(metrics.ageSeconds("NGN", "USDC")).toBe(0);
    expect(metrics.render()).toContain(
      'kora_fx_rate_age_seconds{base="NGN",quote="USDC"} 0',
    );

    // 30 seconds pass with no new publication → age grows
    now += 30_000;
    expect(metrics.ageSeconds("NGN", "USDC")).toBe(30);
    expect(metrics.render()).toContain(
      'kora_fx_rate_age_seconds{base="NGN",quote="USDC"} 30',
    );

    // A fresh publication resets the age
    metrics.recordPublication("NGN", "USDC", Math.floor(now / 1000));
    expect(metrics.ageSeconds("NGN", "USDC")).toBe(0);
  });

  it("reports the last ingestion timestamp gauge", () => {
    let now = 1_000_000_000;
    const metrics = new FxMetrics({ now: () => now });
    metrics.recordPublication("KES", "USDC", Math.floor(now / 1000));
    expect(metrics.render()).toContain(
      `kora_fx_last_ingestion_timestamp_seconds{base="KES",quote="USDC"} 1000000`,
    );
  });

  it("ageSeconds returns null for a pair with no sample", () => {
    const metrics = new FxMetrics();
    expect(metrics.ageSeconds("NGN", "USDC")).toBeNull();
  });

  it("increments skip counters by reason", () => {
    const metrics = new FxMetrics();
    metrics.recordSkip("NGN", "USDC", "no_sources_available");
    metrics.recordSkip("NGN", "USDC", "no_sources_available");
    metrics.recordSkip("KES", "USDC", "source_divergence_300bps_exceeds_200bps");
    const out = metrics.render();
    expect(out).toContain(
      'kora_fx_publication_skipped_total{reason="no_sources_available"} 2',
    );
    expect(out).toContain(
      'kora_fx_publication_skipped_total{reason="source_divergence_300bps_exceeds_200bps"} 1',
    );
  });

  it("folds an empty reason into 'unknown'", () => {
    const metrics = new FxMetrics();
    metrics.recordSkip("NGN", "USDC", "");
    expect(metrics.render()).toContain(
      'kora_fx_publication_skipped_total{reason="unknown"} 1',
    );
  });

  it("increments the cycle counter", () => {
    const metrics = new FxMetrics();
    metrics.recordCycle();
    metrics.recordCycle();
    expect(metrics.render()).toContain("kora_fx_ingestion_cycles_total 2");
  });

  it("emits HELP/TYPE lines for each metric family", () => {
    const metrics = new FxMetrics();
    const out = metrics.render();
    expect(out).toContain("# HELP kora_fx_rate_age_seconds");
    expect(out).toContain("# TYPE kora_fx_rate_age_seconds gauge");
    expect(out).toContain("# HELP kora_fx_ingestion_cycles_total");
    expect(out).toContain("# TYPE kora_fx_ingestion_cycles_total counter");
    expect(out).toContain("# HELP kora_fx_publication_skipped_total");
    expect(out).toContain("# TYPE kora_fx_publication_skipped_total counter");
  });
});

// ── Scrape endpoint ────────────────────────────────────────────────────────────

describe("startMetricsServer", () => {
  let server: http.Server;

  beforeEach(async () => {
    const metrics = new FxMetrics({ now: () => 1_000_000_000 });
    metrics.recordPublication("NGN", "USDC", 1_000_000);
    metrics.recordSkip("KES", "USDC", "no_sources_available");
    server = startMetricsServer(metrics, 0);
    await new Promise<void>((res) => server.once("listening", res));
  });

  afterEach(() => new Promise<void>((res) => server.close(() => res())));

  function port(): number {
    return (server.address() as AddressInfo).port;
  }

  it("serves /metrics as Prometheus text containing the gauges", async () => {
    const { status, body } = await httpGet(port(), "/metrics");
    expect(status).toBe(200);
    expect(body).toContain("kora_fx_rate_age_seconds");
    expect(body).toContain('kora_fx_publication_skipped_total{reason="no_sources_available"} 1');
  });

  it("serves /health", async () => {
    const { status, body } = await httpGet(port(), "/health");
    expect(status).toBe(200);
    expect(JSON.parse(body)).toEqual({ status: "ok" });
  });

  it("returns 404 for unknown paths", async () => {
    const { status } = await httpGet(port(), "/nope");
    expect(status).toBe(404);
  });
});

// ── Env validation ───────────────────────────────────────────────────────────

describe("readFxMetricsPort", () => {
  afterEach(() => {
    delete process.env.FX_METRICS_PORT;
  });

  it("defaults to 8081 when unset", () => {
    delete process.env.FX_METRICS_PORT;
    expect(readFxMetricsPort()).toBe(8081);
  });

  it("returns the configured port", () => {
    process.env.FX_METRICS_PORT = "9099";
    expect(readFxMetricsPort()).toBe(9099);
  });

  it("rejects out-of-range / non-integer values", () => {
    process.env.FX_METRICS_PORT = "0";
    expect(() => readFxMetricsPort()).toThrow("1..65535");
    process.env.FX_METRICS_PORT = "70000";
    expect(() => readFxMetricsPort()).toThrow("1..65535");
    process.env.FX_METRICS_PORT = "1.5";
    expect(() => readFxMetricsPort()).toThrow("1..65535");
  });
});

// ── IngestionEngine + FxMetrics integration ───────────────────────────────────

describe("IngestionEngine + FxMetrics", () => {
  it("records a publication on a successful cycle (gauge age resets)", async () => {
    let now = 1_000_000_000;
    const metrics = new FxMetrics({ now: () => now });
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.000651));
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, BASE_CONFIG, metrics);

    await engine.runCycle();
    expect(metrics.ageSeconds("NGN", "USDC")).toBe(0);
    expect(metrics.render()).toContain(
      'kora_fx_rate_age_seconds{base="NGN",quote="USDC"} 0',
    );

    // Time passes without a new cycle → age grows
    now += 120_000;
    expect(metrics.ageSeconds("NGN", "USDC")).toBe(120);
  });

  it("records a skip and not a publication when cross-check fails", async () => {
    let now = 1_000_000_000;
    const metrics = new FxMetrics({ now: () => now });
    const src1 = new StubSource("a", null);
    const src2 = new StubSource("b", null);
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, BASE_CONFIG, metrics);

    await engine.runCycle();
    expect(metrics.ageSeconds("NGN", "USDC")).toBeNull();
    expect(metrics.render()).toContain(
      'kora_fx_publication_skipped_total{reason="no_sources_available"} 1',
    );
  });

  it("counts completed cycles", async () => {
    let now = 1_000_000_000;
    const metrics = new FxMetrics({ now: () => now });
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.000651));
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay(BASE_CONFIG);
    const engine = new IngestionEngine(checker, relay, BASE_CONFIG, metrics);

    await engine.runCycle();
    await engine.runCycle();
    expect(metrics.render()).toContain("kora_fx_ingestion_cycles_total 2");
  });

  it("records a relay skip (stale_rate) in the metrics when the relay rejects", async () => {
    let now = 1_000_000_000;
    const metrics = new FxMetrics({ now: () => now });
    // fetchedAt=0 → stale relative to now → relay skips with stale_rate
    const src1 = new StubSource("a", makeQuote("a", "NGN", "USDC", 0.00065, 0));
    const src2 = new StubSource("b", makeQuote("b", "NGN", "USDC", 0.000651, 0));
    const checker = new CrossChecker([src1, src2], 200);
    const relay = new MockRelay({ ...BASE_CONFIG, maxStalenessSeconds: 60 });
    const engine = new IngestionEngine(checker, relay, BASE_CONFIG, metrics);

    await engine.runCycle();
    expect(metrics.render()).toContain(
      'kora_fx_publication_skipped_total{reason="stale_rate"} 1',
    );
  });
});
