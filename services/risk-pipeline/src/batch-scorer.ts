/**
 * Batch Scoring — idempotent wrapper around RiskPipeline.score()
 *
 * ## Idempotency key scheme
 *
 * Every batch-scoring request must carry an `idempotencyKey`.  The key is
 * caller-supplied and must be unique per *logical* scoring job, not per
 * invocation.  Recommended key construction:
 *
 *   `<smeAddress>:<invoiceId>:<formulaVersion>:<dataEpochSeconds>`
 *
 * Where `dataEpochSeconds` is the Unix second at which the input signals were
 * snapshotted.  This makes two runs over identical data produce the same key
 * (and therefore return the same result) while a genuine re-score after new
 * data arrives produces a different key.
 *
 * ### Key-collision hazard
 * Two different invoices that share the same key will collide: the second
 * submission returns the first invoice's stored result.  Prevent this by
 * always including a stable invoice identifier in the key.  The store does
 * NOT validate key contents — that is the caller's responsibility.
 *
 * ## Upsert semantics
 *
 * `submit` checks the store before scoring:
 *   - Key present  → return the stored result immediately (no re-scoring).
 *   - Key absent   → score, persist, return.
 *
 * This means re-submitting an identical request is a no-op and cannot produce
 * duplicate side-effects (tier flips, attestation triggers, etc.).
 *
 * ## Duplicate metrics
 *
 * `BatchScoringStore.metrics()` returns a `BatchScoringMetrics` snapshot.
 * The `duplicatesBlocked` counter increments each time a submission is
 * short-circuited by an existing key.  Wire this into your Prometheus exporter
 * as `kora_risk_pipeline_duplicates_blocked_total`.
 *
 * Issue: #755
 */

import { RiskPipeline } from "./scorer";
import { ScoringInput, SuggestedScore } from "./types";

// ── Stored job shape ──────────────────────────────────────────────────────────

export interface BatchScoringJob {
  /** Caller-supplied idempotency key (see key scheme above). */
  idempotencyKey: string;
  /** ISO-8601 timestamp of when this job was first scored. */
  scoredAt: string;
  /** The input signals used for scoring. */
  input: ScoringInput;
  /** The persisted score result. */
  result: SuggestedScore;
}

// ── Metrics ───────────────────────────────────────────────────────────────────

export interface BatchScoringMetrics {
  /** Total jobs stored (unique idempotency keys). */
  totalJobs: number;
  /**
   * Number of times a submission was short-circuited because the key already
   * existed.  Expose as `kora_risk_pipeline_duplicates_blocked_total`.
   */
  duplicatesBlocked: number;
}

// ── Store interface ───────────────────────────────────────────────────────────

export interface BatchScoringStore {
  /**
   * Return the stored job for `key`, or `undefined` when not present.
   */
  get(key: string): Promise<BatchScoringJob | undefined>;

  /**
   * Upsert a job.  If `key` already exists the entry is overwritten.
   * (In practice `submit` never overwrites — it short-circuits — but
   * the interface supports upsert for administrative corrections.)
   */
  set(key: string, job: BatchScoringJob): Promise<void>;

  /** Current metrics snapshot. */
  metrics(): Promise<BatchScoringMetrics>;

  /** Increment the duplicate-blocked counter by 1. */
  recordDuplicate(): Promise<void>;
}

// ── In-memory store ───────────────────────────────────────────────────────────

export class InMemoryBatchScoringStore implements BatchScoringStore {
  private readonly _jobs = new Map<string, BatchScoringJob>();
  private _duplicatesBlocked = 0;

  async get(key: string): Promise<BatchScoringJob | undefined> {
    return this._jobs.get(key);
  }

  async set(key: string, job: BatchScoringJob): Promise<void> {
    this._jobs.set(key, job);
  }

  async metrics(): Promise<BatchScoringMetrics> {
    return {
      totalJobs: this._jobs.size,
      duplicatesBlocked: this._duplicatesBlocked,
    };
  }

  async recordDuplicate(): Promise<void> {
    this._duplicatesBlocked++;
  }
}

// ── BatchScorer ───────────────────────────────────────────────────────────────

export interface BatchSubmitOptions {
  /** Formula version to pass to RiskPipeline.score(). */
  formulaVersion?: string;
}

export interface BatchSubmitResult {
  /** The score result (either freshly computed or retrieved from store). */
  score: SuggestedScore;
  /**
   * True when the result was served from the store (duplicate submission).
   * False when the pipeline was actually invoked for this key.
   */
  fromCache: boolean;
}

/**
 * Idempotent batch scorer.
 *
 * Wrap an existing `RiskPipeline` instance to gain idempotency and duplicate
 * tracking without changing the pipeline's pure scoring logic.
 */
export class BatchScorer {
  constructor(
    private readonly pipeline: RiskPipeline,
    private readonly store: BatchScoringStore,
  ) {}

  /**
   * Submit a scoring job.
   *
   * If `idempotencyKey` is already present in the store, the existing result
   * is returned immediately and `duplicatesBlocked` is incremented.
   * Otherwise the pipeline scores the input, persists the job, and returns
   * the fresh result.
   *
   * @param idempotencyKey  Unique key for this logical scoring job.
   * @param input           Scoring signals.
   * @param options         Optional formula version.
   */
  async submit(
    idempotencyKey: string,
    input: ScoringInput,
    options: BatchSubmitOptions = {},
  ): Promise<BatchSubmitResult> {
    const existing = await this.store.get(idempotencyKey);
    if (existing !== undefined) {
      await this.store.recordDuplicate();
      return { score: existing.result, fromCache: true };
    }

    const result = this.pipeline.score(input, options.formulaVersion);

    const job: BatchScoringJob = {
      idempotencyKey,
      scoredAt: new Date().toISOString(),
      input,
      result,
    };
    await this.store.set(idempotencyKey, job);

    return { score: result, fromCache: false };
  }

  /** Retrieve the stored job for a key, or undefined if not found. */
  async getJob(idempotencyKey: string): Promise<BatchScoringJob | undefined> {
    return this.store.get(idempotencyKey);
  }

  /** Current metrics snapshot. */
  async metrics(): Promise<BatchScoringMetrics> {
    return this.store.metrics();
  }
}
