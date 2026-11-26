/**
 * FX Ingestion — Prometheus staleness metrics.
 *
 * Exports gauges consumed by monitoring alerts so operators detect when an FX
 * pair has not been successfully ingested within the staleness budget. The
 * primary gauge, `kora_fx_rate_age_seconds{base,quote}`, is the number of
 * seconds since the last successful publication for that pair. With no samples
 * yet the gauge is omitted and a comment is emitted (matching the indexer lag
 * convention) so a missing series — not a bogus zero — drives alerts.
 *
 * Issue: #<fx-staleness>
 */

import { createServer, type Server } from "http";
import { CurrencyCode } from "./types";

export interface FxMetricsOptions {
  /** Injectable clock (ms). Defaults to Date.now. */
  now?: () => number;
}

interface PairState {
  /** Unix seconds of the last successful publication. */
  lastIngestionTs: number;
}

/**
 * In-process Prometheus metrics registry for the FX ingestion service.
 *
 * Thread-safe enough for the single-threaded Node event loop; all mutations
 * happen on the ingestion cycle or the scrape handler, both of which run on the
 * loop. Fail-closed: a pair with no successful sample is never reported with a
 * zero age.
 */
export class FxMetrics {
  private pairs = new Map<string, PairState>();
  private cycles = 0;
  private skippedByReason = new Map<string, number>();
  private readonly now: () => number;

  constructor(options: FxMetricsOptions = {}) {
    this.now = options.now ?? Date.now;
  }

  private pairKey(base: CurrencyCode, quote: CurrencyCode): string {
    return `${base}/${quote}`;
  }

  /**
   * Record a successful publication for `base`/`quote`. The ingestion timestamp
   * is taken from the clock so the age gauge reflects wall-clock staleness.
   */
  recordPublication(
    base: CurrencyCode,
    quote: CurrencyCode,
    _fetchedAt: number,
  ): void {
    this.pairs.set(this.pairKey(base, quote), {
      lastIngestionTs: Math.floor(this.now() / 1000),
    });
  }

  /** Record a skipped publication for `base`/`quote` with a reason label. */
  recordSkip(base: CurrencyCode, quote: CurrencyCode, reason: string): void {
    void base;
    void quote;
    const r = reason || "unknown";
    this.skippedByReason.set(r, (this.skippedByReason.get(r) ?? 0) + 1);
  }

  /** Increment the ingestion-cycle counter. */
  recordCycle(): void {
    this.cycles += 1;
  }

  /** Seconds since the last successful publication for a pair, or null if none. */
  ageSeconds(base: CurrencyCode, quote: CurrencyCode): number | null {
    const state = this.pairs.get(this.pairKey(base, quote));
    if (!state) return null;
    return Math.floor(this.now() / 1000) - state.lastIngestionTs;
  }

  /** Render the registry as Prometheus 0.0.4 text. */
  render(): string {
    const nowSec = Math.floor(this.now() / 1000);
    const lines: string[] = [];

    lines.push(
      "# HELP kora_fx_rate_age_seconds Seconds since the last successful FX rate publication for a pair.",
    );
    lines.push("# TYPE kora_fx_rate_age_seconds gauge");
    if (this.pairs.size === 0) {
      // No samples yet: emit a comment rather than a bogus zero so the missing
      // series (not 0) drives the staleness alert.
      lines.push(
        "# No FX rate samples ingested yet — staleness gauges omitted to avoid a bogus zero.",
      );
    } else {
      for (const [key, state] of this.pairs) {
        const [base, quote] = key.split("/");
        const age = nowSec - state.lastIngestionTs;
        lines.push(
          `kora_fx_rate_age_seconds{base="${base}",quote="${quote}"} ${age}`,
        );
      }
    }

    lines.push(
      "# HELP kora_fx_last_ingestion_timestamp_seconds Unix timestamp of the last successful FX rate publication.",
    );
    lines.push("# TYPE kora_fx_last_ingestion_timestamp_seconds gauge");
    for (const [key, state] of this.pairs) {
      const [base, quote] = key.split("/");
      lines.push(
        `kora_fx_last_ingestion_timestamp_seconds{base="${base}",quote="${quote}"} ${state.lastIngestionTs}`,
      );
    }

    lines.push("# HELP kora_fx_ingestion_cycles_total Total FX ingestion cycles completed.");
    lines.push("# TYPE kora_fx_ingestion_cycles_total counter");
    lines.push(`kora_fx_ingestion_cycles_total ${this.cycles}`);

    lines.push("# HELP kora_fx_publication_skipped_total FX rate publications skipped, by reason.");
    lines.push("# TYPE kora_fx_publication_skipped_total counter");
    for (const [reason, count] of this.skippedByReason) {
      lines.push(
        `kora_fx_publication_skipped_total{reason="${reason}"} ${count}`,
      );
    }

    return lines.join("\n") + "\n";
  }
}

/**
 * Read the metrics scrape port from the environment. Defaults to 8081; throws
 * on non-integer or out-of-range values so misconfiguration fails at startup.
 */
export function readFxMetricsPort(
  raw = process.env["FX_METRICS_PORT"],
): number {
  if (raw === undefined || raw === "") return 8081;
  const n = Number(raw);
  if (!Number.isInteger(n) || n <= 0 || n > 65535) {
    throw new Error("FX_METRICS_PORT must be an integer in 1..65535");
  }
  return n;
}

/**
 * Start a minimal HTTP server exposing `/metrics` (Prometheus scrape) and
 * `/health`. Returns the server handle for graceful shutdown.
 */
export function startMetricsServer(metrics: FxMetrics, port: number): Server {
  const server = createServer((req, res) => {
    if (req.url === "/metrics") {
      res.writeHead(200, {
        "Content-Type": "text/plain; version=0.0.4; charset=utf-8",
      });
      res.end(metrics.render());
      return;
    }
    if (req.url === "/health") {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ status: "ok" }));
      return;
    }
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ error: "not found" }));
  });
  server.listen(port);
  return server;
}
