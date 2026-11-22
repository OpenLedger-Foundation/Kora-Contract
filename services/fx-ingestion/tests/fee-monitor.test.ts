/**
 * Surge pricing monitor + fee hint endpoint tests (Issue #1034).
 *
 * Uses recorded Soroban RPC `getFeeStats` JSON fixtures in tests/fixtures/.
 */

import fs from "fs";
import http from "http";
import { AddressInfo } from "net";
import path from "path";
import axios from "axios";
import {
  BPS_ONE,
  FeeMonitorConfig,
  FeeStatsResult,
  STELLAR_MIN_BASE_FEE,
  SurgePricingMonitor,
  computeMultiplierBps,
  loadFeeMonitorConfig,
  renderPrometheusMetrics,
  rpcFeeStatsFetcher,
} from "../src/fee-monitor";
import { FixedWindowLimiter, handleFeeHintRequest, loadFeeHintServerConfig, startFeeHintServer } from "../src/fee-hint-server";

jest.mock("axios");
const mockedAxios = axios as jest.Mocked<typeof axios>;

function fixture(name: string): { result?: FeeStatsResult; error?: { message: string } } {
  return JSON.parse(fs.readFileSync(path.join(__dirname, "fixtures", name), "utf8"));
}
const CALM = fixture("fee-stats-calm.json").result!;
const SURGE = fixture("fee-stats-surge.json").result!;
const EXTREME = fixture("fee-stats-extreme.json").result!;

function withLedger(stats: FeeStatsResult, ledger: number): FeeStatsResult {
  return { ...stats, latestLedger: ledger };
}

const CONFIG: FeeMonitorConfig = {
  rpcUrl: "https://soroban-testnet.stellar.org",
  pollIntervalMs: 15_000,
  surgeThresholdBps: 15_000,
  maxMultiplierBps: 100_000,
  staleAfterMs: 45_000,
  network: "testnet",
};

/** Monitor driven by a scripted sequence of fetch outcomes and a fake clock. */
function scripted(outcomes: Array<FeeStatsResult | Error>, config: FeeMonitorConfig = CONFIG) {
  let clock = 1_700_000_000_000;
  const queue = [...outcomes];
  const fetcher = jest.fn(async () => {
    const next = queue.shift();
    if (!next) throw new Error("no more fixtures");
    if (next instanceof Error) throw next;
    return next;
  });
  const monitor = new SurgePricingMonitor(config, fetcher, () => clock);
  return {
    monitor,
    fetcher,
    advance: (ms: number) => {
      clock += ms;
    },
    now: () => clock,
  };
}

describe("computeMultiplierBps", () => {
  it("is 1.0x when fees sit at the network minimum", () => {
    expect(computeMultiplierBps(CALM, 100_000)).toBe(BPS_ONE);
  });

  it("uses the larger p90 of the classic and soroban lanes", () => {
    // soroban p90 = 250, classic p90 = 200 → 2.5x
    expect(computeMultiplierBps(SURGE, 100_000)).toBe(25_000);
  });

  it("clamps extreme multipliers to the configured maximum", () => {
    expect(computeMultiplierBps(EXTREME, 100_000)).toBe(100_000);
    expect(computeMultiplierBps(EXTREME, 30_000)).toBe(30_000);
  });

  it("never drops below 1.0x, even with a misconfigured cap or garbage stats", () => {
    expect(computeMultiplierBps(SURGE, 5_000)).toBe(BPS_ONE);
    const garbage = { ...CALM, inclusionFee: { ...CALM.inclusionFee, p90: "nan" }, sorobanInclusionFee: { ...CALM.sorobanInclusionFee, p90: "-5" } };
    expect(computeMultiplierBps(garbage, 100_000)).toBe(BPS_ONE);
  });
});

