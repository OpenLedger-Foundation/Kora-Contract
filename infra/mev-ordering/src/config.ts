/**
 * Transaction-ordering observation harness configuration.
 *
 * Every value is read from the environment so the harness behaves identically
 * locally and in the nightly CI job. Defaults target a harmless local mock so
 * that `npm run harness` never touches a live network unless asked to.
 *
 * IMPORTANT: this harness must NEVER be pointed at mainnet. The safety guard in
 * `assertSafeTarget()` refuses public-network RPC URLs and passphrases, and is
 * enforced both by the CLI entrypoint and by the Jest global setup.
 */

import * as dotenv from "dotenv";

dotenv.config({ path: __dirname + "/../.env.mev" });

export type HarnessMode = "mock" | "testnet";

/** Public mainnet Soroban RPC endpoint — always refused. */
export const MAINNET_RPC_URL = "https://soroban.stellar.org";

/** Public testnet Soroban RPC endpoint — the default observation target. */
export const TESTNET_RPC_URL = "https://soroban-testnet.stellar.org";

/**
 * Stellar network passphrases. Inlined rather than taken from the SDK so that
 * config (and therefore the unit-test path) stays free of the SDK dependency.
 */
export const TESTNET_NETWORK_PASSPHRASE = "Test SDF Network ; September 2015";
export const MAINNET_NETWORK_PASSPHRASE = "Public Global Stellar Network ; September 2015";

/** Data-entry name used by the side-effect-free `manageData` probe operation. */
export const PROBE_DATA_KEY = "kora-mev-ordering-probe";

function env(key: string, fallback: string): string {
  const raw = process.env[key];
  return raw === undefined || raw === "" ? fallback : raw;
}

/** Parse an integer env var, failing loudly on non-numeric input. */
export function envInt(key: string, fallback: number): number {
  const raw = process.env[key];
  if (raw === undefined || raw === "") return fallback;
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed)) {
    throw new Error(`MEV harness: ${key} must be an integer, got "${raw}"`);
  }
  return parsed;
}

/** Parse a boolean env var; accepts `true`/`1` (case-insensitive). */
export function envBool(key: string, fallback: boolean): boolean {
  const raw = process.env[key];
  if (raw === undefined || raw === "") return fallback;
  return ["true", "1"].includes(raw.toLowerCase());
}

/** Parse the harness mode, rejecting anything other than `mock`/`testnet`. */
export function parseMode(raw: string): HarnessMode {
  if (raw !== "mock" && raw !== "testnet") {
    throw new Error(`MEV harness: MEV_MODE must be "mock" or "testnet", got "${raw}"`);
  }
  return raw;
}

export const config = {
  /** `mock` runs an in-process chain model; `testnet` drives the real RPC. */
  mode: parseMode(env("MEV_MODE", "mock")),

  /** Environment tag; `production` is refused outright. */
  env: env("MEV_ENV", "staging"),

  /** Soroban RPC endpoint used in `testnet` mode. */
  rpcUrl: env("MEV_RPC_URL", TESTNET_RPC_URL),

  /** Network passphrase matching `rpcUrl`. */
  networkPassphrase: env("MEV_NETWORK_PASSPHRASE", TESTNET_NETWORK_PASSPHRASE),

  /** Comma-separated source-account secret keys (optional; see README). */
  sourceSecrets: env("MEV_SOURCE_SECRETS", "")
    .split(",")
    .map((secret) => secret.trim())
    .filter(Boolean),

  /** Create + fund ephemeral testnet accounts through friendbot when short. */
  autoFundAccounts: envBool("MEV_AUTOFUND_ACCOUNTS", true),

  /** Optional explicit friendbot URL (blank = let the SDK discover it). */
  friendbotUrl: env("MEV_FRIENDBOT_URL", ""),

  /** Competing transactions (and therefore distinct source accounts) per round. */
  accountsPerRound: envInt("MEV_ACCOUNTS_PER_ROUND", 4),

  /** Number of observation rounds to run. */
  rounds: envInt("MEV_ROUNDS", 3),

  /**
   * Base fee (stroops) used for the cheapest transaction in a round. Fee tiers
   * are derived as `baseFee + rank * feeStep`.
   */
  baseFee: envInt("MEV_BASE_FEE", 100),

  /** Fee increment (stroops) between adjacent tiers in a round. */
  feeStep: envInt("MEV_FEE_STEP", 500),

  /** Delay between competing submissions; 0 fires them all concurrently. */
  submitStaggerMs: envInt("MEV_SUBMIT_STAGGER_MS", 0),

  /** Bounded wait for a submitted transaction to reach a settled ledger. */
  observeTimeoutMs: envInt("MEV_OBSERVE_TIMEOUT_MS", 30000),

  /** Delay between observation polls in `testnet` mode. */
  pollIntervalMs: envInt("MEV_POLL_INTERVAL_MS", 1000),

  /**
   * Assert that the target ledger is congested (surge pricing active), which is
   * the only regime where fee priority is guaranteed. Off by default because a
   * probe-sized transaction burst cannot exceed ledger capacity.
   */
  expectFeePriority: envBool("MEV_EXPECT_FEE_PRIORITY", false),

  /**
   * Treat equal-fee arrival-order mismatches as hard failures. Off by default
   * because client-side submission order is not the network arrival order.
   */
  strictArrivalOrder: envBool("MEV_STRICT_ARRIVAL_ORDER", false),

  /** Tolerated hard deviations before the run is considered failed. */
  maxHardDeviations: envInt("MEV_MAX_HARD_DEVIATIONS", 0),

  /** Directory for the JSON artifact; created on demand. */
  outputDir: env("MEV_OUTPUT_DIR", __dirname + "/../results"),
} as const;

export interface SafetyTarget {
  mode: HarnessMode;
  env: string;
  rpcUrl: string;
  networkPassphrase: string;
}

export interface SafetyVerdict {
  safe: boolean;
  reason?: string;
}

/** Pure safety check — used directly by tests and by `assertSafeTarget`. */
export function isSafeTarget(target: SafetyTarget): SafetyVerdict {
  if (target.mode === "mock") return { safe: true };
  if (target.env === "production") {
    return { safe: false, reason: "MEV_ENV is 'production'." };
  }
  if (target.rpcUrl.replace(/\/+$/, "") === MAINNET_RPC_URL) {
    return { safe: false, reason: `MEV_RPC_URL points at mainnet (${MAINNET_RPC_URL}).` };
  }
  if (target.networkPassphrase === MAINNET_NETWORK_PASSPHRASE) {
    return { safe: false, reason: "MEV_NETWORK_PASSPHRASE is the Stellar public network." };
  }
  return { safe: true };
}

/**
 * Guard invoked before any observation run. Throws when the configured target
 * looks like production so a misconfigured nightly job fails closed.
 */
export function assertSafeTarget(target: SafetyTarget = config): void {
  const verdict = isSafeTarget(target);
  if (!verdict.safe) {
    throw new Error(
      `MEV ORDERING HARNESS REFUSED: ${verdict.reason} ` +
        "This harness only observes mock chains or non-production networks.",
    );
  }
}
