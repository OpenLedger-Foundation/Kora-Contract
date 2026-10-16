/**
 * Governance Sync — Prometheus Metrics Exposition
 *
 * Exposes operational telemetry for the governance-to-forum sync bridge:
 * - kora_governance_sync_rpc_errors_total: Counter of Soroban RPC failures.
 * - kora_governance_sync_errors_total: Counter of sync loop failures.
 * - kora_governance_sync_proposals_total: Counter of proposals processed by status (synced/failed).
 * - kora_governance_sync_last_poll_timestamp_seconds: Unix timestamp of the last contract poll.
 * - kora_governance_sync_heartbeat_timestamp_seconds: Unix timestamp of the last service heartbeat.
 *
 * Issue #944
 */

import http from "http";

export class GovernanceSyncMetrics {
  private rpcErrors = 0;
  private syncErrors = 0;
  private proposalsSynced = 0;
  private proposalsFailed = 0;
  private lastPollSeconds = 0;
  private heartbeatSeconds = Math.floor(Date.now() / 1000);

  /** Record a Soroban RPC error */
  recordRpcError(): void {
    this.rpcErrors++;
    this.syncErrors++;
    this.recordHeartbeat();
  }

  /** Record a general sync loop error */
  recordSyncError(): void {
    this.syncErrors++;
    this.recordHeartbeat();
  }

  /** Record proposal sync attempt result */
  recordProposalSync(status: "synced" | "failed"): void {
    if (status === "synced") {
      this.proposalsSynced++;
    } else {
      this.proposalsFailed++;
    }
    this.recordHeartbeat();
  }

  /** Record completion of a polling loop cycle */
  recordPoll(): void {
    this.lastPollSeconds = Math.floor(Date.now() / 1000);
    this.recordHeartbeat();
  }

  /** Record service heartbeat */
  recordHeartbeat(): void {
    this.heartbeatSeconds = Math.floor(Date.now() / 1000);
  }

  /** Current snapshot values */
  getSnapshot() {
    return {
      rpcErrors: this.rpcErrors,
      syncErrors: this.syncErrors,
      proposalsSynced: this.proposalsSynced,
      proposalsFailed: this.proposalsFailed,
      lastPollSeconds: this.lastPollSeconds,
      heartbeatSeconds: this.heartbeatSeconds,
    };
  }

  /** Reset all metrics counters */
  reset(): void {
    this.rpcErrors = 0;
    this.syncErrors = 0;
    this.proposalsSynced = 0;
    this.proposalsFailed = 0;
    this.lastPollSeconds = 0;
    this.heartbeatSeconds = Math.floor(Date.now() / 1000);
  }

  /** Render Prometheus text exposition format (version 0.0.4) */
  render(): string {
    const lines: string[] = [
      "# HELP kora_governance_sync_rpc_errors_total Total number of Soroban RPC query errors encountered.",
      "# TYPE kora_governance_sync_rpc_errors_total counter",
      `kora_governance_sync_rpc_errors_total ${this.rpcErrors}`,
      "",
      "# HELP kora_governance_sync_errors_total Total number of governance sync loop errors.",
      "# TYPE kora_governance_sync_errors_total counter",
      `kora_governance_sync_errors_total ${this.syncErrors}`,
      "",
      "# HELP kora_governance_sync_proposals_total Total governance proposals processed by sync status.",
      "# TYPE kora_governance_sync_proposals_total counter",
      `kora_governance_sync_proposals_total{status="synced"} ${this.proposalsSynced}`,
      `kora_governance_sync_proposals_total{status="failed"} ${this.proposalsFailed}`,
      "",
      "# HELP kora_governance_sync_last_poll_timestamp_seconds Unix timestamp of the most recent contract poll.",
      "# TYPE kora_governance_sync_last_poll_timestamp_seconds gauge",
      `kora_governance_sync_last_poll_timestamp_seconds ${this.lastPollSeconds}`,
      "",
      "# HELP kora_governance_sync_heartbeat_timestamp_seconds Unix timestamp of the most recent service heartbeat.",
      "# TYPE kora_governance_sync_heartbeat_timestamp_seconds gauge",
      `kora_governance_sync_heartbeat_timestamp_seconds ${this.heartbeatSeconds}`,
    ];

    return `${lines.join("\n")}\n`;
  }
}

export const governanceSyncMetrics = new GovernanceSyncMetrics();

/**
 * Creates an HTTP server serving /metrics and /health endpoints.
 */
export function createMetricsServer(
  metricsInstance: GovernanceSyncMetrics = governanceSyncMetrics
): http.Server {
  return http.createServer((req, res) => {
    if (req.method !== "GET") {
      res.writeHead(405, { "Content-Type": "text/plain" });
      res.end("Method Not Allowed\n");
      return;
    }

    if (req.url === "/metrics") {
      res.writeHead(200, {
        "Content-Type": "text/plain; version=0.0.4; charset=utf-8",
      });
      res.end(metricsInstance.render());
      return;
    }

    if (req.url === "/health") {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(
        JSON.stringify({
          status: "ok",
          service: "governance-sync",
          uptime: process.uptime(),
          timestamp: new Date().toISOString(),
        })
      );
      return;
    }

    res.writeHead(404, { "Content-Type": "text/plain" });
    res.end("Not Found\n");
  });
}

export function startMetricsServer(
  port?: number,
  host?: string,
  metricsInstance?: GovernanceSyncMetrics
): http.Server {
  const actualPort =
    port !== undefined ? port : parseInt(process.env.METRICS_PORT || "8082", 10);
  const actualHost =
    host !== undefined ? host : process.env.METRICS_HOST || "0.0.0.0";
  const actualMetrics =
    metricsInstance !== undefined ? metricsInstance : governanceSyncMetrics;
  const server = createMetricsServer(actualMetrics);
  server.listen(actualPort, actualHost);
  return server;
}
