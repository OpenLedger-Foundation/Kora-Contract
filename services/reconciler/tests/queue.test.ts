/**
 * Unit tests for PersistentAlertQueue.
 *
 * Uses an in-memory SQLite database (`:memory:`) so tests are fast and
 * leave no files on disk.
 */

import { PersistentAlertQueue } from "../src/queue";

function makeQueue(): PersistentAlertQueue {
  return new PersistentAlertQueue(":memory:");
}

describe("PersistentAlertQueue — enqueueGap", () => {
  it("inserts a new alert and returns true", () => {
    const q = makeQueue();
    const inserted = q.enqueueGap(100, 100, 110);
    expect(inserted).toBe(true);
    q.close();
  });

  it("returns false on duplicate ledger (dedup suppressed)", () => {
    const q = makeQueue();
    q.enqueueGap(100, 100, 110);
    const second = q.enqueueGap(100, 100, 115);
    expect(second).toBe(false);
    expect(q.depth()).toBe(1);
    q.close();
  });

  it("enqueues separate rows for distinct ledgers", () => {
    const q = makeQueue();
    q.enqueueGap(100, 100, 110);
    q.enqueueGap(101, 100, 110);
    q.enqueueGap(102, 100, 110);
    expect(q.depth()).toBe(3);
    q.close();
  });

  it("stores correct ledger / fromLedger / toLedger values", () => {
    const q = makeQueue();
    q.enqueueGap(200, 198, 250);
    const rows = q.allPending();
    expect(rows).toHaveLength(1);
    expect(rows[0]!.ledger).toBe(200);
    expect(rows[0]!.fromLedger).toBe(198);
    expect(rows[0]!.toLedger).toBe(250);
    expect(rows[0]!.type).toBe("ledger_gap");
    expect(rows[0]!.severity).toBe("critical");
    q.close();
  });
});

describe("PersistentAlertQueue — pending / delivery cycle", () => {
  it("returns due alerts in insertion order", () => {
    const now = Date.now();
    const q = makeQueue();
    q.enqueueGap(1, 1, 5, now - 1_000); // past → due
    q.enqueueGap(2, 1, 5, now - 500);   // past → due
    q.enqueueGap(3, 1, 5, now + 60_000); // future → not due
    const due = q.pending(10);
    expect(due.map((a) => a.ledger)).toEqual([1, 2]);
    q.close();
  });

  it("removes an alert after acknowledge", () => {
    const q = makeQueue();
    q.enqueueGap(10, 10, 20);
    const [alert] = q.pending();
    expect(alert).toBeDefined();
    q.acknowledge(alert!.id);
    expect(q.depth()).toBe(0);
    q.close();
  });

  it("reschedules with backoff on delivery failure", () => {
    const now = Date.now();
    const q = makeQueue();
    q.enqueueGap(50, 50, 60, now - 1_000);
    const [alert] = q.pending();
    expect(alert).toBeDefined();
    q.markDeliveryFailure(alert!.id, "connection refused");

    // After marking failure, the alert should not appear in pending() yet
    // because the backoff has pushed next_attempt_at into the future.
    const dueNow = q.pending();
    expect(dueNow.map((a) => a.id)).not.toContain(alert!.id);
    expect(q.depth()).toBe(1); // still in queue
    q.close();
  });

  it("caps backoff at 300 000 ms regardless of attempt count", () => {
    const now = Date.now();
    const q = makeQueue();
    q.enqueueGap(99, 99, 110, now - 1_000);
    const [alert] = q.pending();
    // Simulate 10 prior failures by calling markDeliveryFailure repeatedly.
    for (let i = 0; i < 10; i++) {
      // Reset next_attempt_at to past so alert stays retrievable between failures.
      (q as unknown as { db: { prepare: (s: string) => { run: (...a: unknown[]) => void } } })
        .db.prepare("UPDATE pending_alerts SET next_attempt_at = ? WHERE id = ?")
        .run(Date.now() - 1, alert!.id);
      q.markDeliveryFailure(alert!.id, "timeout");
    }
    const rows = q.allPending();
    const nextAttempt = rows[0]!.nextAttemptAt;
    // Backoff must not exceed 300 s from now.
    expect(nextAttempt - Date.now()).toBeLessThanOrEqual(300_500);
    q.close();
  });
});

describe("PersistentAlertQueue — depth / clear / oldestCreatedAt", () => {
  it("depth returns zero on empty queue", () => {
    const q = makeQueue();
    expect(q.depth()).toBe(0);
    q.close();
  });

  it("clear removes all rows and returns the count", () => {
    const q = makeQueue();
    q.enqueueGap(1, 1, 5);
    q.enqueueGap(2, 1, 5);
    q.enqueueGap(3, 1, 5);
    const removed = q.clear();
    expect(removed).toBe(3);
    expect(q.depth()).toBe(0);
    q.close();
  });

  it("oldestCreatedAt returns null on empty queue", () => {
    const q = makeQueue();
    expect(q.oldestCreatedAt()).toBeNull();
    q.close();
  });

  it("oldestCreatedAt returns the minimum created_at across all rows", () => {
    const now = Date.now();
    const q = makeQueue();
    q.enqueueGap(1, 1, 5, now - 10_000);
    q.enqueueGap(2, 1, 5, now - 5_000);
    q.enqueueGap(3, 1, 5, now - 1_000);
    const oldest = q.oldestCreatedAt();
    expect(oldest).not.toBeNull();
    // Allow ±50 ms for test execution timing.
    expect(Math.abs(oldest! - (now - 10_000))).toBeLessThan(50);
    q.close();
  });
});

describe("PersistentAlertQueue — allPending", () => {
  it("returns all rows including those not yet due", () => {
    const now = Date.now();
    const q = makeQueue();
    q.enqueueGap(1, 1, 5, now - 1_000);    // due
    q.enqueueGap(2, 1, 5, now + 60_000);   // not yet due
    expect(q.allPending()).toHaveLength(2);
    expect(q.pending()).toHaveLength(1); // only due
    q.close();
  });
});
