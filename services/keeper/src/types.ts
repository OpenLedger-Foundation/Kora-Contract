/**
 * Keeper Service — Types
 *
 * All domain types for the deadline-based background job/keeper service.
 * Issue #765: Background Job Queue for Deadline-Based Contract Triggers.
 */

/** Categories of on-chain deadlines the keeper watches. */
export type DeadlineKind =
  | "funding_expiry"   // Invoice listing funding deadline elapsed → trigger refunds
  | "due_date"         // Invoice repayment due date elapsed → eligible for default
  | "grace_period_end" // Grace period after due_date elapsed → mark defaulted
  | "listing_ttl";     // Active marketplace listing is below its TTL threshold

/** Possible outcomes of a trigger attempt. */
export type TriggerOutcome =
  | "submitted"        // Transaction successfully broadcast
  | "already_triggered"// Contract returned a no-op / already in terminal state
  | "failed"           // Transient failure, will retry
  | "permanent_failure";// Non-retriable error; job moved to dead-letter queue

/** Lifecycle states of a keeper job. */
export type JobStatus =
  | "pending"          // Waiting for deadline to arrive
  | "ready"            // Deadline has passed, not yet dispatched
  | "failed"            // Transient failure waiting for retry back-off
  | "in_flight"        // Transaction submitted, awaiting confirmation
  | "done"             // Successfully triggered (or was already triggered)
  | "dead";            // Exhausted retries; manual inspection required

/** A single tracked on-chain deadline. */
export interface Deadline {
  /** Globally unique key: `${kind}:${invoiceId}` — used for deduplication. */
  dedupKey: string;
  kind: DeadlineKind;
  invoiceId: bigint;
  /** Unix timestamp (seconds) at which the trigger becomes valid. */
  deadlineTs: number;
  /** Stellar contract address for the relevant contract. */
  contractAddress: string;
}

/** Persistent job record stored in SQLite. */
export interface Job {
  id: number;
  dedupKey: string;
  kind: DeadlineKind;
  invoiceId: string;   // stored as string in SQLite; bigint serialization
  deadlineTs: number;
  contractAddress: string;
  status: JobStatus;
  attempts: number;
  lastAttemptAt: number | null;
  lastOutcome: TriggerOutcome | null;
  lastTxHash: string | null;
  lastError: string | null;
  createdAt: number;
  updatedAt: number;
}

/** Result returned from a single trigger execution. */
export interface TriggerResult {
  dedupKey: string;
  outcome: TriggerOutcome;
  txHash?: string;
  error?: string;
}

/** Configuration loaded from environment or constructor. */
export interface KeeperConfig {
  /** Stellar RPC endpoint (e.g. https://soroban-testnet.stellar.org). */
  rpcUrl: string;
  /** Secret key of the dedicated keeper account. */
  keeperSecret: string;
  /** Path to SQLite database file for job persistence. */
  dbPath: string;
  /** How often the scheduler tick runs, in milliseconds. Default: 30_000. */
  pollIntervalMs: number;
  /** Maximum attempts before moving a job to "dead". Default: 5. */
  maxAttempts: number;
  /** Exponential back-off base in milliseconds. Default: 60_000 (1 min). */
  retryBackoffBaseMs: number;
  /** Stellar network passphrase. */
  networkPassphrase: string;
  /** Marketplace contract whose active listings are protected. */
  marketplaceContractAddress: string;
  /** Paginated REST endpoint returning indexed listings. */
  activeListingsUrl: string;
  /** Optional credential sent as a Bearer token to the listing endpoint. */
  indexerApiKey: string;
  /** Minimum remaining TTL in ledgers before a listing is bumped. */
  ttlThresholdLedgers: number;
  /** Indexer deadline export endpoint (INDEXER_URL). Empty = manual seed only. */
  indexerUrl: string;
  /** Ingest deadlines within now + horizon seconds (DEADLINE_HORIZON_SEC). */
  deadlineHorizonSec: number;
  /** Ingestion poll cadence in ms, separate from the dispatch tick. */
  ingestIntervalMs: number;
}
