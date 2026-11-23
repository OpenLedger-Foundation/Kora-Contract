/**
 * Keeper Service — Entry Point
 *
 * Wires together JobStore, Dispatcher, and Scheduler.
 * Exposes a minimal HTTP status endpoint for observability.
 *
 * Issue #765
 */

import * as http from "http";
import { Keypair } from "@stellar/stellar-sdk";
import { JobStore } from "./jobStore";
import { Dispatcher } from "./dispatcher";
import { Scheduler } from "./scheduler";
import { KeeperConfig } from "./types";
import { IndexerDeadlineClient } from "./indexerClient";
import { ListingTtlKeeper } from "./ttlKeeper";
import { TtlDryRun, dryRunReplacer } from "./dryRun";
import pino from "pino";

const logger = pino({ name: "keeper" });

function loadConfig(): KeeperConfig {
  return {
    rpcUrl: process.env.STELLAR_RPC_URL ?? "https://soroban-testnet.stellar.org",
    keeperSecret: process.env.KEEPER_SECRET ?? "",
    dbPath: process.env.KEEPER_DB_PATH ?? "./keeper-jobs.db",
    pollIntervalMs: Number(process.env.KEEPER_POLL_INTERVAL_MS ?? "30000"),
    maxAttempts: Number(process.env.KEEPER_MAX_ATTEMPTS ?? "5"),
    retryBackoffBaseMs: Number(process.env.KEEPER_RETRY_BACKOFF_BASE_MS ?? "60000"),
    networkPassphrase:
      process.env.STELLAR_NETWORK_PASSPHRASE ??
      "Test SDF Network ; September 2015",
    marketplaceContractAddress: process.env.MARKETPLACE_CONTRACT_ID ?? "",
    activeListingsUrl: process.env.KEEPER_ACTIVE_LISTINGS_URL ?? "",
    indexerApiKey: process.env.KEEPER_INDEXER_API_KEY ?? "",
    ttlThresholdLedgers: Number(process.env.KEEPER_TTL_THRESHOLD_LEDGERS ?? "518400"),
    indexerUrl: process.env.INDEXER_URL ?? process.env.KEEPER_ACTIVE_LISTINGS_URL ?? "",
    deadlineHorizonSec: Number(process.env.DEADLINE_HORIZON_SEC ?? "86400"),
    ingestIntervalMs: Number(process.env.DEADLINE_POLL_INTERVAL_MS ?? "60000"),
  };
}

