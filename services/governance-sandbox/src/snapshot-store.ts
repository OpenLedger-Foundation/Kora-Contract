/**
 * Snapshot store — persists governance simulation results keyed by a stable
 * snapshot ID so voters can retrieve an identical summary after the fact.
 *
 * The snapshot ID is a deterministic SHA-256 hex digest of the serialised
 * (change, snapshot) pair so the same proposal/state always produces the
 * same ID.  Callers that need to store multiple runs of a *mutable* state
 * should pass a nonce in the ProtocolSnapshot.capturedAt field.
 *
 * ## Nondeterminism sources — read before relying on snapshot equality
 *
 *  1. **capturedAt** — `ProtocolSnapshot.capturedAt` is typically a ledger
 *     sequence string, but callers sometimes pass `Date.now()` or an ISO
 *     timestamp.  Two simulations with different `capturedAt` values produce
 *     different snapshot IDs even when the protocol state is identical.
 *
 *  2. **BigInt serialisation** — JSON.stringify cannot serialise BigInt natively.
 *     The store uses a replacer that converts BigInt to `{"__bigint__":"<n>"}`
 *     so the digest is stable across runtimes that agree on this convention.
 *
 *  3. **Plugin registration order** — The Impact object is computed by whichever
 *     plugin the registry selects.  If a new plugin is registered between two
 *     simulation runs for the same change type, the Impact payload—and therefore
 *     the snapshot ID—may differ even with identical inputs.
 *
 *  4. **Object key ordering** — `JSON.stringify` traverses keys in insertion
 *     order.  The store normalises keys alphabetically before hashing so
 *     different runtimes produce identical digests.
 *
 * Issue: #769
 */

import { createHash } from "crypto";
import { Impact, ParameterChange, ProtocolSnapshot } from "./index";

// ── Payload size guard ────────────────────────────────────────────────────────

/** Maximum serialised snapshot payload in bytes (1 MiB). */
export const MAX_SNAPSHOT_BYTES = 1 * 1024 * 1024;

// ── Stored snapshot shape ─────────────────────────────────────────────────────

export interface SimulationSnapshot {
  /** Stable deterministic ID (SHA-256 hex of the serialised inputs). */
  snapshotId: string;
  /** ISO-8601 timestamp of when the snapshot was stored. */
  storedAt: string;
  /** The parameter change that was simulated. */
  change: ParameterChange;
  /** The protocol snapshot used as input. */
  protocolSnapshot: ProtocolSnapshot;
  /** The computed impact summary. */
  impact: Impact;
}

// ── Storage interface ─────────────────────────────────────────────────────────

export interface SnapshotStore {
  /**
   * Persist a simulation snapshot.
   * Returns the snapshotId.
   * Upserts: calling save with an identical (change, protocolSnapshot) pair
   * overwrites the previous entry.
   *
   * @throws {Error} If the serialised payload exceeds MAX_SNAPSHOT_BYTES.
   */
  save(
    change: ParameterChange,
    protocolSnapshot: ProtocolSnapshot,
    impact: Impact,
  ): Promise<string>;

  /**
   * Retrieve a previously stored snapshot by its ID.
   * Returns `undefined` when no snapshot with that ID exists.
   */
  fetch(snapshotId: string): Promise<SimulationSnapshot | undefined>;

  /** Return the total number of stored snapshots. */
  size(): Promise<number>;
}

// ── Serialisation helpers ─────────────────────────────────────────────────────

/**
 * JSON replacer that converts BigInt values to a stable tagged representation
 * so they survive the JSON round-trip and produce consistent digests.
 */
export function bigintReplacer(_key: string, value: unknown): unknown {
  if (typeof value === "bigint") {
    return { __bigint__: value.toString() };
  }
  return value;
}

/**
 * JSON reviver that reconstructs BigInt values from the tagged representation
 * written by `bigintReplacer`.
 */
export function bigintReviver(_key: string, value: unknown): unknown {
  if (
    typeof value === "object" &&
    value !== null &&
    "__bigint__" in value &&
    typeof (value as Record<string, unknown>).__bigint__ === "string"
  ) {
    return BigInt((value as Record<string, string>).__bigint__);
  }
  return value;
}

/**
 * Serialise an arbitrary value to a *stable* JSON string:
 * object keys are sorted alphabetically at every level so that insertion-order
 * differences do not produce different digests.
 */
export function stableStringify(value: unknown): string {
  return JSON.stringify(sortKeys(value), bigintReplacer);
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value !== null && typeof value === "object" && !(value instanceof Date)) {
    const sorted: Record<string, unknown> = {};
    for (const key of Object.keys(value as object).sort()) {
      sorted[key] = sortKeys((value as Record<string, unknown>)[key]);
    }
    return sorted;
  }
  return value;
}

/**
 * Compute the deterministic snapshot ID from the simulation inputs.
 */
export function computeSnapshotId(
  change: ParameterChange,
  protocolSnapshot: ProtocolSnapshot,
): string {
  const payload = stableStringify({ change, protocolSnapshot });
  return createHash("sha256").update(payload).digest("hex");
}

// ── In-memory implementation ──────────────────────────────────────────────────

export class InMemorySnapshotStore implements SnapshotStore {
  private readonly _store = new Map<string, SimulationSnapshot>();

  async save(
    change: ParameterChange,
    protocolSnapshot: ProtocolSnapshot,
    impact: Impact,
  ): Promise<string> {
    const snapshotId = computeSnapshotId(change, protocolSnapshot);

    const serialised = stableStringify({ change, protocolSnapshot, impact });
    if (Buffer.byteLength(serialised, "utf8") > MAX_SNAPSHOT_BYTES) {
      throw new Error(
        `Snapshot payload exceeds ${MAX_SNAPSHOT_BYTES} bytes. ` +
          `Consider reducing the number of listings or investors in ProtocolSnapshot.`,
      );
    }

    const entry: SimulationSnapshot = {
      snapshotId,
      storedAt: new Date().toISOString(),
      change,
      protocolSnapshot,
      impact,
    };
    this._store.set(snapshotId, entry);
    return snapshotId;
  }

  async fetch(snapshotId: string): Promise<SimulationSnapshot | undefined> {
    return this._store.get(snapshotId);
  }

  async size(): Promise<number> {
    return this._store.size;
  }
}
