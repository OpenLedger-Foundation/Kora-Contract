import Database from "better-sqlite3";

export interface PendingAlert {
  id: number;
  type: "ledger_gap";
  severity: "critical";
  ledger: number;
  fromLedger: number;
  toLedger: number;
  createdAt: number;
  attempts: number;
  nextAttemptAt: number;
  lastError: string | null;
}

export class PersistentAlertQueue {
  private readonly db: Database.Database;
  private readonly insertAlert: Database.Statement;
  private readonly dueAlerts: Database.Statement;
  private readonly markAttempt: Database.Statement;
  private readonly deleteAlert: Database.Statement;

  constructor(path: string) {
    this.db = new Database(path);
    this.db.pragma("journal_mode = WAL");
    this.db.pragma("busy_timeout = 5000");
    this.db.exec(`CREATE TABLE IF NOT EXISTS pending_alerts (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      dedup_key TEXT NOT NULL UNIQUE,
      type TEXT NOT NULL,
      severity TEXT NOT NULL,
      ledger INTEGER NOT NULL,
      from_ledger INTEGER NOT NULL,
      to_ledger INTEGER NOT NULL,
      created_at INTEGER NOT NULL,
      attempts INTEGER NOT NULL DEFAULT 0,
      next_attempt_at INTEGER NOT NULL,
      last_error TEXT
    );`);
    this.insertAlert = this.db.prepare(`INSERT OR IGNORE INTO pending_alerts (dedup_key, type, severity, ledger, from_ledger, to_ledger, created_at, next_attempt_at) VALUES (?, 'ledger_gap', 'critical', ?, ?, ?, ?, ?)`);
    this.dueAlerts = this.db.prepare("SELECT id, type, severity, ledger, from_ledger AS fromLedger, to_ledger AS toLedger, created_at AS createdAt, attempts, next_attempt_at AS nextAttemptAt, last_error AS lastError FROM pending_alerts WHERE next_attempt_at <= ? ORDER BY id ASC LIMIT ?");
    this.markAttempt = this.db.prepare("UPDATE pending_alerts SET attempts = attempts + 1, next_attempt_at = ?, last_error = ? WHERE id = ?");
    this.deleteAlert = this.db.prepare("DELETE FROM pending_alerts WHERE id = ?");
  }

  enqueueGap(ledger: number, fromLedger: number, toLedger: number, now = Date.now()): void {
    this.insertAlert.run(`ledger-gap:${ledger}`, ledger, fromLedger, toLedger, now, now);
  }

  pending(limit = 100): PendingAlert[] {
    return this.dueAlerts.all(Date.now(), limit) as PendingAlert[];
  }

  allPending(limit = 500): PendingAlert[] {
    return this.db.prepare("SELECT id, type, severity, ledger, from_ledger AS fromLedger, to_ledger AS toLedger, created_at AS createdAt, attempts, next_attempt_at AS nextAttemptAt, last_error AS lastError FROM pending_alerts ORDER BY id ASC LIMIT ?").all(limit) as PendingAlert[];
  }

  markDeliveryFailure(alertId: number, error: string, now = Date.now()): void {
    const attempts = Number(this.db.prepare("SELECT attempts FROM pending_alerts WHERE id = ?").pluck().get(alertId) ?? 0);
    const backoff = Math.min(300_000, 1_000 * 2 ** Math.min(attempts, 8));
    this.markAttempt.run(now + backoff, error.slice(0, 240), alertId);
  }

  acknowledge(alertId: number): void { this.deleteAlert.run(alertId); }

  depth(): number { return Number(this.db.prepare("SELECT COUNT(*) FROM pending_alerts").pluck().get() ?? 0); }

  clear(): number { return Number(this.db.prepare("DELETE FROM pending_alerts").run().changes); }

  close(): void { this.db.close(); }
}
