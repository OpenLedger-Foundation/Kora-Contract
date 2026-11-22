/**
 * Orchestration: build controlled competition rounds, submit them, record what
 * the network did, flag deviations, and persist a JSON artifact.
 *
 * The harness is deliberately thin — ordering policy lives in `policy.ts` and
 * network access lives behind `OrderingClient`, so the same code drives the
 * in-process mock and the live testnet RPC.
 */

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { config, assertSafeTarget } from "./config";
import { logger } from "./logger";
import { buildLedgerOrderingSamples, evaluateRound, summarize } from "./policy";
import type {
  CompetingTxSpec,
  HarnessReport,
  HarnessThresholds,
  OrderingClient,
  RoundKind,
  RoundPlan,
  RoundResult,
  TxObservation,
} from "./types";

export interface HarnessOptions {
  rounds: number;
  accountsPerRound: number;
  baseFee: number;
  feeStep: number;
  thresholds: HarnessThresholds;
  /** Directory for the JSON artifact; `null` skips artifact writing. */
  outputDir: string | null;
  /** Injectable clock (defaults to `Date.now`). */
  now?: () => number;
  /** Injectable sleep (defaults to a real timer). */
  sleep?: (ms: number) => Promise<void>;
}

/** `HarnessOptions` with the injectable clock/sleep resolved to concrete values. */
type ResolvedOptions = HarnessOptions & {
  now: () => number;
  sleep: (ms: number) => Promise<void>;
};

function resolveOptions(options: HarnessOptions): ResolvedOptions {
  return {
    ...options,
    now: options.now ?? (() => Date.now()),
    sleep: options.sleep ?? defaultSleep,
  };
}

/** Cap on observation poll iterations, so a stuck client cannot spin forever. */
const MAX_OBSERVE_ATTEMPTS = 200;

const ROUND_KINDS: RoundKind[] = ["fee_ladder", "equal_fee", "same_account_burst"];

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Assemble a round plan for one competition kind. */
function buildRound(index: number, kind: RoundKind, options: HarnessOptions): RoundPlan {
  if (kind === "same_account_burst") {
    // Two transactions from one source account. Stellar allows at most one
    // transaction per source account per ledger, so these are submitted one at
    // a time (see `submitSerializedRound`) and must land in different ledgers.
    return {
      index,
      kind,
      txs: [
        { id: `${kind}-0`, sourceIndex: 0, fee: options.baseFee, submitOrder: 0 },
        { id: `${kind}-1`, sourceIndex: 0, fee: options.baseFee + options.feeStep, submitOrder: 1 },
      ],
    };
  }

  const txs: CompetingTxSpec[] = [];
  for (let i = 0; i < options.accountsPerRound; i += 1) {
    txs.push({
      id: `${kind}-${i}`,
      sourceIndex: i,
      // `equal_fee` keeps every fee identical to probe arrival-order semantics.
      fee: kind === "equal_fee" ? options.baseFee : options.baseFee + i * options.feeStep,
      submitOrder: i,
    });
  }
  return { index, kind, txs };
}

/** Deterministic round plan; kinds cycle so one run exercises all three. */
export function buildRoundPlans(options: HarnessOptions): RoundPlan[] {
  const plans: RoundPlan[] = [];
  for (let i = 0; i < options.rounds; i += 1) {
    plans.push(buildRound(i, ROUND_KINDS[i % ROUND_KINDS.length], options));
  }
  return plans;
}

/** How many distinct source accounts the plan needs. */
export function requiredAccountCount(plans: RoundPlan[]): number {
  return plans.reduce(
    (max, plan) =>
      plan.txs.reduce((inner, tx) => Math.max(inner, tx.sourceIndex + 1), max),
    1,
  );
}

function isSettled(observation: TxObservation): boolean {
  return observation.outcome !== "PENDING" && observation.outcome !== "NOT_FOUND";
}

/** Submit the round's transactions as close together as configured. */
async function submitRound(
  client: OrderingClient,
  plan: RoundPlan,
  options: ResolvedOptions,
): Promise<TxObservation[]> {
  const now = options.now;
  const sleep = options.sleep;
  const stagger = options.thresholds.submitStaggerMs;

  const tasks = plan.txs.map((spec) =>
    (async (): Promise<TxObservation> => {
      if (stagger > 0 && spec.submitOrder > 0) {
        await sleep(stagger * spec.submitOrder);
      }
      const submittedAtMs = now();
      const base = {
        id: spec.id,
        sourceIndex: spec.sourceIndex,
        fee: spec.fee,
        submitOrder: spec.submitOrder,
      };
      try {
        const { hash } = await client.submit(spec);
        return {
          ...base,
          hash,
          outcome: "PENDING",
          ledger: null,
          applicationOrder: null,
          submittedAtMs,
          observedAtMs: submittedAtMs,
          error: null,
        };
      } catch (error) {
        return {
          ...base,
          hash: null,
          outcome: "SUBMIT_ERROR",
          ledger: null,
          applicationOrder: null,
          submittedAtMs,
          observedAtMs: now(),
          error: errorMessage(error),
        };
      }
    })(),
  );

  return Promise.all(tasks);
}

/**
 * Submit same-source transactions one at a time, waiting for each to settle
 * before firing the next. Stellar rejects future sequence numbers, so a second
 * transaction from one account cannot be pre-queued alongside the first.
 */
async function submitSerializedRound(
  client: OrderingClient,
  plan: RoundPlan,
  options: ResolvedOptions,
): Promise<TxObservation[]> {
  const observations: TxObservation[] = [];
  for (const spec of plan.txs) {
    const [observation] = await submitRound(client, { ...plan, txs: [spec] }, options);
    observations.push(observation);
    if (observation.hash !== null) {
      // Only one transaction per source account can be included per ledger, so
      // the next probe is not valid until this one has been included.
      await observeRound(client, [observation], options);
    }
  }
  return observations;
}

