/**
 * Keeper — Lease Fencing Tests
 *
 * Closes #<issue_number>
 *
 * Covers:
 *  - Only the lease holder runs dispatch; non-holders skip the tick
 *  - Two concurrent workers: exactly one dispatches, the other skips
 *  - Expired lease is reassigned to the next requester
 *  - releaseLease allows immediate takeover without waiting for TTL
 *  - renewLease extends the lease; returns false if not held
 *  - Clock skew tolerance: lease considered expired within tolerance window
 *  - Scheduler integration: lease fencing suppresses dispatch in non-holder
 *  - getLease observability: returns current holder or null
 *  - Explicit shutdown releases the lease
 */

import * as os from "os";
import * as path from "path";
import * as fs from "fs";
import { LeaseStore, ILeaseStore } from "../src/leaseStore";
import { Scheduler, DISPATCH_LEASE_NAME } from "../src/scheduler";
import { JobStore } from "../src/jobStore";
import { Dispatcher } from "../src/dispatcher";
import { Deadline, Job, KeeperConfig, TriggerResult } from "../src/types";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function tmpDb(): string {
  return path.join(os.tmpdir(), `lease-test-${Date.now()}-${Math.random()}.db`);
}

function makeDeadline(overrides: Partial<Deadline> = {}): Deadline {
  return {
    dedupKey: "funding_expiry:99",
    kind: "funding_expiry",
    invoiceId: BigInt(99),
    deadlineTs: Math.floor(Date.now() / 1000) - 10,
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
  retryBackoffBaseMs: 0,
  networkPassphrase: "Test SDF Network ; September 2015",
  marketplaceContractAddress: "CTEST000000000000000000000000000000000000000000000000000",
  activeListingsUrl: "https://indexer.test/api/v1/listings",
  indexerApiKey: "",
  ttlThresholdLedgers: 518_400,
  indexerUrl: "https://indexer.test/api/v1/deadlines",
  deadlineHorizonSec: 86_400,
  ingestIntervalMs: 60_000,
};

class MockDispatcher extends Dispatcher {
  dispatchCalls = 0;
  responses: Array<TriggerResult | Error> = [];

  setNextResponse(r: TriggerResult | Error): void {
    this.responses.push(r);
  }

  protected override async _submitStellarTransaction(): Promise<string> {
    const next = this.responses.shift();
    if (!next) throw new Error("No mock response configured");
    if (next instanceof Error) throw next;
    if (next.outcome === "already_triggered") throw new Error("AlreadyDefaulted");
    if (next.outcome === "failed") throw new Error("network timeout");
    if (next.outcome === "permanent_failure") throw new Error("invalid contract address");
    return next.txHash ?? "0xdeadbeef";
  }

  override async dispatch(job: Job): Promise<TriggerResult> {
    this.dispatchCalls += 1;
    return super.dispatch(job);
  }
}

// ---------------------------------------------------------------------------
// LeaseStore unit tests
// ---------------------------------------------------------------------------

describe("LeaseStore: basic acquire/renew/release", () => {
  let dbPath: string;
  let store: LeaseStore;

  beforeEach(() => {
    dbPath = tmpDb();
    store = new LeaseStore(dbPath);
  });

  afterEach(() => {
    store.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });

  test("first acquirer gets the lease", () => {
    const ok = store.acquireLease("test-lease", "worker-A", 60);
    expect(ok).toBe(true);
  });

  test("same holder can re-acquire (renew) its own lease", () => {
    store.acquireLease("test-lease", "worker-A", 60);
    const renewed = store.acquireLease("test-lease", "worker-A", 60);
    expect(renewed).toBe(true);
  });

  test("second worker cannot acquire a live lease", () => {
    store.acquireLease("test-lease", "worker-A", 60);
    const ok = store.acquireLease("test-lease", "worker-B", 60);
    expect(ok).toBe(false);
  });

  test("getLease returns the current holder", () => {
    store.acquireLease("test-lease", "worker-A", 60);
    const lease = store.getLease("test-lease");
    expect(lease?.holderId).toBe("worker-A");
    expect(lease?.leaseName).toBe("test-lease");
  });

  test("getLease returns null when no lease exists", () => {
    expect(store.getLease("nonexistent")).toBeNull();
  });

  test("releaseLease clears the lease so another worker can acquire", () => {
    store.acquireLease("test-lease", "worker-A", 60);
    store.releaseLease("test-lease", "worker-A");
    const ok = store.acquireLease("test-lease", "worker-B", 60);
    expect(ok).toBe(true);
    expect(store.getLease("test-lease")?.holderId).toBe("worker-B");
  });

  test("releaseLease by non-holder is a no-op (does not evict holder)", () => {
    store.acquireLease("test-lease", "worker-A", 60);
    store.releaseLease("test-lease", "worker-B"); // wrong holder
    expect(store.getLease("test-lease")?.holderId).toBe("worker-A");
  });

  test("renewLease by holder extends expiry and returns true", () => {
    store.acquireLease("test-lease", "worker-A", 10);
    const before = store.getLease("test-lease")!.expiresAt;
    const ok = store.renewLease("test-lease", "worker-A", 120);
    expect(ok).toBe(true);
    const after = store.getLease("test-lease")!.expiresAt;
    expect(after).toBeGreaterThan(before);
  });

  test("renewLease by non-holder returns false", () => {
    store.acquireLease("test-lease", "worker-A", 60);
    const ok = store.renewLease("test-lease", "worker-B", 60);
    expect(ok).toBe(false);
    // A's lease is unchanged
    expect(store.getLease("test-lease")?.holderId).toBe("worker-A");
  });
});

// ---------------------------------------------------------------------------
// Expired lease reassignment
// ---------------------------------------------------------------------------

describe("LeaseStore: expired lease reassignment", () => {
  let dbPath: string;
  let store: LeaseStore;

  beforeEach(() => {
    dbPath = tmpDb();
    store = new LeaseStore(dbPath);
  });

  afterEach(() => {
    store.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });

  test("worker B acquires lease after worker A's lease expires", () => {
    // TTL = 1 second
    store.acquireLease("test-lease", "worker-A", 1);

    // Manually backdate the expiry to simulate expiry
    const db = (store as unknown as { db: { exec: (s: string) => void } }).db;
    db.exec(`UPDATE keeper_leases SET expires_at = 0, renewed_at = 0 WHERE lease_name = 'test-lease'`);

    const ok = store.acquireLease("test-lease", "worker-B", 60);
    expect(ok).toBe(true);
    expect(store.getLease("test-lease")?.holderId).toBe("worker-B");
  });

  test("clock skew tolerance: lease with expires_at = now+1 is treated as expired with skew=2", () => {
    store.acquireLease("test-lease", "worker-A", 60);

    // Set expires_at to now + 1 (within the 2-second clock skew tolerance)
    const nearFutureExpiry = Math.floor(Date.now() / 1000) + 1;
    const db = (store as unknown as { db: { exec: (s: string) => void } }).db;
    db.exec(`UPDATE keeper_leases SET expires_at = ${nearFutureExpiry} WHERE lease_name = 'test-lease'`);

    // With clockSkewTolS=2, this lease (expires in 1s) is within the tolerance window
    const ok = store.acquireLease("test-lease", "worker-B", 60, 2);
    expect(ok).toBe(true);
  });

  test("without clock skew tolerance, nearly-expired lease is not taken", () => {
    store.acquireLease("test-lease", "worker-A", 60);

    // Set expires_at to now + 10 (comfortably in the future)
    const futureExpiry = Math.floor(Date.now() / 1000) + 10;
    const db = (store as unknown as { db: { exec: (s: string) => void } }).db;
    db.exec(`UPDATE keeper_leases SET expires_at = ${futureExpiry} WHERE lease_name = 'test-lease'`);

    // clockSkewTolS=0 — no tolerance
    const ok = store.acquireLease("test-lease", "worker-B", 60, 0);
    expect(ok).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Two-worker simulation: only the lease holder dispatches
// ---------------------------------------------------------------------------

describe("LeaseStore: two-worker fencing — only lease holder dispatches", () => {
  let dbPath: string;
  let leasePath: string;
  let leaseStore: LeaseStore;

  beforeEach(() => {
    dbPath = tmpDb();
    leasePath = tmpDb();
    leaseStore = new LeaseStore(leasePath);
  });

  afterEach(() => {
    leaseStore.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
    if (fs.existsSync(leasePath)) fs.unlinkSync(leasePath);
  });

  test("worker A holds lease, worker B skips tick — only A dispatches", async () => {
    const jobStore = new JobStore(dbPath);
    jobStore.enqueue(makeDeadline());

    const dispatcherA = new MockDispatcher(BASE_CONFIG);
    const dispatcherB = new MockDispatcher(BASE_CONFIG);

    dispatcherA.setNextResponse({ dedupKey: "funding_expiry:99", outcome: "submitted", txHash: "0xA" });

    // Worker A acquires the lease before ticking
    const schedulerA = new Scheduler(jobStore, dispatcherA, BASE_CONFIG, undefined, leaseStore, "worker-A");
    // Worker B uses the same leaseStore (simulates shared backing store)
    const schedulerB = new Scheduler(jobStore, dispatcherB, BASE_CONFIG, undefined, leaseStore, "worker-B");

    // A acquires the lease first
    const aAcquired = leaseStore.acquireLease(DISPATCH_LEASE_NAME, "worker-A", 60);
    expect(aAcquired).toBe(true);

    // Tick both schedulers
    // @ts-expect-error — private method
    await schedulerA._tick();
    // @ts-expect-error — private method
    await schedulerB._tick();

    // Only A should have dispatched
    expect(dispatcherA.dispatchCalls).toBe(1);
    expect(dispatcherB.dispatchCalls).toBe(0);

    schedulerA.stop();
    schedulerB.stop();
    jobStore.close();
  });

  test("after worker A crashes (lease expires), worker B takes over", async () => {
    const jobStore = new JobStore(dbPath);
    jobStore.enqueue(makeDeadline());
    jobStore.enqueue(makeDeadline({ dedupKey: "funding_expiry:100", invoiceId: BigInt(100) }));

    const dispatcherB = new MockDispatcher(BASE_CONFIG);
    dispatcherB.setNextResponse({ dedupKey: "funding_expiry:99", outcome: "submitted", txHash: "0xB1" });
    dispatcherB.setNextResponse({ dedupKey: "funding_expiry:100", outcome: "submitted", txHash: "0xB2" });

    // Worker A acquires the lease
    const acquired = leaseStore.acquireLease(DISPATCH_LEASE_NAME, "worker-A", 1);
    expect(acquired).toBe(true);

    // Simulate A crashing: backdate the expiry
    const db = (leaseStore as unknown as { db: { exec: (s: string) => void } }).db;
    db.exec(`UPDATE keeper_leases SET expires_at = 0 WHERE lease_name = '${DISPATCH_LEASE_NAME}'`);

    // Worker B ticks and should acquire the now-expired lease
    const schedulerB = new Scheduler(jobStore, dispatcherB, BASE_CONFIG, undefined, leaseStore, "worker-B");
    // @ts-expect-error — private method
    await schedulerB._tick();

    // B should have dispatched (took over the expired lease)
    expect(dispatcherB.dispatchCalls).toBeGreaterThan(0);
    expect(leaseStore.getLease(DISPATCH_LEASE_NAME)?.holderId).toBe("worker-B");

    schedulerB.stop();
    jobStore.close();
  });

  test("scheduler.stop() releases the lease", () => {
    const jobStore = new JobStore(dbPath);
    const dispatcher = new MockDispatcher(BASE_CONFIG);
    const scheduler = new Scheduler(jobStore, dispatcher, BASE_CONFIG, undefined, leaseStore, "worker-A");

    leaseStore.acquireLease(DISPATCH_LEASE_NAME, "worker-A", 60);
    expect(leaseStore.getLease(DISPATCH_LEASE_NAME)?.holderId).toBe("worker-A");

    scheduler.stop();

    // After stop, lease should be released — another worker can take it
    const ok = leaseStore.acquireLease(DISPATCH_LEASE_NAME, "worker-B", 60);
    expect(ok).toBe(true);

    jobStore.close();
  });
});

// ---------------------------------------------------------------------------
// No-lease mode: existing tests still pass without a leaseStore
// ---------------------------------------------------------------------------

describe("Scheduler without lease store (backward compat)", () => {
  let dbPath: string;
  let jobStore: JobStore;
  let dispatcher: MockDispatcher;
  let scheduler: Scheduler;

  beforeEach(() => {
    dbPath = tmpDb();
    jobStore = new JobStore(dbPath);
    dispatcher = new MockDispatcher(BASE_CONFIG);
    // No leaseStore — previous behaviour preserved
    scheduler = new Scheduler(jobStore, dispatcher, BASE_CONFIG);
  });

  afterEach(() => {
    scheduler.stop();
    jobStore.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });

  test("dispatches normally when no lease store is configured", async () => {
    jobStore.enqueue(makeDeadline());
    dispatcher.setNextResponse({ dedupKey: "funding_expiry:99", outcome: "submitted", txHash: "0x1" });

    // @ts-expect-error — private method
    await scheduler._tick();

    expect(dispatcher.dispatchCalls).toBe(1);
    expect(jobStore.getByStatus("in_flight")).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// ILeaseStore interface: in-memory test double
// ---------------------------------------------------------------------------

describe("ILeaseStore: in-memory test double wires into Scheduler", () => {
  test("mock lease store can be injected into Scheduler", async () => {
    let held = false;
    const mockLease: ILeaseStore = {
      acquireLease: (_name, _id, _ttl) => { held = !held; return held; },
      renewLease: () => true,
      releaseLease: () => { held = false; },
      getLease: () => null,
      close: () => { /* no-op */ },
    };

    const dbPath = tmpDb();
    const jobStore = new JobStore(dbPath);
    jobStore.enqueue(makeDeadline());

    const dispatcher = new MockDispatcher(BASE_CONFIG);
    dispatcher.setNextResponse({ dedupKey: "funding_expiry:99", outcome: "submitted", txHash: "0x1" });

    const scheduler = new Scheduler(jobStore, dispatcher, BASE_CONFIG, undefined, mockLease, "mock-worker");

    // First tick: acquireLease returns true (held flipped to true)
    // @ts-expect-error — private method
    await scheduler._tick();
    expect(dispatcher.dispatchCalls).toBe(1);

    scheduler.stop();
    jobStore.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });
});
