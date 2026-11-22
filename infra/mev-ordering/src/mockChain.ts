/**
 * In-process model of Stellar's transaction-ordering policy.
 *
 * It implements the same `OrderingClient` surface as the testnet RPC client so
 * `MEV_MODE=mock` and `MEV_MODE=testnet` share one harness code path. The model
 * encodes the guarantees the harness expects, and each option can *break* one
 * of them, which is how tests (and demos) exercise deviation detection:
 *
 * - `disableFeePriority`  — arrivals always win, even with a higher fee.
 * - `equalFeeTieBreak`    — `reverse` inverts arrival order for equal fees.
 * - `submitFailureRate`   — rejects a fraction of submissions (partial failure).
 * - `rpcLagLedgers`       — withholds confirmations for recent ledgers (RPC lag).
 */

import type {
  CompetingTxSpec,
  HarnessMode,
  OrderingClient,
  RawObservation,
} from "./types";

export interface MockChainOptions {
  /** Simulated RPC lag: confirmations for the newest N ledgers are withheld. */
  rpcLagLedgers?: number;
  /** Tie-break applied to equal-fee transactions. */
  equalFeeTieBreak?: "arrival" | "reverse";
  /** Fraction of submissions (0-1) rejected at submit time. */
  submitFailureRate?: number;
  /** When true, fee is ignored and only arrival order is honoured. */
  disableFeePriority?: boolean;
  /** Injectable RNG so failure injection is deterministic under test. */
  random?: () => number;
  /** Sequence number of the first simulated ledger. */
  startingLedger?: number;
}

interface PendingTx {
  spec: CompetingTxSpec;
  hash: string;
}

interface ConfirmedTx {
  ledger: number;
  applicationOrder: number;
  outcome: "SUCCESS" | "FAILED";
}

export class MockChain implements OrderingClient {
  readonly mode: HarnessMode = "mock";

  private readonly rpcLagLedgers: number;
  private readonly equalFeeTieBreak: "arrival" | "reverse";
  private readonly submitFailureRate: number;
  private readonly disableFeePriority: boolean;
  private readonly random: () => number;

  private pending: PendingTx[] = [];
  private readonly confirmed = new Map<string, ConfirmedTx>();
  private nextLedger: number;
  private latestLedger: number;
  private hashCounter = 0;

  constructor(options: MockChainOptions = {}) {
    this.rpcLagLedgers = options.rpcLagLedgers ?? 0;
    this.equalFeeTieBreak = options.equalFeeTieBreak ?? "arrival";
    this.submitFailureRate = options.submitFailureRate ?? 0;
    this.disableFeePriority = options.disableFeePriority ?? false;
    this.random = options.random ?? Math.random;
    this.nextLedger = options.startingLedger ?? 1000;
    this.latestLedger = this.nextLedger - 1;
  }

  async submit(spec: CompetingTxSpec): Promise<{ hash: string }> {
    if (this.submitFailureRate > 0 && this.random() < this.submitFailureRate) {
      throw new Error(`mock: submission rejected for ${spec.id}`);
    }
    this.hashCounter += 1;
    const hash = `mock-${this.hashCounter}-${spec.id}`;
    this.pending.push({ spec, hash });
    return { hash };
  }

  async observe(hash: string): Promise<RawObservation> {
    const confirmed = this.confirmed.get(hash);
    if (!confirmed) {
      return { outcome: "NOT_FOUND", ledger: null, applicationOrder: null, error: null };
    }
    if (confirmed.ledger > this.latestLedger - this.rpcLagLedgers) {
      return { outcome: "NOT_FOUND", ledger: null, applicationOrder: null, error: null };
    }
    return {
      outcome: confirmed.outcome,
      ledger: confirmed.ledger,
      applicationOrder: confirmed.applicationOrder,
      error: null,
    };
  }

  /** Close one ledger, applying at most one transaction per source account. */
  async advance(): Promise<void> {
    const batch = this.pending;
    this.pending = [];

    const ordered = [...batch].sort((a, b) => this.compare(a.spec, b.spec));
    const usedSources = new Set<number>();
    const deferred: PendingTx[] = [];
    let applicationOrder = 0;

    for (const transaction of ordered) {
      if (usedSources.has(transaction.spec.sourceIndex)) {
        deferred.push(transaction);
        continue;
      }
      usedSources.add(transaction.spec.sourceIndex);
      applicationOrder += 1;
      this.confirmed.set(transaction.hash, {
        ledger: this.nextLedger,
        applicationOrder,
        outcome: "SUCCESS",
      });
    }

    this.pending = deferred;
    this.latestLedger = this.nextLedger;
    this.nextLedger += 1;
  }

  private compare(a: CompetingTxSpec, b: CompetingTxSpec): number {
    if (!this.disableFeePriority && a.fee !== b.fee) {
      return b.fee - a.fee; // higher fee executed first
    }
    if (this.equalFeeTieBreak === "reverse") {
      return b.submitOrder - a.submitOrder;
    }
    return a.submitOrder - b.submitOrder;
  }
}
