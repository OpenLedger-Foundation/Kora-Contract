/**
 * SLA classifier matrix tests — issue #771 expansion
 *
 * Covers:
 *  - Partial-degradation combinations for timeseries, risk-pipeline,
 *    and governance-sandbox (table-driven)
 *  - Multi-upstream partial mixes: some ok, some degraded, some down
 *  - All-timeout edge case: every service unreachable → overall down
 *  - Non-blocking probes: Promise.allSettled guarantees no event-loop block
 *  - New DEFAULT_SLAS entries present and correctly shaped
 *  - buildDefaultServices returns all five expected services
 */

import {
  DEFAULT_SLAS,
  HealthAggregator,
  HealthFetcher,
  UpstreamService,
  buildDefaultServices,
} from "../health-aggregator";
import { classifyServiceStatus, rollupStatus } from "../sla-classifier";
import { ServiceHealthReport, ServiceSlaConfig, ServiceStatus } from "../types";

// ── Helpers ───────────────────────────────────────────────────────────────────

function report(
  status: ServiceStatus,
  metrics: Record<string, number> = {},
): ServiceHealthReport {
  return { status, checkedAt: new Date().toISOString(), metrics };
}

const ok   = (m: Record<string, number> = {}) => report("ok",       m);
const deg  = (m: Record<string, number> = {}) => report("degraded", m);

// ── DEFAULT_SLAS shape checks ─────────────────────────────────────────────────

describe("DEFAULT_SLAS — new service entries", () => {
  it("contains a timeseries entry", () => {
    expect(DEFAULT_SLAS.timeseries).toBeDefined();
  });

  it("timeseries degraded threshold: lagSeconds 600, ingestQueueDepth 100", () => {
    expect(DEFAULT_SLAS.timeseries.degradedThresholds?.lagSeconds).toBe(600);
    expect(DEFAULT_SLAS.timeseries.degradedThresholds?.ingestQueueDepth).toBe(100);
  });

  it("timeseries down threshold: lagSeconds 3600, ingestQueueDepth 1000", () => {
    expect(DEFAULT_SLAS.timeseries.downThresholds?.lagSeconds).toBe(3_600);
    expect(DEFAULT_SLAS.timeseries.downThresholds?.ingestQueueDepth).toBe(1_000);
  });

  it("contains a risk-pipeline entry", () => {
    expect(DEFAULT_SLAS["risk-pipeline"]).toBeDefined();
  });

  it("risk-pipeline degraded threshold: duplicateRate 1, scoringBacklogDepth 20", () => {
    expect(DEFAULT_SLAS["risk-pipeline"].degradedThresholds?.duplicateRate).toBe(1);
    expect(DEFAULT_SLAS["risk-pipeline"].degradedThresholds?.scoringBacklogDepth).toBe(20);
  });

  it("risk-pipeline down threshold: duplicateRate 5, scoringBacklogDepth 200", () => {
    expect(DEFAULT_SLAS["risk-pipeline"].downThresholds?.duplicateRate).toBe(5);
    expect(DEFAULT_SLAS["risk-pipeline"].downThresholds?.scoringBacklogDepth).toBe(200);
  });

  it("contains a governance-sandbox entry", () => {
    expect(DEFAULT_SLAS["governance-sandbox"]).toBeDefined();
  });

  it("governance-sandbox degraded threshold: snapshotCount 5000, simLatencyMs 2000", () => {
    expect(DEFAULT_SLAS["governance-sandbox"].degradedThresholds?.snapshotCount).toBe(5_000);
    expect(DEFAULT_SLAS["governance-sandbox"].degradedThresholds?.simLatencyMs).toBe(2_000);
  });

  it("governance-sandbox down threshold: snapshotCount 50000, simLatencyMs 10000", () => {
    expect(DEFAULT_SLAS["governance-sandbox"].downThresholds?.snapshotCount).toBe(50_000);
    expect(DEFAULT_SLAS["governance-sandbox"].downThresholds?.simLatencyMs).toBe(10_000);
  });
});

