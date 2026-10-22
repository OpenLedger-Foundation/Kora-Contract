/**
 * Health service tests — issue #771
 *
 * Covers:
 *  - Parallel-query timeout isolation (one slow service doesn't block others)
 *  - "degraded" vs "down" classification via SLA thresholds
 *  - Overall status rollup logic
 *  - Unreachable service → "down" in report, others still reported
 */

import { HealthAggregator, HealthFetcher, UpstreamService } from "../health-aggregator";
import { createHealthHttpServer, renderPrometheusMetrics } from "../http-server";
import { classifyServiceStatus, rollupStatus } from "../sla-classifier";
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

describe("createHealthHttpServer — smoke (#1024)", () => {
  function get(url: string): Promise<{ statusCode: number; body: string }> {
    return new Promise((resolve, reject) => {
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const http = require("http") as typeof import("http");
      http
        .get(url, (res) => {
          let body = "";
          res.on("data", (chunk) => (body += chunk));
          res.on("end", () => resolve({ statusCode: res.statusCode ?? 0, body }));
        })
        .on("error", reject);
    });
  }

  it("serves 200 /health JSON and /metrics text from the factory server", async () => {
    const aggregator = new HealthAggregator(
      [{ name: "api", url: "http://api/health", sla: { timeoutMs: 1_000 } }],
      async () => okReport({ lagSeconds: 1 })
    );
    const server = createHealthHttpServer(aggregator);
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const address = server.address();
    const port = typeof address === "object" && address !== null ? address.port : 0;
    expect(port).toBeGreaterThan(0);
    try {
      const health = await get(`http://127.0.0.1:${port}/health`);
      expect(health.statusCode).toBe(200);
      expect(JSON.parse(health.body).overall).toBe("ok");

      const metrics = await get(`http://127.0.0.1:${port}/metrics`);
      expect(metrics.statusCode).toBe(200);
      expect(metrics.body).toContain("kora_service_health");
    } finally {
      await new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve()))
      );
    }
  });

  it("records an unreachable upstream as down without hanging the aggregate", async () => {
    const aggregator = new HealthAggregator(
      [{ name: "keeper", url: "http://keeper/health", sla: { timeoutMs: 50 } }],
      async () => {
        throw new Error("ECONNREFUSED");
      }
    );
    const server = createHealthHttpServer(aggregator);
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const address = server.address();
    const port = typeof address === "object" && address !== null ? address.port : 0;
    try {
      const health = await get(`http://127.0.0.1:${port}/health`);
      expect(health.statusCode).toBe(503);
      expect(JSON.parse(health.body).overall).toBe("down");
    } finally {
      await new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve()))
      );
    }
  });
});
