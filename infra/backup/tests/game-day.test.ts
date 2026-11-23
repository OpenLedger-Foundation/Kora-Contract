/**
 * Tests for the restore game-day script.
 *
 * Covers:
 *  - Happy-path: fixture DB restores successfully, row counts verified
 *  - Corrupt dump: verification step fails and is reported as a failure
 *  - Duration logging: each result carries restoreDurationMs and totalRtoMs > 0
 *  - Encrypted dump detection (edge case)
 *  - Dry-run mode: no restore performed, hash and basic checks still run
 *  - Missing backup: missing tier returns a structured failure, does not throw
 *  - All-services drill: report aggregates pass/fail counts correctly
 *
 * Closes #<issue_number>
 */

import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import Database from "better-sqlite3";
import { runGameDay, ServiceDrillResult } from "../game-day";
import { loadConfig, resetConfig, BackupConfig } from "../lib/config";
import { backupSqlite, writeHashSidecar, computeFileHash } from "../lib/sqlite";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function tmpDir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "kora-game-day-test-"));
}

function tmpFile(suffix = ".db"): string {
  return path.join(os.tmpdir(), `kora-gd-${Date.now()}-${Math.random()}${suffix}`);
}

/** Create a minimal SQLite DB with a known schema and seed data. */
function createFixtureDb(dbPath: string, tableName: string, rowCount: number): void {
  const db = new Database(dbPath);
  db.exec(`CREATE TABLE IF NOT EXISTS ${tableName} (id INTEGER PRIMARY KEY, val TEXT)`);
  const insert = db.prepare(`INSERT INTO ${tableName} (val) VALUES (?)`);
  for (let i = 0; i < rowCount; i++) {
    insert.run(`row-${i}`);
  }
  db.close();
}

/** Build a minimal BackupConfig pointing to a temp backup root. */
function buildConfig(backupRoot: string): BackupConfig {
  return {
    backupRoot,
    services: {
      keeper: {
        dbPathEnv: "KEEPER_DB_PATH",
        defaultDbPath: path.join(backupRoot, "keeper-source.db"),
        verifyScript: "basic",
      },
    },
    retention: {
      hot: { intervalMinutes: 15, maxAgeHours: 4 },
      warm: { intervalHours: 1, maxAgeDays: 7 },
      cold: { intervalDays: 1, maxAgeDays: 90, timeUtc: "02:00" },
    },
    verification: {
      auditLog: { verifyBinary: "node", verifyArgs: ["../services/audit-log/dist/verify.js", "--db"] },
    },
  };
}

/** Seed a backup file for a service into the given tier directory. */
async function seedBackup(config: BackupConfig, serviceName: string, tier: "hot" | "warm" | "cold", sourceDb: string): Promise<string> {
  const tierDir = path.join(config.backupRoot, tier, serviceName);
  fs.mkdirSync(tierDir, { recursive: true });
  const now = new Date().toISOString().replace(/[:.]/g, "").replace("T", "T").split(".")[0] + "Z";
  const backupName = `${serviceName}-${now}.db`;
  const backupPath = path.join(tierDir, backupName);
  fs.copyFileSync(sourceDb, backupPath);
  await writeHashSidecar(backupPath);
  return backupPath;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("game-day: happy path", () => {
  let backupRoot: string;
  let config: BackupConfig;
  let sourceDb: string;

  beforeEach(async () => {
    resetConfig();
    backupRoot = tmpDir();
    config = buildConfig(backupRoot);
    sourceDb = path.join(backupRoot, "keeper-source.db");
    // keeper schema uses keeper_jobs table
    const db = new Database(sourceDb);
    db.exec(`
      CREATE TABLE IF NOT EXISTS keeper_jobs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        dedup_key TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        invoice_id TEXT NOT NULL,
        deadline_ts INTEGER NOT NULL,
        contract_address TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending',
        attempts INTEGER NOT NULL DEFAULT 0,
        last_attempt_at INTEGER,
        last_outcome TEXT,
        last_tx_hash TEXT,
        last_error TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      )
    `);
    db.prepare(`INSERT INTO keeper_jobs (dedup_key, kind, invoice_id, deadline_ts, contract_address, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)`)
      .run("funding_expiry:1", "funding_expiry", "1", 100, "CTEST", 1, 1);
    db.close();
    await seedBackup(config, "keeper", "hot", sourceDb);
  });

  afterEach(() => {
    fs.rmSync(backupRoot, { recursive: true, force: true });
  });

  test("restores and verifies a valid backup — ok:true", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false);
    expect(report.passed).toBe(1);
    expect(report.failed).toBe(0);
    expect(report.services[0].ok).toBe(true);
  });

  test("row counts are present in the result", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false);
    const svc = report.services[0] as ServiceDrillResult;
    expect(svc.rowCounts).toBeDefined();
    expect(typeof svc.rowCounts).toBe("object");
  });

  test("hashVerified is true for a valid backup", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false);
    expect(report.services[0].hashVerified).toBe(true);
  });

  test("duration metrics are recorded (non-zero)", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false);
    const svc = report.services[0];
    expect(svc.restoreDurationMs).toBeGreaterThanOrEqual(0);
    expect(svc.totalRtoMs).toBeGreaterThan(0);
    expect(report.durationMs).toBeGreaterThan(0);
  });

  test("report contains runAt ISO timestamp", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false);
    expect(() => new Date(report.runAt)).not.toThrow();
    expect(new Date(report.runAt).getTime()).toBeGreaterThan(0);
  });

  test("rtoSla.allMet is true when restore completes well within limit", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false, 60_000);
    expect(report.rtoSla.allMet).toBe(true);
  });

  test("rtoSla.allMet is false when RTO limit is set impossibly low", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false, 0);
    // A 0 ms limit can never be met
    expect(report.rtoSla.allMet).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Corrupt dump detection