// ── buildDefaultServices ──────────────────────────────────────────────────────

describe("buildDefaultServices", () => {
  it("returns five services", () => {
    expect(buildDefaultServices()).toHaveLength(5);
  });

  it("includes timeseries, risk-pipeline, and governance-sandbox", () => {
    const names = buildDefaultServices().map((s) => s.name);
    expect(names).toContain("timeseries");
    expect(names).toContain("risk-pipeline");
    expect(names).toContain("governance-sandbox");
  });
});

// ── Table-driven SLA matrix ───────────────────────────────────────────────────
//
// Each row: [service label, SLA key, metrics that push into degraded, expected status]
//           [service label, SLA key, metrics that push into down,     expected status]

type MatrixRow = {
  label: string;
  slaKey: string;
  metrics: Record<string, number>;
  expected: ServiceStatus;
};

const MATRIX: MatrixRow[] = [
  // ── timeseries ──────────────────────────────────────────────────────────────
  { label: "timeseries ok",                      slaKey: "timeseries",         metrics: { lagSeconds: 0,   ingestQueueDepth: 0   }, expected: "ok"       },
  { label: "timeseries degraded via lagSeconds", slaKey: "timeseries",         metrics: { lagSeconds: 600, ingestQueueDepth: 0   }, expected: "degraded" },
  { label: "timeseries degraded via queue",      slaKey: "timeseries",         metrics: { lagSeconds: 0,   ingestQueueDepth: 100 }, expected: "degraded" },
  { label: "timeseries down via lagSeconds",     slaKey: "timeseries",         metrics: { lagSeconds: 3_600, ingestQueueDepth: 0  }, expected: "down"     },
  { label: "timeseries down via queue",          slaKey: "timeseries",         metrics: { lagSeconds: 0,   ingestQueueDepth: 1_000 }, expected: "down"   },
  { label: "timeseries down overrides degraded", slaKey: "timeseries",         metrics: { lagSeconds: 3_600, ingestQueueDepth: 100 }, expected: "down"   },

  // ── risk-pipeline ───────────────────────────────────────────────────────────
  { label: "risk-pipeline ok",                        slaKey: "risk-pipeline", metrics: { duplicateRate: 0, scoringBacklogDepth: 0   }, expected: "ok"       },
  { label: "risk-pipeline degraded via duplicateRate",slaKey: "risk-pipeline", metrics: { duplicateRate: 1, scoringBacklogDepth: 0   }, expected: "degraded" },
  { label: "risk-pipeline degraded via backlog",      slaKey: "risk-pipeline", metrics: { duplicateRate: 0, scoringBacklogDepth: 20  }, expected: "degraded" },
  { label: "risk-pipeline down via duplicateRate",    slaKey: "risk-pipeline", metrics: { duplicateRate: 5, scoringBacklogDepth: 0   }, expected: "down"     },
  { label: "risk-pipeline down via backlog",          slaKey: "risk-pipeline", metrics: { duplicateRate: 0, scoringBacklogDepth: 200 }, expected: "down"     },
  { label: "risk-pipeline down overrides degraded",   slaKey: "risk-pipeline", metrics: { duplicateRate: 5, scoringBacklogDepth: 20  }, expected: "down"     },

  // ── governance-sandbox ──────────────────────────────────────────────────────
  { label: "gov-sandbox ok",                          slaKey: "governance-sandbox", metrics: { snapshotCount: 0,       simLatencyMs: 0      }, expected: "ok"       },
  { label: "gov-sandbox degraded via snapshotCount",  slaKey: "governance-sandbox", metrics: { snapshotCount: 5_000,   simLatencyMs: 0      }, expected: "degraded" },
  { label: "gov-sandbox degraded via simLatency",     slaKey: "governance-sandbox", metrics: { snapshotCount: 0,       simLatencyMs: 2_000  }, expected: "degraded" },
  { label: "gov-sandbox down via snapshotCount",      slaKey: "governance-sandbox", metrics: { snapshotCount: 50_000,  simLatencyMs: 0      }, expected: "down"     },
  { label: "gov-sandbox down via simLatency",         slaKey: "governance-sandbox", metrics: { snapshotCount: 0,       simLatencyMs: 10_000 }, expected: "down"     },
  { label: "gov-sandbox down overrides degraded",     slaKey: "governance-sandbox", metrics: { snapshotCount: 50_000,  simLatencyMs: 2_000  }, expected: "down"     },
];

