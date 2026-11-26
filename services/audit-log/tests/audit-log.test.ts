/**
 * Audit Log Service — Test Suite
 *
 * Covers:
 *  - Append and read back audit records
 *  - Hash chain integrity: prevHash/recordHash linkage
 *  - Tamper detection: verifier flags corrupted records
 *  - Concurrent writer ordering: no broken chain under parallel appends
 *  - PII protection: actorRef is hashed, never stored raw
 *  - AuditClient.hashPii utility
 *  - verifyChain on empty log
 *  - Service/range filtering
 *
 * Issue #768 — Minimum 90% coverage.
 */

import * as os from "os";
import * as path from "path";
import * as fs from "fs";
import * as crypto from "crypto";
import { AuditStore } from "../src/store";
import { AuditClient } from "../src/client";
import { verifyChain } from "../src/verify";
import { GENESIS_HASH, computeRecordHash, AuditRecord } from "../src/types";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function tmpDb(): string {
  return path.join(os.tmpdir(), `audit-test-${Date.now()}-${Math.random()}.db`);
}

function makeHash(s: string): string {
  return crypto.createHash("sha256").update(s, "utf8").digest("hex");
}

// ---------------------------------------------------------------------------
// AuditStore tests
// ---------------------------------------------------------------------------

describe("AuditStore", () => {
  let dbPath: string;
  let store: AuditStore;

  beforeEach(() => {
    dbPath = tmpDb();
    store = new AuditStore(dbPath);
  });

  afterEach(() => {
    store.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });

  test("appends a record and reads it back", async () => {
    await store.append("keeper", "keeper.job.enqueued", makeHash("actor1"), { invoiceId: "1" });
    const records = store.getAll();
    expect(records).toHaveLength(1);
    expect(records[0].service).toBe("keeper");
    expect(records[0].action).toBe("keeper.job.enqueued");
    expect(records[0].sequence).toBe(1);
  });

  test("first record's prevHash is the genesis hash", async () => {
    await store.append("keeper", "keeper.job.enqueued", makeHash("a"), {});
    const [rec] = store.getAll();
    expect(rec.prevHash).toBe(GENESIS_HASH);
  });

  test("second record's prevHash equals first record's recordHash", async () => {
    await store.append("keeper", "keeper.job.enqueued", makeHash("a"), {});
    await store.append("fx-ingestion", "fx.rate.published", makeHash("b"), {});
    const [rec1, rec2] = store.getAll();
    expect(rec2.prevHash).toBe(rec1.recordHash);
  });

  test("getByService filters correctly", async () => {
    await store.append("keeper", "keeper.job.enqueued", makeHash("a"), {});
    await store.append("fx-ingestion", "fx.rate.published", makeHash("b"), {});
    await store.append("keeper", "keeper.job.done", makeHash("c"), {});
    const keeperRecords = store.getByService("keeper");
    expect(keeperRecords).toHaveLength(2);
  });

  test("getRange returns records within inclusive sequence bounds", async () => {
    for (let i = 0; i < 5; i++) {
      await store.append("admin-relay", "admin.relay.transaction.submitted", makeHash(`a${i}`), {});
    }
    const range = store.getRange(2, 4);
    expect(range).toHaveLength(3);
    expect(range[0].sequence).toBe(2);
    expect(range[2].sequence).toBe(4);
  });

  test("getCount returns correct number of records", async () => {
    expect(store.getCount()).toBe(0);
    await store.append("keeper", "keeper.job.enqueued", makeHash("x"), {});
    expect(store.getCount()).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// verifyChain tests
// ---------------------------------------------------------------------------

describe("verifyChain", () => {
  let dbPath: string;
  let store: AuditStore;

  beforeEach(() => {
    dbPath = tmpDb();
    store = new AuditStore(dbPath);
  });

  afterEach(() => {
    store.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });

  test("returns intact=true for an empty log", () => {
    const result = verifyChain([]);
    expect(result.intact).toBe(true);
    expect(result.recordsChecked).toBe(0);
  });

  test("verifies a clean chain of 5 records", async () => {
    for (let i = 0; i < 5; i++) {
      await store.append("keeper", "keeper.job.enqueued", makeHash(`a${i}`), { i });
    }
    const records = store.getAll();
    const result = verifyChain(records);
    expect(result.intact).toBe(true);
    expect(result.recordsChecked).toBe(5);
  });

  test("detects tampering: corrupted record contents", async () => {
    await store.append("keeper", "keeper.job.enqueued", makeHash("a"), {});
    await store.append("keeper", "keeper.job.done", makeHash("b"), {});
    const records = store.getAll();

    // Tamper with the first record's metadata
    const tampered = records.map((r, i) =>
      i === 0 ? { ...r, metadata: { injected: "evil" } } : r
    );

    const result = verifyChain(tampered);
    expect(result.intact).toBe(false);
    expect(result.firstBrokenSequence).toBe(1);
    expect(result.reason).toMatch(/tampered/);
  });

  test("detects tampering: broken prevHash linkage", async () => {
    await store.append("keeper", "keeper.job.enqueued", makeHash("a"), {});
    await store.append("keeper", "keeper.job.done", makeHash("b"), {});
    const records = store.getAll();

    // Break the chain by replacing sequence 2's prevHash
    const tampered: AuditRecord[] = records.map((r, i) =>
      i === 1 ? { ...r, prevHash: "0".repeat(64) } : r
    );

    const result = verifyChain(tampered);
    expect(result.intact).toBe(false);
    expect(result.firstBrokenSequence).toBe(2);
    expect(result.reason).toMatch(/prevHash mismatch/);
  });

  test("detects a single deleted record (sequence gap breaks prevHash)", async () => {
    for (let i = 0; i < 3; i++) {
      await store.append("keeper", "keeper.job.enqueued", makeHash(`a${i}`), {});
    }
    const records = store.getAll();
    // Simulate deletion of middle record
    const withDeleted = [records[0], records[2]]; // skip sequence 2

    const result = verifyChain(withDeleted);
    // Sequence 3's prevHash won't match sequence 1's recordHash
    expect(result.intact).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// AuditClient tests
// ---------------------------------------------------------------------------

describe("AuditClient", () => {
  let dbPath: string;
  let store: AuditStore;
  let client: AuditClient;

  beforeEach(() => {
    dbPath = tmpDb();
    store = new AuditStore(dbPath);
    client = new AuditClient(store);
  });

  afterEach(() => {
    store.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });

  test("emits an audit record with hashed actor", async () => {
    const rawActor = "GBMOCKACTOR0000000000000000000000000";
    await client.emit("keeper", "keeper.job.enqueued", rawActor, { invoiceId: 1 });
    const [rec] = store.getAll();
    // actorHash must NOT equal the raw actor address
    expect(rec.actorHash).not.toBe(rawActor);
    // It must equal sha256(rawActor)
    expect(rec.actorHash).toBe(makeHash(rawActor));
  });

  test("emits multiple records that form a valid chain", async () => {
    for (let i = 0; i < 4; i++) {
      await client.emit("admin-relay", "admin.relay.transaction.submitted", `actor${i}`, { i });
    }
    const records = store.getAll();
    const result = verifyChain(records);
    expect(result.intact).toBe(true);
  });

  test("accepts feature-flag mutation records and keeps the chain intact (#1116)", async () => {
    await client.emit("feature-flags", "feature-flag.disabled", "admin@kora.finance", {
      flagName: "secondary-market",
      previousValue: true,
      newValue: false,
    });
    await client.emit("feature-flags", "feature-flag.mutation-rejected", "intruder", {
      flagName: "secondary-market",
      attemptedAction: "enabled",
    });
    const records = store.getByService("feature-flags");
    expect(records.map((r) => r.action)).toEqual([
      "feature-flag.disabled",
      "feature-flag.mutation-rejected",
    ]);
    expect(verifyChain(store.getAll()).intact).toBe(true);
  });

  test("hashPii produces a deterministic hex string", () => {
    const h1 = AuditClient.hashPii("user@example.com");
    const h2 = AuditClient.hashPii("user@example.com");
    expect(h1).toBe(h2);
    expect(h1).toMatch(/^[0-9a-f]{64}$/);
  });

  test("hashPii produces different hashes for different inputs", () => {
    expect(AuditClient.hashPii("a")).not.toBe(AuditClient.hashPii("b"));
  });
});

// ---------------------------------------------------------------------------
// Concurrent writer test
// ---------------------------------------------------------------------------

describe("Concurrent writers", () => {
  test("parallel appends produce an intact chain", async () => {
    const dbPath = tmpDb();
    const store = new AuditStore(dbPath);

    // Launch 10 concurrent appends
    const promises = Array.from({ length: 10 }, (_, i) =>
      store.append("keeper", "keeper.job.enqueued", makeHash(`actor${i}`), { i })
    );
    await Promise.all(promises);

    const records = store.getAll();
    expect(records).toHaveLength(10);

    const result = verifyChain(records);
    expect(result.intact).toBe(true);
    expect(result.recordsChecked).toBe(10);

    store.close();
    if (fs.existsSync(dbPath)) fs.unlinkSync(dbPath);
  });
});
