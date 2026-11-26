/**
 * Keeper Service — Scheduler
 *
 * Orchestrates the tick loop:
 *   1. Acquire the dispatch lease (lease fencing, Closes #<issue_number>)
 *   2. Promote expired pending jobs → ready
 *   3. Fetch all dispatchable jobs
 *   4. Dispatch each, recording outcome
 *   5. Move exhausted jobs to dead-letter
 *   6. Renew the lease on successful tick completion
 *
 * Issue #765
 */

import { JobStore } from "./jobStore";
import { Dispatcher } from "./dispatcher";
import { ILeaseStore } from "./leaseStore";
import { KeeperConfig, Deadline } from "./types";
import pino from "pino";

const logger = pino({ name: "keeper:scheduler" });

/** Lease TTL in seconds.  Lease expires if the holder crashes without renewing. */
const LEASE_TTL_S = 60;

/** Default lease name — single dispatch lock across all replicas. */
export const DISPATCH_LEASE_NAME = "keeper:dispatch";

export class Scheduler {
  private timer: ReturnType<typeof setInterval> | null = null;
  private running = false;

  constructor(
    private readonly store: JobStore,
    private readonly dispatcher: Dispatcher,
    private readonly config: KeeperConfig,
    private readonly scanListingTtl?: () => Promise<Deadline[]>,
    private readonly leaseStore?: ILeaseStore,
    private readonly holderId: string = `${process.env["HOSTNAME"] ?? "local"}-${process.pid}`
  ) {}

  /**
   * Enqueue a batch of deadlines sourced from the indexer.
   * Duplicate dedupKeys are silently ignored (idempotent).
   */
  enqueueDeadlines(deadlines: Deadline[]): void {
    for (const d of deadlines) {
      this.store.enqueue(d);
      logger.debug({ dedupKey: d.dedupKey }, "Deadline enqueued");
    }
  }

  /**
   * Start the periodic tick loop.
   */
  start(): void {
    if (this.running) return;
    this.running = true;
    logger.info(
      { intervalMs: this.config.pollIntervalMs, holderId: this.holderId, leaseFencing: !!this.leaseStore },
      "Keeper scheduler started"
    );
    // Run once immediately, then on interval
    void this._tick();
    this.timer = setInterval(() => void this._tick(), this.config.pollIntervalMs);
  }

  /**
   * Gracefully stop the scheduler and release any held lease.
   */
  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
    this.running = false;

    // Release the dispatch lease so a sibling replica can take over immediately
    // rather than waiting for the TTL to expire.
    if (this.leaseStore) {
      try {
        this.leaseStore.releaseLease(DISPATCH_LEASE_NAME, this.holderId);
      } catch (err) {
        logger.warn({ err }, "Failed to release lease during shutdown");
      }
    }

    logger.info("Keeper scheduler stopped");
  }

  // ---------------------------------------------------------------------------
  // Tick
  // ---------------------------------------------------------------------------

  private async _tick(): Promise<void> {
    // ── Lease fencing ──────────────────────────────────────────────────────
    // When a leaseStore is configured, only the replica that holds the
    // "keeper:dispatch" lease is allowed to dispatch jobs.  Other replicas
    // skip the tick silently and try again on the next poll cycle.
    if (this.leaseStore) {
      const acquired = this.leaseStore.acquireLease(
        DISPATCH_LEASE_NAME,
        this.holderId,
        LEASE_TTL_S
      );
      if (!acquired) {
        logger.debug(
          { holderId: this.holderId, leaseName: DISPATCH_LEASE_NAME },
          "lease not held — skipping tick"
        );
        return;
      }
    }

    if (this.scanListingTtl) {
      try {
        const ttlJobs = await this.scanListingTtl();
        this.enqueueDeadlines(ttlJobs);
      } catch (error) {
        logger.error({ error }, "Listing TTL scan failed; existing jobs will continue");
      }
    }

    const nowTs = Math.floor(Date.now() / 1000);

    // Step 1: promote pending jobs whose deadline has elapsed
    const promoted = this.store.promoteExpired(nowTs);
    if (promoted > 0) {
      logger.info({ promoted }, "Jobs promoted to ready");
    }

    // Step 2: fetch dispatchable jobs
    const jobs = this.store.getDispatchable(
      nowTs,
      this.config.retryBackoffBaseMs,
      this.config.maxAttempts
    );

    if (jobs.length === 0) {
      // Renew lease even on empty tick so it does not expire while idle
      this._renewLease();
      return;
    }
    logger.info({ count: jobs.length }, "Dispatching jobs");

    // Step 3: dispatch sequentially (rate limiter inside Dispatcher)
    for (const job of jobs) {
      const result = await this.dispatcher.dispatch(job);
      this.store.recordAttempt(
        job.dedupKey,
        result.outcome,
        result.txHash,
        result.error
      );

      // Step 4: if exhausted, move to dead
      const updated = this.store.getByStatus("failed").find(
        (j) => j.dedupKey === job.dedupKey
      );
      if (updated && updated.attempts >= this.config.maxAttempts) {
        this.store.markDead(
          job.dedupKey,
          `Exhausted ${this.config.maxAttempts} attempts. Last error: ${updated.lastError}`
        );
        logger.warn({ dedupKey: job.dedupKey }, "Job moved to dead-letter queue");
      }
    }

    // Renew lease after a successful tick
    this._renewLease();
  }

  private _renewLease(): void {
    if (!this.leaseStore) return;
    const renewed = this.leaseStore.renewLease(DISPATCH_LEASE_NAME, this.holderId, LEASE_TTL_S);
    if (!renewed) {
      logger.warn(
        { holderId: this.holderId },
        "Failed to renew lease after tick — lease may have been taken by another replica"
      );
    }
  }
}
