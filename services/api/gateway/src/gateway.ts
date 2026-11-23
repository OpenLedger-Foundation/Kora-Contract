/**
 * API Gateway — Fail-Closed Stale Cache (Chaos GAP-1, Closes #<issue_number>)
 *
 * Problem: during a DB partition the read-through cache kept serving 200s with
 * stale data for up to ~8 s, violating the resilience contract.
 *
 * Fix: every cache entry is timestamped at write time.  `beginRead` checks
 * whether the entry is still within the freshness SLA before returning it.
 * If the entry has expired it is evicted and the caller falls through to the
 * live DB path.  When `store.health.available` is false all cache entries are
 * treated as expired regardless of age — the gateway fails closed rather than
 * serving silent stale data.
 *
 * Freshness SLA: `CACHE_FRESHNESS_SLA_MS` (default 8 000 ms).  Operators
 * tighten this to match their indexer lag SLA.  Set to 0 to disable caching
 * entirely (read-only mode — no cache, all reads hit the DB).
 */

import { FieldFilter, InvoiceFilter, InvoiceRecord, ListingFilter, ListingRecord, Page, PageArgs, PositionFilter, PositionRecord, RiskScoreFilter, RiskScoreRecord, SortDir } from "./types";
import { IndexedStore, markStoreFailure, markStoreRecovered } from "./store";
import { RateLimiter } from "./rateLimiter";
import { mapDbError, DbTimeoutError } from "./dbTimeout";

/** Default freshness SLA: entries older than this are evicted and re-fetched. */
export const DEFAULT_CACHE_FRESHNESS_SLA_MS = 8_000;

/**
 * Read `CACHE_FRESHNESS_SLA_MS` from the environment.
 *
 * Returns 0 when the env var is explicitly "0" (read-only / no-cache mode).
 * Throws if the value is negative or non-integer.
 */
export function readCacheFreshnessSlaMs(raw = process.env["CACHE_FRESHNESS_SLA_MS"]): number {
  if (raw === undefined || raw === "") return DEFAULT_CACHE_FRESHNESS_SLA_MS;
  const value = Number(raw);
  if (!Number.isInteger(value) || value < 0) {
    throw new Error(
      `CACHE_FRESHNESS_SLA_MS must be a non-negative integer (got ${JSON.stringify(raw)})`
    );
  }
  return value;
}

export class RateLimitedError extends Error {
  constructor(public resetAfterMs: number) { super(`RATE_LIMITED: retry after ${resetAfterMs}ms`); }
}

/** Transport adapters should map this error to 503 + Retry-After. */
export class ApiUnavailableError extends Error {
  readonly statusCode = 503;
  readonly retryAfterSeconds: number;
  constructor(retryAfterSeconds = 5, cause?: unknown) {
    super("INDEXER_UNAVAILABLE: indexed data is temporarily unavailable");
    this.name = "ApiUnavailableError";
    this.retryAfterSeconds = retryAfterSeconds;
    if (cause) this.cause = cause;
  }
}

/**
 * Thrown when a cache entry exists but has exceeded the freshness SLA and the
 * backing store is unavailable, so live data cannot be fetched to replace it.
 *
 * Transport adapters MUST map this to 503 + `Stale-While-Revalidate: 0` (not
 * a silent 200) so callers know the data cannot be trusted.
 *
 * Code: "STALE_CACHE" — distinct from DB_TIMEOUT so clients can apply
 * targeted retry logic (e.g. wait for the partition to heal rather than
 * immediately retrying a timed-out statement).
 */
export class StaleCacheError extends Error {
  readonly statusCode = 503;
  readonly code = "STALE_CACHE";
  readonly retryable = true;
  readonly retryAfterSeconds: number;
  readonly ageMs: number;

  constructor(ageMs: number, slaMs: number, retryAfterSeconds = 5) {
    super(
      `STALE_CACHE: cached response is ${ageMs} ms old (SLA: ${slaMs} ms); ` +
      "DB is unavailable — refusing to serve stale data"
    );
    this.name = "StaleCacheError";
    this.retryAfterSeconds = retryAfterSeconds;
    this.ageMs = ageMs;
  }
}

// ---------------------------------------------------------------------------
// Internal cache entry with timestamp
// ---------------------------------------------------------------------------

interface CacheEntry {
  page: Page<unknown>;
  storedAt: number; // Date.now() when this entry was written
}

