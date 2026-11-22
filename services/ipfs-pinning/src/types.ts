/**
 * IPFS Pinning Service — Types
 *
 * Accepts SME-submitted invoice documents/metadata, pins them redundantly, and
 * returns the CID recorded on-chain in `invoice_nft`. On-chain CID recording
 * itself is out of scope (the SME's own transaction handles that).
 *
 * Sensitive debtor PII must be hashed/encrypted per the protocol's PII-hashing
 * approach BEFORE pinning raw documents — this service rejects payloads that
 * declare raw PII fields.
 *
 * Issue #754
 */

export interface PinRequest {
  /** Original filename (used for validation + audit). */
  filename: string;
  /** Raw document bytes. */
  content: Uint8Array;
  /** MIME type, e.g. application/pdf or application/json. */
  contentType: string;
  /** SHA-256 hex digest of `content`, computed by the client for integrity check. */
  claimedSha256Hex: string;
  /** Caller asserts PII was hashed/encrypted before submission. */
  piiHandled: boolean;
}

export interface PinResult {
  /** CID now pinned (hex of SHA-256 in this implementation; CIDv0/v1 mapping at the gateway). */
  cid: string;
  sizeBytes: number;
  sha256Hex: string;
  /** Per-provider outcomes (at least two providers configured for redundancy). */
  providers: Array<{ providerId: string; ok: boolean; error?: string }>;
  /** True when fewer than all providers succeeded (degraded redundancy). */
  degraded: boolean;
  pinnedAt: number;
}

export type PinStatus = "pinned" | "degraded" | "failed" | "repinned";

export interface PinStatusRecord {
  cid: string;
  filename: string;
  sizeBytes: number;
  sha256Hex: string;
  status: PinStatus;
  providers: string; // JSON-encoded per-provider state
  createdAt: number;
  updatedAt: number;
  lastVerifiedAt: number | null;
  consecutiveFailures: number;
}

export interface PinDeadLetter {
  id: number;
  cid: string;
  providerId: string;
  error: string;
  attempts: number;
  status: "pending" | "resolved";
  createdAt: number;
  updatedAt: number;
}

export interface PinningServiceConfig {
  /** Hard cap per file; uploads above are rejected before pinning. */
  maxFileSizeBytes: number;
  /** Allowed MIME types (allowlist). */
  allowedContentTypes: string[];
  /** Allowed filename extensions (allowlist, lowercase, with dot). */
  allowedExtensions: string[];
  /** Minimum successful provider pins required (default 1; 2 = full redundancy). */
  minSuccesses?: number;
  /** Total pin attempts per provider, including the first attempt. */
  pinRetryAttempts?: number;
  /** Initial exponential retry delay (ms). */
  pinRetryBackoffMs?: number;
  /** Maximum delay between pin attempts (ms). */
  pinRetryMaxBackoffMs?: number;
}

export interface VerifierConfig {
  /** How often the periodic persistence check runs (ms). */
  intervalMs: number;
  /** Consecutive verification failures before alerting. */
  alertAfterFailures: number;
  onAlert?: (rec: PinStatusRecord, message: string) => void;
}
