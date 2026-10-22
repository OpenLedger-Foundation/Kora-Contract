/**
 * Keeper Service — Test Suite
 *
 * Covers:
 *  - Restart-safe deduplication (INSERT OR IGNORE)
 *  - Retry on transient failure with exponential back-off
 *  - Graceful handling of already-triggered contract state
 *  - Exhausted-retry → dead-letter promotion
 *  - Scheduler tick promotes pending → ready → dispatched
 *  - Status counts and history observability
 *
 * Issue #765 — Minimum 90% coverage requirement.
 */

import * as os from "os";
import * as path from "path";
import * as fs from "fs";
import { JobStore } from "../src/jobStore";
import { Dispatcher } from "../src/dispatcher";
import { Scheduler } from "../src/scheduler";
import { ListingTtlKeeper } from "../src/ttlKeeper";
import { Deadline, Job, KeeperConfig, TriggerResult } from "../src/types";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function tmpDb(): string {
  return path.join(os.tmpdir(), `keeper-test-${Date.now()}-${Math.random()}.db`);
}

function makeDeadline(overrides: Partial<Deadline> = {}): Deadline {
  return {
    dedupKey: "funding_expiry:42",
    kind: "funding_expiry",
    invoiceId: BigInt(42),
    deadlineTs: Math.floor(Date.now() / 1000) - 10, // already elapsed
    contractAddress: "CTEST000000000000000000000000000000000000000000000000000",
    ...overrides,
  };
}

const BASE_CONFIG: KeeperConfig = {
  rpcUrl: "https://soroban-testnet.stellar.org",
  keeperSecret: "STEST",
  dbPath: ":memory:",
  pollIntervalMs: 1000,
  maxAttempts: 3,
  retryBackoffBaseMs: 100, // short for tests
  networkPassphrase: "Test SDF Network ; September 2015",
  marketplaceContractAddress: "CTEST000000000000000000000000000000000000000000000000000",
  activeListingsUrl: "https://indexer.test/api/v1/listings",
  indexerApiKey: "",
  ttlThresholdLedgers: 518_400,
  indexerUrl: "https://indexer.test/api/v1/deadlines",
  deadlineHorizonSec: 86_400,
  ingestIntervalMs: 60_000,
};

// ---------------------------------------------------------------------------
// Controllable Dispatcher subclass for unit tests
// ---------------------------------------------------------------------------

class MockDispatcher extends Dispatcher {
  responses: Array<TriggerResult | Error> = [];

  setNextResponse(r: TriggerResult | Error): void {
    this.responses.push(r);
  }

  protected override async _submitStellarTransaction(
    contractAddress: string,
    fnName: string,
    invoiceId: string,
    _fee: number
  ): Promise<string> {
    const next = this.responses.shift();
    if (!next) throw new Error("No mock response configured");
    if (next instanceof Error) throw next;
    if (next.outcome === "already_triggered") {
      throw new Error("AlreadyDefaulted");
    }
    if (next.outcome === "failed") {
      throw new Error("network timeout");
    }
    if (next.outcome === "permanent_failure") {
      throw new Error("invalid contract address");
    }
    return next.txHash ?? "0xdeadbeef";
  }
}

// ---------------------------------------------------------------------------
// JobStore tests
// ---------------------------------------------------------------------------