describe("SurgePricingMonitor", () => {
  it("returns a degraded 1.0x hint before the first observation", () => {
    const { monitor } = scripted([]);
    const hint = monitor.getHint();
    expect(hint).toMatchObject({
      base_fee: STELLAR_MIN_BASE_FEE,
      surge_multiplier_bps: BPS_ONE,
      suggested_fee: STELLAR_MIN_BASE_FEE,
      surge_active: false,
      observed_at: null,
      stale: true,
      degraded: true,
      advisory: true,
    });
  });

  it("reports calm conditions as fresh, non-surge", async () => {
    const { monitor } = scripted([CALM]);
    await monitor.poll();
    const hint = monitor.getHint();
    expect(hint.surge_active).toBe(false);
    expect(hint.stale).toBe(false);
    expect(hint.degraded).toBe(false);
    expect(hint.latest_ledger).toBe(CALM.latestLedger);
    expect(monitor.getMetrics().surgeActive).toBe(0);
  });

  it("flags a surge episode and increments the metric within one poll", async () => {
    const { monitor } = scripted([CALM, SURGE]);
    await monitor.poll();
    expect(monitor.getMetrics().surgeEpisodesTotal).toBe(0);
    await monitor.poll();
    const metrics = monitor.getMetrics();
    expect(metrics.surgeActive).toBe(1);
    expect(metrics.surgeEpisodesTotal).toBe(1);
    expect(metrics.multiplierBps).toBe(25_000);

    const hint = monitor.getHint();
    expect(hint.surge_active).toBe(true);
    expect(hint.surge_multiplier).toBe(2.5);
    expect(hint.suggested_fee).toBe(250);
  });

  it("counts one episode per contiguous surge and closes it when fees normalise", async () => {
    const { monitor } = scripted([
      SURGE,
      withLedger(EXTREME, SURGE.latestLedger + 1),
      withLedger(CALM, SURGE.latestLedger + 2),
      withLedger(SURGE, SURGE.latestLedger + 3),
    ]);
    await monitor.poll();
    await monitor.poll();
    expect(monitor.getMetrics().surgeEpisodesTotal).toBe(1);
    expect(monitor.getEpisodes()[0].peakMultiplierBps).toBe(100_000);

    await monitor.poll();
    expect(monitor.getMetrics().surgeActive).toBe(0);
    expect(monitor.getEpisodes()[0].endedAt).not.toBeNull();

    await monitor.poll();
    expect(monitor.getMetrics().surgeEpisodesTotal).toBe(2);
    expect(monitor.getEpisodes()).toHaveLength(2);
  });

  it("keeps serving last-known-good (degraded, stale) when the RPC fee endpoint errors", async () => {
    const { monitor } = scripted([SURGE, new Error("ECONNRESET")]);
    await monitor.poll();
    await expect(monitor.poll()).resolves.toBeUndefined();
    const hint = monitor.getHint();
    expect(hint.degraded).toBe(true);
    expect(hint.stale).toBe(true);
    expect(hint.surge_multiplier_bps).toBe(25_000);
    expect(monitor.getMetrics().pollErrorsTotal).toBe(1);
  });

  it("recovers from degraded once a poll succeeds", async () => {
    const { monitor } = scripted([new Error("timeout"), CALM]);
    await monitor.poll();
    expect(monitor.getHint().degraded).toBe(true);
    await monitor.poll();
    expect(monitor.getHint().degraded).toBe(false);
  });

  it("treats flat stats (ledger did not advance) as stale last-known-good", async () => {
    const { monitor } = scripted([SURGE, withLedger(CALM, SURGE.latestLedger)]);
    await monitor.poll();
    await monitor.poll();
    const hint = monitor.getHint();
    expect(hint.stale).toBe(true);
    expect(hint.degraded).toBe(false);
    expect(hint.surge_active).toBe(true); // last-known-good, not the stuck calm reading
    expect(hint.surge_multiplier_bps).toBe(25_000);
  });

  it("marks hints stale after staleAfterMs without a fresh observation", async () => {
    const { monitor, advance } = scripted([CALM]);
    await monitor.poll();
    advance(CONFIG.staleAfterMs);
    expect(monitor.getHint().stale).toBe(false);
    advance(1);
    expect(monitor.getHint().stale).toBe(true);
  });

  it("start/stop schedules and clears polling", async () => {
    jest.useFakeTimers();
    try {
      const { monitor, fetcher } = scripted([CALM, withLedger(CALM, CALM.latestLedger + 1)]);
      monitor.start();
      monitor.start(); // idempotent
      expect(fetcher).toHaveBeenCalledTimes(1);
      jest.advanceTimersByTime(CONFIG.pollIntervalMs);
      expect(fetcher).toHaveBeenCalledTimes(2);
      monitor.stop();
      monitor.stop();
      jest.advanceTimersByTime(CONFIG.pollIntervalMs * 3);
      expect(fetcher).toHaveBeenCalledTimes(2);
    } finally {
      jest.useRealTimers();
    }
  });
});

describe("rpcFeeStatsFetcher", () => {
  afterEach(() => mockedAxios.post.mockReset());

  it("posts a getFeeStats JSON-RPC request and returns the result", async () => {
    mockedAxios.post.mockResolvedValueOnce({ data: fixture("fee-stats-surge.json") });
    const result = await rpcFeeStatsFetcher("https://rpc.example")();
    expect(result.latestLedger).toBe(SURGE.latestLedger);
    expect(mockedAxios.post).toHaveBeenCalledWith(
      "https://rpc.example",
      { jsonrpc: "2.0", id: 1, method: "getFeeStats" },
      expect.objectContaining({ timeout: 5_000 })
    );
  });

  it("throws on a JSON-RPC error body so the monitor degrades", async () => {
    mockedAxios.post.mockResolvedValueOnce({ data: fixture("fee-stats-error.json") });
    await expect(rpcFeeStatsFetcher("https://rpc.example")()).rejects.toThrow("fee stats not available yet");
  });

  it("throws on an empty result", async () => {
    mockedAxios.post.mockResolvedValueOnce({ data: {} });
    await expect(rpcFeeStatsFetcher("https://rpc.example")()).rejects.toThrow("empty result");
  });
});

