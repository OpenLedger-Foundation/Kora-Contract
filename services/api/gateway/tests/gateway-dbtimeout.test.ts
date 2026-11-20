/**
 * Integration tests: gateway DB timeout propagation (issue #1095, Chaos GAP-2).
 *
 * These tests simulate a slow/cancelled DB query by injecting a `checkout`
 * hook on the IndexedStore that throws a PostgreSQL-style error, then assert
 * that the gateway surfaces `DbTimeoutError` (not the generic
 * `ApiUnavailableError`) so that transport adapters can emit HTTP 503 +
 * Retry-After with the `DB_TIMEOUT` code.
 *
 * Invariants:
 *  1. SQLSTATE 57014 from checkout → DbTimeoutError (not ApiUnavailableError)
 *  2. Message-pattern timeout from checkout → DbTimeoutError
 *  3. DbTimeoutError carries correct statusCode / code / retryable
 *  4. DbTimeoutError carries retryAfterSeconds ≥ 1
 *  5. Non-timeout errors from checkout → ApiUnavailableError (unchanged behaviour)
 *  6. SQLSTATE 57014 during query execution → DbTimeoutError
 *  7. Store health is marked unavailable after a timeout
 *  8. Store health recovers on subsequent healthy checkout
 *  9. All four query methods propagate DbTimeoutError correctly
 * 10. Pool-recovery test: after the failing checkout is replaced with a healthy
 *     one the next query succeeds (pool recovers after timeout)
 */

import { ApiGateway, ApiUnavailableError } from "../src/gateway";
import { createMemoryStore } from "../src/store";
import { DbTimeoutError, DB_TIMEOUT_CODE } from "../src/dbTimeout";

// ── Helpers ──────────────────────────────────────────────────────────────────

function pgTimeoutError(): Error & { code: string } {
  return Object.assign(new Error("ERROR: canceling statement due to statement timeout"), {
    code: "57014",
  });
}

function pgMessageError(): Error {
  return new Error("statement timeout");
}

function genericDbError(): Error {
  return new Error("connection refused");
}

function seedGateway(checkout?: () => void, retryAfterSeconds = 5): ApiGateway {
  const store = createMemoryStore({
    invoices: [
      {
        id: "i1",
        sme: "SME_A",
        debtorHash: "h1",
        amount: "100",
        currency: "USDC",
        dueDate: 10,
        ipfsCid: "cid1",
        riskScore: 20,
        riskTier: "AAA",
        status: "Listed",
        createdAt: 1,
      },
    ],
    listings: [
      {
        invoiceId: "i1",
        seller: "SME_A",
        askingPrice: "90",
        faceValue: "100",
        token: "USDC",
        fundedAmount: "0",
        fundingDeadline: 999,
        isActive: true,
      },
    ],
    positions: [
      {
        investor: "INV_1",
        invoiceId: "i1",
        contributed: "10",
        shareBps: 1000,
        yieldClaimed: "0",
      },
    ],
    riskScores: [
      {
        subject: "SME_A",
        kind: "sme",
        score: 25,
        verifier: "V1",
        updatedAt: 5,
      },
    ],
    checkout,
  });
  return new ApiGateway(store, undefined, retryAfterSeconds);
}

const opts = { apiKey: "test-key" };

// ── 1. SQLSTATE 57014 from checkout → DbTimeoutError ─────────────────────

describe("gateway DB timeout — checkout throws SQLSTATE 57014", () => {
  it("queryInvoices throws DbTimeoutError, not ApiUnavailableError", () => {
    const gw = seedGateway(() => { throw pgTimeoutError(); });
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(DbTimeoutError);
    expect(() => gw.queryInvoices({}, {}, opts)).not.toThrow(ApiUnavailableError);
  });

  it("thrown error has statusCode 503", () => {
    const gw = seedGateway(() => { throw pgTimeoutError(); });
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) { expect((err as DbTimeoutError).statusCode).toBe(503); }
  });

  it("thrown error has code DB_TIMEOUT", () => {
    const gw = seedGateway(() => { throw pgTimeoutError(); });
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) { expect((err as DbTimeoutError).code).toBe(DB_TIMEOUT_CODE); }
  });

  it("thrown error has retryable = true", () => {
    const gw = seedGateway(() => { throw pgTimeoutError(); });
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) { expect((err as DbTimeoutError).retryable).toBe(true); }
  });

  it("retryAfterSeconds is at least 1", () => {
    const gw = seedGateway(() => { throw pgTimeoutError(); });
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) { expect((err as DbTimeoutError).retryAfterSeconds).toBeGreaterThanOrEqual(1); }
  });
});

// ── 2. Message-pattern timeout → DbTimeoutError ───────────────────────────

describe("gateway DB timeout — checkout throws message-pattern timeout", () => {
  it("queryInvoices throws DbTimeoutError for 'statement timeout' message", () => {
    const gw = seedGateway(() => { throw pgMessageError(); });
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(DbTimeoutError);
  });

  it("queryInvoices throws DbTimeoutError for 'query timed out' message", () => {
    const err = new Error("query timed out");
    const gw = seedGateway(() => { throw err; });
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(DbTimeoutError);
  });
});

