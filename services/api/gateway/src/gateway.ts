import { FieldFilter, InvoiceFilter, InvoiceRecord, ListingFilter, ListingRecord, Page, PageArgs, PositionFilter, PositionRecord, RiskScoreFilter, RiskScoreRecord, SortDir } from "./types";
import { IndexedStore, markStoreFailure, markStoreRecovered } from "./store";
import { RateLimiter } from "./rateLimiter";
import { mapDbError, DbTimeoutError } from "./dbTimeout";

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
  private readonly cache = new Map<string, Page<unknown>>();
  constructor(private store: IndexedStore, private limiter: RateLimiter = new RateLimiter(), private retryAfterSeconds = 5) {}
  private checkRateLimit(apiKey: string): void { const res = this.limiter.consume(apiKey); if (!res.allowed) throw new RateLimitedError(res.resetAfterMs); }
  private beginRead(apiKey: string, kind: QueryKind, key: string): Page<unknown> | undefined {
    this.checkRateLimit(apiKey);
    if (this.store.health?.available === false) { this.store.metrics && (this.store.metrics.cacheMisses += 1); throw new ApiUnavailableError(this.retryAfterSeconds); }
    const cached = this.cache.get(`${kind}:${key}`); if (cached) { this.store.metrics && (this.store.metrics.cacheHits += 1); return cached; }
    this.store.metrics && (this.store.metrics.cacheMisses += 1);
    try { this.store.checkout?.(); markStoreRecovered(this.store); return undefined; }
    catch (cause) { markStoreFailure(this.store); const mapped = mapDbError(cause); if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause); throw new ApiUnavailableError(this.retryAfterSeconds, cause); }
  }
  private finish<T>(kind: QueryKind, key: string, page: Page<T>): Page<T> { this.cache.set(`${kind}:${key}`, page); return page; }
  invalidateReadCache(): void { this.cache.clear(); this.store.metrics && (this.store.metrics.cacheInvalidations += 1); this.store.invalidateCache?.(); }
  get health() { return this.store.health ?? { available: true, degraded: false, consecutiveFailures: 0 }; }
  queryInvoices(filter: InvoiceFilter = {}, page: PageArgs = {}, opts: QueryOptions): Page<InvoiceRecord> {
    const key = JSON.stringify([filter, page, opts.sortBy, opts.sortDir]); const cached = this.beginRead(opts.apiKey, "invoices", key); if (cached) return cached as Page<InvoiceRecord>;
    try { return this.finish("invoices", key, paginate(sortBy(this.store.invoices.filter(i => matches(i.sme, filter.sme) && matches(i.status, filter.status) && matches(i.riskTier, filter.riskTier) && matches(i.riskScore, filter.riskScore) && matches(i.currency, filter.currency)), opts.sortBy, opts.sortDir), page, this.store.lastIndexedLedger)); } catch (cause) { markStoreFailure(this.store); this.invalidateReadCache(); const mapped = mapDbError(cause); if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause); throw new ApiUnavailableError(this.retryAfterSeconds, cause); }
  }
  queryListings(filter: ListingFilter = {}, page: PageArgs = {}, opts: QueryOptions): Page<ListingRecord> {
    const key = JSON.stringify([filter, page, opts.sortBy, opts.sortDir]); const cached = this.beginRead(opts.apiKey, "listings", key); if (cached) return cached as Page<ListingRecord>;
    try { return this.finish("listings", key, paginate(sortBy(this.store.listings.filter(l => matches(l.seller, filter.seller) && matches(l.token, filter.token) && (filter.isActive === undefined || l.isActive === filter.isActive)), opts.sortBy, opts.sortDir), page, this.store.lastIndexedLedger)); } catch (cause) { markStoreFailure(this.store); this.invalidateReadCache(); const mapped = mapDbError(cause); if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause); throw new ApiUnavailableError(this.retryAfterSeconds, cause); }
  }
  queryPositions(filter: PositionFilter = {}, page: PageArgs = {}, opts: QueryOptions): Page<PositionRecord> {
    const key = JSON.stringify([filter, page, opts.sortBy, opts.sortDir]); const cached = this.beginRead(opts.apiKey, "positions", key); if (cached) return cached as Page<PositionRecord>;
    try { return this.finish("positions", key, paginate(sortBy(this.store.positions.filter(p => matches(p.investor, filter.investor) && matches(p.invoiceId, filter.invoiceId)), opts.sortBy, opts.sortDir), page, this.store.lastIndexedLedger)); } catch (cause) { markStoreFailure(this.store); this.invalidateReadCache(); const mapped = mapDbError(cause); if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause); throw new ApiUnavailableError(this.retryAfterSeconds, cause); }
  }
  queryRiskScores(filter: RiskScoreFilter = {}, page: PageArgs = {}, opts: QueryOptions): Page<RiskScoreRecord> {
    const key = JSON.stringify([filter, page, opts.sortBy, opts.sortDir]); const cached = this.beginRead(opts.apiKey, "riskScores", key); if (cached) return cached as Page<RiskScoreRecord>;
    try { return this.finish("riskScores", key, paginate(sortBy(this.store.riskScores.filter(r => (filter.kind === undefined || r.kind === filter.kind) && matches(r.verifier, filter.verifier) && matches(r.score, filter.score)), opts.sortBy, opts.sortDir), page, this.store.lastIndexedLedger)); } catch (cause) { markStoreFailure(this.store); this.invalidateReadCache(); const mapped = mapDbError(cause); if (mapped) throw new DbTimeoutError(mapped.retryAfterSeconds, cause); throw new ApiUnavailableError(this.retryAfterSeconds, cause); }
  }
}
