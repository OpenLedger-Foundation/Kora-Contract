/** HTTP adapter for the health aggregation library. */

import * as http from "http";
import { HealthAggregator, HEALTH_SCHEMA_VERSION } from "./health-aggregator";
import { AggregatedHealthReport } from "./types";

function escapeLabel(value: string): string {
  return value.replace(/\\/g, "\\\\").replace(/\n/g, "\\n").replace(/"/g, '\\"');
}

function metricName(value: string): string {
  const normalized = value.replace(/[^a-zA-Z0-9_:]/g, "_");
  return /^[a-zA-Z_:]/.test(normalized) ? normalized : `_${normalized}`;
}

export function renderPrometheusMetrics(report: AggregatedHealthReport): string {
  const lines = [
    "# HELP kora_service_health Service health state, represented by one active status gauge.",
    "# TYPE kora_service_health gauge",
  ];

  for (const service of report.services) {
    for (const status of ["ok", "degraded", "down"] as const) {
      lines.push(
        `kora_service_health{service="${escapeLabel(service.name)}",status="${status}"} ${service.status === status ? 1 : 0}`
      );
    }

    for (const [name, value] of Object.entries(service.metrics)) {
      if (typeof value !== "number" || !Number.isFinite(value)) continue;
      const metric = metricName(`kora_service_${service.name}_${name}`);
      lines.push(`${metric} ${value}`);
    }
  }

  return `${lines.join("\n")}\n`;
}

export function createHealthHttpServer(aggregator: HealthAggregator): http.Server {
  return http.createServer(async (request, response) => {
    if (request.method !== "GET" || (request.url !== "/health" && request.url !== "/metrics")) {
      response.writeHead(404);
      response.end();
      return;
    }

    try {
      const report = await aggregator.aggregate();
      report.schemaVersion = (report as Partial<AggregatedHealthReport>).schemaVersion ?? HEALTH_SCHEMA_VERSION;
      if (request.url === "/metrics") {
        response.writeHead(200, { "Content-Type": "text/plain; version=0.0.4; charset=utf-8" });
        response.end(renderPrometheusMetrics(report));
        return;
      }

      response.writeHead(report.overall === "down" ? 503 : 200, {
        "Content-Type": "application/json; charset=utf-8",
      });
      response.end(JSON.stringify(report));
    } catch {
      response.writeHead(503, { "Content-Type": "application/json; charset=utf-8" });
      response.end(JSON.stringify({ overall: "down", services: [] }));
    }
  });
}