describe("config loaders", () => {
  it("applies defaults", () => {
    const c = loadFeeMonitorConfig({});
    expect(c).toMatchObject({ pollIntervalMs: 15_000, surgeThresholdBps: 15_000, maxMultiplierBps: 100_000, staleAfterMs: 45_000, network: "testnet" });
    expect(loadFeeHintServerConfig({})).toEqual({ port: 8089, rateLimitPerWindow: 60, rateLimitWindowMs: 60_000, maxAgeSeconds: 15 });
  });

  it("reads overrides from env", () => {
    const c = loadFeeMonitorConfig({ FEE_HINT_MAX_MULTIPLIER_BPS: "50000", STELLAR_NETWORK: "pubnet", FEE_HINT_STALE_AFTER_MS: "1000" });
    expect(c.maxMultiplierBps).toBe(50_000);
    expect(c.network).toBe("pubnet");
    expect(c.staleAfterMs).toBe(1_000);
  });
});

describe("Prometheus metrics", () => {
  it("exposes kora_surge_pricing_active as a gauge", async () => {
    const { monitor } = scripted([SURGE]);
    await monitor.poll();
    const text = renderPrometheusMetrics(monitor.getMetrics());
    expect(text).toContain("# TYPE kora_surge_pricing_active gauge");
    expect(text).toContain("kora_surge_pricing_active 1");
    expect(text).toContain("kora_surge_episodes_total 1");
    expect(text).toContain("kora_fee_hint_multiplier_bps 25000");
  });
});

describe("GET /network/fee-hint", () => {
  const serverConfig = { maxAgeSeconds: 15 };

  it("returns 200 with structured JSON", async () => {
    const { monitor, now } = scripted([SURGE]);
    await monitor.poll();
    const limiter = new FixedWindowLimiter(10, 60_000, now);
    const res = handleFeeHintRequest("GET", "/network/fee-hint?x=1", "1.2.3.4", monitor, limiter, serverConfig, now);
    expect(res.status).toBe(200);
    expect(res.headers["X-Fee-Hint-Stale"]).toBe("false");
    expect(res.headers["Cache-Control"]).toBe("public, max-age=15");
    expect(res.headers["Age"]).toBe("0");
    const body = JSON.parse(res.body);
    expect(body).toMatchObject({ base_fee: 100, surge_multiplier_bps: 25_000, surge_active: true, advisory: true });
    expect(typeof body.observed_at).toBe("string");
  });

  it("still returns 200 with a degraded hint when RPC is down", async () => {
    const { monitor, now } = scripted([new Error("down")]);
    await monitor.poll();
    const res = handleFeeHintRequest("GET", "/network/fee-hint", "k", monitor, new FixedWindowLimiter(10, 60_000, now), serverConfig, now);
    expect(res.status).toBe(200);
    expect(res.headers["X-Fee-Hint-Degraded"]).toBe("true");
    expect(res.headers["X-Fee-Hint-Stale"]).toBe("true");
    expect(res.headers["Age"]).toBeUndefined();
    expect(JSON.parse(res.body).surge_multiplier_bps).toBe(BPS_ONE);
  });

  it("rate limits per client with 429 + Retry-After", async () => {
    const { monitor, now, advance } = scripted([CALM]);
    await monitor.poll();
    const limiter = new FixedWindowLimiter(2, 60_000, now);
    const call = (key: string) => handleFeeHintRequest("GET", "/network/fee-hint", key, monitor, limiter, serverConfig, now);
    expect(call("a").status).toBe(200);
    expect(call("a").status).toBe(200);
    const limited = call("a");
    expect(limited.status).toBe(429);
    expect(limited.headers["Retry-After"]).toBe("60");
    expect(call("b").status).toBe(200); // independent client
    advance(60_000);
    expect(call("a").status).toBe(200); // window reset
  });

  it("serves metrics, rejects other paths and methods", async () => {
    const { monitor, now } = scripted([CALM]);
    await monitor.poll();
    const limiter = new FixedWindowLimiter(1, 60_000, now);
    expect(handleFeeHintRequest("GET", "/metrics", "k", monitor, limiter, serverConfig, now).body).toContain("kora_surge_pricing_active 0");
    expect(handleFeeHintRequest("GET", "/nope", "k", monitor, limiter, serverConfig, now).status).toBe(404);
    expect(handleFeeHintRequest("POST", "/network/fee-hint", "k", monitor, limiter, serverConfig, now).status).toBe(405);
  });
});

describe("startFeeHintServer", () => {
  it("serves the hint over HTTP", async () => {
    const { monitor } = scripted([CALM]);
    await monitor.poll();
    const server = startFeeHintServer(monitor, { port: 0, rateLimitPerWindow: 5, rateLimitWindowMs: 60_000, maxAgeSeconds: 15 });
    await new Promise<void>((resolve) => server.once("listening", () => resolve()));
    try {
      const { port } = server.address() as AddressInfo;
      const { status, body } = await new Promise<{ status: number; body: string }>((resolve, reject) => {
        http
          .get(`http://127.0.0.1:${port}/network/fee-hint`, (res) => {
            let data = "";
            res.on("data", (chunk) => (data += chunk));
            res.on("end", () => resolve({ status: res.statusCode ?? 0, body: data }));
          })
          .on("error", reject);
      });
      expect(status).toBe(200);
      expect(JSON.parse(body).surge_multiplier_bps).toBe(BPS_ONE);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});
