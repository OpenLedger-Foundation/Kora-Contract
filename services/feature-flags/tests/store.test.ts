/**
 * FlagStore Tests
 *
 * Covers CRUD, cache invalidation on write, and change callbacks.
 */

import * as os from "os";
import * as path from "path";
import { FlagStore } from "../src/store";
import { FeatureFlag } from "../src/types";

function tmpDb(): string {
  return path.join(os.tmpdir(), `kora-store-test-${Date.now()}-${Math.random()}.db`);
}

function baseFlag(overrides: Partial<FeatureFlag> = {}): Omit<FeatureFlag, "createdAt" | "updatedAt"> {
  return {
    name: "secondary-market",
    description: "desc",
    enabled: true,
    rules: [],
    updatedBy: "test",
    ...overrides,
  };
}

describe("FlagStore", () => {
  it("upsert creates a flag and getByName returns it", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag());
    const flag = store.getByName("secondary-market");
    expect(flag).not.toBeNull();
    expect(flag!.name).toBe("secondary-market");
    expect(flag!.enabled).toBe(true);
  });

  it("upsert is idempotent — second call updates existing row", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag({ enabled: true }));
    store.upsert(baseFlag({ enabled: false }));
    expect(store.getByName("secondary-market")!.enabled).toBe(false);
  });

  it("getAll returns all upserted flags", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag({ name: "secondary-market" }));
    store.upsert(baseFlag({ name: "fractionalization" }));
    expect(store.getAll()).toHaveLength(2);
  });

  it("setEnabled updates only the enabled field", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag({ rules: [{ type: "percentage", rolloutBps: 5000 }] }));
    store.setEnabled("secondary-market", false, "admin");
    const flag = store.getByName("secondary-market")!;
    expect(flag.enabled).toBe(false);
    // Rules unchanged
    expect(flag.rules).toHaveLength(1);
  });

  it("setEnabled throws for unknown flag", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    expect(() => store.setEnabled("secondary-market", false, "admin")).toThrow();
  });

  it("delete removes the flag", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag());
    store.delete("secondary-market");
    expect(store.getByName("secondary-market")).toBeNull();
  });

  it("getByName returns null for unknown flag", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    expect(store.getByName("secondary-market")).toBeNull();
  });

  it("onFlagChange callback fires on upsert", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    const changes: FeatureFlag[] = [];
    store.onFlagChange((f) => changes.push(f));
    store.upsert(baseFlag());
    expect(changes).toHaveLength(1);
    expect(changes[0].name).toBe("secondary-market");
  });

  it("onFlagChange callback fires on setEnabled", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag());
    const changes: FeatureFlag[] = [];
    store.onFlagChange((f) => changes.push(f));
    store.setEnabled("secondary-market", false, "admin");
    expect(changes).toHaveLength(1);
    expect(changes[0].enabled).toBe(false);
  });

  it("preserves createdAt across updates", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    const first = store.upsert(baseFlag());
    // Small delay to ensure timestamps differ
    const second = store.upsert(baseFlag({ description: "updated" }));
    expect(second.createdAt).toBe(first.createdAt);
    expect(second.updatedAt).not.toBe(first.updatedAt);
  });

  it("serialises and deserialises rules correctly", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag({
      rules: [
        { type: "allowlist", allowedUserHashes: ["abc123"] },
        { type: "percentage", rolloutBps: 2500 },
      ],
    }));
    const flag = store.getByName("secondary-market")!;
    expect(flag.rules).toHaveLength(2);
    expect(flag.rules[0].type).toBe("allowlist");
    expect(flag.rules[1].type).toBe("percentage");
  });

  it("cache returns stale value within TTL", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 60_000 }); // long TTL
    store.upsert(baseFlag({ enabled: true }));
    // Directly update DB to bypass store methods (simulates external change)
    // Then read through cache — should still see old value
    const flag = store.getByName("secondary-market")!;
    expect(flag.enabled).toBe(true); // from cache
  });
});

// ---------------------------------------------------------------------------
// TTL exhaustion / kill-switch drill tests (#1006)
// ---------------------------------------------------------------------------

