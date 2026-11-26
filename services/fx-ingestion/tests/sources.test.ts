/**
 * FX data source adapter tests — covers sources.ts (axios-mocked).
 *
 * Both adapters share the same contract: return a quote on success, null on an
 * invalid rate or a network/HTTP failure. Issue #<fx-staleness> coverage.
 */

import axios from "axios";
import { ExchangeRateApiSource, CurrencyApiSource } from "../src/sources";

jest.mock("axios");
const mockedGet = axios.get as jest.Mock;

afterEach(() => mockedGet.mockReset());

describe("ExchangeRateApiSource", () => {
  it("returns a scaled quote on a successful fetch", async () => {
    mockedGet.mockResolvedValue({
      data: { conversion_rate: 0.00065, time_last_update_unix: 123 },
    });
    const src = new ExchangeRateApiSource("key");
    const quote = await src.fetchRate("NGN", "USDC");
    expect(quote).not.toBeNull();
    expect(quote!.rateScaled).toBe(6500n);
    expect(quote!.fetchedAt).toBe(123);
    expect(quote!.source).toBe("exchangerate-api");
  });

  it("falls back to now() when the API omits the timestamp", async () => {
    mockedGet.mockResolvedValue({ data: { conversion_rate: 0.00065 } });
    const src = new ExchangeRateApiSource("key");
    const quote = await src.fetchRate("NGN", "USDC");
    expect(quote).not.toBeNull();
    expect(quote!.fetchedAt).toBeGreaterThan(0);
  });

  it("returns null when the rate is invalid (<=0)", async () => {
    mockedGet.mockResolvedValue({ data: { conversion_rate: 0 } });
    const src = new ExchangeRateApiSource("key");
    expect(await src.fetchRate("NGN", "USDC")).toBeNull();
  });

  it("returns null on a network error", async () => {
    mockedGet.mockRejectedValue(new Error("timeout"));
    const src = new ExchangeRateApiSource("key");
    expect(await src.fetchRate("NGN", "USDC")).toBeNull();
  });
});

describe("CurrencyApiSource", () => {
  it("returns a scaled quote on a successful fetch", async () => {
    mockedGet.mockResolvedValue({
      data: { data: { USDC: { code: "USDC", value: 0.00065 } } },
    });
    const src = new CurrencyApiSource("key");
    const quote = await src.fetchRate("NGN", "USDC");
    expect(quote).not.toBeNull();
    expect(quote!.rateScaled).toBe(6500n);
    expect(quote!.source).toBe("currencyapi");
  });

  it("returns null when the rate is invalid", async () => {
    mockedGet.mockResolvedValue({
      data: { data: { USDC: { code: "USDC", value: 0 } } },
    });
    const src = new CurrencyApiSource("key");
    expect(await src.fetchRate("NGN", "USDC")).toBeNull();
  });

  it("returns null on a network error", async () => {
    mockedGet.mockRejectedValue(new Error("timeout"));
    const src = new CurrencyApiSource("key");
    expect(await src.fetchRate("NGN", "USDC")).toBeNull();
  });
});
