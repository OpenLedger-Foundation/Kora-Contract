/**
 * Keeper TTL-bump dry-run tests — issue #<keeper-dry-run>.
 *
 * Covers:
 *  - Classifies instance vs persistent keys
 *  - Reports which keys need a TTL bump (below threshold) and which are healthy
 *  - Missing/archived entries are flagged needsBump + missing (no silent skip)
 *  - Empty config fails closed with DryRunConfigError
 *  - Dry-run never writes: only read-only getLedgerEntries is exercised
 *  - Idempotent: repeated runs produce identical reports
 *  - Batches >100 keys across multiple getLedgerEntries calls
 *  - No listings → only the contract instance entry
 */

import {
  TtlDryRun,
  DryRunConfigError,
  type DryRunReport,
} from "../src/dryRun";
import { buildInstanceLedgerKey } from "../src/ttlKeeper";
import { Address } from "@stellar/stellar-sdk";
import type { KeeperConfig } from "../src/types";

// A valid (checksum-correct) contract address: the all-zero contract hash.
const CONTRACT = Address.contract(Buffer.alloc(32)).toString();

const BASE_CONFIG: KeeperConfig = {
  rpcUrl: "https://soroban-testnet.stellar.org",
  keeperSecret: "STEST",
  dbPath: ":memory:",
  pollIntervalMs: 1000,
  maxAttempts: 3,
  retryBackoffBaseMs: 100,
  networkPassphrase: "Test SDF Network ; September 2015",
  marketplaceContractAddress: CONTRACT,
  activeListingsUrl: "https://indexer.test/api/v1/listings",
  indexerApiKey: "",
  ttlThresholdLedgers: 200,
  indexerUrl: "",
  deadlineHorizonSec: 86_400,
  ingestIntervalMs: 60_000,
};

interface MockEntry {
  key: { toXDR: (f: "base64") => string };
  liveUntilLedgerSeq: number;
}

function makeRpc(liveUntilByKey: (keyIndex: number, key: { toXDR: (f: "base64") => string }) => number) {
  const calls: number[] = [];
  const fn = async (
    ...keys: Array<{ toXDR: (f: "base64") => string }>
  ): Promise<{ latestLedger: number; entries: MockEntry[] }> => {
    calls.push(keys.length);
    return {
      latestLedger: 1000,
      entries: keys.map((key, i) => ({
        key,
        liveUntilLedgerSeq: liveUntilByKey(i, key),
      })),
    };
  };
  return { fn, calls };
}

const instanceXdr = buildInstanceLedgerKey(CONTRACT).toXDR("base64");

