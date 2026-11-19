/** Indexed store contract with fail-closed health state. */
import { InvoiceRecord, ListingRecord, PositionRecord, RiskScoreRecord } from "./types";

export interface StoreHealth {
  available: boolean;
  degraded: boolean;
  consecutiveFailures: number;
  lastFailureAt?: number;
  recoveredAt?: number;
}

export interface StoreMetrics {
  cacheHits: number;
  cacheMisses: number;
  cacheInvalidations: number;
  checkoutFailures: number;
}

export interface IndexedStore {
  invoices: InvoiceRecord[];
  listings: ListingRecord[];
  positions: PositionRecord[];
  riskScores: RiskScoreRecord[];
  lastIndexedLedger: number;
  health?: StoreHealth;
  metrics?: StoreMetrics;
  /** Optional adapter hook for a connection-pool checkout/query preflight. */
  checkout?: () => void;
  /** Optional adapter hook to discard read-through cache entries. */
  invalidateCache?: () => void;
}

export function createMemoryStore(seed: Partial<IndexedStore> = {}): IndexedStore {
  return {
    invoices: seed.invoices ?? [],
    listings: seed.listings ?? [],
    positions: seed.positions ?? [],
    riskScores: seed.riskScores ?? [],
    lastIndexedLedger: seed.lastIndexedLedger ?? 0,
    health: seed.health ?? { available: true, degraded: false, consecutiveFailures: 0 },
    metrics: seed.metrics ?? { cacheHits: 0, cacheMisses: 0, cacheInvalidations: 0, checkoutFailures: 0 },
    checkout: seed.checkout,
    invalidateCache: seed.invalidateCache,
  };
}

export function markStoreFailure(store: IndexedStore): void {
  const health = store.health ?? { available: true, degraded: false, consecutiveFailures: 0 };
  health.available = false;
  health.degraded = true;
  health.consecutiveFailures += 1;
  health.lastFailureAt = Date.now();
  store.health = health;
  store.metrics ??= { cacheHits: 0, cacheMisses: 0, cacheInvalidations: 0, checkoutFailures: 0 };
  store.metrics.checkoutFailures += 1;
  store.metrics.cacheInvalidations += 1;
  store.invalidateCache?.();
}

export function markStoreRecovered(store: IndexedStore): void {
  store.health = {
    available: true,
    degraded: false,
    consecutiveFailures: 0,
    recoveredAt: Date.now(),
  };
}