async function main(): Promise<void> {
  const config = loadConfig();

  if (!config.keeperSecret) {
    logger.error("KEEPER_SECRET env var is required");
    process.exit(1);
  }
  // Fail fast on invalid key material so the keeper never starts in a state
  // where on-chain submission would silently no-op instead of erroring.
  try {
    Keypair.fromSecret(config.keeperSecret);
  } catch {
    logger.error("KEEPER_SECRET is not a valid Stellar secret key");
    process.exit(1);
  }
  if (!config.marketplaceContractAddress || !config.activeListingsUrl) {
    logger.error("MARKETPLACE_CONTRACT_ID and KEEPER_ACTIVE_LISTINGS_URL are required");
    process.exit(1);
  }
  if (!Number.isSafeInteger(config.ttlThresholdLedgers) || config.ttlThresholdLedgers <= 0) {
    logger.error("KEEPER_TTL_THRESHOLD_LEDGERS must be a positive integer");
    process.exit(1);
  }

  // ── Dry-run mode (#<keeper-dry-run>) ──────────────────────────────────────
  // Rehearses the TTL-bump decision read-only and exits without starting the
  // scheduler/dispatcher, so no transaction is ever submitted. Operators run
  // this with KEEPER_DRY_RUN=1 before paying for a mass extend.
  if (process.env.KEEPER_DRY_RUN === "true" || process.env.KEEPER_DRY_RUN === "1") {
    try {
      const report = await new TtlDryRun(config).analyze();
      process.stdout.write(`${JSON.stringify(report, dryRunReplacer)}\n`);
      process.exit(0);
    } catch (err) {
      logger.error({ err: String(err) }, "Keeper TTL dry-run failed");
      process.exit(1);
    }
  }

  const store = new JobStore(config.dbPath);
  const dispatcher = new Dispatcher(config);
  const ttlKeeper = new ListingTtlKeeper(config);
  const dryRun = new TtlDryRun(config);
  const scheduler = new Scheduler(store, dispatcher, config, () => ttlKeeper.scan());

  scheduler.start();

  // ── Indexer deadline ingestion (poll adapter, #1022) ──────────────────────
  // Runs on its own cadence so ingestion storms never coincide with dispatch
  // ticks. Outages degrade health only; the scheduler keeps dispatching.
  const deadlineClient = new IndexerDeadlineClient({
    indexerUrl: config.indexerUrl,
    apiKey: config.indexerApiKey || undefined,
    horizonSec: config.deadlineHorizonSec,
    contractAddress: config.marketplaceContractAddress,
  });

  const ingestOnce = async (): Promise<void> => {
    try {
      const deadlines = await deadlineClient.fetchDeadlines();
      if (deadlines.length > 0) {
        scheduler.enqueueDeadlines(deadlines);
        logger.info({ count: deadlines.length }, "Enqueued indexer deadlines");
      }
    } catch (err) {
      logger.warn({ err: String(err) }, "Deadline ingestion tick failed");
    }
  };
  void ingestOnce();
  const ingestTimer = setInterval(() => void ingestOnce(), config.ingestIntervalMs);
  const clearIngest = (): void => clearInterval(ingestTimer);

  // ── Observability HTTP server ────────────────────────────────────────────
  const server = http.createServer(async (req, res) => {
    if (req.url === "/health") {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ status: "ok" }));
      return;
    }

    if (req.url === "/status") {
      const counts = store.getStatusCounts();
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ counts }));
      return;
    }

    if (req.url === "/metrics") {
      const counts = store.getStatusCounts();
      const statuses = ["pending", "ready", "failed", "in_flight", "done", "dead"] as const;
      const metrics = [
        "# HELP kora_keeper_jobs Current keeper jobs by lifecycle status.",
        "# TYPE kora_keeper_jobs gauge",
        ...statuses.map((status) => `kora_keeper_jobs{status=\"${status}\"} ${counts[status] ?? 0}`),
        "# HELP kora_keeper_ttl_listings_scanned_total Active listings checked for TTL risk.",
        "# TYPE kora_keeper_ttl_listings_scanned_total counter",
        `kora_keeper_ttl_listings_scanned_total ${ttlKeeper.getMetrics().scanned}`,
        "# HELP kora_keeper_ttl_listings_at_risk_total Listings observed below the TTL threshold.",
        "# TYPE kora_keeper_ttl_listings_at_risk_total counter",
        `kora_keeper_ttl_listings_at_risk_total ${ttlKeeper.getMetrics().atRisk}`,
        "# HELP kora_keeper_ttl_scan_errors_total Failed active-listing TTL scans.",
        "# TYPE kora_keeper_ttl_scan_errors_total counter",
        `kora_keeper_ttl_scan_errors_total ${ttlKeeper.getMetrics().errors}`,
        "# HELP kora_keeper_metrics_scrape_ready Whether the keeper metrics endpoint is serving.",
        "# TYPE kora_keeper_metrics_scrape_ready gauge",
        "kora_keeper_metrics_scrape_ready 1",
      ];
      res.writeHead(200, { "Content-Type": "text/plain; version=0.0.4; charset=utf-8" });
      res.end(`${metrics.join("\n")}\n`);
      return;
    }

    if (req.url === "/history") {
      const history = store.getHistory(200);
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ history }));
      return;
    }

    // Read-only TTL-bump rehearsal: classifies instance vs persistent keys and
    // reports which need a bump without enqueuing or dispatching anything.
    if (req.url === "/dry-run") {
      try {
        const report = await dryRun.analyze();
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify(report, dryRunReplacer));
      } catch (err) {
        res.writeHead(503, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: "dry-run failed", detail: String(err) }));
      }
      return;
    }

    res.writeHead(404);
    res.end();
  });

  const port = Number(process.env.KEEPER_HTTP_PORT ?? "8080");
  server.listen(port, () => {
    logger.info({ port }, "Keeper observability server listening");
  });

  // ── Graceful shutdown ───────────────────────────────────────────────────
  const shutdown = (): void => {
    logger.info("Shutting down keeper service…");
    clearIngest();
    scheduler.stop();
    store.close();
    server.close(() => process.exit(0));
  };

  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
}

main().catch((err) => {
  logger.error(err, "Keeper service crashed");
  process.exit(1);
});
