export type Listing = { id: string; concentration: bigint; feeTier: string; fundedAmount: bigint };
export type Investor = { address: string; exposure: bigint };
export type ProtocolSnapshot = {
  capturedAt: string;
  concentrationCap: bigint;
  listings: Listing[];
  investors: Investor[];
};

export type ParameterChange =
  | { type: 'concentration_cap'; from: bigint; to: bigint; proximityBps?: number }
  | { type: 'fee_tier'; tier: string; fromBps: number; toBps: number }
  | { type: string; from: unknown; to: unknown };

export type Impact = {
  supported: boolean;
  proposalDiff: { type: string; from: unknown; to: unknown };
  currentStateAt: string;
  disclaimer: 'Current-state simulation; state may change before execution.';
  affectedListings?: string[];
  investorsNearNewCap?: number;
  affectedFeeTierListings?: number;
  estimatedFeeDelta?: bigint;
};

type Plugin = (change: ParameterChange, snapshot: ProtocolSnapshot) => Impact;

const unsupported: Plugin = (change, snapshot) => {
  const c = change as { type: string; from?: unknown; to?: unknown };
  return {
    supported: false,
    proposalDiff: { type: c.type, from: c.from, to: c.to },
    currentStateAt: snapshot.capturedAt,
    disclaimer: 'Current-state simulation; state may change before execution.',
  };
};

const plugins: Record<string, Plugin> = {
  concentration_cap: (change, snapshot) => {
    if (change.type !== 'concentration_cap') return unsupported(change, snapshot);
    const c = change as Extract<ParameterChange, { type: 'concentration_cap' }>;
    const proximityBps = c.proximityBps ?? 500;
    const floor = (c.to * BigInt(10_000 - proximityBps)) / 10_000n;
    return {
      supported: true,
      proposalDiff: { type: c.type, from: c.from, to: c.to },
      currentStateAt: snapshot.capturedAt,
      disclaimer: 'Current-state simulation; state may change before execution.',
      affectedListings: snapshot.listings.filter((item) => item.concentration > c.to).map((item) => item.id),
      investorsNearNewCap: snapshot.investors.filter((item) => item.exposure >= floor && item.exposure <= c.to).length,
    };
  },
  fee_tier: (change, snapshot) => {
    if (change.type !== 'fee_tier') return unsupported(change, snapshot);
    const c = change as Extract<ParameterChange, { type: 'fee_tier' }>;
    const listings = snapshot.listings.filter((item) => item.feeTier === c.tier);
    const delta = listings.reduce(
      (total, item) => total + (item.fundedAmount * BigInt(c.toBps - c.fromBps)) / 10_000n,
      0n,
    );
    return {
      supported: true,
      proposalDiff: { type: c.type, from: c.fromBps, to: c.toBps },
      currentStateAt: snapshot.capturedAt,
      disclaimer: 'Current-state simulation; state may change before execution.',
      affectedFeeTierListings: listings.length,
      estimatedFeeDelta: delta,
    };
  },
};

export function simulateImpact(change: ParameterChange, snapshot: ProtocolSnapshot): Impact {
  return (plugins[change.type] ?? unsupported)(change, snapshot);
}

// ── Snapshot API ──────────────────────────────────────────────────────────────

export type {
  SimulationSnapshot,
  SnapshotStore,
} from "./snapshot-store";

export {
  InMemorySnapshotStore,
  computeSnapshotId,
  MAX_SNAPSHOT_BYTES,
} from "./snapshot-store";

/**
 * Simulate a governance proposal and persist the result as a snapshot.
 *
 * @returns An object containing the computed Impact and the stable snapshotId.
 *   The snapshotId can be passed to `fetchSnapshot` at any later point to
 *   retrieve the same summary, giving voters a reproducible reference.
 */
export async function simulateAndSnapshot(
  change: ParameterChange,
  protocolSnapshot: ProtocolSnapshot,
  store: import("./snapshot-store").SnapshotStore,
): Promise<{ snapshotId: string; impact: Impact }> {
  const impact = simulateImpact(change, protocolSnapshot);
  const snapshotId = await store.save(change, protocolSnapshot, impact);
  return { snapshotId, impact };
}

/**
 * Fetch a previously stored simulation snapshot by its ID.
 *
 * Returns `undefined` when the ID is unknown (snapshot was never stored, or
 * the store was reset between runs).
 */
export async function fetchSnapshot(
  snapshotId: string,
  store: import("./snapshot-store").SnapshotStore,
): Promise<import("./snapshot-store").SimulationSnapshot | undefined> {
  return store.fetch(snapshotId);
}