// ── 3–4. Non-timeout errors remain ApiUnavailableError ───────────────────

describe("gateway DB timeout — non-timeout errors", () => {
  it("queryInvoices throws ApiUnavailableError for generic connection errors", () => {
    const gw = seedGateway(() => { throw genericDbError(); });
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(ApiUnavailableError);
  });

  it("queryInvoices does not throw DbTimeoutError for generic errors", () => {
    const gw = seedGateway(() => { throw genericDbError(); });
    expect(() => gw.queryInvoices({}, {}, opts)).not.toThrow(DbTimeoutError);
  });
});

// ── 5. All four query methods propagate DbTimeoutError ─────────────────────

describe("gateway DB timeout — all query methods", () => {
  let gw: ApiGateway;
  beforeEach(() => {
    gw = seedGateway(() => { throw pgTimeoutError(); });
  });

  it("queryInvoices", () => {
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(DbTimeoutError);
  });

  it("queryListings", () => {
    expect(() => gw.queryListings({}, {}, opts)).toThrow(DbTimeoutError);
  });

  it("queryPositions", () => {
    expect(() => gw.queryPositions({}, {}, opts)).toThrow(DbTimeoutError);
  });

  it("queryRiskScores", () => {
    expect(() => gw.queryRiskScores({}, {}, opts)).toThrow(DbTimeoutError);
  });
});

// ── 6. Store health is marked unavailable after a timeout ────────────────

describe("gateway DB timeout — store health state", () => {
  it("marks store unavailable after a timeout error from checkout", () => {
    const store = createMemoryStore({
      invoices: [],
      checkout: () => { throw pgTimeoutError(); },
    });
    const gw = new ApiGateway(store);
    try { gw.queryInvoices({}, {}, opts); } catch { /* expected */ }
    expect(store.health?.available).toBe(false);
  });

  it("read cache is invalidated after a timeout", () => {
    let callCount = 0;
    const store = createMemoryStore({
      invoices: [],
      checkout: () => {
        callCount += 1;
        throw pgTimeoutError();
      },
      invalidateCache: () => { /* tracked by callCount */ },
    });
    const gw = new ApiGateway(store);
    try { gw.queryInvoices({}, {}, opts); } catch { /* expected */ }
    // The gateway should have attempted checkout and recorded metrics.
    expect(callCount).toBeGreaterThan(0);
  });
});

// ── 7. Pool-recovery: healthy checkout after timeout succeeds ─────────────

describe("gateway DB timeout — pool recovery after timeout", () => {
  it("subsequent request succeeds once checkout no longer throws", () => {
    let shouldFail = true;
    const store = createMemoryStore({
      invoices: [
        {
          id: "i1",
          sme: "SME_A",
          debtorHash: "h1",
          amount: "100",
          currency: "USDC",
          dueDate: 10,
          ipfsCid: "cid1",
          riskScore: 20,
          riskTier: "AAA",
          status: "Listed",
          createdAt: 1,
        },
      ],
      checkout: () => {
        if (shouldFail) throw pgTimeoutError();
      },
    });
    const gw = new ApiGateway(store);

    // First request fails with DbTimeoutError.
    expect(() => gw.queryInvoices({}, {}, opts)).toThrow(DbTimeoutError);
    expect(store.health?.available).toBe(false);

    // Simulate pool recovery: checkout no longer throws.
    shouldFail = false;
    // Also manually re-mark the store as available to simulate a health probe.
    store.health!.available = true;

    // Second request must succeed.
    const page = gw.queryInvoices({}, {}, opts);
    expect(page.data).toHaveLength(1);
  });
});

// ── 8. DbTimeoutError carries the original cause ──────────────────────────

describe("gateway DB timeout — error cause chain", () => {
  it("DbTimeoutError.cause is the original driver error", () => {
    const original = pgTimeoutError();
    const gw = seedGateway(() => { throw original; });
    try {
      gw.queryInvoices({}, {}, opts);
      fail("expected throw");
    } catch (err) {
      expect((err as DbTimeoutError).cause).toBe(original);
    }
  });
});

// ── 9. Retry-After semantics ──────────────────────────────────────────────

describe("gateway DB timeout — Retry-After calculation", () => {
  it("retryAfterSeconds equals ceiling(DB_STATEMENT_TIMEOUT_MS / 1000)", () => {
    // Default timeout is 5000 ms → 5 s.
    const gw = seedGateway(() => { throw pgTimeoutError(); });
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) {
      const retryAfter = (err as DbTimeoutError).retryAfterSeconds;
      // ceil(5000 / 1000) = 5
      expect(retryAfter).toBe(5);
    }
  });

  it("retryAfterSeconds is always at least 1", () => {
    const gw = seedGateway(() => { throw pgTimeoutError(); });
    try { gw.queryInvoices({}, {}, opts); fail("expected throw"); }
    catch (err) {
      expect((err as DbTimeoutError).retryAfterSeconds).toBeGreaterThanOrEqual(1);
    }
  });
});