describe("SLA classifier matrix — table-driven", () => {
  for (const row of MATRIX) {
    it(row.label, () => {
      const sla = DEFAULT_SLAS[row.slaKey] as ServiceSlaConfig;
      const status = classifyServiceStatus(ok(row.metrics), sla);
      expect(status).toBe(row.expected);
    });
  }
});

// ── Multi-upstream partial mixes ──────────────────────────────────────────────

describe("HealthAggregator — multi-upstream partial-degradation mixes", () => {
  function makeAllServices(): UpstreamService[] {
    return [
      { name: "timeseries",         url: "http://ts/health",   sla: DEFAULT_SLAS.timeseries },
      { name: "risk-pipeline",      url: "http://rp/health",   sla: DEFAULT_SLAS["risk-pipeline"] },
      { name: "governance-sandbox", url: "http://gov/health",  sla: DEFAULT_SLAS["governance-sandbox"] },
    ];
  }

  it("all ok → overall ok", async () => {
    const fetcher: HealthFetcher = async () => ok();
    const report = await new HealthAggregator(makeAllServices(), fetcher).aggregate();
    expect(report.overall).toBe("ok");
    for (const svc of report.services) {
      expect(svc.status).toBe("ok");
    }
  });

  it("one degraded, rest ok → overall degraded", async () => {
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("ts")) return ok({ lagSeconds: 600 }); // hits degraded threshold
      return ok();
    };
    const report = await new HealthAggregator(makeAllServices(), fetcher).aggregate();
    expect(report.overall).toBe("degraded");
    expect(report.services.find((s) => s.name === "timeseries")!.status).toBe("degraded");
    expect(report.services.find((s) => s.name === "risk-pipeline")!.status).toBe("ok");
    expect(report.services.find((s) => s.name === "governance-sandbox")!.status).toBe("ok");
  });

  it("two degraded, one ok → overall degraded", async () => {
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("ts"))  return ok({ lagSeconds: 600 });
      if (url.includes("rp"))  return ok({ duplicateRate: 1 });
      return ok();
    };
    const report = await new HealthAggregator(makeAllServices(), fetcher).aggregate();
    expect(report.overall).toBe("degraded");
  });

  it("one down, rest ok → overall down", async () => {
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("gov")) return ok({ simLatencyMs: 10_000 }); // hits down threshold
      return ok();
    };
    const report = await new HealthAggregator(makeAllServices(), fetcher).aggregate();
    expect(report.overall).toBe("down");
    expect(report.services.find((s) => s.name === "governance-sandbox")!.status).toBe("down");
  });

  it("one down, one degraded, one ok → overall down", async () => {
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("ts"))  return ok({ lagSeconds: 3_600 });     // down
      if (url.includes("rp"))  return ok({ duplicateRate: 1 });       // degraded
      return ok();
    };
    const report = await new HealthAggregator(makeAllServices(), fetcher).aggregate();
    expect(report.overall).toBe("down");
    expect(report.services.find((s) => s.name === "timeseries")!.status).toBe("down");
    expect(report.services.find((s) => s.name === "risk-pipeline")!.status).toBe("degraded");
    expect(report.services.find((s) => s.name === "governance-sandbox")!.status).toBe("ok");
  });

  it("service-self-reported degraded is preserved when metrics are ok", async () => {
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("gov")) return deg(); // service reports itself degraded
      return ok();
    };
    const report = await new HealthAggregator(makeAllServices(), fetcher).aggregate();
    expect(report.overall).toBe("degraded");
    expect(report.services.find((s) => s.name === "governance-sandbox")!.status).toBe("degraded");
  });
});

