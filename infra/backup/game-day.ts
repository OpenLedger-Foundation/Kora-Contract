#!/usr/bin/env node
/**
 * Restore Game-Day Script — Structured disaster-recovery drill for all backed-up services.
 *
 * Closes #<issue_number>
 *
 * What it does for each service:
 *   1. Restores the latest backup to a fresh scratch directory (no live DB touched)
 *   2. Verifies row counts and SHA-256 checksums against stored sidecars
 *   3. Records restore duration and overall RTO per service
 *   4. Optionally tests encrypted dump detection (--encrypted flag)
 *   5. Writes a machine-readable JSON report to stdout (--json) or a human
 *      summary to the terminal
 *
 * Usage:
 *   node game-day.js [--all] [--service <name>] [--tier <hot|warm|cold>]
 *                    [--config <path>] [--json] [--encrypted] [--dry-run]
 *
 * Examples:
 *   node game-day.js --all --json
 *   node game-day.js --service keeper --tier warm
 *   node game-day.js --all --encrypted
 *
 * Exit codes:
 *   0  All services passed
 *   1  One or more services failed verification
 *   2  Invalid arguments / configuration error
 */

import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { loadConfig, getTierDir } from "./lib/config";
import { restoreSqlite, verifyHashSidecar, verifySqliteBasic, getDbInfo } from "./lib/sqlite";
import { verifyService, VerificationResult, findLatestBackup } from "./lib/verify";
import { listBackups } from "./lib/retention";
import pino from "pino";

const logger = pino({ name: "backup:game-day", level: process.env.LOG_LEVEL ?? "info" });

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface ServiceDrillResult {
  service: string;
  tier: string;
  backupFile: string;
  backupSizeBytes: number;
  hashVerified: boolean;
  restoreDurationMs: number;
  verificationDurationMs: number;
  totalRtoMs: number;
  rowCounts: Record<string, number>;
  pageCount: number | null;
  ok: boolean;
  error?: string;
  encrypted?: boolean;
}

export interface GameDayReport {
  runAt: string;
  durationMs: number;
  services: ServiceDrillResult[];
  passed: number;
  failed: number;
  rtoSla: { limitMs: number; allMet: boolean };
}

// 5-minute RTO SLA per the backup README
const RTO_LIMIT_MS = 5 * 60 * 1000;

// ---------------------------------------------------------------------------
// CLI parsing
// ---------------------------------------------------------------------------

interface CliArgs {
  all: boolean;
  service?: string;
  tier?: "hot" | "warm" | "cold";
  config?: string;
  json: boolean;
  encrypted: boolean;
  dryRun: boolean;
}

function parseArgs(): CliArgs {
  const args = process.argv.slice(2);
  const result: CliArgs = { all: false, json: false, encrypted: false, dryRun: false };
  for (let i = 0; i < args.length; i++) {
    switch (args[i]) {
      case "--all":          result.all = true;                                    break;
      case "--service":      result.service = args[++i];                           break;
      case "--tier":         result.tier = args[++i] as "hot" | "warm" | "cold"; break;
      case "--config":       result.config = args[++i];                            break;
      case "--json":         result.json = true;                                   break;
      case "--encrypted":    result.encrypted = true;                              break;
      case "--dry-run":      result.dryRun = true;                                 break;
      case "--help": case "-h": printUsage(); process.exit(0);                     break;
      default:
        console.error(`Unknown argument: ${args[i]}`);
        printUsage();
        process.exit(2);
    }
  }
  return result;
}

function printUsage(): void {
  console.log(`
Usage: node game-day.js [options]

Options:
  --all                   Drill all configured services (default if no --service)
  --service <name>        Drill a single service (audit-log, keeper, reporting)
  --tier <hot|warm|cold>  Specific tier to restore from (default: latest across all tiers)
  --config <path>         Path to config.json
  --json                  Emit machine-readable JSON report to stdout
  --encrypted             Detect and report encrypted dump files (see edge cases)
  --dry-run               Discover and check backups without performing the restore
  --help, -h              Show this help

Exit codes:
  0  All drilled services passed
  1  One or more services failed
  2  Invalid arguments / configuration error

Environment variables:
  BACKUP_ROOT             Root backup directory (default: /backups)
  AUDIT_DB_PATH           Source database for audit-log
  KEEPER_DB_PATH          Source database for keeper
  REPORTING_DB_PATH       Source database for reporting
  LOG_LEVEL               Logging level (debug, info, warn, error)
  GAME_DAY_RTO_LIMIT_MS   Override the 5-minute RTO SLA (milliseconds)
`);
}

// ---------------------------------------------------------------------------
// Encrypted dump detection
// ---------------------------------------------------------------------------

/**
 * Returns true when the file looks like an encrypted blob rather than a valid
 * SQLite database.  An unencrypted SQLite file starts with the 16-byte magic
 * "SQLite format 3\000".
 */