function matches<T>(value: T, filter?: FieldFilter<T>): boolean {
  if (!filter) return true;
  if (filter.eq !== undefined && value !== filter.eq) return false;
  if (filter.in !== undefined && !filter.in.includes(value)) return false;
  if (filter.gte !== undefined && (value as unknown as number) < (filter.gte as unknown as number)) return false;
  if (filter.lte !== undefined && (value as unknown as number) > (filter.lte as unknown as number)) return false;
  return true;
}
function decodeCursor(cursor?: string): number { if (!cursor) return 0; try { const n = Number(Buffer.from(cursor, "base64").toString("utf8")); return Number.isFinite(n) && n >= 0 ? Math.floor(n) : 0; } catch { return 0; } }
function encodeCursor(offset: number): string { return Buffer.from(String(offset)).toString("base64"); }
function paginate<T>(items: T[], args: PageArgs, lastIndexedLedger: number): Page<T> { const limit = Math.max(1, Math.min(100, args.limit ?? 20)); const offset = decodeCursor(args.cursor); const data = items.slice(offset, offset + limit); const next = offset + data.length < items.length ? encodeCursor(offset + data.length) : null; return { data, nextCursor: next, lastIndexedLedger }; }
export interface QueryOptions { apiKey: string; sortBy?: string; sortDir?: SortDir; }
function sortBy<T>(items: T[], key: string | undefined, dir: SortDir | undefined): T[] { if (!key) return items; const d = dir === "desc" ? -1 : 1; return [...items].sort((a, b) => { const av = (a as Record<string, unknown>)[key] as number | string; const bv = (b as Record<string, unknown>)[key] as number | string; return av < bv ? -1 * d : av > bv ? 1 * d : 0; }); }

type QueryKind = "invoices" | "listings" | "positions" | "riskScores";

export class ApiGateway {
  /**
   * Timestamped read-through cache.  Entries are evicted when they exceed
   * `freshnessSlaMs` or when the store health is degraded (fail-closed).
   */
  private readonly cache = new Map<string, CacheEntry>();
  private readonly freshnessSlaMs: number;

  constructor(
    private store: IndexedStore,
    private limiter: RateLimiter = new RateLimiter(),
    private retryAfterSeconds = 5,
    freshnessSlaMs?: number
  ) {
    this.freshnessSlaMs = freshnessSlaMs ?? readCacheFreshnessSlaMs();
  }

  private checkRateLimit(apiKey: string): void {
    const res = this.limiter.consume(apiKey);
    if (!res.allowed) throw new RateLimitedError(res.resetAfterMs);
  }

  /**
   * Determine whether a cache entry is fresh enough to serve.
   *
   * Rules (in priority order):
   *   1. If `store.health.available === false` → never serve from cache
   *      (fail-closed: DB partition must not be masked by stale data).
   *   2. If `freshnessSlaMs === 0` → caching is disabled; always miss.
   *   3. If the entry age exceeds `freshnessSlaMs` → evict and miss.
   *   4. Otherwise → cache hit.
   *
   * Returns the entry if it is fresh, or null with the stale ageMs if it
   * exists but is expired (so the caller can decide whether to throw
   * `StaleCacheError` or just re-fetch).
   */
  private checkCacheEntry(cacheKey: string, now = Date.now()): {
    hit: Page<unknown>;
    stale: false;
  } | {
    hit: null;
    stale: false;
  } | {
    hit: null;
    stale: true;
    ageMs: number;
  } {
    const entry = this.cache.get(cacheKey);
    if (!entry) return { hit: null, stale: false };

    const ageMs = now - entry.storedAt;

    // Rule 1: store degraded → evict immediately (fail-closed)
    if (this.store.health?.available === false) {
      this.cache.delete(cacheKey);
      return { hit: null, stale: true, ageMs };
    }

    // Rule 2: caching disabled
    if (this.freshnessSlaMs === 0) {
      this.cache.delete(cacheKey);
      return { hit: null, stale: false };
    }

    // Rule 3: expired
    if (ageMs > this.freshnessSlaMs) {
      this.cache.delete(cacheKey);
      return { hit: null, stale: false };
    }

    // Rule 4: fresh hit
    return { hit: entry.page, stale: false };
  }

