/**
 * CLI entrypoint for the transaction-ordering observation harness.
 *
 *   npm run harness                    # mock (default)
 *   npm run harness -- --mode=testnet  # observe the live testnet
 *   npm run harness -- --rounds=6 --accounts=5 --output-dir=results
 *
 * Exits 0 when no hard policy deviation exceeds `MEV_MAX_HARD_DEVIATIONS`,
 * 1 when the ordering policy regressed, and 2 when the harness itself failed.
 */

import { assertSafeTarget, config } from "./config";
import {
  buildRoundPlans,
  defaultOptions,
  requiredAccountCount,
  runHarness,
} from "./harness";
import { logger } from "./logger";
import { MockChain } from "./mockChain";
import { createTestnetClient } from "./testnetClient";
import type { HarnessMode, HarnessReport } from "./types";

interface CliArgs {
  mode?: HarnessMode;
  rounds?: number;
  accounts?: number;
  outputDir?: string;
  help: boolean;
}

const USAGE = `Kora MEV ordering observation harness

Usage: npm run harness -- [options]

Options:
  --mode=mock|testnet   Observation target (default: MEV_MODE, "mock")
  --rounds=N            Number of competition rounds (default: MEV_ROUNDS)
  --accounts=N          Competing accounts per round (default: MEV_ACCOUNTS_PER_ROUND)
  --output-dir=PATH     Artifact directory (default: MEV_OUTPUT_DIR)
  --help                Show this message

Environment variables are documented in infra/mev-ordering/README.md.
`;

function parseArgs(argv: string[]): CliArgs {
  const args: CliArgs = { help: false };
  for (const token of argv) {
    if (token === "--help" || token === "-h") {
      args.help = true;
      continue;
    }
    const [flag, value] = token.split("=");
    switch (flag) {
      case "--mode": {
        if (value !== "mock" && value !== "testnet") {
          throw new Error(`--mode must be "mock" or "testnet", got "${value ?? ""}"`);
        }
        args.mode = value;
        break;
      }
      case "--rounds": {
        const parsed = Number.parseInt(value ?? "", 10);
        if (!Number.isFinite(parsed)) throw new Error(`--rounds must be an integer, got "${value ?? ""}"`);
        args.rounds = parsed;
        break;
      }
      case "--accounts": {
        const parsed = Number.parseInt(value ?? "", 10);
        if (!Number.isFinite(parsed)) throw new Error(`--accounts must be an integer, got "${value ?? ""}"`);
        args.accounts = parsed;
        break;
      }
      case "--output-dir":
        args.outputDir = value;
        break;
      default:
        throw new Error(`Unknown argument "${token}". Use --help.`);
    }
  }
  return args;
}

function formatReport(report: HarnessReport): string {
  const lines: string[] = [];
  const { summary } = report;
  lines.push(
    `MEV ordering observation — mode=${report.mode} result=${summary.passed ? "PASS" : "FAIL"}`,
  );
  lines.push(
    `rounds=${summary.rounds} txs=${summary.txsPlanned} submitted=${summary.txsSubmitted} ` +
      `observed=${summary.txsObserved} samples=${summary.orderingSamples} ` +
      `hard=${summary.hardDeviations} advisory=${summary.advisoryDeviations}`,
  );
  lines.push("");
  for (const round of report.rounds) {
    const sequences = round.samples
      .map((sample) => `ledger ${sample.ledger}: ${sample.observedSequence.join(" -> ")}`)
      .join(" | ");
    lines.push(`round ${round.index} (${round.kind}) ${sequences || "no settled ledger sample"}`);
  }
  if (report.deviations.length > 0) {
    lines.push("");
    lines.push("deviations:");
    for (const deviation of report.deviations) {
      lines.push(`  [${deviation.severity}] ${deviation.code}: ${deviation.detail}`);
    }
  }
  return lines.join("\n");
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.log(USAGE);
    return;
  }

  const mode: HarnessMode = args.mode ?? config.mode;
  const options = defaultOptions({
    ...(args.rounds !== undefined ? { rounds: args.rounds } : {}),
    ...(args.accounts !== undefined ? { accountsPerRound: args.accounts } : {}),
    ...(args.outputDir !== undefined ? { outputDir: args.outputDir } : {}),
  });

  if (mode === "testnet") {
    assertSafeTarget();
  }

  const client =
    mode === "mock"
      ? new MockChain()
      : await createTestnetClient({
          rpcUrl: config.rpcUrl,
          networkPassphrase: config.networkPassphrase,
          sourceSecrets: config.sourceSecrets,
          requiredAccounts: requiredAccountCount(buildRoundPlans(options)),
          autoFund: config.autoFundAccounts,
          friendbotUrl: config.friendbotUrl,
        });

  const report = await runHarness(client, options);
  console.log(formatReport(report));
  process.exitCode = report.summary.passed ? 0 : 1;
}

main().catch((error: unknown) => {
  logger.error(
    { err: error instanceof Error ? error.message : String(error) },
    "mev ordering harness failed",
  );
  process.exitCode = 2;
});