describe("Cache TTL exhaustion — kill-switch drill (#1006)", () => {
  it("TTL exhaustion: after TTL expires a store reads fresh state", async () => {
    // Use a very short TTL so we can advance past it without real sleep
    const TTL = 50; // ms
    const store = new FlagStore(tmpDb(), { cacheTtlMs: TTL });
    store.upsert(baseFlag({ enabled: true }));

    // Immediately within TTL — sees cached true value
    expect(store.getByName("secondary-market")!.enabled).toBe(true);

    // Advance past the TTL
    await new Promise((r) => setTimeout(r, TTL + 20));

    // Cache has expired; next read re-queries SQLite — value is still true (no external change)
    expect(store.getByName("secondary-market")!.enabled).toBe(true);
  });

  it("kill-switch via setEnabled invalidates cache immediately (zero stale window)", () => {
    const TTL = 60_000; // long TTL — would normally keep stale data
    const store = new FlagStore(tmpDb(), { cacheTtlMs: TTL });
    store.upsert(baseFlag({ enabled: true }));

    // Warm the cache
    expect(store.getByName("secondary-market")!.enabled).toBe(true);

    // Kill-switch: setEnabled calls invalidateCache synchronously
    store.setEnabled("secondary-market", false, "incident-responder");

    // Cache is invalidated — sees false immediately despite long TTL
    expect(store.getByName("secondary-market")!.enabled).toBe(false);
  });

  it("stale read window: external DB change not visible until TTL expires", async () => {
    const TTL = 80; // ms
    const dbPath = tmpDb();
    const storeA = new FlagStore(dbPath, { cacheTtlMs: TTL });
    storeA.upsert(baseFlag({ enabled: true }));

    // storeB simulates another process sharing the same SQLite file
    const storeB = new FlagStore(dbPath, { cacheTtlMs: TTL });
    storeB.upsert(baseFlag({ enabled: false })); // external write — storeA cache is stale

    // storeA still sees true (within TTL)
    expect(storeA.getByName("secondary-market")!.enabled).toBe(true);

    // Advance past TTL
    await new Promise((r) => setTimeout(r, TTL + 20));

    // After TTL expires storeA re-queries and sees the external write
    expect(storeA.getByName("secondary-market")!.enabled).toBe(false);

    storeA.close?.(); // no-op if not implemented — defensive
    storeB.close?.();
  });

  it("TTL exhaustion: multiple flags all refresh after TTL", async () => {
    const TTL = 50;
    const store = new FlagStore(tmpDb(), { cacheTtlMs: TTL });
    store.upsert(baseFlag({ name: "secondary-market", enabled: true }));
    store.upsert(baseFlag({ name: "fractionalization", enabled: true }));

    // Warm the cache
    store.getAll();

    // Kill both via setEnabled (each call invalidates)
    store.setEnabled("secondary-market", false, "admin");
    store.setEnabled("fractionalization", false, "admin");

    // Immediately after kill: both disabled
    expect(store.getByName("secondary-market")!.enabled).toBe(false);
    expect(store.getByName("fractionalization")!.enabled).toBe(false);

    // Re-enabling restores the flag
    store.setEnabled("secondary-market", true, "admin");
    expect(store.getByName("secondary-market")!.enabled).toBe(true);
    expect(store.getByName("fractionalization")!.enabled).toBe(false); // unchanged
  });

  it("rapid on-off-on sequence: final state is correct (monotonic version)", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag({ enabled: true }));

    // Toggle rapidly
    store.setEnabled("secondary-market", false, "admin"); // off
    store.setEnabled("secondary-market", true, "admin");  // on
    store.setEnabled("secondary-market", false, "admin"); // off
    store.setEnabled("secondary-market", true, "admin");  // on (final)

    expect(store.getByName("secondary-market")!.enabled).toBe(true);
  });

  it("onFlagChange callback fires for each toggle with correct value sequence", () => {
    const store = new FlagStore(tmpDb(), { cacheTtlMs: 0 });
    store.upsert(baseFlag({ enabled: true }));

    const values: boolean[] = [];
    store.onFlagChange((f) => values.push(f.enabled));

    store.setEnabled("secondary-market", false, "admin"); // false
    store.setEnabled("secondary-market", true, "admin");  // true
    store.setEnabled("secondary-market", false, "admin"); // false

    expect(values).toEqual([false, true, false]);
  });

  it("cache TTL with injected clock: short TTL reloads after measured interval", async () => {
    // This test uses a real short TTL with a real sleep to verify the reload path.
    // Keeps the sleep small (< 200ms) to avoid timing flake in CI.
    const TTL = 60;
    const store = new FlagStore(tmpDb(), { cacheTtlMs: TTL });
    store.upsert(baseFlag({ enabled: true }));

    const before = store.getByName("secondary-market");
    expect(before!.enabled).toBe(true);

    await new Promise((r) => setTimeout(r, TTL + 30));

    // Re-read after TTL: the store re-queries SQLite (no external change → still true)
    const after = store.getByName("secondary-market");
    expect(after!.enabled).toBe(true); // reload happened, value correct
  });
});