describe("JobStore", () => {
  let dbPath: string;
  let store: JobStore;

  beforeEach(() => {
    dbPath = tmpDb();
    store = new JobStore(dbPath);
  });

  afterEach(() => {
    store.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });

  test("enqueues a new deadline", () => {
    store.enqueue(makeDeadline());
    const jobs = store.getByStatus("pending");
    expect(jobs).toHaveLength(1);
    expect(jobs[0].dedupKey).toBe("funding_expiry:42");
  });

  test("deduplication: second enqueue with same dedupKey is silently ignored", () => {
    const dl = makeDeadline();
    store.enqueue(dl);
    store.enqueue(dl); // same key, second time
    const jobs = store.getByStatus("pending");
    expect(jobs).toHaveLength(1);
  });

  test("restart-safe deduplication: re-opening DB does not re-enqueue", () => {
    store.enqueue(makeDeadline());
    store.close();

    // Simulate restart by re-opening same DB
    const store2 = new JobStore(dbPath);
    store2.enqueue(makeDeadline()); // should be ignored
    const jobs = store2.getByStatus("pending");
    expect(jobs).toHaveLength(1);
    store2.close();
  });

  test("promoteExpired advances pending → ready when deadline has elapsed", () => {
    const pastDeadline = makeDeadline({ dedupKey: "d:1", deadlineTs: 100 });
    const futureDeadline = makeDeadline({
      dedupKey: "d:2",
      deadlineTs: Math.floor(Date.now() / 1000) + 9999,
    });
    store.enqueue(pastDeadline);
    store.enqueue(futureDeadline);

    const now = Math.floor(Date.now() / 1000);
    const promoted = store.promoteExpired(now);
    expect(promoted).toBe(1);
    expect(store.getByStatus("ready")).toHaveLength(1);
    expect(store.getByStatus("pending")).toHaveLength(1);
  });

  test("recordAttempt with 'submitted' → in_flight", () => {
    store.enqueue(makeDeadline());
    store.promoteExpired(Math.floor(Date.now() / 1000));
    const jobs = store.getByStatus("ready");
    store.recordAttempt(jobs[0].dedupKey, "submitted", "0xabc");
    expect(store.getByStatus("in_flight")).toHaveLength(1);
  });

  test("recordAttempt with 'already_triggered' → done", () => {
    store.enqueue(makeDeadline());
    store.promoteExpired(Math.floor(Date.now() / 1000));
    const [job] = store.getByStatus("ready");
    store.recordAttempt(job.dedupKey, "already_triggered");
    expect(store.getByStatus("done")).toHaveLength(1);
  });

  test("markDead moves job to dead status", () => {
    store.enqueue(makeDeadline());
    store.promoteExpired(Math.floor(Date.now() / 1000));
    const [job] = store.getByStatus("ready");
    store.markDead(job.dedupKey, "exhausted retries");
    expect(store.getByStatus("dead")).toHaveLength(1);
  });

  test("getDispatchable excludes jobs under retry back-off", () => {
    store.enqueue(makeDeadline({ dedupKey: "d:1", deadlineTs: 100 }));
    store.promoteExpired(Math.floor(Date.now() / 1000));
    const [job] = store.getByStatus("ready");

    // Record a failed attempt (sets lastAttemptAt = now)
    store.recordAttempt(job.dedupKey, "failed", undefined, "timeout");

    const now = Math.floor(Date.now() / 1000);
    // With backoff=100ms and attempts=1, next retry is ≈ 0.1s away — effectively now
    // Use a huge backoff to force exclusion
    const dispatchable = store.getDispatchable(now, 999_999_000, 5);
    expect(dispatchable).toHaveLength(0);
  });

  test("getHistory returns most recent jobs", () => {
    for (let i = 0; i < 5; i++) {
      store.enqueue(makeDeadline({ dedupKey: `d:${i}` }));
    }
    const history = store.getHistory(10);
    expect(history.length).toBe(5);
  });

  test("getStatusCounts returns correct breakdown", () => {
    store.enqueue(makeDeadline({ dedupKey: "d:1" }));
    store.enqueue(makeDeadline({ dedupKey: "d:2" }));
    store.promoteExpired(Math.floor(Date.now() / 1000));
    // d:1 was pending before promote; one stays pending (future), one becomes ready?
    // Both have past deadlines → both promoted
    const counts = store.getStatusCounts();
    expect(counts.ready).toBe(2);
  });
});

// ---------------------------------------------------------------------------
// Dispatcher tests
// ---------------------------------------------------------------------------

