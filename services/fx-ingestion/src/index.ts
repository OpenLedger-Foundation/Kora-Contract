/**
 * FX Ingestion Service — Entry Point
 * Issue #766
 */

import { ExchangeRateApiSource, CurrencyApiSource } from "./sources";
import { CrossChecker } from "./crosscheck";
import { OracleRelay } from "./relay";
import { IngestionEngine } from "./engine";
import { FxIngestionConfig, CurrencyCode } from "./types";
import { FxMetrics, readFxMetricsPort, startMetricsServer } from "./metrics";
import pino from "pino";

const logger = pino({ name: "fx-ingestion" });

export function loadConfig(): FxIngestionConfig {
  const currencies = (process.env.FX_CURRENCIES ?? "NGN,KES,GHS,ZAR,TZS,UGX,XOF,EGP")
    .split(",")
    .map((c) => c.trim() as CurrencyCode);

  return {
    currencies,
    quoteCurrency: (process.env.FX_QUOTE_CURRENCY ?? "USDC") as CurrencyCode,
    maxDivergenceBps: Number(process.env.FX_MAX_DIVERGENCE_BPS ?? "200"),
    pollIntervalMs: Number(process.env.FX_POLL_INTERVAL_MS ?? "300000"),
    oracleContractAddress: process.env.ORACLE_CONTRACT_ADDRESS ?? "",
    relaySecret: process.env.FX_RELAY_SECRET ?? "",
    rpcUrl: process.env.STELLAR_RPC_URL ?? "https://soroban-testnet.stellar.org",
    networkPassphrase:
      process.env.STELLAR_NETWORK_PASSPHRASE ?? "Test SDF Network ; September 2015",
    maxStalenessSeconds: Number(process.env.FX_MAX_STALENESS_SECONDS ?? "3600"),
  };
}

/** Validate that required secrets/config are present; throws on missing. */
export function validateConfig(config: FxIngestionConfig): void {
  if (!config.relaySecret) {
    throw new Error("FX_RELAY_SECRET is required");
  }
  if (!config.oracleContractAddress) {
    throw new Error("ORACLE_CONTRACT_ADDRESS is required");
  }
}

export async function main(): Promise<void> {
  const config = loadConfig();

  try {
    validateConfig(config);
  } catch (err) {
    logger.error(err instanceof Error ? err.message : String(err));
    process.exit(1);
  }

  const sources = [
    new ExchangeRateApiSource(process.env.EXCHANGE_RATE_API_KEY ?? ""),
    new CurrencyApiSource(process.env.CURRENCY_API_KEY ?? ""),
  ];

  const checker = new CrossChecker(sources, config.maxDivergenceBps);
  const relay = new OracleRelay(config);
  const metrics = new FxMetrics();
  const engine = new IngestionEngine(checker, relay, config, metrics);

  const metricsPort = readFxMetricsPort();
  const metricsServer = startMetricsServer(metrics, metricsPort);
  logger.info({ port: metricsPort }, "FX ingestion metrics server listening");

  engine.start();

  const shutdown = (): void => {
    logger.info("Shutting down FX ingestion service…");
    engine.stop();
    metricsServer.close();
    process.exit(0);
  };

  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
}

if (require.main === module) {
  main().catch((err) => {
    logger.error(err, "FX ingestion service crashed");
    process.exit(1);
  });
}
