import http from "http";
import {
  RiskPipelineMetrics,
  createMetricsServer,
  riskPipelineMetrics,
} from "../src/metrics";
import { suggestScore } from "../src/index";

describe("RiskPipelineMetrics (issue #944)", () => {
  let metrics: RiskPipelineMetrics;

  beforeEach(() => {
    metrics = new RiskPipelineMetrics();
  });

  test("initial state has zeroes and valid heartbeat", () => {
    const snap = metrics.getSnapshot();
    expect(snap.jobFailures).toBe(0);
    expect(snap.executionsSuccess).toBe(0);
    expect(snap.executionsFailure).toBe(0);
    expect(snap.staleScores).toBe(0);
    expect(snap.heartbeatSeconds).toBeGreaterThan(0);
  });

  test("records success and updates counters", () => {
    metrics.recordSuccess();
    const snap = metrics.getSnapshot();
    expect(snap.executionsSuccess).toBe(1);
    expect(snap.executionsFailure).toBe(0);
    expect(snap.jobFailures).toBe(0);
    expect(snap.lastRunSeconds).toBeGreaterThan(0);
  });

  test("records failure and increments failure and job failure counters", () => {
    metrics.recordFailure();
    const snap = metrics.getSnapshot();
    expect(snap.executionsSuccess).toBe(0);
    expect(snap.executionsFailure).toBe(1);
    expect(snap.jobFailures).toBe(1);
  });

  test("sets stale scores gauge accurately", () => {
    metrics.setStaleScores(14);
    expect(metrics.getSnapshot().staleScores).toBe(14);
    metrics.setStaleScores(-5); // clamped to 0
    expect(metrics.getSnapshot().staleScores).toBe(0);
  });

  test("renders valid Prometheus text exposition format", () => {
    metrics.recordSuccess();
    metrics.recordFailure();
    metrics.setStaleScores(3);

    const rendered = metrics.render();
    expect(rendered).toContain("kora_risk_pipeline_job_failures_total 1");
    expect(rendered).toContain('kora_risk_pipeline_executions_total{status="success"} 1');
    expect(rendered).toContain('kora_risk_pipeline_executions_total{status="failure"} 1');
    expect(rendered).toContain("kora_risk_pipeline_stale_scores 3");
    expect(rendered).toContain("kora_risk_pipeline_heartbeat_timestamp_seconds");
    expect(rendered).toContain("kora_risk_pipeline_last_run_timestamp_seconds");
  });

  test("reset clears all metrics", () => {
    metrics.recordSuccess();
    metrics.recordFailure();
    metrics.setStaleScores(5);
    metrics.reset();

    const snap = metrics.getSnapshot();
    expect(snap.executionsSuccess).toBe(0);
    expect(snap.executionsFailure).toBe(0);
    expect(snap.jobFailures).toBe(0);
    expect(snap.staleScores).toBe(0);
  });
});

describe("Metrics HTTP Server (issue #944)", () => {
  let server: http.Server;
  let testMetrics: RiskPipelineMetrics;
  let port: number;

  beforeAll((done) => {
    testMetrics = new RiskPipelineMetrics();
    server = createMetricsServer(testMetrics);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address() as any;
      port = addr.port;
      done();
    });
  });

  afterAll((done) => {
    server.close(done);
  });

  test("GET /metrics returns 200 with text/plain format", (done) => {
    testMetrics.recordSuccess();
    http.get(`http://127.0.0.1:${port}/metrics`, (res) => {
      expect(res.statusCode).toBe(200);
      expect(res.headers["content-type"]).toContain("text/plain");

      let body = "";
      res.on("data", (chunk) => (body += chunk));
      res.on("end", () => {
        expect(body).toContain("kora_risk_pipeline_executions_total");
        done();
      });
    });
  });

  test("GET /health returns 200 with json status", (done) => {
    http.get(`http://127.0.0.1:${port}/health`, (res) => {
      expect(res.statusCode).toBe(200);
      expect(res.headers["content-type"]).toContain("application/json");

      let body = "";
      res.on("data", (chunk) => (body += chunk));
      res.on("end", () => {
        const json = JSON.parse(body);
        expect(json.status).toBe("ok");
        expect(json.service).toBe("risk-pipeline");
        done();
      });
    });
  });

  test("POST /metrics returns 405 Method Not Allowed", (done) => {
    const req = http.request(
      `http://127.0.0.1:${port}/metrics`,
      { method: "POST" },
      (res) => {
        expect(res.statusCode).toBe(405);
        done();
      }
    );
    req.end();
  });

  test("GET /unknown returns 404 Not Found", (done) => {
    http.get(`http://127.0.0.1:${port}/nonexistent`, (res) => {
      expect(res.statusCode).toBe(404);
      done();
    });
  });
});

describe("suggestScore instrumentation (issue #944)", () => {
  beforeEach(() => {
    riskPipelineMetrics.reset();
  });

  test("suggestScore records success metrics on valid input", () => {
    const score = suggestScore({
      sme: {
        repayments: [{ paidOnTime: true }],
        defaults: 0,
        totalInvoices: 5,
        invoicesLast90d: 1,
      },
      debtors: [{ debtorHash: "ff".repeat(32), exposureShareBps: 10000, avgDebtorScore: 40 }],
    });

    expect(score).toBeDefined();
    expect(score.score).toBeGreaterThan(0);
    expect(riskPipelineMetrics.getSnapshot().executionsSuccess).toBe(1);
    expect(riskPipelineMetrics.getSnapshot().jobFailures).toBe(0);
  });

  test("suggestScore records failure metrics when scoring encounters an invalid version", () => {
    expect(() => {
      suggestScore(
        {
          sme: { repayments: [] },
          debtors: [],
        },
        "nonexistent_formula_v99"
      );
    }).toThrow();

    expect(riskPipelineMetrics.getSnapshot().executionsFailure).toBe(1);
    expect(riskPipelineMetrics.getSnapshot().jobFailures).toBe(1);
  });
});

describe("startMetricsServer bootstrap (issue #944)", () => {
  test("startMetricsServer starts listening on specified port and closes cleanly", (done) => {
    const { startMetricsServer } = require("../src/metrics");
    const srv = startMetricsServer(0, "127.0.0.1");
    srv.on("listening", () => {
      const addr = srv.address() as any;
      expect(addr.port).toBeGreaterThan(0);
      srv.close(done);
    });
  });
});

describe("Factor describe coverage", () => {
  test("all factors provide descriptive string", () => {
    const { DEFAULT_FACTORS } = require("../src/scorer");
    for (const factor of DEFAULT_FACTORS) {
      expect(typeof factor.describe()).toBe("string");
      expect(factor.describe().length).toBeGreaterThan(0);
    }
  });

  test("validateFormula rejects invalid weight sum", () => {
    const { validateFormula } = require("../src/formulas");
    expect(() => {
      validateFormula({
        version: "bad",
        weights: { payment_history: 0.5 },
      });
    }).toThrow();
  });
});