  private beginRead(apiKey: string, kind: QueryKind, key: string): Page<unknown> | undefined {
    this.checkRateLimit(apiKey);

    // Fail-closed path: store marked unavailable → check if we have a stale
    // entry to report explicitly rather than silently returning one.
    if (this.store.health?.available === false) {
      this.store.metrics && (this.store.metrics.cacheMisses += 1);
      const entry = this.cache.get(`${kind}:${key}`);
      if (entry) {
        const ageMs = Date.now() - entry.storedAt;
        this.cache.delete(`${kind}:${key}`);
        throw new StaleCacheError(ageMs, this.freshnessSlaMs, this.retryAfterSeconds);
      }
      throw new ApiUnavailableError(this.retryAfterSeconds);
    }

    const cacheResult = this.checkCacheEntry(`${kind}:${key}`);
    if (cacheResult.hit) {
      this.store.metrics && (this.store.metrics.cacheHits += 1);
      return cacheResult.hit;
    }

    this.store.metrics && (this.store.metrics.cacheMisses += 1);
    try {
      this.store.checkout?.();
      markStoreRecovered(this.store);
      return undefined;
    } catch (cause) {
      markStoreFailure(this.store);
      this.invalidateReadCache();
      const mapped = mapDbError(cause);
      if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause);
      throw new ApiUnavailableError(this.retryAfterSeconds, cause);
    }
  }

  private finish<T>(kind: QueryKind, key: string, page: Page<T>): Page<T> {
    // Only cache when the SLA is non-zero and the store is healthy
    if (this.freshnessSlaMs > 0 && this.store.health?.available !== false) {
      this.cache.set(`${kind}:${key}`, { page, storedAt: Date.now() });
    }
    return page;
  }

  invalidateReadCache(): void {
    this.cache.clear();
    this.store.metrics && (this.store.metrics.cacheInvalidations += 1);
    this.store.invalidateCache?.();
  }

  get health() { return this.store.health ?? { available: true, degraded: false, consecutiveFailures: 0 }; }

  /**
   * Expose freshness SLA for observability (e.g. /health endpoint can include
   * this so operators know the configured window).
   */
  get cacheFreshnessSlaMs(): number { return this.freshnessSlaMs; }

  queryInvoices(filter: InvoiceFilter = {}, page: PageArgs = {}, opts: QueryOptions): Page<InvoiceRecord> {
    const key = JSON.stringify([filter, page, opts.sortBy, opts.sortDir]);
    const cached = this.beginRead(opts.apiKey, "invoices", key);
    if (cached) return cached as Page<InvoiceRecord>;
    try {
      return this.finish("invoices", key, paginate(sortBy(this.store.invoices.filter(i => matches(i.sme, filter.sme) && matches(i.status, filter.status) && matches(i.riskTier, filter.riskTier) && matches(i.riskScore, filter.riskScore) && matches(i.currency, filter.currency)), opts.sortBy, opts.sortDir), page, this.store.lastIndexedLedger));
    } catch (cause) {
      markStoreFailure(this.store);
      this.invalidateReadCache();
      const mapped = mapDbError(cause);
      if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause);
      throw new ApiUnavailableError(this.retryAfterSeconds, cause);
    }
  }

  queryListings(filter: ListingFilter = {}, page: PageArgs = {}, opts: QueryOptions): Page<ListingRecord> {
    const key = JSON.stringify([filter, page, opts.sortBy, opts.sortDir]);
    const cached = this.beginRead(opts.apiKey, "listings", key);
    if (cached) return cached as Page<ListingRecord>;
    try {
      return this.finish("listings", key, paginate(sortBy(this.store.listings.filter(l => matches(l.seller, filter.seller) && matches(l.token, filter.token) && (filter.isActive === undefined || l.isActive === filter.isActive)), opts.sortBy, opts.sortDir), page, this.store.lastIndexedLedger));
    } catch (cause) {
      markStoreFailure(this.store);
      this.invalidateReadCache();
      const mapped = mapDbError(cause);
      if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause);
      throw new ApiUnavailableError(this.retryAfterSeconds, cause);
    }
  }

  queryPositions(filter: PositionFilter = {}, page: PageArgs = {}, opts: QueryOptions): Page<PositionRecord> {
    const key = JSON.stringify([filter, page, opts.sortBy, opts.sortDir]);
    const cached = this.beginRead(opts.apiKey, "positions", key);
    if (cached) return cached as Page<PositionRecord>;
    try {
      return this.finish("positions", key, paginate(sortBy(this.store.positions.filter(p => matches(p.investor, filter.investor) && matches(p.invoiceId, filter.invoiceId)), opts.sortBy, opts.sortDir), page, this.store.lastIndexedLedger));
    } catch (cause) {
      markStoreFailure(this.store);
      this.invalidateReadCache();
      const mapped = mapDbError(cause);
      if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause);
      throw new ApiUnavailableError(this.retryAfterSeconds, cause);
    }
  }

  queryRiskScores(filter: RiskScoreFilter = {}, page: PageArgs = {}, opts: QueryOptions): Page<RiskScoreRecord> {
    const key = JSON.stringify([filter, page, opts.sortBy, opts.sortDir]);
    const cached = this.beginRead(opts.apiKey, "riskScores", key);
    if (cached) return cached as Page<RiskScoreRecord>;
    try {
      return this.finish("riskScores", key, paginate(sortBy(this.store.riskScores.filter(r => (filter.kind === undefined || r.kind === filter.kind) && matches(r.verifier, filter.verifier) && matches(r.score, filter.score)), opts.sortBy, opts.sortDir), page, this.store.lastIndexedLedger));
    } catch (cause) {
      markStoreFailure(this.store);
      this.invalidateReadCache();
      const mapped = mapDbError(cause);
      if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause);
      throw new ApiUnavailableError(this.retryAfterSeconds, cause);
    }
  }
}
