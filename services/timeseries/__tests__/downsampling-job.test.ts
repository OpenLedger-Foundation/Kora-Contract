/**
 * Downsampling job tests — issue #763
 *
 * Covers:
 *  - DownsamplingJob runs and pre-computes buckets for all granularities
 *  - 5-minute granularity produces correct bucket alignment
 *  - API resolution param (queryDownsampled) returns the right granularity
 *  - Empty buckets: job completes without error when subjects have no data
 *  - Retention enforcement runs at job end and prunes stale data
 *  - granularityRetentionMs helper returns correct windows
 */

import { bucketStart, bucketWindowMs } from "../downsampler";
import { DownsamplingJob, granularityRetentionMs } from "../downsampling-job";
import { InMemoryTimeSeriesStore } from "../store";
import { TimeSeriesService } from "../timeseries-service";
import { GRANULARITY_RETENTION_DAYS, TimeSeriesPoint } from "../types";

// ── Helpers ───────────────────────────────────────────────────────────────────

const MS_PER_MIN  = 60 * 1_000;
const MS_PER_HOUR = 60 * MS_PER_MIN;
const MS_PER_DAY  = 24 * MS_PER_HOUR;

function makePoint(subject: string, timestampMs: number, value: number): TimeSeriesPoint {
  return { kind: "pool-yield", subject, timestampMs, value };
}

function makeService(): { service: TimeSeriesService; store: InMemoryTimeSeriesStore } {
  const store = new InMemoryTimeSeriesStore();
  const service = new TimeSeriesService(store);
  return { service, store };
}

// ── bucketWindowMs — 5min ─────────────────────────────────────────────────────

describe("bucketWindowMs — 5min granularity", () => {
  it("returns 5 minutes in milliseconds", () => {
    expect(bucketWindowMs("5min")).toBe(5 * 60 * 1_000);
  });
});

// ── bucketStart — 5min alignment ─────────────────────────────────────────────

describe("bucketStart — 5min alignment", () => {
  it("aligns a timestamp to the start of its 5-minute bucket", () => {
    // 12:03:30 UTC should align to 12:00:00 UTC
    const ts = new Date("2025-06-15T12:03:30Z").getTime();
    const expected = new Date("2025-06-15T12:00:00Z").getTime();
    expect(bucketStart(ts, "5min")).toBe(expected);
  });

  it("timestamps in the same 5-min window share a bucket", () => {
    const t1 = new Date("2025-06-15T12:00:01Z").getTime();
    const t2 = new Date("2025-06-15T12:04:59Z").getTime();
    expect(bucketStart(t1, "5min")).toBe(bucketStart(t2, "5min"));
  });

  it("timestamps in adjacent 5-min windows get different buckets", () => {
    const t1 = new Date("2025-06-15T12:04:59Z").getTime();
    const t2 = new Date("2025-06-15T12:05:00Z").getTime();
    expect(bucketStart(t1, "5min")).not.toBe(bucketStart(t2, "5min"));
  });
});

// ── DownsamplingJob — basic run ───────────────────────────────────────────────

describe("DownsamplingJob — basic run", () => {
  it("pre-computes buckets for all four granularities after a run", async () => {
    const { service, store } = makeService();
    const now = Date.now();

    // Ingest two points within the default look-back window (2 h).
    await service.ingestBatch([
      makePoint("INV_J1", now - 10 * MS_PER_MIN, 100),
      makePoint("INV_J1", now - 5  * MS_PER_MIN, 200),
    ]);

    const job = new DownsamplingJob(service, {
      kinds: ["pool-yield"],
      subjects: ["INV_J1"],
    });

    const result = await job.run();

    expect(result.kinds).toEqual(["pool-yield"]);
    expect(result.subjectsProcessed).toBe(1);
    expect(new Date(result.ranAt).toISOString()).toBe(result.ranAt);

    // All four granularities should now have pre-computed buckets.
    // Use a wide window (0 → far future) so coarse buckets (weekly) whose
    // aligned start falls before `now - 2h` are still captured.
    for (const gran of ["5min", "hourly", "daily", "weekly"] as const) {
      const buckets = await store.queryDownsampled(
        "pool-yield",
        "INV_J1",
        0,
        now + MS_PER_HOUR,
        gran,
      );
      expect(buckets.length).toBeGreaterThan(0);
    }
  });

  it("processes multiple subjects across multiple kinds", async () => {
    const { service } = makeService();
    const now = Date.now();

    await service.ingestBatch([
      { kind: "pool-yield",  subject: "INV_A", timestampMs: now - MS_PER_MIN, value: 50 },
      { kind: "risk-score",  subject: "SME_X", timestampMs: now - MS_PER_MIN, value: 70 },
      { kind: "oracle-rate", subject: "USDC",  timestampMs: now - MS_PER_MIN, value: 1 },
    ]);

    const job = new DownsamplingJob(service, {
      kinds: ["pool-yield", "risk-score", "oracle-rate"],
      subjects: ["INV_A", "SME_X", "USDC"],
    });

    const result = await job.run();
    expect(result.subjectsProcessed).toBe(3);
  });
});

// ── DownsamplingJob — empty buckets ──────────────────────────────────────────

describe("DownsamplingJob — empty bucket edge case", () => {
  it("completes without error when a subject has no data", async () => {
    const { service } = makeService();

    const job = new DownsamplingJob(service, {
      kinds: ["pool-yield"],
      subjects: ["GHOST_INVOICE"],
    });

    // Should not throw even though the subject has zero data points.
    await expect(job.run()).resolves.not.toThrow();
  });

  it("completes without error when the subject list is empty", async () => {
    const { service } = makeService();

    const job = new DownsamplingJob(service, {
      kinds: ["pool-yield"],
      subjects: [],
    });

    const result = await job.run();
    expect(result.subjectsProcessed).toBe(0);
  });
});