/**
 * Poll until every transaction has settled or the deadline passes. Mock runs
 * additionally close ledgers between polls; testnet runs simply wait, which is
 * how RPC lag and delayed inclusion are observed rather than assumed.
 */
async function observeRound(
  client: OrderingClient,
  observations: TxObservation[],
  options: ResolvedOptions,
): Promise<void> {
  const now = options.now;
  const sleep = options.sleep;
  const deadline = now() + options.thresholds.observeTimeoutMs;

  for (let attempt = 0; attempt < MAX_OBSERVE_ATTEMPTS; attempt += 1) {
    if (observations.every(isSettled)) return;
    if (now() >= deadline) break;

    for (const observation of observations) {
      if (isSettled(observation) || observation.hash === null) continue;
      try {
        const raw = await client.observe(observation.hash);
        if (raw.outcome !== "NOT_FOUND") {
          observation.outcome = raw.outcome;
          observation.ledger = raw.ledger;
          observation.applicationOrder = raw.applicationOrder;
          observation.error = raw.error;
        }
        observation.observedAtMs = now();
      } catch (error) {
        observation.error = errorMessage(error);
        observation.observedAtMs = now();
      }
    }

    if (client.advance) {
      await client.advance();
    } else {
      await sleep(options.thresholds.pollIntervalMs);
    }
  }

  // Anything still unresolved is recorded as pending, not silently dropped.
  for (const observation of observations) {
    if (!isSettled(observation)) {
      observation.outcome = "PENDING";
      observation.observedAtMs = now();
    }
  }
}

async function runRound(
  client: OrderingClient,
  plan: RoundPlan,
  options: ResolvedOptions,
): Promise<RoundResult> {
  logger.info({ round: plan.index, kind: plan.kind, txs: plan.txs.length }, "submitting round");

  const observations =
    plan.kind === "same_account_burst"
      ? await submitSerializedRound(client, plan, options)
      : await submitRound(client, plan, options);
  await observeRound(client, observations, options);

  const samples = buildLedgerOrderingSamples(observations);
  const deviations = evaluateRound(plan.index, observations, samples, options.thresholds);

  for (const deviation of deviations) {
    const context = { code: deviation.code, ledger: deviation.ledger };
    if (deviation.severity === "hard") {
      logger.error(context, deviation.detail);
    } else {
      logger.warn(context, deviation.detail);
    }
  }

  return { index: plan.index, kind: plan.kind, observations, samples, deviations };
}

/** Serialize a report to `mev-ordering-<timestamp>.json` and `-latest.json`. */
export function writeReportArtifact(
  report: HarnessReport,
  outputDir: string,
): { timestampedPath: string; latestPath: string } {
  mkdirSync(outputDir, { recursive: true });
  const serialized = JSON.stringify(report, null, 2);
  const stamp = report.generatedAt.replace(/[:.]/g, "-");
  const timestampedPath = join(outputDir, `mev-ordering-${stamp}.json`);
  const latestPath = join(outputDir, "mev-ordering-latest.json");
  writeFileSync(timestampedPath, serialized, "utf8");
  writeFileSync(latestPath, serialized, "utf8");
  return { timestampedPath, latestPath };
}

export function defaultOptions(overrides: Partial<HarnessOptions> = {}): HarnessOptions {
  return {
    rounds: config.rounds,
    accountsPerRound: config.accountsPerRound,
    baseFee: config.baseFee,
    feeStep: config.feeStep,
    thresholds: {
      expectFeePriority: config.expectFeePriority,
      strictArrivalOrder: config.strictArrivalOrder,
      maxHardDeviations: config.maxHardDeviations,
      observeTimeoutMs: config.observeTimeoutMs,
      pollIntervalMs: config.pollIntervalMs,
      submitStaggerMs: config.submitStaggerMs,
    },
    outputDir: config.outputDir,
    ...overrides,
  };
}

/** Run every round against the supplied client and produce the report. */
export async function runHarness(
  client: OrderingClient,
  options: HarnessOptions,
): Promise<HarnessReport> {
  if (client.mode === "testnet") {
    // Fail closed if the configured target looks like production.
    assertSafeTarget();
  }

  const resolved = resolveOptions(options);
  const plans = buildRoundPlans(resolved);

  logger.info(
    { mode: client.mode, rounds: plans.length, accounts: requiredAccountCount(plans) },
    "mev ordering harness starting",
  );

  const rounds: RoundResult[] = [];
  for (const plan of plans) {
    rounds.push(await runRound(client, plan, resolved));
  }

  const summary = summarize(
    rounds,
    plans.reduce((count, plan) => count + plan.txs.length, 0),
    resolved.thresholds,
  );

  const report: HarnessReport = {
    schemaVersion: 1,
    generatedAt: new Date(resolved.now()).toISOString(),
    mode: client.mode,
    network:
      client.mode === "testnet"
        ? { rpcUrl: config.rpcUrl, networkPassphrase: config.networkPassphrase }
        : { rpcUrl: "mock://in-process", networkPassphrase: "Mock Network ; Observation Only" },
    thresholds: resolved.thresholds,
    rounds,
    deviations: rounds.flatMap((round) => round.deviations),
    orderingSamples: rounds.flatMap((round) => round.samples),
    summary,
  };

  if (resolved.outputDir) {
    const paths = writeReportArtifact(report, resolved.outputDir);
    logger.info(paths, "wrote ordering artifact");
  }

  logger.info(summary, summary.passed ? "PASS — ordering matches policy" : "FAIL — policy deviations detected");
  return report;
}
