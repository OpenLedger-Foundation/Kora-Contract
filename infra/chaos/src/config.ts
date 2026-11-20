/**
 * Chaos suite configuration.
 *
 * All values are read from environment variables so the suite works
 * identically in local dev and CI.  Every variable has a safe default
 * that points at the staging stack.
 *
 * IMPORTANT: this suite must NEVER be pointed at production.
 * The CHAOS_ENV guard in global-setup.ts enforces this.
 */

import * as dotenv from "dotenv";
dotenv.config({ path: __dirname + "/../.env.chaos" });
import { positiveMilliseconds } from "./dbTimeout";

function env(key: string, fallback: string): string {
  return process.env[key] ?? fallback;
}

export const config = {
  /** Human-readable environment tag; must not be "production". */
  env: env("CHAOS_ENV", "staging"),

  /** Base URL of the Kora API service under test. */
  apiBaseUrl: env("CHAOS_API_URL", "http://localhost:3000"),

  /** Base URL of the indexer service admin/health endpoint. */
  indexerAdminUrl: env("CHAOS_INDEXER_ADMIN_URL", "http://localhost:4000"),

  /** Keeper service admin/health endpoint. */
  keeperAdminUrl: env("CHAOS_KEEPER_ADMIN_URL", "http://localhost:8080"),

  /**
   * Docker / docker-compose project name used to pause/resume containers.
   * When empty the suite falls back to tc-netem / iptables for network
   * partition injection (useful in non-Docker staging setups).
   */
  dockerProject: env("CHAOS_DOCKER_PROJECT", "kora-staging"),

  /** Name of the database container (for latency injection). */
  dbContainer: env("CHAOS_DB_CONTAINER", "kora-postgres"),

  /** Name of the indexer container. */
  indexerContainer: env("CHAOS_INDEXER_CONTAINER", "kora-indexer"),

  /** Name of the keeper container. */
  keeperContainer: env("CHAOS_KEEPER_CONTAINER", "kora-keeper"),

  /** Name of the API container. */
  apiContainer: env("CHAOS_API_CONTAINER", "kora-api"),

  /**
   * Milliseconds to wait for a service to recover after failure injection
   * is removed before asserting the resilience claim.
   */
  recoveryTimeoutMs: parseInt(env("CHAOS_RECOVERY_TIMEOUT_MS", "30000"), 10),

  /**
   * Milliseconds to hold an injected failure before beginning cleanup.
   * Long enough to let the service notice and react, short enough not to
   * leave staging broken if the test runner is killed.
   */
  failureDurationMs: parseInt(env("CHAOS_FAILURE_DURATION_MS", "10000"), 10),

  /** Artificial DB latency in milliseconds injected via tc-netem. */
  dbLatencyMs: parseInt(env("CHAOS_DB_LATENCY_MS", "2000"), 10),

  /** API PostgreSQL statement timeout; invalid zero/negative values fail startup. */
  dbStatementTimeoutMs: positiveMilliseconds("DB_STATEMENT_TIMEOUT_MS", "5000"),

  /**
   * Name of the mock Stellar RPC container that the indexer talks to.
   * Stopping this container simulates an RPC outage.
   */
  rpcContainer: env("CHAOS_RPC_CONTAINER", "kora-stellar-rpc-mock"),

  /**
   * Network name (Docker) bridging the API container to the DB container.
   * Disconnecting the API from this network simulates a network partition.
   */
  apiDbNetwork: env("CHAOS_API_DB_NETWORK", "kora-internal"),

  /** How many times to poll a health endpoint while waiting for recovery. */
  healthPollIntervalMs: parseInt(env("CHAOS_HEALTH_POLL_MS", "1000"), 10),

  /**
   * Maximum tolerated lag between a ledger gap being created and a
   * reconciliation alert being emitted, in milliseconds.  The reconciler
   * is expected to fire an alert within this window.
   */
  reconcilerAlertLagMaxMs: parseInt(env("CHAOS_RECONCILER_ALERT_LAG_MS", "15000"), 10),

  /**
   * How many synthetic ledger advances to inject during the indexer
   * checkpointing scenario so we have concrete new events to count.
   */
  mockLedgerAdvanceCount: parseInt(env("CHAOS_MOCK_LEDGER_COUNT", "3"), 10),
} as const;
