/**
 * Downsampler — computes aggregate buckets (min/max/avg) from raw points.
 *
 * The downsampler is called by the time-series service on a scheduled basis
 * (e.g., every hour for daily buckets) and also eagerly after a backfill run.
 * It preserves min/max/avg so coarser granularity can always reconstruct
 * key statistics.
 *
 * Issue: #763
 */

import {
  DownsampledBucket,
  DownsampleGranularity,
  SeriesKind,
  TimeSeriesPoint,
} from "./types";

// ── Bucket window helpers ─────────────────────────────────────────────────────

const MS_PER_5MIN  = 5  * 60 * 1_000;
const MS_PER_HOUR = 60 * 60 * 1_000;
const MS_PER_DAY  = 24 * MS_PER_HOUR;
const MS_PER_WEEK = 7  * MS_PER_DAY;

export function bucketWindowMs(granularity: DownsampleGranularity): number {
  switch (granularity) {
    case "5min":   return MS_PER_5MIN;
    case "hourly": return MS_PER_HOUR;
    case "daily":  return MS_PER_DAY;
    case "weekly": return MS_PER_WEEK;
  }
}

/**
 * Returns the start timestamp of the bucket that contains `timestampMs`
 * for the given granularity (always aligned to UTC midnight / week start).
 */
export function bucketStart(timestampMs: number, granularity: DownsampleGranularity): number {
  const windowMs = bucketWindowMs(granularity);
  return Math.floor(timestampMs / windowMs) * windowMs;
}

// ── Downsampler ───────────────────────────────────────────────────────────────

/**
 * Groups raw data points by bucket window and computes min/max/avg per bucket.
 *
 * Returns only buckets that contain at least one data point.
 * Does NOT persist anything — persistence is the caller's responsibility.
 */
export function downsample(
  points: TimeSeriesPoint[],
  granularity: DownsampleGranularity
): DownsampledBucket[] {
  if (points.length === 0) return [];

  // All points must share the same kind + subject.
  const kind = points[0].kind as SeriesKind;
  const subject = points[0].subject;

  // Group by bucket start.
  const groups = new Map<number, number[]>();
  for (const p of points) {
    const start = bucketStart(p.timestampMs, granularity);
    const bucket = groups.get(start);
    if (bucket) {
      bucket.push(p.value);
    } else {
      groups.set(start, [p.value]);
    }
  }

  const buckets: DownsampledBucket[] = [];
  for (const [bucketStartMs, values] of groups) {
    const min = Math.min(...values);
    const max = Math.max(...values);
    const avg = values.reduce((sum, v) => sum + v, 0) / values.length;
    buckets.push({
      kind,
      subject,
      bucketStartMs,
      granularity,
      min,
      max,
      avg,
      count: values.length,
    });
  }

  return buckets.sort((a, b) => a.bucketStartMs - b.bucketStartMs);
}
