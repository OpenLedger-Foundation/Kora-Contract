/**
 * Time-Series Service — shared types
 *
 * Defines the core data model for per-pool yield history, risk-score history,
 * and oracle rate history.  Designed for efficient range queries and
 * downsampling at daily/weekly granularity.
 *
 * Issue: #763
 */

// ── Series types ──────────────────────────────────────────────────────────────

export type SeriesKind =
  | "pool-yield"       // per-pool APY / yield-bps at a point in time
  | "risk-score"       // per-subject (SME / debtor hash) risk score
  | "oracle-rate";     // price oracle rate for a currency pair

// ── Raw data point ────────────────────────────────────────────────────────────

/**
 * A single immutable time-series data point.
 *
 * Primary key: (kind, subject, timestampMs)
 * The combination must be unique to prevent duplicate backfill insertions.
 */
export interface TimeSeriesPoint {
  kind: SeriesKind;
  /**
   * Subject identifier:
   *  - pool-yield:   invoice ID as decimal string
   *  - risk-score:   SME Stellar address or debtor hash (hex)
   *  - oracle-rate:  currency symbol (e.g. "USDC", "EURC")
   */
  subject: string;
  /** Unix timestamp in milliseconds. */
  timestampMs: number;
  /** The value at this point (bps, score 0-100, or price in stroops). */
  value: number;
}

// ── Downsampled / aggregated bucket ──────────────────────────────────────────

export type DownsampleGranularity = "5min" | "hourly" | "daily" | "weekly";

export interface DownsampledBucket {
  kind: SeriesKind;
  subject: string;
  /** Start of the bucket window (Unix ms). */
  bucketStartMs: number;
  granularity: DownsampleGranularity;
  min: number;
  max: number;
  avg: number;
  /** Number of raw points that contributed to this bucket. */
  count: number;
}

// ── Query inputs ──────────────────────────────────────────────────────────────

export interface RangeQueryInput {
  kind: SeriesKind;
  subject: string;
  fromMs: number;
  toMs: number;
}

export interface DownsampleQueryInput extends RangeQueryInput {
  granularity: DownsampleGranularity;
}

// ── Retention policy ──────────────────────────────────────────────────────────

/** How long raw and downsampled data are retained. */
export interface RetentionPolicy {
  rawRetentionDays: number;
  downsampledRetentionDays: number;
}

export const DEFAULT_RETENTION: RetentionPolicy = {
  rawRetentionDays: 90,
  downsampledRetentionDays: 730, // 2 years
};

/**
 * Retention windows per granularity tier.
 *
 * | Granularity | Retained for  | Rationale                                           |
 * |-------------|---------------|-----------------------------------------------------|
 * | 5min        | 7 days        | High-resolution; only needed for recent dashboards. |
 * | hourly      | 90 days       | Matches raw retention; covers quarterly views.      |
 * | daily       | 730 days      | 2-year rolling window; primary chart resolution.    |
 * | weekly      | 1825 days     | 5-year archive; long-range trend views.             |
 *
 * Raw points follow `RetentionPolicy.rawRetentionDays` (default 90 days).
 * After raw pruning, downsampled buckets remain queryable up to their tier
 * window, ensuring long-range APR/marketplace charts continue to work even
 * after raw data has been pruned.
 */
export const GRANULARITY_RETENTION_DAYS: Record<DownsampleGranularity, number> = {
  "5min":   7,
  hourly:  90,
  daily:   730,
  weekly:  1_825,
};
