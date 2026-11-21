/**
 * Shared types for the transaction-ordering observation harness.
 *
 * A *round* submits a controlled set of competing transactions (one per source
 * account, with known fees and a known submission order) and records what the
 * network actually did with them. The recorded `applicationOrder` from RPC is
 * the intrinsic execution index within a ledger, which is the ordering sample
 * the harness compares against the documented policy in
 * `docs/MEV_RESISTANCE_NOTES.md`.
 */

export type HarnessMode = "mock" | "testnet";

/**
 * Shape of the competition inside a round:
 * - `fee_ladder`         — strictly increasing fees, distinct accounts.
 * - `equal_fee`          — identical fees, distinct accounts (arrival order).
 * - `same_account_burst` — two transactions from one account (serialization).
 */
export type RoundKind = "fee_ladder" | "equal_fee" | "same_account_burst";

export interface CompetingTxSpec {
  /** Logical label, unique within a round (e.g. `fee_ladder-2`). */
  id: string;
  /** Index into the round's source-account pool. */
  sourceIndex: number;
  /** Total fee offered, in stroops. */
  fee: number;
  /** 0-based order in which the harness fired the transaction. */
  submitOrder: number;
}

export interface RoundPlan {
  index: number;
  kind: RoundKind;
  txs: CompetingTxSpec[];
}

/** Terminal state of one submitted transaction. */
export type TxOutcome =
  | "SUCCESS"
  | "FAILED"
  | "NOT_FOUND"
  | "SUBMIT_ERROR"
  | "PENDING";

export interface TxObservation {
  id: string;
  sourceIndex: number;
  fee: number;
  submitOrder: number;
  hash: string | null;
  outcome: TxOutcome;
  ledger: number | null;
  applicationOrder: number | null;
  submittedAtMs: number;
  observedAtMs: number;
  error: string | null;
}

export interface OrderingSampleEntry {
  id: string;
  sourceIndex: number;
  fee: number;
  submitOrder: number;
  applicationOrder: number;
}

/** Observed execution order of the harness's own txs within one ledger. */
export interface LedgerOrderingSample {
  ledger: number;
  /** Entries sorted ascending by `applicationOrder`. */
  entries: OrderingSampleEntry[];
  /** Transaction ids in observed execution order. */
  observedSequence: string[];
}

export type DeviationSeverity = "hard" | "advisory";

export type DeviationCode =
  | "FEE_PRIORITY_VIOLATION"
  | "ARRIVAL_ORDER_VIOLATION"
  | "PER_ACCOUNT_SERIALIZATION_VIOLATION"
  | "MISSING_ORDERING_DATA"
  | "PARTIAL_SUBMISSION_FAILURE";

export interface Deviation {
  code: DeviationCode;
  severity: DeviationSeverity;
  roundIndex: number;
  ledger: number | null;
  detail: string;
  txIds: string[];
}

export interface RoundResult {
  index: number;
  kind: RoundKind;
  observations: TxObservation[];
  samples: LedgerOrderingSample[];
  deviations: Deviation[];
}

export interface HarnessThresholds {
  /** Promote fee-priority mismatches to hard failures (see README). */
  expectFeePriority: boolean;
  strictArrivalOrder: boolean;
  maxHardDeviations: number;
  observeTimeoutMs: number;
  pollIntervalMs: number;
  submitStaggerMs: number;
}

export interface HarnessSummary {
  rounds: number;
  txsPlanned: number;
  txsSubmitted: number;
  txsObserved: number;
  submissionFailures: number;
  hardDeviations: number;
  advisoryDeviations: number;
  orderingSamples: number;
  passed: boolean;
}

export interface HarnessReport {
  schemaVersion: 1;
  generatedAt: string;
  mode: HarnessMode;
  network: { rpcUrl: string; networkPassphrase: string };
  thresholds: HarnessThresholds;
  rounds: RoundResult[];
  deviations: Deviation[];
  /** Flat view of every ledger ordering sample collected across rounds. */
  orderingSamples: LedgerOrderingSample[];
  summary: HarnessSummary;
}

/** Raw result of a single transaction lookup; the client hides RPC specifics. */
export interface RawObservation {
  outcome: TxOutcome;
  ledger: number | null;
  applicationOrder: number | null;
  error: string | null;
}

/**
 * Minimal network surface the harness needs. `MockChain` and the testnet RPC
 * client both implement it, which is what makes "runs against mock or testnet
 * flag" work without a parallel code path.
 */
export interface OrderingClient {
  readonly mode: HarnessMode;
  submit(spec: CompetingTxSpec): Promise<{ hash: string }>;
  observe(hash: string): Promise<RawObservation>;
  /** Optional: close a ledger (mock only). Testnet relies on real ledgers. */
  advance?(): Promise<void>;
}
