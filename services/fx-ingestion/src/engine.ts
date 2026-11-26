/**
 * FX Ingestion Service — Ingestion Engine
 *
 * Orchestrates the periodic FX ingestion cycle:
 *   1. For each configured currency pair, cross-check all sources
 *   2. If cross-check passes, publish the consensus rate to the oracle
 *   3. Log skipped pairs (unavailable or divergent) without publishing
 *
 * Issue #766
 */

import { CrossChecker } from "./crosscheck";
import { OracleRelay } from "./relay";
import { FxIngestionConfig, CurrencyCode } from "./types";
import { FxMetrics } from "./metrics";
import pino from "pino";

const logger = pino({ name: "fx-ingestion:engine" });

export class IngestionEngine {
  private timer: ReturnType<typeof setInterval> | null = null;
  private running = false;

  constructor(
    private readonly checker: CrossChecker,
    private readonly relay: OracleRelay,
    private readonly config: FxIngestionConfig,
    private readonly metrics?: FxMetrics
  ) {}

  start(): void {
    if (this.running) return;
    this.running = true;
    logger.info({ intervalMs: this.config.pollIntervalMs }, "FX ingestion engine started");
    void this._cycle();
    this.timer = setInterval(() => void this._cycle(), this.config.pollIntervalMs);
  }

  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
    this.running = false;
    logger.info("FX ingestion engine stopped");
  }

  /**
   * Run a single ingestion cycle.  Exposed for testing.
   */
  async runCycle(): Promise<void> {
    return this._cycle();
  }

  private async _cycle(): Promise<void> {
    logger.info("Starting FX ingestion cycle");
    const promises = this.config.currencies.map((base) =>
      this._processPair(base, this.config.quoteCurrency)
    );
    await Promise.allSettled(promises);
    this.metrics?.recordCycle();
    logger.info("FX ingestion cycle complete");
  }

  private async _processPair(
    base: CurrencyCode,
    quote: CurrencyCode
  ): Promise<void> {
    if (base === quote) return;

    const result = await this.checker.check(base, quote);

    if (!result.passed || result.consensusRate == null) {
      logger.warn(
        { base, quote, reason: result.reason },
        "Cross-check failed — skipping publication"
      );
      this.metrics?.recordSkip(base, quote, result.reason ?? "cross_check_failed");
      return;
    }

    // Use the most recent fetchedAt timestamp among the passing quotes
    const fetchedAt = result.quotes.reduce(
      (max, q) => Math.max(max, q.fetchedAt),
      0
    );

    const published = await this.relay.publish(
      base,
      quote,
      result.consensusRate,
      fetchedAt
    );
    if (published.skipped) {
      this.metrics?.recordSkip(base, quote, published.reason ?? "publish_skipped");
    } else {
      this.metrics?.recordPublication(base, quote, fetchedAt);
    }
  }
}
