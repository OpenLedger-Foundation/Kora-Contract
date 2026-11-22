/**
 * Expected-policy checks and deviation detection.
 *
 * The rules encode the policy documented in `docs/MEV_RESISTANCE_NOTES.md`:
 *
 * | Rule | Documented expectation | Severity |
 * |------|------------------------|----------|
 * | Fee priority | During surge, higher fees are executed first | advisory* |
 * | Arrival order | Equal fees execute in arrival order | advisory** |
 * | Per-account serialization | 1 tx per source account per ledger | hard |
 * | Complete observation | Every submitted tx is observed | advisory |
 *
 * (*) The documented expectation is conditional on surge pricing (§1.1), which
 * a handful of probes cannot induce — ledger capacity far exceeds the probing
 * volume, so an idle ledger applies arrival order. Fee-priority mismatches are
 * therefore advisory unless `MEV_EXPECT_FEE_PRIORITY=true` asserts that the
 * target is congested.
 * (**) Arrival order is advisory by default because client-side submission
 * order is not the network's arrival order; set `MEV_STRICT_ARRIVAL_ORDER=true`
 * to promote it to a hard failure.
 *
 * Everything here is pure: it takes recorded observations and returns
 * deviations, so the policy can be unit-tested without touching a network.
 */

import type {
  Deviation,
  DeviationCode,
  DeviationSeverity,
  HarnessSummary,
  HarnessThresholds,
  LedgerOrderingSample,
  OrderingSampleEntry,
  RoundResult,
  TxObservation,
} from "./types";

export function severityOf(
  code: DeviationCode,
  thresholds: HarnessThresholds,
): DeviationSeverity {
  switch (code) {
    case "FEE_PRIORITY_VIOLATION":
      // Only a hard failure when congestion (surge pricing) is asserted.
      return thresholds.expectFeePriority ? "hard" : "advisory";
    case "PER_ACCOUNT_SERIALIZATION_VIOLATION":
      return "hard";
    case "ARRIVAL_ORDER_VIOLATION":
      return thresholds.strictArrivalOrder ? "hard" : "advisory";
    case "MISSING_ORDERING_DATA":
    case "PARTIAL_SUBMISSION_FAILURE":
      return "advisory";
  }
}

function makeDeviation(
  code: DeviationCode,
  thresholds: HarnessThresholds,
  roundIndex: number,
  ledger: number | null,
  detail: string,
  txIds: string[],
): Deviation {
  return {
    code,
    severity: severityOf(code, thresholds),
    roundIndex,
    ledger,
    detail,
    txIds,
  };
}

/**
 * Group successfully observed transactions by the ledger they landed in and
 * order them by the RPC-provided `applicationOrder`.
 */
export function buildLedgerOrderingSamples(
  observations: TxObservation[],
): LedgerOrderingSample[] {
  const byLedger = new Map<number, OrderingSampleEntry[]>();
  for (const observation of observations) {
    if (
      observation.outcome !== "SUCCESS" ||
      observation.ledger === null ||
      observation.applicationOrder === null
    ) {
      continue;
    }
    const bucket = byLedger.get(observation.ledger) ?? [];
    bucket.push({
      id: observation.id,
      sourceIndex: observation.sourceIndex,
      fee: observation.fee,
      submitOrder: observation.submitOrder,
      applicationOrder: observation.applicationOrder,
    });
    byLedger.set(observation.ledger, bucket);
  }

  return [...byLedger.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([ledger, entries]) => {
      entries.sort((a, b) => a.applicationOrder - b.applicationOrder);
      return {
        ledger,
        entries,
        observedSequence: entries.map((entry) => entry.id),
      };
    });
}

/**
 * Fee priority: under surge pricing a transaction is never executed before a
 * transaction that offered a strictly higher fee. Outside surge pricing the
 * ledger is not congested and arrival order governs, so this is only reported
 * as a hard deviation when `expectFeePriority` asserts congestion.
 */
function checkFeePriority(
  sample: LedgerOrderingSample,
  roundIndex: number,
  thresholds: HarnessThresholds,
): Deviation[] {
  const offenders: string[] = [];
  const { entries } = sample;
  for (let i = 0; i < entries.length; i += 1) {
    for (let j = i + 1; j < entries.length; j += 1) {
      if (entries[i].fee < entries[j].fee) {
        offenders.push(`${entries[i].id} (fee=${entries[i].fee}) before ${entries[j].id} (fee=${entries[j].fee})`);
        break;
      }
    }
  }
  if (offenders.length === 0) return [];

  return [
    makeDeviation(
      "FEE_PRIORITY_VIOLATION",
      thresholds,
      roundIndex,
      sample.ledger,
      `Higher-fee transaction executed after a lower-fee one in ledger ${sample.ledger}: ` +
        `${offenders.join("; ")}. Observed sequence: ${sample.observedSequence.join(" -> ")}`,
      entries.map((entry) => entry.id),
    ),
  ];
}

/**
 * Arrival order: transactions that offered the same fee should execute in the
 * order they were submitted.
 */
