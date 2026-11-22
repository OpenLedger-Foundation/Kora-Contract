import { envBool, envInt, isSafeTarget, parseMode } from "../src/config";

describe("envInt", () => {
  afterEach(() => {
    delete process.env["MEV_TEST_INT"];
  });

  it("returns the fallback when unset or empty", () => {
    expect(envInt("MEV_TEST_INT", 7)).toBe(7);
    process.env["MEV_TEST_INT"] = "";
    expect(envInt("MEV_TEST_INT", 7)).toBe(7);
  });

  it("parses integers", () => {
    process.env["MEV_TEST_INT"] = "42";
    expect(envInt("MEV_TEST_INT", 0)).toBe(42);
  });

  it("throws on non-numeric input", () => {
    process.env["MEV_TEST_INT"] = "not-a-number";
    expect(() => envInt("MEV_TEST_INT", 0)).toThrow(/must be an integer/);
  });
});

describe("envBool", () => {
  afterEach(() => {
    delete process.env["MEV_TEST_BOOL"];
  });

  it("accepts true/1 case-insensitively and everything else as false", () => {
    process.env["MEV_TEST_BOOL"] = "TRUE";
    expect(envBool("MEV_TEST_BOOL", false)).toBe(true);
    process.env["MEV_TEST_BOOL"] = "1";
    expect(envBool("MEV_TEST_BOOL", false)).toBe(true);
    process.env["MEV_TEST_BOOL"] = "false";
    expect(envBool("MEV_TEST_BOOL", true)).toBe(false);
  });

  it("falls back when unset", () => {
    expect(envBool("MEV_TEST_BOOL", true)).toBe(true);
  });
});

describe("parseMode", () => {
  it("accepts mock and testnet", () => {
    expect(parseMode("mock")).toBe("mock");
    expect(parseMode("testnet")).toBe("testnet");
  });

  it("rejects anything else", () => {
    expect(() => parseMode("production")).toThrow(/MEV_MODE/);
  });
});

describe("isSafeTarget", () => {
  const base = {
    mode: "testnet" as const,
    env: "staging",
    rpcUrl: "https://soroban-testnet.stellar.org",
    networkPassphrase: "Test SDF Network ; September 2015",
  };

  it("always allows mock mode", () => {
    expect(isSafeTarget({ ...base, mode: "mock", rpcUrl: "https://soroban.stellar.org" })).toEqual({
      safe: true,
    });
  });

  it("allows a non-production testnet target", () => {
    expect(isSafeTarget(base)).toEqual({ safe: true });
  });

  it("refuses production env, mainnet RPC, and public passphrase", () => {
    expect(isSafeTarget({ ...base, env: "production" }).safe).toBe(false);
    expect(isSafeTarget({ ...base, rpcUrl: "https://soroban.stellar.org/" }).safe).toBe(false);
    expect(isSafeTarget({ ...base, networkPassphrase: "Public Global Stellar Network ; September 2015" }).safe).toBe(false);
  });
});