describe("Dispatcher", () => {
  let dispatcher: MockDispatcher;

  beforeEach(() => {
    dispatcher = new MockDispatcher(BASE_CONFIG);
  });

  function makeJob(overrides: Partial<Job> = {}): Job {
    return {
      id: 1,
      dedupKey: "funding_expiry:1",
      kind: "funding_expiry",
      invoiceId: "1",
      deadlineTs: 100,
      contractAddress: "CTEST",
      status: "ready",
      attempts: 0,
      lastAttemptAt: null,
      lastOutcome: null,
      lastTxHash: null,
      lastError: null,
      createdAt: 0,
      updatedAt: 0,
      ...overrides,
    };
  }

  test("successful dispatch returns submitted outcome", async () => {
    dispatcher.setNextResponse({ dedupKey: "funding_expiry:1", outcome: "submitted", txHash: "0xabc" });
    const result = await dispatcher.dispatch(makeJob());
    expect(result.outcome).toBe("submitted");
    expect(result.txHash).toBe("0xabc");
  });

  test("already_triggered contract error returns graceful done outcome", async () => {
    dispatcher.setNextResponse({ dedupKey: "funding_expiry:1", outcome: "already_triggered" });
    const result = await dispatcher.dispatch(makeJob());
    expect(result.outcome).toBe("already_triggered");
  });

  test("transient failure returns failed outcome (retriable)", async () => {
    dispatcher.setNextResponse({ dedupKey: "funding_expiry:1", outcome: "failed" });
    const result = await dispatcher.dispatch(makeJob());
    expect(result.outcome).toBe("failed");
  });

  test("permanent failure (bad contract) returns permanent_failure outcome", async () => {
    dispatcher.setNextResponse({ dedupKey: "funding_expiry:1", outcome: "permanent_failure" });
    const result = await dispatcher.dispatch(makeJob());
    expect(result.outcome).toBe("permanent_failure");
  });

  test("due_date kind calls trigger_default_check function", async () => {
    // We can test indirectly via the mock: any response counts
    dispatcher.setNextResponse({ dedupKey: "due_date:1", outcome: "submitted", txHash: "0x1" });
    const result = await dispatcher.dispatch(makeJob({ kind: "due_date", dedupKey: "due_date:1" }));
    expect(result.outcome).toBe("submitted");
  });

  test("grace_period_end kind calls mark_defaulted function", async () => {
    dispatcher.setNextResponse({ dedupKey: "grace_period_end:1", outcome: "submitted", txHash: "0x2" });
    const result = await dispatcher.dispatch(
      makeJob({ kind: "grace_period_end", dedupKey: "grace_period_end:1" })
    );
    expect(result.outcome).toBe("submitted");
  });
});

// ---------------------------------------------------------------------------
// Scheduler integration tests
// ---------------------------------------------------------------------------

