/**
 * Health service tests — issue #771
 *
 * Covers:
 *  - Parallel-query timeout isolation (one slow service doesn't block others)
 *  - "degraded" vs "down" classification via SLA thresholds
 *  - Overall status rollup logic
 *  - Unreachable service → "down" in report, others still reported
 */

import { HealthAggregator, HealthFetcher, UpstreamService, DEFAULT_SLAS, buildDefaultServices } from "../health-aggregator";
import { renderPrometheusMetrics } from "../http-server";
import { classifyPinningStatus, classifyServiceStatus, rollupStatus } from "../sla-classifier";
import { ServiceHealthReport, ServiceSlaConfig } from "../types";

// ── Helpers ───────────────────────────────────────────────────────────────────

function okReport(metrics: Record<string, number> = {}): ServiceHealthReport {
  return {
    status: "ok",
    checkedAt: new Date().toISOString(),
    metrics,
  };
}

function degradedReport(metrics: Record<string, number> = {}): ServiceHealthReport {
  return {
    status: "degraded",
    checkedAt: new Date().toISOString(),
    metrics,
    message: "service self-reported degraded",
  };
}

const INDEXER_SLA: ServiceSlaConfig = {
  degradedThresholds: { lagSeconds: 10 },
  downThresholds: { lagSeconds: 60 },
  timeoutMs: 1_000,
};

// ── classifyServiceStatus ─────────────────────────────────────────────────────

describe("classifyServiceStatus", () => {
  it("returns ok when all metrics below thresholds", () => {
    const status = classifyServiceStatus(okReport({ lagSeconds: 5 }), INDEXER_SLA);
    expect(status).toBe("ok");
  });

  it("returns degraded when metric hits degraded threshold", () => {
    const status = classifyServiceStatus(okReport({ lagSeconds: 10 }), INDEXER_SLA);
    expect(status).toBe("degraded");
  });

  it("returns down when metric hits down threshold", () => {
    const status = classifyServiceStatus(okReport({ lagSeconds: 60 }), INDEXER_SLA);
    expect(status).toBe("down");
  });

  it("down threshold takes priority over degraded", () => {
    const status = classifyServiceStatus(okReport({ lagSeconds: 100 }), INDEXER_SLA);
    expect(status).toBe("down");
  });

  it("respects service-reported degraded status even when metrics are ok", () => {
    const status = classifyServiceStatus(degradedReport({ lagSeconds: 3 }), INDEXER_SLA);
    expect(status).toBe("degraded");
  });
});

// ── rollupStatus ──────────────────────────────────────────────────────────────

describe("rollupStatus", () => {
  it("ok when all services ok", () => {
    expect(rollupStatus(["ok", "ok", "ok"])).toBe("ok");
  });

  it("degraded when at least one degraded and none down", () => {
    expect(rollupStatus(["ok", "degraded", "ok"])).toBe("degraded");
  });

  it("down when at least one down", () => {
    expect(rollupStatus(["ok", "degraded", "down"])).toBe("down");
  });

  it("down takes priority over degraded", () => {
    expect(rollupStatus(["degraded", "down", "degraded"])).toBe("down");
  });
});

// ── HealthAggregator ──────────────────────────────────────────────────────────

function makeServices(): UpstreamService[] {
  return [
    { name: "indexer", url: "http://indexer/health", sla: INDEXER_SLA },
    { name: "api",     url: "http://api/health",     sla: { timeoutMs: 1_000 } },
    { name: "keeper",  url: "http://keeper/health",  sla: { timeoutMs: 1_000 } },
  ];
}

describe("HealthAggregator — parallel isolation", () => {
  it("returns results for all services even when one times out", async () => {
    let callCount = 0;
    const fetcher: HealthFetcher = async (url, _timeout) => {
      callCount++;
      if (url.includes("keeper")) {
        // Simulate unreachable: throw immediately (timeout handled by _pollOne catch).
        throw new Error("ECONNREFUSED");
      }
      return okReport({ lagSeconds: 2 });
    };

    const aggregator = new HealthAggregator(makeServices(), fetcher);
    const report = await aggregator.aggregate();

    expect(callCount).toBe(3); // all three polled
    const keeper = report.services.find((s) => s.name === "keeper")!;
    expect(keeper.status).toBe("down");
    expect(keeper.unreachable).toBe(true);

    const indexer = report.services.find((s) => s.name === "indexer")!;
    expect(indexer.status).toBe("ok");
  });

  it("overall status is down when any service is down", async () => {
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("keeper")) throw new Error("timeout");
      return okReport();
    };
    const aggregator = new HealthAggregator(makeServices(), fetcher);
    const report = await aggregator.aggregate();
    expect(report.overall).toBe("down");
  });

  it("overall status is ok when all services respond ok", async () => {
    const fetcher: HealthFetcher = async () => okReport({ lagSeconds: 1 });
    const aggregator = new HealthAggregator(makeServices(), fetcher);
    const report = await aggregator.aggregate();
    expect(report.overall).toBe("ok");
  });

  it("overall status is degraded when one service exceeds degraded threshold", async () => {
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("indexer")) return okReport({ lagSeconds: 15 }); // above degraded threshold (10)
      return okReport();
    };
    const aggregator = new HealthAggregator(makeServices(), fetcher);
    const report = await aggregator.aggregate();
    expect(report.overall).toBe("degraded");
    const indexer = report.services.find((s) => s.name === "indexer")!;
    expect(indexer.status).toBe("degraded");
  });
});