// ── All-timeout edge case ─────────────────────────────────────────────────────

describe("HealthAggregator — all-timeout edge case", () => {
  it("every upstream unreachable → overall down, all entries unreachable", async () => {
    const services: UpstreamService[] = [
      { name: "timeseries",         url: "http://ts/health",  sla: DEFAULT_SLAS.timeseries },
      { name: "risk-pipeline",      url: "http://rp/health",  sla: DEFAULT_SLAS["risk-pipeline"] },
      { name: "governance-sandbox", url: "http://gov/health", sla: DEFAULT_SLAS["governance-sandbox"] },
    ];
    const fetcher: HealthFetcher = async () => {
      throw new Error("ECONNREFUSED");
    };

    const report = await new HealthAggregator(services, fetcher).aggregate();

    expect(report.overall).toBe("down");
    for (const entry of report.services) {
      expect(entry.status).toBe("down");
      expect(entry.unreachable).toBe(true);
    }
  });

  it("all-timeout does not hang — resolves within 500 ms", async () => {
    const services: UpstreamService[] = [
      { name: "timeseries",    url: "http://ts/health",  sla: { timeoutMs: 50 } },
      { name: "risk-pipeline", url: "http://rp/health",  sla: { timeoutMs: 50 } },
    ];
    const fetcher: HealthFetcher = async () => {
      throw new Error("timeout");
    };

    const start = Date.now();
    await new HealthAggregator(services, fetcher).aggregate();
    expect(Date.now() - start).toBeLessThan(500);
  });
});

// ── Non-blocking probe assertion ──────────────────────────────────────────────

describe("HealthAggregator — non-blocking probes", () => {
  it("slow upstream does not block other probes from completing", async () => {
    const completionOrder: string[] = [];

    const services: UpstreamService[] = [
      { name: "fast-a", url: "http://fast-a/health", sla: { timeoutMs: 3_000 } },
      { name: "slow",   url: "http://slow/health",   sla: { timeoutMs: 3_000 } },
      { name: "fast-b", url: "http://fast-b/health", sla: { timeoutMs: 3_000 } },
    ];

    const fetcher: HealthFetcher = async (url): Promise<ServiceHealthReport> => {
      if (url.includes("slow")) {
        // Simulate a slow service that throws after a short delay.
        await new Promise<void>((r) => setTimeout(r, 50));
        completionOrder.push("slow");
        throw new Error("slow service timeout");
      }
      completionOrder.push(url.includes("fast-a") ? "fast-a" : "fast-b");
      return ok();
    };

    const report = await new HealthAggregator(services, fetcher).aggregate();

    // All three entries must be present regardless of timing.
    expect(report.services).toHaveLength(3);
    // Both fast services completed; slow one was recorded as unreachable.
    expect(completionOrder).toContain("fast-a");
    expect(completionOrder).toContain("fast-b");
    expect(report.services.find((s) => s.name === "slow")!.unreachable).toBe(true);
    expect(report.services.find((s) => s.name === "fast-a")!.status).toBe("ok");
    expect(report.services.find((s) => s.name === "fast-b")!.status).toBe("ok");
  });
});

// ── rollupStatus — additional combinations ────────────────────────────────────

describe("rollupStatus — partial-mix combinations", () => {
  const cases: [ServiceStatus[], ServiceStatus][] = [
    [["ok"],                                  "ok"],
    [["ok", "ok"],                            "ok"],
    [["degraded"],                            "degraded"],
    [["ok", "degraded"],                      "degraded"],
    [["ok", "degraded", "degraded"],          "degraded"],
    [["down"],                                "down"],
    [["ok", "down"],                          "down"],
    [["degraded", "down"],                    "down"],
    [["ok", "degraded", "down"],              "down"],
    [["down", "down", "down"],                "down"],
  ];

  for (const [input, expected] of cases) {
    it(`[${input.join(", ")}] → ${expected}`, () => {
      expect(rollupStatus(input)).toBe(expected);
    });
  }
});
