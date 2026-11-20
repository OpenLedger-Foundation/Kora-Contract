#!/usr/bin/env node
/**
 * Kora Load Test — Report Generator
 *
 * Reads a k6 JSON output file and produces a clear, actionable text report.
 * When thresholds are breached, it identifies the bottleneck component.
 *
 * Usage:
 *   node scripts/report.js results/run.json
 *   node scripts/report.js results/run.json --output results/report.txt
 */

"use strict";

const fs   = require("fs");
const path = require("path");

const inputFile  = process.argv[2];
const outputArg  = process.argv.indexOf("--output");
const outputFile = outputArg !== -1 ? process.argv[outputArg + 1] : null;

if (!inputFile) {
  console.error("Usage: node report.js <k6-json-file> [--output <report-file>]");
  process.exit(1);
}

const raw = fs.readFileSync(inputFile, "utf8");
const lines = raw.trim().split("\n").filter(Boolean);

// k6 JSON output is newline-delimited JSON
const metrics = {};
let thresholdsPassed = true;
let metricSummary = null;

for (const line of lines) {
  let obj;
  try { obj = JSON.parse(line); } catch { continue; }

  if (obj.type === "Metric") {
    metrics[obj.data.name] = obj.data;
  }
  if (obj.type === "Point" && obj.metric === "http_req_duration") {
    // Aggregate by tag for component-level attribution
  }
  if (obj.type === "summary") {
    metricSummary = obj.data;
  }
}

// ── Build report ──────────────────────────────────────────────────────────────

const lines_out = [];
const p = (...args) => lines_out.push(args.join(" "));

p("=== Kora Protocol — Load Test Report ===");
p(`Input : ${inputFile}`);
p(`Date  : ${new Date().toISOString()}`);
p("");

if (!metricSummary) {
  p("WARNING: No summary data found in output file.");
  p("Make sure k6 was run with --out json=<file>");
  writeAndExit(1);
}

// ── Thresholds ────────────────────────────────────────────────────────────────

p("--- Threshold Results ---");
p("");

const breaches = [];
for (const [name, data] of Object.entries(metricSummary.metrics || {})) {
  if (!data.thresholds) continue;
  for (const [threshold, passed] of Object.entries(data.thresholds)) {
    const status = passed ? "✓ PASS" : "✗ FAIL";
    p(`  ${status}  ${name}: ${threshold}`);
    if (!passed) {
      thresholdsPassed = false;
      breaches.push({ metric: name, threshold });
    }
  }
}

p("");

// ── Key Metrics ───────────────────────────────────────────────────────────────

p("--- Key Metrics ---");
p("");

const printMetric = (name, label) => {
  const m = (metricSummary.metrics || {})[name];
  if (!m) return;
  const vals = m.values || {};
  p(`  ${label}`);
  if (vals.p90  !== undefined) p(`    p90  : ${vals.p90.toFixed(0)} ms`);
  if (vals.p95  !== undefined) p(`    p95  : ${vals.p95.toFixed(0)} ms`);
  if (vals.p99  !== undefined) p(`    p99  : ${vals.p99.toFixed(0)} ms`);
  if (vals.avg  !== undefined) p(`    avg  : ${vals.avg.toFixed(0)} ms`);
  if (vals.max  !== undefined) p(`    max  : ${vals.max.toFixed(0)} ms`);
  if (vals.rate !== undefined) p(`    rate : ${(vals.rate * 100).toFixed(2)}%`);
  p("");
};

printMetric("http_req_duration", "HTTP Request Duration (all)");
printMetric("http_req_failed",   "HTTP Error Rate");
printMetric("fund_contribution_duration", "Fund Contribution Duration");
printMetric("listing_load_time", "Listing Load Time");
printMetric("sse_connect_time",  "SSE Connect Time");
printMetric("keeper_metrics_scrape_duration", "Keeper Metrics Scrape Duration");
printMetric("keeper_metrics_scrape_success", "Keeper Metrics Scrape Success");
printMetric("indexer_query_duration", "Indexer Query Duration");
printMetric("indexer_query_success", "Indexer Query Success");

// ── Bottleneck Attribution ────────────────────────────────────────────────────

if (!thresholdsPassed) {
  p("--- Bottleneck Analysis ---");
  p("");
  p("One or more thresholds were BREACHED. Investigate the following:");
  p("");

  for (const { metric, threshold } of breaches) {
    p(`  ✗ ${metric}: ${threshold}`);

    if (metric.includes("fund")) {
      p("    → Likely bottleneck: API gateway write path or Stellar RPC latency");
      p("      Check: stellar-cli RPC response times, contract invocation logs");
    } else if (metric.includes("listing") || metric.includes("browse")) {
      p("    → Likely bottleneck: API gateway read path or database query");
      p("      Check: PostgreSQL slow query log, API gateway CPU/memory");
    } else if (metric.includes("sse") || metric.includes("subscription")) {
      p("    → Likely bottleneck: nginx connection limits or Node.js event loop");
      p("      Check: nginx worker_connections, Node.js heap/fd limits");
    } else if (metric === "http_req_failed") {
      p("    → Likely bottleneck: service unavailability or rate limiting");
      p("      Check: API gateway 5xx logs, rate limit headers in responses");
    } else {
      p("    → Check service logs for the affected component");
    }
    p("");
  }
} else {
  p("✓ All thresholds passed.");
}

// ── Output ────────────────────────────────────────────────────────────────────

const report = lines_out.join("\n");
console.log(report);

if (outputFile) {
  fs.mkdirSync(path.dirname(outputFile), { recursive: true });
  fs.writeFileSync(outputFile, report, "utf8");
  console.log(`\nReport written to: ${outputFile}`);
}

function writeAndExit(code) {
  const report = lines_out.join("\n");
  console.log(report);
  if (outputFile) fs.writeFileSync(outputFile, report, "utf8");
  process.exit(code);
}

process.exit(thresholdsPassed ? 0 : 1);
