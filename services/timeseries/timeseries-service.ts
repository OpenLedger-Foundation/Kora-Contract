/**
 * Time-series service — orchestrates ingestion, downsampling, querying,
 * backfill, and data retention.
 *
 * Issue: #763
 */

import { downsample } from "./downsampler";
import { TimeSeriesStore } from "./store";
import {
  DEFAULT_RETENTION,
  DownsampledBucket,
  DownsampleGranularity,
  DownsampleQueryInput,
  RangeQueryInput,
  RetentionPolicy,
  SeriesKind,
  TimeSeriesPoint,
} from "./types";

const ALL_GRANULARITIES: DownsampleGranularity[] = ["5min", "hourly", "daily", "weekly"];

export class TimeSeriesService {
  constructor(
    private readonly store: TimeSeriesStore,
    private readonly retention: RetentionPolicy = DEFAULT_RETENTION
  ) {}

  // ── Ingestion ──────────────────────────────────────────────────────────────

  /**
   * Ingest a single data point.
   * Idempotent: a duplicate (same kind + subject + timestampMs) is silently
   * discarded, so backfill re-runs are safe.
   */
  async ingest(point: TimeSeriesPoint): Promise<void> {
    await this.store.insert(point);
  }

  /**
   * Batch-ingest data points — typically called by the indexer on each new
   * ledger close.  Returns the count of *new* points written (duplicates 0).
   */
  async ingestBatch(points: TimeSeriesPoint[]): Promise<number> {
    return this.store.insertBatch(points);
  }

  // ── Backfill ───────────────────────────────────────────────────────────────

  /**
   * Backfill historical data without creating duplicate points.
   * After backfill, eagerly recomputes downsampled buckets for the
   * affected time range.
   *
   * @returns Number of new points written.
   */
  async backfill(points: TimeSeriesPoint[]): Promise<number> {
    if (points.length === 0) return 0;

    const written = await this.store.insertBatch(points);

    // Group by (kind, subject) and recompute buckets for the affected range.
    const groups = new Map<string, TimeSeriesPoint[]>();
    for (const p of points) {
      const key = `${p.kind}|${p.subject}`;
      const g = groups.get(key);
      if (g) g.push(p);
      else groups.set(key, [p]);
    }

    for (const pts of groups.values()) {
      const fromMs = Math.min(...pts.map((p) => p.timestampMs));
      const toMs   = Math.max(...pts.map((p) => p.timestampMs));
      await this._rebuildBuckets(pts[0].kind as SeriesKind, pts[0].subject, fromMs, toMs);
    }

    return written;
  }

  // ── Queries ────────────────────────────────────────────────────────────────

  /** Raw range query — returns individual data points. */
  async queryRaw(input: RangeQueryInput): Promise<TimeSeriesPoint[]> {
    return this.store.queryRange(input);
  }

  /**
   * Downsampled range query — returns aggregated buckets.
   * Falls back to computing on-the-fly from raw data if no pre-computed
   * buckets exist in the requested range.
   */
  async queryDownsampled(input: DownsampleQueryInput): Promise<DownsampledBucket[]> {
    const persisted = await this.store.queryDownsampled(
      input.kind,
      input.subject,
      input.fromMs,
      input.toMs,
      input.granularity
    );

    if (persisted.length > 0) return persisted;

    // On-the-fly fallback — compute from raw data.
    const raw = await this.store.queryRange(input);
    return downsample(raw, input.granularity);
  }

  // ── Downsampling ───────────────────────────────────────────────────────────

  /**
   * Computes and persists downsampled buckets for all granularities over the
   * given range.  Called on a schedule and after backfill.
   */
  async computeDownsampledBuckets(
    kind: SeriesKind,
    subject: string,
    fromMs: number,
    toMs: number
  ): Promise<void> {
    await this._rebuildBuckets(kind, subject, fromMs, toMs);
  }

  // ── Retention enforcement ──────────────────────────────────────────────────

  /**
   * Prunes raw and downsampled data older than the configured retention windows.
   * Intended to run daily via the keeper / cron job.
   *
   * @returns Counts of pruned raw points and downsampled buckets per kind.
   */
  async enforceRetention(kinds: SeriesKind[]): Promise<Record<string, { raw: number; downsampled: number }>> {
    const now = Date.now();
    const rawCutoff = now - this.retention.rawRetentionDays * 24 * 60 * 60 * 1_000;
    const dsampCutoff = now - this.retention.downsampledRetentionDays * 24 * 60 * 60 * 1_000;

    const result: Record<string, { raw: number; downsampled: number }> = {};
    for (const kind of kinds) {
      const raw = await this.store.pruneRaw(kind, rawCutoff);
      const downsampled = await this.store.pruneDownsampled(kind, dsampCutoff);
      result[kind] = { raw, downsampled };
    }
    return result;
  }

  // ── Private helpers ────────────────────────────────────────────────────────

  private async _rebuildBuckets(
    kind: SeriesKind,
    subject: string,
    fromMs: number,
    toMs: number
  ): Promise<void> {
    const raw = await this.store.queryRange({ kind, subject, fromMs, toMs });
    if (raw.length === 0) return;

    for (const granularity of ALL_GRANULARITIES) {
      const buckets = downsample(raw, granularity);
      for (const bucket of buckets) {
        await this.store.upsertBucket(bucket);
      }
    }
  }
}