// ── API resolution param — queryDownsampled ───────────────────────────────────

describe("TimeSeriesService — queryDownsampled resolution param", () => {
  it("returns 5min buckets when resolution is '5min'", async () => {
    const { service } = makeService();
    const base = bucketStart(Date.now(), "5min"); // align to a clean boundary

    await service.backfill([
      makePoint("INV_R1", base + 1 * MS_PER_MIN, 10),
      makePoint("INV_R1", base + 2 * MS_PER_MIN, 20),
      makePoint("INV_R1", base + 7 * MS_PER_MIN, 30), // next 5-min bucket
    ]);

    const buckets = await service.queryDownsampled({
      kind: "pool-yield",
      subject: "INV_R1",
      fromMs: base,
      toMs: base + 10 * MS_PER_MIN,
      granularity: "5min",
    });

    // Points at base+1m and base+2m fall into bucket[0]; base+7m into bucket[1].
    expect(buckets).toHaveLength(2);
    expect(buckets[0].min).toBe(10);
    expect(buckets[0].max).toBe(20);
    expect(buckets[0].avg).toBeCloseTo(15);
    expect(buckets[1].min).toBe(30);
    expect(buckets[1].max).toBe(30);
  });

  it("returns daily buckets when resolution is 'daily'", async () => {
    const { service } = makeService();
    const day1 = Math.floor(Date.now() / MS_PER_DAY) * MS_PER_DAY;
    const day2 = day1 + MS_PER_DAY;

    await service.backfill([
      makePoint("INV_R2", day1 + 1_000, 100),
      makePoint("INV_R2", day2 + 1_000, 200),
    ]);

    const buckets = await service.queryDownsampled({
      kind: "pool-yield",
      subject: "INV_R2",
      fromMs: day1,
      toMs: day2 + MS_PER_DAY,
      granularity: "daily",
    });

    expect(buckets).toHaveLength(2);
    expect(buckets[0].bucketStartMs).toBe(day1);
    expect(buckets[1].bucketStartMs).toBe(day2);
  });

  it("falls back to on-the-fly computation when no pre-computed '5min' buckets exist", async () => {
    const { service } = makeService();
    const base = bucketStart(Date.now(), "5min");

    // Ingest without backfill (no pre-computed buckets).
    await service.ingestBatch([makePoint("INV_R3", base + MS_PER_MIN, 42)]);

    const buckets = await service.queryDownsampled({
      kind: "pool-yield",
      subject: "INV_R3",
      fromMs: base,
      toMs: base + 5 * MS_PER_MIN,
      granularity: "5min",
    });

    expect(buckets).toHaveLength(1);
    expect(buckets[0].avg).toBe(42);
  });
});

// ── Retention enforcement via job ─────────────────────────────────────────────

describe("DownsamplingJob — retention enforcement", () => {
  it("prunes raw points beyond the configured retention window", async () => {
    const store = new InMemoryTimeSeriesStore();
    const service = new TimeSeriesService(store, {
      rawRetentionDays: 1,
      downsampledRetentionDays: 7,
    });

    const oldTs = Date.now() - 2 * MS_PER_DAY; // 2 days ago — beyond 1-day window
    const newTs = Date.now() - MS_PER_HOUR;     // 1 hour ago — within window

    await service.ingestBatch([
      makePoint("INV_S1", oldTs, 99),
      makePoint("INV_S1", newTs, 55),
    ]);

    const job = new DownsamplingJob(service, {
      kinds: ["pool-yield"],
      subjects: ["INV_S1"],
    });

    const result = await job.run();
    expect(result.retention["pool-yield"].raw).toBe(1);

    const remaining = await service.queryRaw({
      kind: "pool-yield",
      subject: "INV_S1",
      fromMs: 0,
      toMs: Date.now(),
    });
    expect(remaining).toHaveLength(1);
    expect(remaining[0].value).toBe(55);
  });

  it("retention result contains an entry for every processed kind", async () => {
    const { service } = makeService();
    const job = new DownsamplingJob(service, {
      kinds: ["pool-yield", "risk-score", "oracle-rate"],
      subjects: [],
    });
    const result = await job.run();
    expect(Object.keys(result.retention)).toEqual(
      expect.arrayContaining(["pool-yield", "risk-score", "oracle-rate"]),
    );
  });
});

// ── GRANULARITY_RETENTION_DAYS / granularityRetentionMs ──────────────────────

describe("granularityRetentionMs helper", () => {
  it("returns 7 days for 5min", () => {
    expect(granularityRetentionMs("5min")).toBe(7 * MS_PER_DAY);
  });

  it("returns 90 days for hourly", () => {
    expect(granularityRetentionMs("hourly")).toBe(90 * MS_PER_DAY);
  });

  it("returns 730 days for daily", () => {
    expect(granularityRetentionMs("daily")).toBe(730 * MS_PER_DAY);
  });

  it("returns 1825 days for weekly", () => {
    expect(granularityRetentionMs("weekly")).toBe(1_825 * MS_PER_DAY);
  });

  it("GRANULARITY_RETENTION_DAYS covers all four granularities", () => {
    const keys = Object.keys(GRANULARITY_RETENTION_DAYS);
    expect(keys).toEqual(expect.arrayContaining(["5min", "hourly", "daily", "weekly"]));
    expect(keys).toHaveLength(4);
  });
});
