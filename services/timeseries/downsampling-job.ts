/**
 * Downsampling Job — scheduled runner that pre-computes and persists
 * downsampled buckets for all series kinds and granularities.
 *
 * Run this on a cron schedule (e.g. every 5 minutes) to keep pre-computed
 * buckets fresh.  The job is intentionally stateless: it always queries the
 * raw store for the configured look-back window and upserts buckets, so
 * re-runs are idempotent.
 *
 * Retention is enforced at the end of each run so the job doubles as the
 * daily pruner when scheduled frequently enough.
 *
 * Issue: #763
 */

import { TimeSeriesService } from "./timeseries-service";
import { GRANULARITY_RETENTION_DAYS, SeriesKind } from "./types";

export interface DownsamplingJobConfig {
  /**
   * Series kinds to process.
   * Defaults to all three known kinds when omitted.
   */
  kinds?: SeriesKind[];

  /**
   * Subject identifiers to process for each kind.
   * Provide all active invoice IDs / SME addresses / currency pairs here.
   */
  subjects: string[];

  /**
   * How far back (ms) to look when rebuilding buckets.
   * Defaults to 2 hours so that the job always overlaps the previous run's
   * window and never misses points ingested late.
   */
  lookBackMs?: number;
}

const DEFAULT_KINDS: SeriesKind[] = ["pool-yield", "risk-score", "oracle-rate"];
const DEFAULT_LOOK_BACK_MS = 2 * 60 * 60 * 1_000; // 2 hours

export interface DownsamplingJobResult {
  ranAt: string;         // ISO-8601
  kinds: SeriesKind[];
  subjectsProcessed: number;
  retention: Record<string, { raw: number; downsampled: number }>;
}

export class DownsamplingJob {
  private readonly kinds: SeriesKind[];
  private readonly lookBackMs: number;

  constructor(
    private readonly service: TimeSeriesService,
    private readonly config: DownsamplingJobConfig,
  ) {
    this.kinds = config.kinds ?? DEFAULT_KINDS;
    this.lookBackMs = config.lookBackMs ?? DEFAULT_LOOK_BACK_MS;
  }

  /**
   * Execute one pass of the downsampling job.
   *
   * For every (kind, subject) pair:
   *   1. Rebuilds pre-computed buckets for the look-back window.
   *
   * Then enforces retention across all kinds.
   */
  async run(): Promise<DownsamplingJobResult> {
    const now = Date.now();
    const fromMs = now - this.lookBackMs;

    for (const kind of this.kinds) {
      for (const subject of this.config.subjects) {
        await this.service.computeDownsampledBuckets(kind, subject, fromMs, now);
      }
    }

    const retention = await this.service.enforceRetention(this.kinds);

    return {
      ranAt: new Date(now).toISOString(),
      kinds: this.kinds,
      subjectsProcessed: this.config.subjects.length,
      retention,
    };
  }
}

/**
 * Returns the per-granularity retention window in milliseconds.
 * Exported so callers can reason about data availability without
 * hard-coding numbers.
 */
export function granularityRetentionMs(granularity: keyof typeof GRANULARITY_RETENTION_DAYS): number {
  return GRANULARITY_RETENTION_DAYS[granularity] * 24 * 60 * 60 * 1_000;
}
