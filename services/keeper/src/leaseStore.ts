/**
 * Keeper Service — Distributed Lease Store
 *
 * Closes #<issue_number>
 *
 * Prevents multiple keeper replicas from executing the same jobs simultaneously
 * ("double execution"). Each replica tries to acquire a named lease before
 * running any dispatch tick.  Only the holder is allowed to dispatch; replicas
 * that do not hold the lease skip the tick silently and re-try on the next
 * poll cycle.
 *
 * Design decisions
 * ────────────────
 * • SQLite as the lease backend — same runtime already used for JobStore,
 *   no new dependency.  Fine for deployments where all replicas share the same
 *   filesystem (NFS/EBS).  For multi-node Kubernetes, replace `LeaseStore`
 *   with a Postgres advisory-lock adapter or a Redis SETNX adapter while
 *   keeping the `ILeaseStore` interface.
 *
 * • Lease record: `(lease_name, holder_id, acquired_at, expires_at,
 *   renewed_at)`.  `expires_at` is the wall-clock expiry (epoch seconds).
 *
 * • Acquire semantics (atomic upsert in a single SQLite transaction):
 *     1. Read current row.
 *     2. If no row or row has expired → INSERT/UPDATE unconditionally.
 *     3. If row is held by this replica → renew (extend expires_at).
 *     4. If row is held by another non-expired replica → refuse.
 *   The read+write is wrapped in `BEGIN EXCLUSIVE` so concurrent SQLite
 *   writers see a fully serialised view.
 *
 * • Clock skew tolerance: `acquireLease` accepts a `clockSkewToleranceS`
 *   parameter (default 2 s).  When comparing an existing lease's expiry to
 *   "now", a lease whose `expires_at ≤ now + skew` is treated as expired.
 *   This handles replicas whose clocks drift by up to the tolerance.
 *
 * • Renewal: the lease holder calls `renewLease` on each successful tick to
 *   push `expires_at` forward.  If a replica crashes without releasing its
 *   lease, the lease expires after `leaseTtlS` seconds and a new holder is
 *   elected automatically.
 *
 * Usage in Scheduler:
 *   const acquired = leaseStore.acquireLease("dispatch", holderId, ttlS);
 *   if (!acquired) return; // another replica is active
 *   try {
 *     await dispatchJobs();
 *     leaseStore.renewLease("dispatch", holderId, ttlS);
 *   } finally {
 *     // Release on shutdown; lease expires naturally on crash
 *     leaseStore.releaseLease("dispatch", holderId);
 *   }
 */

import Database, { Database as DB } from "better-sqlite3";
import pino from "pino";

const logger = pino({ name: "keeper:lease" });

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface LeaseRecord {
  leaseName: string;
  holderId: string;
  acquiredAt: number;
  expiresAt: number;
  renewedAt: number;
}

/**
 * Minimal interface so callers can inject a test double without depending on
 * the SQLite implementation.
 */
export interface ILeaseStore {
  /**
   * Try to acquire or renew `leaseName` for `holderId`.
   *
   * @param leaseName        Logical name of the lease (e.g. "dispatch")
   * @param holderId         Unique ID of this replica (e.g. hostname + PID)
   * @param leaseTtlS        Lease duration in seconds
   * @param clockSkewTolS    Clock-skew tolerance in seconds (default: 2)
   * @returns true if this replica is now the lease holder; false if another
   *          non-expired replica holds it.
   */
  acquireLease(leaseName: string, holderId: string, leaseTtlS: number, clockSkewTolS?: number): boolean;

  /**
   * Renew an existing lease held by `holderId`, extending its expiry by
   * `leaseTtlS` seconds from now.  Returns false if the lease is no longer
   * held by this replica (it may have expired and been taken).
   */
  renewLease(leaseName: string, holderId: string, leaseTtlS: number): boolean;

  /**
   * Explicitly release a lease held by `holderId`.  No-op if the lease is
   * held by someone else (guards against accidental self-eviction after a
   * short expiry).
   */
  releaseLease(leaseName: string, holderId: string): void;

  /**
   * Return the current lease record for `leaseName`, or null if no lease exists.
   * Callers can use this for observability (e.g. /health endpoint).
   */
  getLease(leaseName: string): LeaseRecord | null;

  close(): void;
}

// ---------------------------------------------------------------------------
// SQLite implementation
// ---------------------------------------------------------------------------

export class LeaseStore implements ILeaseStore {
  private readonly db: DB;

  constructor(dbPath: string) {
    this.db = new Database(dbPath);
    this._migrate();
  }

  // ── Schema ──────────────────────────────────────────────────────────────