describe("TtlDryRun", () => {
  test("classifies instance vs persistent and reports needsBump vs healthy", async () => {
    const { fn, calls } = makeRpc((_i, key) =>
      key.toXDR("base64") === instanceXdr ? 1100 : 1500,
    );
    const source = {
      getActiveListings: async () => [
        { invoiceId: "42", isActive: true },
        { invoiceId: "43", isActive: true },
      ],
    };
    const dryRun = new TtlDryRun(BASE_CONFIG, { getLedgerEntries: fn } as never, source as never);

    const report = await dryRun.analyze();

    expect(report.latestLedger).toBe(1000);
    expect(report.thresholdLedgers).toBe(200);
    expect(report.entries).toHaveLength(3);

    const instance = report.entries.find((e) => e.durability === "instance")!;
    expect(instance.key).toBe("instance");
    expect(instance.remainingLedgers).toBe(100);
    expect(instance.needsBump).toBe(true);
    expect(instance.missing).toBe(false);

    const listings = report.entries.filter((e) => e.durability === "persistent");
    expect(listings).toHaveLength(2);
    for (const e of listings) {
      expect(e.remainingLedgers).toBe(500);
      expect(e.needsBump).toBe(false);
      expect(e.invoiceId).toBeDefined();
    }

    expect(report.summary).toEqual({
      total: 3,
      instance: 1,
      persistent: 2,
      needsBump: 1,
      healthy: 2,
      missing: 0,
    });
    // Read-only: only getLedgerEntries was called (once for ≤100 keys).
    expect(calls).toEqual([3]);
  });

  test("missing/archived entries are flagged needsBump + missing, not silently skipped", async () => {
    const fn = async (
      ...keys: Array<{ toXDR: (f: "base64") => string }>
    ): Promise<{ latestLedger: number; entries: MockEntry[] }> => {
      // Return NO entry for the instance; return a live entry for listings.
      return {
        latestLedger: 1000,
        entries: keys
          .filter((k) => k.toXDR("base64") !== instanceXdr)
          .map((k) => ({ key: k, liveUntilLedgerSeq: 1500 })),
      };
    };
    const source = {
      getActiveListings: async () => [{ invoiceId: "7", isActive: true }],
    };
    const dryRun = new TtlDryRun(BASE_CONFIG, { getLedgerEntries: fn } as never, source as never);

    const report = await dryRun.analyze();
    const instance = report.entries.find((e) => e.durability === "instance")!;
    expect(instance.missing).toBe(true);
    expect(instance.needsBump).toBe(true);
    expect(instance.remainingLedgers).toBe(0);

    const listing = report.entries.find((e) => e.key === "listing:7")!;
    expect(listing.missing).toBe(false);
    expect(listing.needsBump).toBe(false);

    expect(report.summary.missing).toBe(1);
    expect(report.summary.needsBump).toBe(1);
  });

  test("no listings → only the contract instance entry", async () => {
    const { fn } = makeRpc(() => 1500);
    const source = { getActiveListings: async () => [] };
    const dryRun = new TtlDryRun(BASE_CONFIG, { getLedgerEntries: fn } as never, source as never);

    const report = await dryRun.analyze();
    expect(report.entries).toHaveLength(1);
    expect(report.entries[0].durability).toBe("instance");
    expect(report.summary.persistent).toBe(0);
  });

  test("never writes: only read-only getLedgerEntries + getActiveListings are called", async () => {
    const getLedgerEntries = jest.fn(
      async (...keys: Array<{ toXDR: (f: "base64") => string }>) => ({
        latestLedger: 1000,
        entries: keys.map((k) => ({ key: k, liveUntilLedgerSeq: 1500 })),
      }),
    );
    const getActiveListings = jest.fn(async () => [
      { invoiceId: "1", isActive: true },
    ]);
    const dryRun = new TtlDryRun(
      BASE_CONFIG,
      { getLedgerEntries } as never,
      { getActiveListings } as never,
    );

    const report = await dryRun.analyze();
    expect(getActiveListings).toHaveBeenCalledTimes(1);
    expect(getLedgerEntries).toHaveBeenCalledTimes(1);
    // No write interfaces exist on the dry-run; report is the only output.
    expect(report.entries.length).toBeGreaterThan(0);
  });

  test("idempotent: repeated runs produce identical reports", async () => {
    const { fn } = makeRpc((_i, key) =>
      key.toXDR("base64") === instanceXdr ? 1100 : 1500,
    );
    const source = {
      getActiveListings: async () => [
        { invoiceId: "42", isActive: true },
        { invoiceId: "43", isActive: true },
      ],
    };
    const dryRun = new TtlDryRun(BASE_CONFIG, { getLedgerEntries: fn } as never, source as never);

    const a = await dryRun.analyze();
    const b = await dryRun.analyze();
    expect(b).toEqual(a);
  });

  test("batches more than 100 keys across multiple getLedgerEntries calls", async () => {
    const { fn, calls } = makeRpc(() => 1500);
    const listings = Array.from({ length: 250 }, (_, i) => ({
      invoiceId: String(i + 1),
      isActive: true,
    }));
    const source = { getActiveListings: async () => listings };
    const dryRun = new TtlDryRun(BASE_CONFIG, { getLedgerEntries: fn } as never, source as never);

    const report = await dryRun.analyze();
    // 1 instance + 250 listings = 251 keys → 3 batches (100, 100, 51)
    expect(report.entries).toHaveLength(251);
    expect(calls).toEqual([100, 100, 51]);
  });

  test("ignores listings with non-numeric invoice ids", async () => {
    const { fn } = makeRpc(() => 1500);
    const source = {
      getActiveListings: async () => [
        { invoiceId: "42", isActive: true },
        { invoiceId: "not-a-number", isActive: true },
      ],
    };
    const dryRun = new TtlDryRun(BASE_CONFIG, { getLedgerEntries: fn } as never, source as never);
    const report = await dryRun.analyze();
    expect(report.entries.filter((e) => e.durability === "persistent")).toHaveLength(1);
  });
});

describe("TtlDryRun config validation (fail closed)", () => {
  test("throws DryRunConfigError when marketplaceContractAddress is missing", () => {
    expect(
      () =>
        new TtlDryRun({ ...BASE_CONFIG, marketplaceContractAddress: "" }),
    ).toThrow(DryRunConfigError);
  });

  test("throws DryRunConfigError when ttlThresholdLedgers is non-positive", () => {
    expect(
      () => new TtlDryRun({ ...BASE_CONFIG, ttlThresholdLedgers: 0 }),
    ).toThrow(DryRunConfigError);
    expect(
      () => new TtlDryRun({ ...BASE_CONFIG, ttlThresholdLedgers: -5 }),
    ).toThrow(DryRunConfigError);
  });
});
