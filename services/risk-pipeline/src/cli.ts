import { riskPipelineMetrics, startMetricsServer } from "./metrics";

const port = parseInt(process.env.METRICS_PORT || process.env.PORT || "8081", 10);
const host = process.env.METRICS_HOST || "0.0.0.0";
const server = startMetricsServer(port, host);
console.log(`[risk-pipeline] Metrics server listening on ${host}:${port}`);

const heartbeatInterval = setInterval(() => {
  riskPipelineMetrics.recordHeartbeat();
}, 15000);

const shutdown = () => {
  clearInterval(heartbeatInterval);
  server.close(() => {
    process.exit(0);
  });
};

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
