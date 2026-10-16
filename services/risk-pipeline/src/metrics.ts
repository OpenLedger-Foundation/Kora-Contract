/**
 * Risk Pipeline — Prometheus Metrics Exposition
 *
 * Exposes operational telemetry for the risk scoring data pipeline:
 * - kora_risk_pipeline_job_failures_total: Counter of scoring execution failures.
 * - kora_risk_pipeline_executions_total: Counter of score calculations by status (success/failure).
 * - kora_risk_pipeline_stale_scores: Gauge of SME profiles with scores older than staleness threshold.
 * - kora_risk_pipeline_heartbeat_timestamp_seconds: Gauge of last service heartbeat timestamp.
 * - kora_risk_pipeline_last_run_timestamp_seconds: Gauge of last scoring run completion.
 *
 * Issue #944
 */

import http from "http";

export class RiskPipelineMetrics {
  private jobFailures = 0;
  private executionsSuccess = 0;
  private executionsFailure = 0;
  private staleScoreCount = 0;
  private lastRunSeconds = 0;
  private heartbeatSeconds = Math.floor(Date.now() / 1000);

  /** Record a successful score calculation */
  recordSuccess(): void {
    this.executionsSuccess++;
    this.lastRunSeconds = Math.floor(Date.now() / 1000);
    this.recordHeartbeat();
  }

  /** Record a failed scoring job evaluation */
  recordFailure(): void {
    this.executionsFailure++;
    this.jobFailures++;
    this.lastRunSeconds = Math.floor(Date.now() / 1000);
    this.recordHeartbeat();
  }

  /** Update current number of stale scores awaiting recomputation */
  setStaleScores(count: number): void {
    this.staleScoreCount = Math.max(0, count);
  }

  /** Update heartbeat timestamp */
  recordHeartbeat(): void {
    this.heartbeatSeconds = Math.floor(Date.now() / 1000);
  }

  /** Current snapshot values */
  getSnapshot() {
    return {
      jobFailures: this.jobFailures,
      executionsSuccess: this.executionsSuccess,
      executionsFailure: this.executionsFailure,
      staleScores: this.staleScoreCount,
      lastRunSeconds: this.lastRunSeconds,
      heartbeatSeconds: this.heartbeatSeconds,
    };
  }

  /** Reset all counters (primarily for unit tests) */
  reset(): void {
    this.jobFailures = 0;
    this.executionsSuccess = 0;
    this.executionsFailure = 0;
    this.staleScoreCount = 0;
    this.lastRunSeconds = 0;
    this.heartbeatSeconds = Math.floor(Date.now() / 1000);
  }

  /** Render Prometheus text exposition format (version 0.0.4) */
  render(): string {
    const lines: string[] = [
      "# HELP kora_risk_pipeline_job_failures_total Total number of risk pipeline job evaluation failures.",
      "# TYPE kora_risk_pipeline_job_failures_total counter",
      `kora_risk_pipeline_job_failures_total ${this.jobFailures}`,
      "",
      "# HELP kora_risk_pipeline_executions_total Total risk score evaluations by status.",
      "# TYPE kora_risk_pipeline_executions_total counter",
      `kora_risk_pipeline_executions_total{status="success"} ${this.executionsSuccess}`,
      `kora_risk_pipeline_executions_total{status="failure"} ${this.executionsFailure}`,
      "",
      "# HELP kora_risk_pipeline_stale_scores Current number of evaluated SME scores exceeding staleness threshold.",
      "# TYPE kora_risk_pipeline_stale_scores gauge",
      `kora_risk_pipeline_stale_scores ${this.staleScoreCount}`,
      "",
      "# HELP kora_risk_pipeline_last_run_timestamp_seconds Unix timestamp of the most recent scoring run.",
      "# TYPE kora_risk_pipeline_last_run_timestamp_seconds gauge",
      `kora_risk_pipeline_last_run_timestamp_seconds ${this.lastRunSeconds}`,
      "",
      "# HELP kora_risk_pipeline_heartbeat_timestamp_seconds Unix timestamp of the most recent service heartbeat.",
      "# TYPE kora_risk_pipeline_heartbeat_timestamp_seconds gauge",
      `kora_risk_pipeline_heartbeat_timestamp_seconds ${this.heartbeatSeconds}`,
    ];

    return `${lines.join("\n")}\n`;
  }
}

export const riskPipelineMetrics = new RiskPipelineMetrics();

/**
 * Creates an HTTP server serving /metrics and /health endpoints.
 */
export function createMetricsServer(
  metricsInstance: RiskPipelineMetrics = riskPipelineMetrics
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
          service: "risk-pipeline",
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

/**
 * Starts metrics HTTP server listening on configured port (default: 8081).
 */
export function startMetricsServer(
  port = parseInt(process.env.METRICS_PORT || "8081", 10),
  host = process.env.METRICS_HOST || "0.0.0.0",
  metricsInstance: RiskPipelineMetrics = riskPipelineMetrics
): http.Server {
  const server = createMetricsServer(metricsInstance);
  server.listen(port, host);
  return server;
}