  private _migrate(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS keeper_leases (
        lease_name   TEXT    NOT NULL PRIMARY KEY,
        holder_id    TEXT    NOT NULL,
        acquired_at  INTEGER NOT NULL,
        expires_at   INTEGER NOT NULL,
        renewed_at   INTEGER NOT NULL
      );
    `);
  }

  // ── Acquire / renew ──────────────────────────────────────────────────────

  acquireLease(
    leaseName: string,
    holderId: string,
    leaseTtlS: number,
    clockSkewTolS = 2
  ): boolean {
    const now = Math.floor(Date.now() / 1000);
    const expiresAt = now + leaseTtlS;

    // BEGIN EXCLUSIVE ensures no concurrent writer can race this read-modify-write
    const result = this.db.transaction((): boolean => {
      const existing = this.db
        .prepare(`SELECT * FROM keeper_leases WHERE lease_name = ?`)
        .get(leaseName) as RawLease | undefined;

      if (!existing) {
        // No lease yet — take it
        this.db
          .prepare(
            `INSERT INTO keeper_leases (lease_name, holder_id, acquired_at, expires_at, renewed_at)
             VALUES (?, ?, ?, ?, ?)`
          )
          .run(leaseName, holderId, now, expiresAt, now);
        logger.info({ leaseName, holderId, expiresAt }, "lease: acquired (new)");
        return true;
      }

      const isHeldByUs = existing.holder_id === holderId;
      const isExpired = existing.expires_at <= now + clockSkewTolS;

      if (isHeldByUs) {
        // Renew our own lease
        this.db
          .prepare(
            `UPDATE keeper_leases
             SET expires_at = ?, renewed_at = ?
             WHERE lease_name = ? AND holder_id = ?`
          )
          .run(expiresAt, now, leaseName, holderId);
        logger.debug({ leaseName, holderId, expiresAt }, "lease: renewed");
        return true;
      }

      if (isExpired) {
        // Expired lease from another holder — take it over
        this.db
          .prepare(
            `UPDATE keeper_leases
             SET holder_id = ?, acquired_at = ?, expires_at = ?, renewed_at = ?
             WHERE lease_name = ?`
          )
          .run(holderId, now, expiresAt, now, leaseName);
        logger.info(
          { leaseName, holderId, previousHolder: existing.holder_id, expiresAt },
          "lease: acquired (expired from previous holder)"
        );
        return true;
      }

      // Another replica holds a live lease
      logger.debug(
        { leaseName, holderId, activeHolder: existing.holder_id, expiresAt: existing.expires_at },
        "lease: refused (held by another replica)"
      );
      return false;
    }).exclusive();

    return result;
  }

  // ── Renew ────────────────────────────────────────────────────────────────

  renewLease(leaseName: string, holderId: string, leaseTtlS: number): boolean {
    const now = Math.floor(Date.now() / 1000);
    const expiresAt = now + leaseTtlS;

    const result = this.db
      .prepare(
        `UPDATE keeper_leases
         SET expires_at = ?, renewed_at = ?
         WHERE lease_name = ? AND holder_id = ?`
      )
      .run(expiresAt, now, leaseName, holderId);

    const renewed = result.changes > 0;
    if (renewed) {
      logger.debug({ leaseName, holderId, expiresAt }, "lease: renewed (explicit)");
    } else {
      logger.warn({ leaseName, holderId }, "lease: renewLease called but lease not held by this replica");
    }
    return renewed;
  }

  // ── Release ──────────────────────────────────────────────────────────────

  releaseLease(leaseName: string, holderId: string): void {
    const result = this.db
      .prepare(
        `DELETE FROM keeper_leases WHERE lease_name = ? AND holder_id = ?`
      )
      .run(leaseName, holderId);

    if (result.changes > 0) {
      logger.info({ leaseName, holderId }, "lease: released");
    } else {
      logger.debug({ leaseName, holderId }, "lease: releaseLease called but lease not held (already expired or taken)");
    }
  }

  // ── Read ─────────────────────────────────────────────────────────────────

  getLease(leaseName: string): LeaseRecord | null {
    const row = this.db
      .prepare(`SELECT * FROM keeper_leases WHERE lease_name = ?`)
      .get(leaseName) as RawLease | undefined;

    return row ? rowToRecord(row) : null;
  }

  close(): void {
    this.db.close();
  }
}

// ---------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------

interface RawLease {
  lease_name: string;
  holder_id: string;
  acquired_at: number;
  expires_at: number;
  renewed_at: number;
}

function rowToRecord(r: RawLease): LeaseRecord {
  return {
    leaseName: r.lease_name,
    holderId: r.holder_id,
    acquiredAt: r.acquired_at,
    expiresAt: r.expires_at,
    renewedAt: r.renewed_at,
  };
}
