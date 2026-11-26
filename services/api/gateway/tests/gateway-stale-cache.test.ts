/**
 * Tests: stale cache fail-closed during DB partition (Chaos GAP-1).
 *
 * Closes #<issue_number>
 *
 * Invariants:
 *  1.  When store.health.available is false, a fresh cached entry is evicted
 *      and StaleCacheError is thrown (not a silent 200).
 *  2.  When store.health.available is false and there is NO cache entry,
 *      ApiUnavailableError is thrown (not StaleCacheError).
 *  3.  StaleCacheError carries statusCode 503, code "STALE_CACHE", retryable true.
 *  4.  StaleCacheError.ageMs reflects how old the entry was.
 *  5.  A cache entry is served while still within the freshness SLA.
 *  6.  A cache entry older than the freshness SLA is evicted and re-fetched
 *      (store healthy — no error, just a cache miss + re-query).
 *  7.  freshnessSlaMs === 0 disables caching entirely; every request hits the DB.
 *  8.  All four query methods propagate StaleCacheError when degraded.
 *  9.  Partition recovery: once store is re-marked available, queries succeed again.
 * 10.  readCacheFreshnessSlaMs validates the env var correctly.
 * 11.  Chaos partition scenario: simulated partition → stale error → recovery → 200.
 */

import { ApiGateway, ApiUnavailableError, StaleCacheError, readCacheFreshnessSlaMs } from "../src/gateway";
import { createMemoryStore, markStoreFailure, markStoreRecovered } from "../src/store";
import { DbTimeoutError } from "../src/dbTimeout";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const opts = { apiKey: "test-key" };

function minimalStore(overrides = {}) {
  return createMemoryStore({
    invoices: [{
      id: "i1", sme: "SME_A", debtorHash: "h1", amount: "100", currency: "USDC",
      dueDate: 10, ipfsCid: "cid1", riskScore: 20, riskTier: "AAA", status: "Listed", createdAt: 1,
    }],
    listings: [{
      invoiceId: "i1", seller: "SME_A", askingPrice: "90", faceValue: "100",
      token: "USDC", fundedAmount: "0", fundingDeadline: 999, isActive: true,
    }],
    positions: [{ investor: "INV_1", invoiceId: "i1", contributed: "10", shareBps: 1000, yieldClaimed: "0" }],
    riskScores: [{ subject: "SME_A", kind: "sme" as const, score: 25, verifier: "V1", updatedAt: 5 }],
    ...overrides,
  });
}

// ---------------------------------------------------------------------------
// 1. Store degraded + stale entry → StaleCacheError (not silent 200)
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: degraded store with cached entry", () => {
  it("throws StaleCacheError, not ApiUnavailableError, when a stale entry exists", () => {
    const store = minimalStore();
    // SLA long enough that the entry is "fresh" on first read
    const gw = new ApiGateway(store, undefined, 5, 60_000);

    // Populate the cache while store is healthy
    gw.queryInvoices({}, {}, opts);

    // Now simulate a DB partition
    markStoreFailure(store);

    // The cached entry must be surfaced as STALE_CACHE, not served silently
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(StaleCacheError);
    expect(() => gw.queryInvoices({}, {}, opts)).not.toThrow(ApiUnavailableError);
  });

  it("StaleCacheError has statusCode 503", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);
    gw.queryInvoices({}, {}, opts);
    markStoreFailure(store);
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) { expect((err as StaleCacheError).statusCode).toBe(503); }
  });

  it("StaleCacheError has code STALE_CACHE", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);
    gw.queryInvoices({}, {}, opts);
    markStoreFailure(store);
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) { expect((err as StaleCacheError).code).toBe("STALE_CACHE"); }
  });

  it("StaleCacheError has retryable = true", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);
    gw.queryInvoices({}, {}, opts);
    markStoreFailure(store);
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) { expect((err as StaleCacheError).retryable).toBe(true); }
  });

  it("StaleCacheError.ageMs is non-negative", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);
    gw.queryInvoices({}, {}, opts);
    markStoreFailure(store);
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) { expect((err as StaleCacheError).ageMs).toBeGreaterThanOrEqual(0); }
  });
});

// ---------------------------------------------------------------------------
// 2. Store degraded + no cache entry → ApiUnavailableError
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: degraded store, no cache", () => {
  it("throws ApiUnavailableError when no cache entry exists and store is degraded", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);
    // No cache populated; fail the store immediately
    markStoreFailure(store);
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(ApiUnavailableError);
    expect(() => gw.queryInvoices({}, {}, opts)).not.toThrow(StaleCacheError);
  });
});

// ---------------------------------------------------------------------------
// 3. Fresh cache hit while store is healthy
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: fresh cache hit", () => {
  it("serves a cached result within the SLA without hitting the store", () => {
    const store = minimalStore();
    let checkoutCalls = 0;
    store.checkout = () => { checkoutCalls += 1; };
    const gw = new ApiGateway(store, undefined, 5, 60_000);

    // First call populates cache
    gw.queryInvoices({}, {}, opts);
    const checkoutAfterFirst = checkoutCalls;

    // Second call should be a cache hit — no extra checkout
    const page = gw.queryInvoices({}, {}, opts);
    expect(page.data).toHaveLength(1);
    expect(checkoutCalls).toBe(checkoutAfterFirst); // no additional checkout
  });
});