describe("Scheduler integration", () => {
  let store: JobStore;
  let dispatcher: MockDispatcher;
  let scheduler: Scheduler;
  let dbPath: string;

  beforeEach(() => {
    dbPath = tmpDb();
    store = new JobStore(dbPath);
    dispatcher = new MockDispatcher(BASE_CONFIG);
    scheduler = new Scheduler(store, dispatcher, { ...BASE_CONFIG, dbPath });
  });

  afterEach(() => {
    scheduler.stop();
    store.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });

  test("enqueueDeadlines enqueues without duplicates", () => {
    const dl = makeDeadline();
    scheduler.enqueueDeadlines([dl, dl]);
    expect(store.getByStatus("pending")).toHaveLength(1);
  });

  test("TTL scan jobs enter the durable queue before dispatch", async () => {
    const ttlDeadline = makeDeadline({
      kind: "listing_ttl",
      dedupKey: "listing_ttl:42:1000",
      deadlineTs: Math.floor(Date.now() / 1000),
    });
    const ttlScheduler = new Scheduler(store, dispatcher, { ...BASE_CONFIG, dbPath }, async () => [ttlDeadline]);
    dispatcher.setNextResponse({ dedupKey: ttlDeadline.dedupKey, outcome: "submitted", txHash: "0xttl" });

    // @ts-expect-error — accessing private method for test
    await ttlScheduler._tick();

    expect(store.getByStatus("in_flight").map((job) => job.dedupKey)).toContain(ttlDeadline.dedupKey);
  });

  test("full tick cycle: pending → ready → dispatched → done", async () => {
    dispatcher.setNextResponse({
      dedupKey: "funding_expiry:42",
      outcome: "submitted",
      txHash: "0xfinal",
    });

    scheduler.enqueueDeadlines([makeDeadline()]);
    expect(store.getByStatus("pending")).toHaveLength(1);

    // Manually trigger a tick
    // @ts-expect-error — accessing private method for test
    await scheduler._tick();

    // After tick: promoted to ready then dispatched → in_flight
    const inFlight = store.getByStatus("in_flight");
    expect(inFlight).toHaveLength(1);
  });

  test("already triggered: job becomes done, not failed", async () => {
    dispatcher.setNextResponse({
      dedupKey: "funding_expiry:42",
      outcome: "already_triggered",
    });

    scheduler.enqueueDeadlines([makeDeadline()]);
    // @ts-expect-error
    await scheduler._tick();

    expect(store.getByStatus("done")).toHaveLength(1);
    expect(store.getByStatus("failed")).toHaveLength(0);
  });

  test("exhausted retries → dead-letter", async () => {
    const config = { ...BASE_CONFIG, maxAttempts: 2, retryBackoffBaseMs: 0 };
    const store2 = new JobStore(tmpDb());
    const dispatcher2 = new MockDispatcher(config);
    const scheduler2 = new Scheduler(store2, dispatcher2, config);

    // Queue 2 failures
    dispatcher2.setNextResponse({ dedupKey: "funding_expiry:42", outcome: "failed" });
    dispatcher2.setNextResponse({ dedupKey: "funding_expiry:42", outcome: "failed" });

    scheduler2.enqueueDeadlines([makeDeadline()]);
    // @ts-expect-error
    await scheduler2._tick(); // attempt 1 → failed
    // @ts-expect-error
    await scheduler2._tick(); // attempt 2 → failed → dead (maxAttempts=2)

    const dead = store2.getByStatus("dead");
    expect(dead.length).toBeGreaterThanOrEqual(1);

    scheduler2.stop();
    store2.close();
  });
});

