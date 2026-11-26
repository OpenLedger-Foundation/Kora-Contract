/**
 * FX ingestion entry-point tests — loadConfig, validateConfig, and main exit
 * paths. Issue #<fx-staleness> coverage.
 */

import { loadConfig, validateConfig, main } from "../src/index";
import type { FxIngestionConfig } from "../src/types";

const ENV_KEYS = [
  "FX_CURRENCIES",
  "FX_QUOTE_CURRENCY",
  "FX_MAX_DIVERGENCE_BPS",
  "FX_POLL_INTERVAL_MS",
  "ORACLE_CONTRACT_ADDRESS",
  "FX_RELAY_SECRET",
  "STELLAR_RPC_URL",
  "STELLAR_NETWORK_PASSPHRASE",
  "FX_MAX_STALENESS_SECONDS",
] as const;

function clearEnv(): void {
  for (const k of ENV_KEYS) delete process.env[k];
}

describe("loadConfig", () => {
  beforeEach(clearEnv);
  afterAll(clearEnv);

  it("applies documented defaults when no env is set", () => {
    const cfg = loadConfig();
    expect(cfg.currencies).toEqual([
      "NGN",
      "KES",
      "GHS",
      "ZAR",
      "TZS",
      "UGX",
      "XOF",
      "EGP",
    ]);
    expect(cfg.quoteCurrency).toBe("USDC");
    expect(cfg.maxDivergenceBps).toBe(200);
    expect(cfg.pollIntervalMs).toBe(300_000);
    expect(cfg.maxStalenessSeconds).toBe(3600);
  });

  it("reads overrides from the environment", () => {
    process.env.FX_CURRENCIES = "ngn, kes";
    process.env.FX_QUOTE_CURRENCY = "EURC";
    process.env.FX_MAX_DIVERGENCE_BPS = "300";
    process.env.FX_POLL_INTERVAL_MS = "60000";
    process.env.ORACLE_CONTRACT_ADDRESS = "CORACLE";
    process.env.FX_RELAY_SECRET = "SRELAY";
    process.env.STELLAR_RPC_URL = "https://rpc";
    process.env.STELLAR_NETWORK_PASSPHRASE = "Custom";
    process.env.FX_MAX_STALENESS_SECONDS = "120";

    const cfg = loadConfig();
    expect(cfg.currencies).toEqual(["ngn", "kes"]);
    expect(cfg.quoteCurrency).toBe("EURC");
    expect(cfg.maxDivergenceBps).toBe(300);
    expect(cfg.pollIntervalMs).toBe(60_000);
    expect(cfg.oracleContractAddress).toBe("CORACLE");
    expect(cfg.relaySecret).toBe("SRELAY");
    expect(cfg.rpcUrl).toBe("https://rpc");
    expect(cfg.networkPassphrase).toBe("Custom");
    expect(cfg.maxStalenessSeconds).toBe(120);
  });
});

describe("validateConfig", () => {
  const valid: FxIngestionConfig = {
    currencies: ["NGN"],
    quoteCurrency: "USDC",
    maxDivergenceBps: 200,
    pollIntervalMs: 300_000,
    oracleContractAddress: "CORACLE",
    relaySecret: "SRELAY",
    rpcUrl: "https://test",
    networkPassphrase: "Test",
    maxStalenessSeconds: 3600,
  };

  it("passes when relay secret and oracle address are present", () => {
    expect(() => validateConfig(valid)).not.toThrow();
  });

  it("throws when the relay secret is missing", () => {
    expect(() => validateConfig({ ...valid, relaySecret: "" })).toThrow(
      "FX_RELAY_SECRET is required",
    );
  });

  it("throws when the oracle address is missing", () => {
    expect(() =>
      validateConfig({ ...valid, oracleContractAddress: "" }),
    ).toThrow("ORACLE_CONTRACT_ADDRESS is required");
  });
});

describe("main — startup validation", () => {
  beforeEach(clearEnv);
  afterAll(clearEnv);

  it("exits with code 1 when the relay secret is missing", async () => {
    process.env.ORACLE_CONTRACT_ADDRESS = "CORACLE";
    const exitSpy = jest
      .spyOn(process, "exit")
      .mockImplementation((() => {
        throw new Error("exit:1");
      }) as never);
    await expect(main()).rejects.toThrow("exit:1");
    exitSpy.mockRestore();
  });

  it("exits with code 1 when the oracle address is missing", async () => {
    process.env.FX_RELAY_SECRET = "SRELAY";
    const exitSpy = jest
      .spyOn(process, "exit")
      .mockImplementation((() => {
        throw new Error("exit:1");
      }) as never);
    await expect(main()).rejects.toThrow("exit:1");
    exitSpy.mockRestore();
  });
});