function isEncryptedDump(filePath: string): boolean {
  try {
    const fd = fs.openSync(filePath, "r");
    const header = Buffer.alloc(16);
    fs.readSync(fd, header, 0, 16, 0);
    fs.closeSync(fd);
    const SQLITE_MAGIC = Buffer.from("SQLite format 3\x00");
    return !header.equals(SQLITE_MAGIC);
  } catch {
    return true; // If we cannot read the header assume it is not usable as-is
  }
}

// ---------------------------------------------------------------------------
// Per-service drill
// ---------------------------------------------------------------------------

async function drillService(
  serviceName: string,
  config: ReturnType<typeof loadConfig>,
  preferredTier: "hot" | "warm" | "cold" | undefined,
  encrypted: boolean,
  dryRun: boolean
): Promise<ServiceDrillResult> {
  const overallStart = Date.now();

  // --- find backup --------------------------------------------------------
  let backupPath: string | null = null;
  let tierUsed = "unknown";

  if (preferredTier) {
    const backups = listBackups(config, serviceName, preferredTier);
    if (backups.length > 0) {
      backupPath = backups[0].path;
      tierUsed = preferredTier;
    }
  } else {
    backupPath = findLatestBackup(config, serviceName);
    if (backupPath) {
      for (const t of ["hot", "warm", "cold"] as const) {
        if (listBackups(config, serviceName, t).some((b) => b.path === backupPath)) {
          tierUsed = t;
          break;
        }
      }
    }
  }

  if (!backupPath) {
    return {
      service: serviceName, tier: tierUsed, backupFile: "", backupSizeBytes: 0,
      hashVerified: false, restoreDurationMs: 0, verificationDurationMs: 0,
      totalRtoMs: Date.now() - overallStart, rowCounts: {}, pageCount: null,
      ok: false, error: "No backup found in any tier",
    };
  }

  const backupSizeBytes = fs.statSync(backupPath).size;
  const backupFile = path.basename(backupPath);

  // --- encrypted dump detection (edge case) --------------------------------
  if (encrypted && isEncryptedDump(backupPath)) {
    logger.warn({ service: serviceName, backupPath }, "game-day: encrypted dump detected — skipping SQLite verification");
    return {
      service: serviceName, tier: tierUsed, backupFile, backupSizeBytes,
      hashVerified: false, restoreDurationMs: 0, verificationDurationMs: 0,
      totalRtoMs: Date.now() - overallStart, rowCounts: {}, pageCount: null,
      ok: false, encrypted: true,
      error: "Encrypted dump: SQLite magic header not found. Decrypt before verification.",
    };
  }

  // --- hash sidecar --------------------------------------------------------
  const hashVerified = await verifyHashSidecar(backupPath);
  if (!hashVerified) {
    logger.error({ service: serviceName, backupPath }, "game-day: SHA-256 sidecar verification failed");
    return {
      service: serviceName, tier: tierUsed, backupFile, backupSizeBytes,
      hashVerified: false, restoreDurationMs: 0, verificationDurationMs: 0,
      totalRtoMs: Date.now() - overallStart, rowCounts: {}, pageCount: null,
      ok: false, error: "SHA-256 sidecar mismatch — backup file may be corrupt",
    };
  }

  if (dryRun) {
    logger.info({ service: serviceName, backupPath, hashVerified }, "game-day: dry-run — skipping restore");
    const basic = await verifySqliteBasic(backupPath);
    return {
      service: serviceName, tier: tierUsed, backupFile, backupSizeBytes,
      hashVerified, restoreDurationMs: 0, verificationDurationMs: 0,
      totalRtoMs: Date.now() - overallStart,
      rowCounts: basic.rowCounts, pageCount: getDbInfo(backupPath)?.pageCount ?? null,
      ok: basic.ok, error: basic.error,
    };
  }

  // --- restore to scratch --------------------------------------------------
  const scratchDir = fs.mkdtempSync(path.join(os.tmpdir(), `kora-game-day-${serviceName}-`));
  const restoredPath = path.join(scratchDir, `${serviceName}.db`);

  try {
    logger.info({ service: serviceName, backupPath, scratchDir }, "game-day: restoring to scratch directory");

    const restoreStart = Date.now();
    restoreSqlite(backupPath, restoredPath);
    const restoreDurationMs = Date.now() - restoreStart;

    logger.info({ service: serviceName, restoreDurationMs }, "game-day: restore complete — running verification");

    // --- verify row counts + checksums ----------------------------------
    const verifyStart = Date.now();
    const result: VerificationResult = await verifyService(serviceName, restoredPath, config);
    const verificationDurationMs = Date.now() - verifyStart;

    // row counts from basic verification detail
    const rowCounts: Record<string, number> =
      (result.details.basic as { rowCounts?: Record<string, number> } | undefined)?.rowCounts ??
      (result.details.rowCounts as Record<string, number> | undefined) ??
      {};

    const pageInfo = getDbInfo(restoredPath);
    const totalRtoMs = Date.now() - overallStart;

    logger.info(
      { service: serviceName, ok: result.ok, restoreDurationMs, verificationDurationMs, totalRtoMs, rowCounts },
      result.ok ? "game-day: service PASSED" : "game-day: service FAILED"
    );

    return {
      service: serviceName, tier: tierUsed, backupFile, backupSizeBytes,
      hashVerified, restoreDurationMs, verificationDurationMs, totalRtoMs,
      rowCounts, pageCount: pageInfo?.pageCount ?? null,
      ok: result.ok, error: result.error,
    };
  } finally {
    // Always clean up scratch directory
    try {
      if (fs.existsSync(restoredPath)) fs.unlinkSync(restoredPath);
      const walPath = `${restoredPath}-wal`;
      const shmPath = `${restoredPath}-shm`;
      if (fs.existsSync(walPath)) fs.unlinkSync(walPath);
      if (fs.existsSync(shmPath)) fs.unlinkSync(shmPath);
      fs.rmdirSync(scratchDir);
    } catch {
      // Ignore cleanup errors; temp OS will reclaim on reboot
    }
  }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

function printHumanReport(report: GameDayReport): void {
  const divider = "─".repeat(70);
  console.log(`\n${divider}`);
  console.log(` Kora Restore Game-Day Report — ${report.runAt}`);
  console.log(divider);

  for (const svc of report.services) {
    const icon = svc.ok ? "✅" : "❌";
    const enc  = svc.encrypted ? " [ENCRYPTED DUMP]" : "";
    console.log(`\n${icon}  ${svc.service}${enc}`);
    if (svc.backupFile) {
      console.log(`   Backup:    ${svc.backupFile} (${(svc.backupSizeBytes / 1024).toFixed(1)} KB)`);
      console.log(`   Tier:      ${svc.tier}`);
    }
    console.log(`   Hash OK:   ${svc.hashVerified}`);
    if (!svc.ok && svc.error) {
      console.log(`   Error:     ${svc.error}`);
    }
    if (svc.restoreDurationMs > 0) {
      console.log(`   Restore:   ${svc.restoreDurationMs} ms`);
      console.log(`   Verify:    ${svc.verificationDurationMs} ms`);
    }
    console.log(`   RTO:       ${svc.totalRtoMs} ms ${svc.totalRtoMs > report.rtoSla.limitMs ? "⚠️  EXCEEDS SLA" : "(within SLA)"}`);
    if (Object.keys(svc.rowCounts).length > 0) {
      console.log(`   Row counts: ${JSON.stringify(svc.rowCounts)}`);
    }
    if (svc.pageCount != null) {
      console.log(`   DB pages:  ${svc.pageCount}`);
    }
  }

  console.log(`\n${divider}`);
  console.log(` Summary: ${report.passed} passed, ${report.failed} failed`);
  console.log(` Total duration: ${report.durationMs} ms`);
  console.log(` RTO SLA (${report.rtoSla.limitMs} ms): ${report.rtoSla.allMet ? "ALL MET ✅" : "VIOLATED ❌"}`);
  console.log(divider + "\n");
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

export async function runGameDay(
  services: string[],
  config: ReturnType<typeof loadConfig>,
  tier: "hot" | "warm" | "cold" | undefined,
  encrypted: boolean,
  dryRun: boolean,
  rtoLimitMs = RTO_LIMIT_MS
): Promise<GameDayReport> {
  const start = Date.now();
  const runAt = new Date().toISOString();

  logger.info(
    { services, tier: tier ?? "auto", encrypted, dryRun },
    "game-day: starting restore drill"
  );

  const results: ServiceDrillResult[] = [];
  for (const svc of services) {
    results.push(await drillService(svc, config, tier, encrypted, dryRun));
  }

  const passed = results.filter((r) => r.ok).length;
  const failed = results.filter((r) => !r.ok).length;
  const allRtoMet = results.every((r) => r.ok && r.totalRtoMs <= rtoLimitMs);

  const report: GameDayReport = {
    runAt, durationMs: Date.now() - start,
    services: results, passed, failed,
    rtoSla: { limitMs: rtoLimitMs, allMet: allRtoMet },
  };

  logger.info({ passed, failed, totalMs: report.durationMs }, "game-day: drill complete");
  return report;
}

async function main(): Promise<void> {
  const args = parseArgs();
  const config = loadConfig(args.config);

  const rtoLimitMs = process.env["GAME_DAY_RTO_LIMIT_MS"]
    ? Number(process.env["GAME_DAY_RTO_LIMIT_MS"])
    : RTO_LIMIT_MS;

  const services: string[] = args.service
    ? [args.service]
    : Object.keys(config.services);

  for (const svc of services) {
    if (!config.services[svc]) {
      console.error(`Unknown service: ${svc}`);
      process.exit(2);
    }
  }

  const report = await runGameDay(services, config, args.tier, args.encrypted, args.dryRun, rtoLimitMs);

  if (args.json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    printHumanReport(report);
  }

  if (report.failed > 0) process.exit(1);
}

main().catch((err) => {
  logger.error(err, "game-day: script crashed");
  process.exit(1);
});