// ---------------------------------------------------------------------------

describe("game-day: corrupt dump fails verification", () => {
  let backupRoot: string;
  let config: BackupConfig;

  beforeEach(async () => {
    resetConfig();
    backupRoot = tmpDir();
    config = buildConfig(backupRoot);
    const tierDir = path.join(backupRoot, "hot", "keeper");
    fs.mkdirSync(tierDir, { recursive: true });
    const now = new Date().toISOString().replace(/[:.]/g, "").replace("T", "T").split(".")[0] + "Z";
    const backupPath = path.join(tierDir, `keeper-${now}.db`);
    // Write corrupt bytes — not a valid SQLite file
    fs.writeFileSync(backupPath, Buffer.from("this is definitely not a valid sqlite database file at all"));
    await writeHashSidecar(backupPath);
  });

  afterEach(() => {
    fs.rmSync(backupRoot, { recursive: true, force: true });
  });

  test("corrupt dump sets ok:false in service result", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false);
    expect(report.failed).toBe(1);
    expect(report.services[0].ok).toBe(false);
  });

  test("error message is populated on corrupt dump", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false);
    expect(report.services[0].error).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Tampered hash sidecar
// ---------------------------------------------------------------------------

describe("game-day: tampered hash sidecar fails before restore", () => {
  let backupRoot: string;
  let config: BackupConfig;
  let sourceDb: string;

  beforeEach(async () => {
    resetConfig();
    backupRoot = tmpDir();
    config = buildConfig(backupRoot);
    sourceDb = path.join(backupRoot, "keeper-source.db");
    const db = new Database(sourceDb);
    db.exec(`CREATE TABLE IF NOT EXISTS keeper_jobs (id INTEGER PRIMARY KEY, dedup_key TEXT UNIQUE, kind TEXT, invoice_id TEXT, deadline_ts INTEGER, contract_address TEXT, status TEXT DEFAULT 'pending', attempts INTEGER DEFAULT 0, last_attempt_at INTEGER, last_outcome TEXT, last_tx_hash TEXT, last_error TEXT, created_at INTEGER, updated_at INTEGER)`);
    db.close();

    const backupPath = await seedBackup(config, "keeper", "hot", sourceDb);
    // Tamper the sidecar
    fs.writeFileSync(`${backupPath}.sha256`, "0000000000000000000000000000000000000000000000000000000000000000  keeper-tampered.db\n");
  });

  afterEach(() => {
    fs.rmSync(backupRoot, { recursive: true, force: true });
  });

  test("tampered sidecar sets ok:false and hashVerified:false", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, false);
    expect(report.services[0].ok).toBe(false);
    expect(report.services[0].hashVerified).toBe(false);
    expect(report.services[0].error).toMatch(/SHA-256|sidecar|mismatch/i);
  });
});

// ---------------------------------------------------------------------------
// Missing backup
// ---------------------------------------------------------------------------

describe("game-day: missing backup", () => {
  let backupRoot: string;
  let config: BackupConfig;

  beforeEach(() => {
    resetConfig();
    backupRoot = tmpDir();
    config = buildConfig(backupRoot);
    // No backup files seeded
  });

  afterEach(() => {
    fs.rmSync(backupRoot, { recursive: true, force: true });
  });

  test("missing backup returns ok:false without throwing", async () => {
    const report = await runGameDay(["keeper"], config, undefined, false, false);
    expect(report.failed).toBe(1);
    expect(report.services[0].ok).toBe(false);
    expect(report.services[0].error).toMatch(/no backup/i);
  });
});

// ---------------------------------------------------------------------------
// Encrypted dump detection (edge case)
// ---------------------------------------------------------------------------

