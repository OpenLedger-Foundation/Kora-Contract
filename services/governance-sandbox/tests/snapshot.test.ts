/**
 * Governance-sandbox snapshot tests — issue #769
 *
 * Covers:
 *  - simulateAndSnapshot produces a snapshotId and correct impact
 *  - fetchSnapshot returns an identical summary to the original result
 *  - Identical inputs always produce the same snapshotId (determinism)
 *  - Different inputs produce different snapshotIds
 *  - capturedAt is a nondeterminism source (different values → different IDs)
 *  - Huge payloads are rejected with a clear error
 *  - Unknown snapshot ID returns undefined
 *  - Upsert semantics: re-saving the same ID overwrites without growing the store
 *  - stableStringify key-ordering is insertion-order independent
 *  - bigintReplacer / bigintReviver round-trip
 */

import {
  fetchSnapshot,
  InMemorySnapshotStore,
  MAX_SNAPSHOT_BYTES,
  ParameterChange,
  ProtocolSnapshot,
  simulateAndSnapshot,
  simulateImpact,
  computeSnapshotId,
} from '../src';
import {
  bigintReplacer,
  bigintReviver,
  stableStringify,
} from '../src/snapshot-store';

// ── Fixtures ──────────────────────────────────────────────────────────────────

const BASE_SNAPSHOT: ProtocolSnapshot = {
  capturedAt: 'ledger-200',
  concentrationCap: 100n,
  listings: [
    { id: 'over',  concentration: 120n, feeTier: 'standard', fundedAmount: 10_000n },
    { id: 'under', concentration:  80n, feeTier: 'standard', fundedAmount: 20_000n },
  ],
  investors: [{ address: 'near', exposure: 96n }],
};

const CAP_CHANGE: ParameterChange = {
  type: 'concentration_cap',
  from: 150n,
  to: 100n,
};

const FEE_CHANGE: ParameterChange = {
  type: 'fee_tier',
  tier: 'standard',
  fromBps: 50,
  toBps: 100,
};

// ── simulateAndSnapshot ───────────────────────────────────────────────────────

test('simulateAndSnapshot returns a snapshotId and the computed impact', async () => {
  const store = new InMemorySnapshotStore();
  const { snapshotId, impact } = await simulateAndSnapshot(CAP_CHANGE, BASE_SNAPSHOT, store);

  expect(typeof snapshotId).toBe('string');
  expect(snapshotId).toHaveLength(64); // SHA-256 hex
  expect(impact.supported).toBe(true);
  expect(impact.affectedListings).toEqual(['over']);
});

test('simulateAndSnapshot impact matches a direct simulateImpact call', async () => {
  const store = new InMemorySnapshotStore();
  const { impact: snapshotImpact } = await simulateAndSnapshot(CAP_CHANGE, BASE_SNAPSHOT, store);
  const directImpact = simulateImpact(CAP_CHANGE, BASE_SNAPSHOT);

  // Deep-equal: stringify through the bigint replacer for comparison.
  expect(JSON.stringify(snapshotImpact, bigintReplacer)).toBe(
    JSON.stringify(directImpact, bigintReplacer),
  );
});

// ── fetchSnapshot ─────────────────────────────────────────────────────────────

test('fetchSnapshot returns the identical summary stored by simulateAndSnapshot', async () => {
  const store = new InMemorySnapshotStore();
  const { snapshotId, impact } = await simulateAndSnapshot(CAP_CHANGE, BASE_SNAPSHOT, store);

  const fetched = await fetchSnapshot(snapshotId, store);

  expect(fetched).toBeDefined();
  expect(fetched!.snapshotId).toBe(snapshotId);
  expect(JSON.stringify(fetched!.impact, bigintReplacer)).toBe(
    JSON.stringify(impact, bigintReplacer),
  );
  expect(JSON.stringify(fetched!.change, bigintReplacer)).toBe(
    JSON.stringify(CAP_CHANGE, bigintReplacer),
  );
  expect(JSON.stringify(fetched!.protocolSnapshot, bigintReplacer)).toBe(
    JSON.stringify(BASE_SNAPSHOT, bigintReplacer),
  );
  expect(new Date(fetched!.storedAt).toISOString()).toBe(fetched!.storedAt);
});

test('fetchSnapshot returns undefined for an unknown snapshotId', async () => {
  const store = new InMemorySnapshotStore();
  const result = await fetchSnapshot('0'.repeat(64), store);
  expect(result).toBeUndefined();
});

// ── Determinism ───────────────────────────────────────────────────────────────

test('identical inputs always produce the same snapshotId', async () => {
  const store = new InMemorySnapshotStore();
  const { snapshotId: id1 } = await simulateAndSnapshot(CAP_CHANGE, BASE_SNAPSHOT, store);
  const { snapshotId: id2 } = await simulateAndSnapshot(CAP_CHANGE, BASE_SNAPSHOT, store);
  expect(id1).toBe(id2);
});