function checkArrivalOrder(
  sample: LedgerOrderingSample,
  roundIndex: number,
  thresholds: HarnessThresholds,
): Deviation[] {
  const equalFeeGroups = new Map<number, OrderingSampleEntry[]>();
  for (const entry of sample.entries) {
    const group = equalFeeGroups.get(entry.fee) ?? [];
    group.push(entry);
    equalFeeGroups.set(entry.fee, group);
  }

  const deviations: Deviation[] = [];
  for (const [fee, group] of equalFeeGroups.entries()) {
    if (group.length < 2) continue;
    for (let i = 0; i < group.length - 1; i += 1) {
      if (group[i].submitOrder > group[i + 1].submitOrder) {
        deviations.push(
          makeDeviation(
            "ARRIVAL_ORDER_VIOLATION",
            thresholds,
            roundIndex,
            sample.ledger,
            `Equal-fee (fee=${fee}) transactions did not execute in submission order in ` +
              `ledger ${sample.ledger}: ${group.map((entry) => `${entry.id}(submitted#${entry.submitOrder})`).join(" -> ")}.`,
            group.map((entry) => entry.id),
          ),
        );
        break;
      }
    }
  }
  return deviations;
}

/**
 * Stellar allows at most one transaction per source account per ledger. Two of
 * the harness's transactions from the same account landing in one ledger means
 * the serialization guarantee regressed.
 */
function checkSerialization(
  observations: TxObservation[],
  roundIndex: number,
  thresholds: HarnessThresholds,
): Deviation[] {
  const perSource = new Map<number, Map<number, string[]>>();
  for (const observation of observations) {
    if (observation.ledger === null) continue;
    if (observation.outcome !== "SUCCESS" && observation.outcome !== "FAILED") continue;
    const ledgers = perSource.get(observation.sourceIndex) ?? new Map<number, string[]>();
    const ids = ledgers.get(observation.ledger) ?? [];
    ids.push(observation.id);
    ledgers.set(observation.ledger, ids);
    perSource.set(observation.sourceIndex, ledgers);
  }

  const deviations: Deviation[] = [];
  for (const [sourceIndex, ledgers] of perSource.entries()) {
    for (const [ledger, ids] of ledgers.entries()) {
      if (ids.length > 1) {
        deviations.push(
          makeDeviation(
            "PER_ACCOUNT_SERIALIZATION_VIOLATION",
            thresholds,
            roundIndex,
            ledger,
            `Source account #${sourceIndex} had ${ids.length} transactions in ledger ${ledger}: ${ids.join(", ")}.`,
            ids,
          ),
        );
      }
    }
  }
  return deviations;
}

/**
 * Edge-case handling: transactions the harness could not submit and ones that
 * never settled within the observation deadline are recorded as advisory
 * deviations rather than silently dropped.
 */
function checkOutcomes(
  observations: TxObservation[],
  roundIndex: number,
  thresholds: HarnessThresholds,
): Deviation[] {
  const deviations: Deviation[] = [];

  const failed = observations.filter(
    (observation) => observation.outcome === "SUBMIT_ERROR" || observation.outcome === "FAILED",
  );
  if (failed.length > 0) {
    deviations.push(
      makeDeviation(
        "PARTIAL_SUBMISSION_FAILURE",
        thresholds,
        roundIndex,
        null,
        `${failed.length}/${observations.length} transactions did not settle successfully in round ${roundIndex}.`,
        failed.map((observation) => observation.id),
      ),
    );
  }

  const unresolved = observations.filter(
    (observation) => observation.outcome === "PENDING" || observation.outcome === "NOT_FOUND",
  );
  if (unresolved.length > 0) {
    const lagged = unresolved.some((observation) => observation.hash !== null);
    deviations.push(
      makeDeviation(
        "MISSING_ORDERING_DATA",
        thresholds,
        roundIndex,
        null,
        `${unresolved.length} transaction(s) had no ordering data before the observation ` +
          `deadline (${lagged ? "RPC lag or delayed ledger inclusion" : "submission failure"}) in round ${roundIndex}.`,
        unresolved.map((observation) => observation.id),
      ),
    );
  }

  return deviations;
}

/** Evaluate every policy rule for one round. */
export function evaluateRound(
  roundIndex: number,
  observations: TxObservation[],
  samples: LedgerOrderingSample[],
  thresholds: HarnessThresholds,
): Deviation[] {
  const deviations: Deviation[] = [];
  for (const sample of samples) {
    deviations.push(...checkFeePriority(sample, roundIndex, thresholds));
    deviations.push(...checkArrivalOrder(sample, roundIndex, thresholds));
  }
  deviations.push(...checkSerialization(observations, roundIndex, thresholds));
  deviations.push(...checkOutcomes(observations, roundIndex, thresholds));
  return deviations;
}

/** Roll rounds up into the report summary, deciding pass/fail. */
export function summarize(
  rounds: RoundResult[],
  txsPlanned: number,
  thresholds: HarnessThresholds,
): HarnessSummary {
  const deviations = rounds.flatMap((round) => round.deviations);
  const hardDeviations = deviations.filter((deviation) => deviation.severity === "hard").length;
  const advisoryDeviations = deviations.length - hardDeviations;
  const observations = rounds.flatMap((round) => round.observations);

  return {
    rounds: rounds.length,
    txsPlanned,
    txsSubmitted: observations.filter((observation) => observation.hash !== null).length,
    txsObserved: observations.filter((observation) => observation.outcome === "SUCCESS").length,
    submissionFailures: observations.filter(
      (observation) => observation.outcome === "SUBMIT_ERROR" || observation.outcome === "FAILED",
    ).length,
    hardDeviations,
    advisoryDeviations,
    orderingSamples: rounds.reduce((count, round) => count + round.samples.length, 0),
    passed: hardDeviations <= thresholds.maxHardDeviations,
  };
}