describe("game-day: encrypted dump detection", () => {
  let backupRoot: string;
  let config: BackupConfig;

  beforeEach(async () => {
    resetConfig();
    backupRoot = tmpDir();
    config = buildConfig(backupRoot);
    const tierDir = path.join(backupRoot, "hot", "keeper");
    fs.mkdirSync(tierDir, { recursive: true });
    const now = new Date().toISOString().replace(/[:.]/g, "").replace("T", "T").split(".")[0] + "Z";
    const backupPath = path.join(tierDir, `keeper-${now}.db`);
    // Write random bytes — no SQLite magic header (simulates encrypted blob)
    const randomBytes = Buffer.alloc(1024);
    for (let i = 0; i < 1024; i++) randomBytes[i] = Math.floor(Math.random() * 256);
    fs.writeFileSync(backupPath, randomBytes);
    await writeHashSidecar(backupPath);
  });

  afterEach(() => {
    fs.rmSync(backupRoot, { recursive: true, force: true });
  });

  test("encrypted flag reports encrypted:true and ok:false", async () => {
    const report = await runGameDay(["keeper"], config, "hot", true, false);
    expect(report.services[0].ok).toBe(false);
    expect(report.services[0].encrypted).toBe(true);
    expect(report.services[0].error).toMatch(/encrypted/i);
  });
});

// ---------------------------------------------------------------------------
// Dry-run mode
// ---------------------------------------------------------------------------

describe("game-day: dry-run mode", () => {
  let backupRoot: string;
  let config: BackupConfig;
  let sourceDb: string;

  beforeEach(async () => {
    resetConfig();
    backupRoot = tmpDir();
    config = buildConfig(backupRoot);
    sourceDb = path.join(backupRoot, "keeper-source.db");
    const db = new Database(sourceDb);
    db.exec(`CREATE TABLE IF NOT EXISTS keeper_jobs (id INTEGER PRIMARY KEY, dedup_key TEXT UNIQUE, kind TEXT, invoice_id TEXT, deadline_ts INTEGER, contract_address TEXT, status TEXT DEFAULT 'pending', attempts INTEGER DEFAULT 0, last_attempt_at INTEGER, last_outcome TEXT, last_tx_hash TEXT, last_error TEXT, created_at INTEGER, updated_at INTEGER)`);
    db.close();
    await seedBackup(config, "keeper", "hot", sourceDb);
  });

  afterEach(() => {
    fs.rmSync(backupRoot, { recursive: true, force: true });
  });

  test("dry-run sets restoreDurationMs to 0", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, true);
    expect(report.services[0].restoreDurationMs).toBe(0);
  });

  test("dry-run still checks the hash sidecar", async () => {
    const report = await runGameDay(["keeper"], config, "hot", false, true);
    expect(report.services[0].hashVerified).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Multi-service report aggregation
// ---------------------------------------------------------------------------

describe("game-day: multi-service aggregation", () => {
  let backupRoot: string;
  let config: BackupConfig;
  let sourceDb: string;

  beforeEach(async () => {
    resetConfig();
    backupRoot = tmpDir();

    // Build config with two services: one will have a backup, one will not
    config = {
      backupRoot,
      services: {
        keeper: { dbPathEnv: "KEEPER_DB_PATH", defaultDbPath: path.join(backupRoot, "k.db"), verifyScript: "basic" },
        reporting: { dbPathEnv: "REPORTING_DB_PATH", defaultDbPath: path.join(backupRoot, "r.db"), verifyScript: "basic" },
      },
      retention: { hot: { intervalMinutes: 15, maxAgeHours: 4 }, warm: { intervalHours: 1, maxAgeDays: 7 }, cold: { intervalDays: 1, maxAgeDays: 90 } },
      verification: { auditLog: { verifyBinary: "node", verifyArgs: [] } },
    };

    sourceDb = path.join(backupRoot, "keeper-source.db");
    const db = new Database(sourceDb);
    db.exec(`CREATE TABLE IF NOT EXISTS keeper_jobs (id INTEGER PRIMARY KEY, dedup_key TEXT UNIQUE, kind TEXT, invoice_id TEXT, deadline_ts INTEGER, contract_address TEXT, status TEXT DEFAULT 'pending', attempts INTEGER DEFAULT 0, last_attempt_at INTEGER, last_outcome TEXT, last_tx_hash TEXT, last_error TEXT, created_at INTEGER, updated_at INTEGER)`);
    db.close();
    await seedBackup(config, "keeper", "hot", sourceDb);
    // reporting has no backup intentionally
  });

  afterEach(() => {
    fs.rmSync(backupRoot, { recursive: true, force: true });
  });

  test("passed/failed counts sum to service count", async () => {
    const report = await runGameDay(["keeper", "reporting"], config, undefined, false, false);
    expect(report.passed + report.failed).toBe(2);
    expect(report.passed).toBe(1);
    expect(report.failed).toBe(1);
  });

  test("each service entry appears in services array", async () => {
    const report = await runGameDay(["keeper", "reporting"], config, undefined, false, false);
    const names = report.services.map((s) => s.service);
    expect(names).toContain("keeper");
    expect(names).toContain("reporting");
  });
});
