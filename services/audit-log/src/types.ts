/**
 * Audit Log Service — Types
 *
 * Domain types for the off-chain, hash-chained, tamper-evident audit log.
 * Issue #768: Build Audit Log Aggregation and Tamper-Evidence Service.
 */

import * as crypto from "crypto";

/** Which off-chain service originated the audit record. */
export type AuditService =
  | "keeper"
  | "fx-ingestion"
  | "kyb"
  | "debtor-verification"
  | "reporting"
  | "admin-relay"
  | "feature-flags";

/** Canonical action categories for off-chain privileged operations. */
export type AuditAction =
  // Keeper actions
  | "keeper.job.enqueued"
  | "keeper.job.dispatched"
  | "keeper.job.done"
  | "keeper.job.dead"
  // FX ingestion actions
  | "fx.rate.published"
  | "fx.rate.skipped"
  // KYB actions
  | "kyb.submission.received"
  | "kyb.submission.approved"
  | "kyb.submission.rejected"
  // Debtor verification
  | "debtor.verification.started"
  | "debtor.verification.completed"
  // Admin relay
  | "admin.relay.transaction.submitted"
  | "admin.relay.transaction.confirmed"
  // Reporting
  | "reporting.export.requested"
  | "reporting.export.completed"
  // Feature flags (#1116) — every mutation, and every rejected attempt
  | "feature-flag.created"
  | "feature-flag.enabled"
  | "feature-flag.disabled"
  | "feature-flag.rules-updated"
  | "feature-flag.deleted"
  | "feature-flag.bulk-updated"
  | "feature-flag.mutation-rejected";

/** A single audit record — PII-free; only hashes/references allowed. */
export interface AuditRecord {
  /**
   * Monotonically increasing sequence number (global across all writers).
   * Assigned by the AuditStore at write time.
   */
  sequence: number;
  /** Unix timestamp (ms) when the record was appended. */
  timestamp: number;
  service: AuditService;
  action: AuditAction;
  /**
   * Actor reference — must NOT be raw PII.
   * Use a SHA-256 hash of the actor's address/identity.
   */
  actorHash: string;
  /** Arbitrary structured metadata. Must NOT contain raw PII. */
  metadata: Record<string, unknown>;
  /**
   * SHA-256 of the previous record's canonical bytes.
   * Genesis record uses 64 × '0' (zeroed hash).
   */
  prevHash: string;
  /**
   * SHA-256 of this record's canonical bytes (excluding this field).
   * Computed by AuditStore at write time.
   */
  recordHash: string;
}

/** Genesis (empty) previous hash — 32 zero bytes as hex. */
export const GENESIS_HASH = "0".repeat(64);

/**
 * Compute the canonical hash of an AuditRecord.
 * The hash covers all fields EXCEPT `recordHash` itself to avoid circularity.
 */
export function computeRecordHash(record: Omit<AuditRecord, "recordHash">): string {
  const canonical = JSON.stringify({
    sequence: record.sequence,
    timestamp: record.timestamp,
    service: record.service,
    action: record.action,
    actorHash: record.actorHash,
    metadata: record.metadata,
    prevHash: record.prevHash,
  });
  return crypto.createHash("sha256").update(canonical, "utf8").digest("hex");
}
