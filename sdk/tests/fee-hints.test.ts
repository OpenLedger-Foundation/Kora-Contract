import { applyFeeHint, fetchFeeHint, feeHintsEnabled, resolveFee } from "../src/feeHints";

const SURGE_HINT = {
  network: "testnet",
  base_fee: 100,
  surge_multiplier_bps: 25_000,
  surge_multiplier: 2.5,
  suggested_fee: 250,
  surge_active: true,
  observed_at: "2026-11-22T00:00:00.000Z",
  latest_ledger: 1185010,
  stale: false,
  degraded: false,
  advisory: true as const,
};

function okFetch(body: unknown) {
  return jest.fn(async () => ({ ok: true, json: async () => body }));
}

describe("fee hints", () => {
  const original = { ...process.env };
  afterEach(() => {
    process.env = { ...original };
  });

  it("applyFeeHint multiplies and rounds up", () => {
    expect(applyFeeHint("100", SURGE_HINT)).toBe("250");
    expect(applyFeeHint(101, { surge_multiplier_bps: 15_000 })).toBe("152");
  });

  it("applyFeeHint clamps to [1x, max] and never lowers the base fee", () => {
    expect(applyFeeHint("100", { surge_multiplier_bps: 5_000 })).toBe("100");
    expect(applyFeeHint("100", { surge_multiplier_bps: 1_000_000 }, 30_000)).toBe("300");
    expect(applyFeeHint("100", null)).toBe("100");
    expect(applyFeeHint("100", { surge_multiplier_bps: Number.NaN })).toBe("100");
  });

  it("is opt-in via KORA_USE_FEE_HINTS", async () => {
    delete process.env.KORA_USE_FEE_HINTS;
    const fetch = okFetch(SURGE_HINT);
    expect(feeHintsEnabled()).toBe(false);
    await expect(resolveFee("100", { url: "https://hint", fetch })).resolves.toBe("100");
    expect(fetch).not.toHaveBeenCalled();

    process.env.KORA_USE_FEE_HINTS = "true";
    expect(feeHintsEnabled()).toBe(true);
    await expect(resolveFee("100", { url: "https://hint", fetch })).resolves.toBe("250");
  });

  it("respects KORA_FEE_HINT_MAX_BPS", async () => {
    process.env.KORA_USE_FEE_HINTS = "true";
    process.env.KORA_FEE_HINT_MAX_BPS = "20000";
    await expect(resolveFee("100", { url: "https://hint", fetch: okFetch(SURGE_HINT) })).resolves.toBe("200");
  });

  it("falls back to the base fee when the hint endpoint fails", async () => {
    process.env.KORA_USE_FEE_HINTS = "true";
    const failing = jest.fn(async () => {
      throw new Error("ECONNREFUSED");
    });
    await expect(resolveFee("100", { url: "https://hint", fetch: failing })).resolves.toBe("100");
    await expect(
      resolveFee("100", { url: "https://hint", fetch: jest.fn(async () => ({ ok: false, json: async () => ({}) })) })
    ).resolves.toBe("100");
    await expect(resolveFee("100", { url: "https://hint", fetch: okFetch({ nope: 1 }) })).resolves.toBe("100");
  });

  it("fetchFeeHint returns null without a URL", async () => {
    delete process.env.KORA_FEE_HINT_URL;
    await expect(fetchFeeHint({ fetch: okFetch(SURGE_HINT) })).resolves.toBeNull();
    process.env.KORA_FEE_HINT_URL = "https://hint";
    await expect(fetchFeeHint({ fetch: okFetch(SURGE_HINT) })).resolves.toEqual(SURGE_HINT);
  });
});