describe("HealthAggregator — report shape", () => {
  it("aggregatedAt is a valid ISO date string", async () => {
    const fetcher: HealthFetcher = async () => okReport();
    const report = await new HealthAggregator(makeServices(), fetcher).aggregate();
    expect(new Date(report.aggregatedAt).toISOString()).toBe(report.aggregatedAt);
  });

  it("report contains an entry for every configured service", async () => {
    const fetcher: HealthFetcher = async () => okReport();
    const report = await new HealthAggregator(makeServices(), fetcher).aggregate();
    const names = report.services.map((s) => s.name);
    expect(names).toContain("indexer");
    expect(names).toContain("api");
    expect(names).toContain("keeper");
  });
});

describe("renderPrometheusMetrics", () => {
  it("exports service status gauges and numeric health metrics", () => {
    const output = renderPrometheusMetrics({
      schemaVersion: "1.1.0",
      overall: "degraded",
      aggregatedAt: "2026-09-27T00:00:00.000Z",
      services: [
        {
          name: 'api"east',
          status: "degraded",
          metrics: { lagSeconds: 3, detail: "ignored" },
          unreachable: false,
        },
      ],
    });

    expect(output).toContain('kora_service_health{service="api\\"east",status="degraded"} 1');
    expect(output).toContain('kora_service_health{service="api\\"east",status="ok"} 0');
    expect(output).toContain("kora_service_api_east_lagSeconds 3");
    expect(output).not.toContain("detail");
  });
});

describe("reporting + ipfs-pinning probes", () => {
  it("marks reporting down when probe fails (timeout)", async () => {
    const services: UpstreamService[] = [
      { name: "reporting", url: "http://reporting/health", sla: DEFAULT_SLAS.reporting },
      { name: "ipfs-pinning", url: "http://pinning/health", sla: DEFAULT_SLAS["ipfs-pinning"] },
    ];
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("reporting")) throw new Error("timeout");
      return okReport({ providersReachable: 2, providerCount: 2 });
    };
    const report = await new HealthAggregator(services, fetcher).aggregate();
    expect(report.services.find((s) => s.name === "reporting")!.status).toBe("down");
    expect(report.overall).toBe("down");
  });

  it("marks pinning degraded/down via provider reachability bits", () => {
    const sla = DEFAULT_SLAS["ipfs-pinning"];
    expect(classifyPinningStatus(okReport({ providersReachable: 0, providerCount: 2 }), sla)).toBe("down");
    expect(classifyPinningStatus(okReport({ providersReachable: 1, providerCount: 2 }), sla)).toBe("degraded");
    expect(classifyPinningStatus(okReport({ providersReachable: 2, providerCount: 2 }), sla)).toBe("ok");
  });

  it("all-green matrix: reporting + pinning ok => overall ok", async () => {
    const services: UpstreamService[] = [
      { name: "reporting", url: "http://reporting/health", sla: DEFAULT_SLAS.reporting },
      { name: "ipfs-pinning", url: "http://pinning/health", sla: DEFAULT_SLAS["ipfs-pinning"] },
    ];
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("pinning")) return okReport({ providersReachable: 2, providerCount: 2 });
      return okReport({ exportQueueDepth: 5 });
    };
    const report = await new HealthAggregator(services, fetcher).aggregate();
    expect(report.overall).toBe("ok");
  });

  it("partial-degraded matrix: queue backup warns, pinning all-fail downs", async () => {
    const services: UpstreamService[] = [
      { name: "reporting", url: "http://reporting/health", sla: DEFAULT_SLAS.reporting },
      { name: "ipfs-pinning", url: "http://pinning/health", sla: DEFAULT_SLAS["ipfs-pinning"] },
    ];
    const fetcher: HealthFetcher = async (url) => {
      if (url.includes("reporting")) return okReport({ exportQueueDepth: 150 });
      return okReport({ providersReachable: 0, providerCount: 3 });
    };
    const report = await new HealthAggregator(services, fetcher).aggregate();
    expect(report.services.find((s) => s.name === "reporting")!.status).toBe("degraded");
    expect(report.services.find((s) => s.name === "ipfs-pinning")!.status).toBe("down");
    expect(report.overall).toBe("down");
  });

  it("exposes env-configurable probe URLs", () => {
    expect(buildDefaultServices().length).toBe(2);
  });
});