// ---------------------------------------------------------------------------
// 4. Cache entry exceeds SLA with healthy store → evict and re-fetch
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: expired entry evicted when store healthy", () => {
  it("evicts an expired entry and re-queries the DB without throwing", () => {
    const store = minimalStore();
    let checkoutCalls = 0;
    store.checkout = () => { checkoutCalls += 1; };

    // SLA of 1 ms so the entry expires almost immediately
    const gw = new ApiGateway(store, undefined, 5, 1);

    // First call populates cache
    gw.queryInvoices({}, {}, opts);
    const afterFirst = checkoutCalls;

    // Wait for the entry to expire, then query again
    return new Promise<void>((resolve) => {
      setTimeout(() => {
        const page = gw.queryInvoices({}, {}, opts);
        expect(page.data).toHaveLength(1);
        // A new checkout was issued because the entry expired
        expect(checkoutCalls).toBeGreaterThan(afterFirst);
        resolve();
      }, 10);
    });
  });
});

// ---------------------------------------------------------------------------
// 5. freshnessSlaMs = 0 disables caching entirely
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: caching disabled (SLA=0)", () => {
  it("every request triggers a checkout when slaMs is 0", () => {
    const store = minimalStore();
    let checkoutCalls = 0;
    store.checkout = () => { checkoutCalls += 1; };

    const gw = new ApiGateway(store, undefined, 5, 0);

    gw.queryInvoices({}, {}, opts);
    gw.queryInvoices({}, {}, opts);
    gw.queryInvoices({}, {}, opts);

    expect(checkoutCalls).toBe(3);
  });

  it("degraded store + slaMs=0 throws ApiUnavailableError (no stale entry possible)", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 0);
    // Populate one request first
    gw.queryInvoices({}, {}, opts);
    // Then degrade
    markStoreFailure(store);
    // With sla=0, nothing was cached, so ApiUnavailableError not StaleCacheError
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(ApiUnavailableError);
  });
});

// ---------------------------------------------------------------------------
// 6. All four query methods surface StaleCacheError when degraded
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: all query methods propagate StaleCacheError", () => {
  function degradedGateway() {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);
    // Populate cache while healthy
    gw.queryInvoices({}, {}, opts);
    gw.queryListings({}, {}, opts);
    gw.queryPositions({}, {}, opts);
    gw.queryRiskScores({}, {}, opts);
    // Now degrade
    markStoreFailure(store);
    return gw;
  }

  it("queryInvoices", () => {
    expect(() => degradedGateway().queryInvoices({}, {}, opts)).toThrow(StaleCacheError);
  });

  it("queryListings", () => {
    expect(() => degradedGateway().queryListings({}, {}, opts)).toThrow(StaleCacheError);
  });

  it("queryPositions", () => {
    expect(() => degradedGateway().queryPositions({}, {}, opts)).toThrow(StaleCacheError);
  });

  it("queryRiskScores", () => {
    expect(() => degradedGateway().queryRiskScores({}, {}, opts)).toThrow(StaleCacheError);
  });
});

// ---------------------------------------------------------------------------
// 7. Recovery: once store re-marked available, queries succeed
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: partition recovery", () => {
  it("returns live data once store health is restored", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);

    // Populate cache while healthy, then partition
    gw.queryInvoices({}, {}, opts);
    markStoreFailure(store);

    // During partition: stale error
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(StaleCacheError);

    // Heal: mark available again
    markStoreRecovered(store);

    // Should succeed and return data
    const page = gw.queryInvoices({}, {}, opts);
    expect(page.data).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// 8. readCacheFreshnessSlaMs env var validation
// ---------------------------------------------------------------------------

describe("readCacheFreshnessSlaMs", () => {
  it("returns default when env var is unset", () => {
    expect(readCacheFreshnessSlaMs(undefined)).toBe(8_000);
  });

  it("returns default when env var is empty string", () => {
    expect(readCacheFreshnessSlaMs("")).toBe(8_000);
  });

  it("returns 0 when env var is '0' (disable caching)", () => {
    expect(readCacheFreshnessSlaMs("0")).toBe(0);
  });

  it("returns configured value for a positive integer string", () => {
    expect(readCacheFreshnessSlaMs("3000")).toBe(3_000);
  });

  it("throws for negative values", () => {
    expect(() => readCacheFreshnessSlaMs("-1")).toThrow(/non-negative/);
  });

  it("throws for non-integer values", () => {
    expect(() => readCacheFreshnessSlaMs("1.5")).toThrow(/non-negative/);
  });
});

// ---------------------------------------------------------------------------
// 9. cacheFreshnessSlaMs property is accessible for observability
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: gateway exposes SLA for observability", () => {
  it("cacheFreshnessSlaMs returns the configured value", () => {
    const gw = new ApiGateway(minimalStore(), undefined, 5, 12_000);
    expect(gw.cacheFreshnessSlaMs).toBe(12_000);
  });
});

// ---------------------------------------------------------------------------
// 10. Chaos partition simulation (unit-level integration scenario)
// ---------------------------------------------------------------------------

describe("stale cache fail-closed: simulated partition scenario", () => {
  it("baseline ok → partition stale error → recovery ok", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);

    // Baseline: healthy request succeeds
    expect(() => gw.queryInvoices({}, {}, opts)).not.toThrow();

    // Partition: mark store failed with stale entry
    markStoreFailure(store);
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(StaleCacheError);

    // Recovery: mark store healthy again
    markStoreRecovered(store);
    const page = gw.queryInvoices({}, {}, opts);
    expect(page.data).toHaveLength(1);
    expect(page.data[0].id).toBe("i1");
  });

  it("no silent 200 is served during partition at any point", () => {
    const store = minimalStore();
    const gw = new ApiGateway(store, undefined, 5, 60_000);
    gw.queryInvoices({}, {}, opts); // populate cache

    markStoreFailure(store);

    // Multiple polls during partition must all throw, never return a page
    for (let i = 0; i < 5; i++) {
      expect(() => gw.queryInvoices({}, {}, opts)).toThrow();
    }
  });
});