test('different parameter changes produce different snapshotIds', async () => {
  const store = new InMemorySnapshotStore();
  const { snapshotId: capId }  = await simulateAndSnapshot(CAP_CHANGE,  BASE_SNAPSHOT, store);
  const { snapshotId: feeId }  = await simulateAndSnapshot(FEE_CHANGE,  BASE_SNAPSHOT, store);
  expect(capId).not.toBe(feeId);
});

test('capturedAt is a nondeterminism source: different values yield different IDs', () => {
  const snapshotA: ProtocolSnapshot = { ...BASE_SNAPSHOT, capturedAt: 'ledger-100' };
  const snapshotB: ProtocolSnapshot = { ...BASE_SNAPSHOT, capturedAt: 'ledger-999' };

  const idA = computeSnapshotId(CAP_CHANGE, snapshotA);
  const idB = computeSnapshotId(CAP_CHANGE, snapshotB);

  expect(idA).not.toBe(idB);
});

test('object key insertion order does not affect the snapshotId', () => {
  // Build two ProtocolSnapshots with the same data but different key ordering.
  const orderA: ProtocolSnapshot = {
    capturedAt: 'ledger-200',
    concentrationCap: 100n,
    listings: [{ id: 'x', concentration: 50n, feeTier: 'standard', fundedAmount: 1000n }],
    investors: [],
  };
  // Spread re-orders capturedAt after concentrationCap.
  const { capturedAt, concentrationCap, ...rest } = orderA;
  const orderB: ProtocolSnapshot = { concentrationCap, capturedAt, ...rest };

  expect(computeSnapshotId(CAP_CHANGE, orderA)).toBe(computeSnapshotId(CAP_CHANGE, orderB));
});

// ── Upsert semantics ──────────────────────────────────────────────────────────

test('re-saving the same inputs upserts without growing the store', async () => {
  const store = new InMemorySnapshotStore();

  await simulateAndSnapshot(CAP_CHANGE, BASE_SNAPSHOT, store);
  expect(await store.size()).toBe(1);

  await simulateAndSnapshot(CAP_CHANGE, BASE_SNAPSHOT, store);
  expect(await store.size()).toBe(1); // still 1 — upserted, not duplicated

  // A distinct change produces a second entry.
  await simulateAndSnapshot(FEE_CHANGE, BASE_SNAPSHOT, store);
  expect(await store.size()).toBe(2);
});

// ── Payload size guard ────────────────────────────────────────────────────────

test('save throws when the payload exceeds MAX_SNAPSHOT_BYTES', async () => {
  const store = new InMemorySnapshotStore();

  // Build a snapshot whose serialised form exceeds the 1 MiB limit.
  const hugeListings = Array.from({ length: 15_000 }, (_, i) => ({
    id: `inv-${i.toString().padStart(6, '0')}`,
    concentration: BigInt(i),
    feeTier: 'standard',
    fundedAmount: BigInt(i * 1_000),
  }));

  const hugeSnapshot: ProtocolSnapshot = {
    ...BASE_SNAPSHOT,
    listings: hugeListings,
  };

  await expect(
    simulateAndSnapshot(CAP_CHANGE, hugeSnapshot, store),
  ).rejects.toThrow(/exceeds/);
});

test('MAX_SNAPSHOT_BYTES is 1 MiB (1048576 bytes)', () => {
  expect(MAX_SNAPSHOT_BYTES).toBe(1_048_576);
});

// ── BigInt serialisation helpers ──────────────────────────────────────────────

test('bigintReplacer converts BigInt to tagged object', () => {
  const result = JSON.stringify({ v: 42n }, bigintReplacer);
  expect(result).toBe('{"v":{"__bigint__":"42"}}');
});

test('bigintReviver reconstructs BigInt from tagged object', () => {
  const json = '{"v":{"__bigint__":"99"}}';
  const parsed = JSON.parse(json, bigintReviver) as { v: bigint };
  expect(parsed.v).toBe(99n);
});

test('bigintReplacer / bigintReviver round-trip preserves value', () => {
  const original = { cap: 123_456_789_012_345_678n };
  const json = JSON.stringify(original, bigintReplacer);
  const restored = JSON.parse(json, bigintReviver) as typeof original;
  expect(restored.cap).toBe(original.cap);
});

// ── stableStringify ───────────────────────────────────────────────────────────

test('stableStringify produces the same output regardless of key insertion order', () => {
  const a = stableStringify({ z: 1, a: 2, m: 3 });
  const b = stableStringify({ m: 3, z: 1, a: 2 });
  expect(a).toBe(b);
  expect(a).toBe('{"a":2,"m":3,"z":1}');
});

test('stableStringify sorts nested object keys', () => {
  const result = stableStringify({ outer: { z: 'last', a: 'first' } });
  expect(result).toBe('{"outer":{"a":"first","z":"last"}}');
});