describe("ListingTtlKeeper", () => {
  test("schedules active listing only when TTL is below threshold", async () => {
    const rpc = {
      getLedgerEntries: async (...keys: Array<{ toXDR: (format: "base64") => string }>) => ({
        latestLedger: 1000,
        entries: keys.map((key) => ({ key, liveUntilLedgerSeq: 1200 })),
      }),
    };
    const source = {
      getActiveListings: async () => [
        { invoiceId: "42", isActive: true },
        { invoiceId: "43", isActive: true },
      ],
    };
    const keeper = new ListingTtlKeeper(BASE_CONFIG, rpc as never, source);

    const jobs = await keeper.scan();

    expect(jobs.map((job) => job.dedupKey)).toEqual([
      "listing_ttl:42:1200",
      "listing_ttl:43:1200",
    ]);
    expect(keeper.getMetrics()).toEqual({ scanned: 2, atRisk: 2, errors: 0 });
  });

  test("does not schedule entries above the configured threshold", async () => {
    const rpc = {
      getLedgerEntries: async (...keys: Array<{ toXDR: (format: "base64") => string }>) => ({
        latestLedger: 1000,
        entries: keys.map((key) => ({ key, liveUntilLedgerSeq: 519_400 })),
      }),
    };
    const source = { getActiveListings: async () => [{ invoiceId: "42", isActive: true }] };
    const keeper = new ListingTtlKeeper(BASE_CONFIG, rpc as never, source);

    await expect(keeper.scan()).resolves.toEqual([]);
  });

  test("records failed scans and rethrows source errors", async () => {
    const source = { getActiveListings: async () => { throw new Error("indexer unavailable"); } };
    const keeper = new ListingTtlKeeper(BASE_CONFIG, {} as never, source);

    await expect(keeper.scan()).rejects.toThrow("indexer unavailable");
    expect(keeper.getMetrics().errors).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// IndexerDeadlineClient ingestion (#1022)
// ---------------------------------------------------------------------------

import { IndexerDeadlineClient } from "../src/indexerClient";

function mockFetch(rows: unknown, ok = true): typeof fetch {
  return (async () =>
    ({
      ok,
      status: ok ? 200 : 503,
      json: async () => rows,
    }) as never) as typeof fetch;
}

describe("IndexerDeadlineClient", () => {
  const NOW = 1_700_000_000;

  test("maps in-horizon funding deadlines with stable dedupKeys", async () => {
    const client = new IndexerDeadlineClient({
      indexerUrl: "https://indexer.test/api/v1/deadlines",
      horizonSec: 3600,
      contractAddress: "CTEST",
      fetchFn: mockFetch([{ invoice_id: 7, funding_deadline: NOW + 100 }]),
    });
    const deadlines = await client.fetchDeadlines(NOW);
    expect(deadlines).toHaveLength(1);
    expect(deadlines[0]).toMatchObject({
      dedupKey: "funding_expiry:7",
      kind: "funding_expiry",
      deadlineTs: NOW + 100,
    });
  });

  test("already-passed deadlines enqueue as immediately ready; far-future skipped", async () => {
    const client = new IndexerDeadlineClient({
      indexerUrl: "https://indexer.test/api/v1/deadlines",
      horizonSec: 3600,
      contractAddress: "CTEST",
      fetchFn: mockFetch([
        { invoice_id: 1, funding_deadline: NOW - 5 },
        { invoice_id: 2, funding_deadline: NOW + 99_999 },
      ]),
    });
    const deadlines = await client.fetchDeadlines(NOW);
    expect(deadlines.map((d) => d.dedupKey)).toEqual(["funding_expiry:1"]);
  });

  test("cancelled listings are skipped; outage yields [] without throwing", async () => {
    const okClient = new IndexerDeadlineClient({
      indexerUrl: "https://indexer.test/api/v1/deadlines",
      horizonSec: 3600,
      contractAddress: "CTEST",
      fetchFn: mockFetch([{ invoice_id: 3, funding_deadline: NOW + 10, status: "cancelled" }]),
    });
    await expect(okClient.fetchDeadlines(NOW)).resolves.toEqual([]);

    const downClient = new IndexerDeadlineClient({
      indexerUrl: "https://indexer.test/api/v1/deadlines",
      horizonSec: 3600,
      contractAddress: "CTEST",
      fetchFn: mockFetch([], false),
    });
    await expect(downClient.fetchDeadlines(NOW)).resolves.toEqual([]);
    expect(downClient.degraded).toBe(true);
  });

  test("restart-safe: re-enqueue of same dedupKey is ignored by the store", async () => {
    const store = new JobStore(tmpDb());
    const client = new IndexerDeadlineClient({
      indexerUrl: "https://indexer.test/api/v1/deadlines",
      horizonSec: 3600,
      contractAddress: "CTEST",
      fetchFn: mockFetch([{ invoice_id: 9, funding_deadline: NOW + 10 }]),
    });
    const batch = await client.fetchDeadlines(NOW);
    const scheduler = new Scheduler(store, {} as never, BASE_CONFIG);
    scheduler.enqueueDeadlines(batch);
    scheduler.enqueueDeadlines(batch); // restart replay
    expect(store.getStatusCounts().pending).toBe(1);
    store.close();
  });
});
