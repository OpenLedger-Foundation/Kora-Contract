import http from "http";
import {
  GovernanceSyncMetrics,
  createMetricsServer,
  startMetricsServer,
} from "../src/metrics";

describe("GovernanceSyncMetrics", () => {
  let metrics: GovernanceSyncMetrics;

  beforeEach(() => {
    metrics = new GovernanceSyncMetrics();
  });

  it("should initialize with default zero counters and recent heartbeat", () => {
    const snapshot = metrics.getSnapshot();
    expect(snapshot.rpcErrors).toBe(0);
    expect(snapshot.syncErrors).toBe(0);
    expect(snapshot.proposalsSynced).toBe(0);
    expect(snapshot.proposalsFailed).toBe(0);
    expect(snapshot.lastPollSeconds).toBe(0);
    expect(snapshot.heartbeatSeconds).toBeGreaterThan(0);
  });

  it("should record RPC error and increment both rpcErrors and syncErrors", () => {
    metrics.recordRpcError();
    const snapshot = metrics.getSnapshot();
    expect(snapshot.rpcErrors).toBe(1);
    expect(snapshot.syncErrors).toBe(1);
  });

  it("should record sync error and increment syncErrors only", () => {
    metrics.recordSyncError();
    const snapshot = metrics.getSnapshot();
    expect(snapshot.rpcErrors).toBe(0);
    expect(snapshot.syncErrors).toBe(1);
  });

  it("should record proposal sync successes and failures", () => {
    metrics.recordProposalSync("synced");
    metrics.recordProposalSync("synced");
    metrics.recordProposalSync("failed");

    const snapshot = metrics.getSnapshot();
    expect(snapshot.proposalsSynced).toBe(2);
    expect(snapshot.proposalsFailed).toBe(1);
  });

  it("should record poll timestamp", () => {
    const before = Math.floor(Date.now() / 1000);
    metrics.recordPoll();
    const snapshot = metrics.getSnapshot();
    expect(snapshot.lastPollSeconds).toBeGreaterThanOrEqual(before);
  });

  it("should record heartbeat timestamp", () => {
    const before = Math.floor(Date.now() / 1000);
    metrics.recordHeartbeat();
    const snapshot = metrics.getSnapshot();
    expect(snapshot.heartbeatSeconds).toBeGreaterThanOrEqual(before);
  });

  it("should reset all counters", () => {
    metrics.recordRpcError();
    metrics.recordProposalSync("synced");
    metrics.recordProposalSync("failed");
    metrics.recordPoll();

    metrics.reset();
    const snapshot = metrics.getSnapshot();
    expect(snapshot.rpcErrors).toBe(0);
    expect(snapshot.syncErrors).toBe(0);
    expect(snapshot.proposalsSynced).toBe(0);
    expect(snapshot.proposalsFailed).toBe(0);
    expect(snapshot.lastPollSeconds).toBe(0);
  });

  it("should render compliant Prometheus exposition text", () => {
    metrics.recordRpcError();
    metrics.recordProposalSync("synced");
    metrics.recordProposalSync("failed");
    metrics.recordPoll();

    const output = metrics.render();
    expect(output).toContain("# HELP kora_governance_sync_rpc_errors_total");
    expect(output).toContain("# TYPE kora_governance_sync_rpc_errors_total counter");
    expect(output).toContain("kora_governance_sync_rpc_errors_total 1");

    expect(output).toContain("# HELP kora_governance_sync_errors_total");
    expect(output).toContain("kora_governance_sync_errors_total 1");

    expect(output).toContain('kora_governance_sync_proposals_total{status="synced"} 1');
    expect(output).toContain('kora_governance_sync_proposals_total{status="failed"} 1');

    expect(output).toContain("# HELP kora_governance_sync_last_poll_timestamp_seconds");
    expect(output).toContain("kora_governance_sync_last_poll_timestamp_seconds ");

    expect(output).toContain("# HELP kora_governance_sync_heartbeat_timestamp_seconds");
    expect(output).toContain("kora_governance_sync_heartbeat_timestamp_seconds ");
  });
});

describe("Governance Sync Metrics HTTP Server", () => {
  let server: http.Server;
  let testPort: number;
  let metrics: GovernanceSyncMetrics;

  beforeEach((done) => {
    metrics = new GovernanceSyncMetrics();
    server = createMetricsServer(metrics);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      if (typeof addr === "object" && addr !== null) {
        testPort = addr.port;
      }
      done();
    });
  });

  afterEach((done) => {
    if (server && server.listening) {
      server.close(done);
    } else {
      done();
    }
  });

  it("should serve /metrics with Prometheus text format", async () => {
    metrics.recordProposalSync("synced");

    const response = await fetch(`http://127.0.0.1:${testPort}/metrics`);
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toContain("text/plain");

    const body = await response.text();
    expect(body).toContain('kora_governance_sync_proposals_total{status="synced"} 1');
  });

  it("should serve /health with JSON status ok", async () => {
    const response = await fetch(`http://127.0.0.1:${testPort}/health`);
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toContain("application/json");

    const body = (await response.json()) as { status: string; service: string; uptime: number };
    expect(body.status).toBe("ok");
    expect(body.service).toBe("governance-sync");
    expect(typeof body.uptime).toBe("number");
  });

  it("should return 405 for non-GET requests", async () => {
    const response = await fetch(`http://127.0.0.1:${testPort}/metrics`, {
      method: "POST",
    });
    expect(response.status).toBe(405);
  });

  it("should return 404 for unknown endpoints", async () => {
    const response = await fetch(`http://127.0.0.1:${testPort}/not-found`);
    expect(response.status).toBe(404);
  });

  it("should start via startMetricsServer helper with explicit params", (done) => {
    const srv = startMetricsServer(0, "127.0.0.1", metrics);
    srv.on("listening", () => {
      expect(srv.listening).toBe(true);
      srv.close(done);
    });
  });

  it("should support default parameters and environment variables in startMetricsServer and createMetricsServer", (done) => {
    const defaultSrv = createMetricsServer();
    expect(defaultSrv).toBeDefined();

    const origPort = process.env.METRICS_PORT;
    const origHost = process.env.METRICS_HOST;
    process.env.METRICS_PORT = "0";
    process.env.METRICS_HOST = "127.0.0.1";

    const srv = startMetricsServer();
    srv.on("listening", () => {
      expect(srv.listening).toBe(true);
      process.env.METRICS_PORT = origPort;
      process.env.METRICS_HOST = origHost;
      srv.close(done);
    });
  });
});
