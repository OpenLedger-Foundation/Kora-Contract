# Kora Protocol — Disaster Recovery & Backup Strategy

This directory contains the backup, restore, and verification infrastructure for Kora's stateful off-chain services.

## Services Covered

| Service | Storage | Database Path (Env Var) | Criticality |
|---------|---------|-------------------------|-------------|
| `audit-log` | SQLite (WAL mode) | `AUDIT_DB_PATH` (default: `./audit.db`) | **Critical** — tamper-evident audit trail |
| `keeper` | SQLite | `KEEPER_DB_PATH` (default: `./keeper.db`) | **High** — deadline-based job queue |
| `reporting` | SQLite | `REPORTING_DB_PATH` (default: `./reporting.db`) | **Medium** — export job queue & outputs |

> **Note:** The `timeseries` and `kyb` services currently use in-memory stores for development. Production deployments should configure persistent backends (TimescaleDB/InfluxDB for timeseries, Postgres/DynamoDB for KYB) which have their own backup procedures.

---

## Backup Strategy

### Schedule & Retention

| Tier | Frequency | Retention | RPO (Max Data Loss) |
|------|-----------|-----------|---------------------|
| **Hot** | Every 15 minutes | 4 hours | ≤ 15 minutes |
| **Warm** | Hourly | 7 days | ≤ 1 hour |
| **Cold** | Daily at 02:00 UTC | 90 days | ≤ 24 hours |

**RPO (Recovery Point Objective):** Maximum 15 minutes of data loss (hot tier backup interval).

**RTO (Recovery Time Objective):** Estimated **< 5 minutes** for SQLite databases (single-file restore + integrity verification). Measured during quarterly restore drills.

### Backup Format

- **SQLite databases:** Binary copy via `sqlite3 .backup` command (online, consistent, non-blocking)
- **Audit log outputs:** Separate integrity verification manifest (SHA-256 of each backup file)
- **Export job outputs (`reporting`):** Archived alongside the database

### Storage Layout

```
/backups/
├── hot/
│   ├── audit-log/
│   │   ├── audit-log-20260115T143000Z.db
│   │   └── audit-log-20260115T143000Z.db.sha256
│   ├── keeper/
│   └── reporting/
├── warm/
│   ├── audit-log/
│   ├── keeper/
│   └── reporting/
└── cold/
    ├── audit-log/
    ├── keeper/
    └── reporting/
```

---

## Quick Start

### Prerequisites

```bash
# Install sqlite3 CLI (for .backup command)
# Ubuntu/Debian:
sudo apt-get install sqlite3
# macOS:
brew install sqlite3
```

### Run a Manual Backup

```bash
# From repo root
cd infra/backup

# Backup all services (uses env vars for DB paths)
node backup.js --all

# Backup specific service
node backup.js --service audit-log --tier hot
```

### Restore a Database

```bash
# Restore latest hot backup for audit-log
node restore.js --service audit-log --tier hot --target ./restored-audit.db

# Verify the restored database
node verify-restore.js --service audit-log --db ./restored-audit.db
```

### Automated Restore Verification (CI/Scheduled Job)

```bash
# Runs in isolation: restores latest backup to temp dir, verifies integrity
node verify-restore.js --service audit-log --auto
```

---

## Configuration

Create `infra/backup/config.json` (or use environment variables):

```json
{
  "backupRoot": "/backups",
  "services": {
    "audit-log": {
      "dbPathEnv": "AUDIT_DB_PATH",
      "defaultDbPath": "./audit.db",
      "verifyScript": "verify-chain"
    },
    "keeper": {
      "dbPathEnv": "KEEPER_DB_PATH",
      "defaultDbPath": "./keeper.db",
      "verifyScript": "basic"
    },
    "reporting": {
      "dbPathEnv": "REPORTING_DB_PATH",
      "defaultDbPath": "./reporting.db",
      "verifyScript": "basic"
    }
  },
  "retention": {
    "hot": { "intervalMinutes": 15, "maxAgeHours": 4 },
    "warm": { "intervalHours": 1, "maxAgeDays": 7 },
    "cold": { "intervalDays": 1, "maxAgeDays": 90, "timeUtc": "02:00" }
  }
}
```

Environment variable overrides:
- `BACKUP_ROOT` — root backup directory (default: `/backups`)
- `AUDIT_DB_PATH`, `KEEPER_DB_PATH`, `REPORTING_DB_PATH` — source database paths

---

## Automated Scheduling (systemd timers / cron)

### systemd (recommended for production)

```ini
# /etc/systemd/system/kora-backup-hot.service
[Unit]
Description=Kora Hot Backup (15-min)
[Service]
Type=oneshot
WorkingDirectory=/opt/kora/infra/backup
ExecStart=/usr/bin/node backup.js --all --tier hot
Environment=BACKUP_ROOT=/backups
Environment=AUDIT_DB_PATH=/var/lib/kora/audit.db
Environment=KEEPER_DB_PATH=/var/lib/kora/keeper.db
Environment=REPORTING_DB_PATH=/var/lib/kora/reporting.db
```

```ini
# /etc/systemd/system/kora-backup-hot.timer
[Unit]
Description=Run hot backup every 15 minutes
[Timer]
OnCalendar=*:0/15
Persistent=true
[Install]
WantedBy=timers.target
```

Repeat for `warm` (hourly) and `cold` (daily at 02:00).

### Cron (alternative)

```cron
# Hot: every 15 minutes
*/15 * * * * cd /opt/kora/infra/backup && node backup.js --all --tier hot >> /var/log/kora-backup-hot.log 2>&1

# Warm: hourly
0 * * * * cd /opt/kora/infra/backup && node backup.js --all --tier warm >> /var/log/kora-backup-warm.log 2>&1

# Cold: daily at 02:00 UTC
0 2 * * * cd /opt/kora/infra/backup && node backup.js --all --tier cold >> /var/log/kora-backup-cold.log 2>&1
```

---

## Restore Procedure (Documented & Tested)

### Scenario: Audit Log Database Corruption / Loss

1. **Stop the audit-log service** (prevent writes to potentially corrupt DB)
2. **Identify the latest valid backup tier** (hot → warm → cold)
3. **Restore to a temporary location:**
   ```bash
   node restore.js --service audit-log --tier hot --target /tmp/audit-restored.db
   ```
4. **Verify integrity:**
   ```bash
   node verify-restore.js --service audit-log --db /tmp/audit-restored.db
   ```
   - Must exit code 0 (chain intact)
   - Records checked should match expected count
5. **Swap in the restored database:**
   ```bash
   mv /tmp/audit-restored.db /var/lib/kora/audit.db
   # Ensure WAL/SHM files are cleaned up
   rm -f /var/lib/kora/audit.db-wal /var/lib/kora/audit.db-shm
   ```
6. **Restart the audit-log service**
7. **Verify service health:** `curl http://localhost:PORT/health`

### Scenario: Keeper / Reporting Database Loss

Same procedure, but verification is basic (schema + row counts):
```bash
node verify-restore.js --service keeper --db /tmp/keeper-restored.db
```

---

## Restore Verification Job (Automated)

The `verify-restore.js` script supports an `--auto` mode that:

1. Finds the latest backup for the service (across all tiers)
2. Restores to an isolated temporary directory
3. Runs the appropriate verification:
   - `audit-log`: Full hash-chain verification via `verifyChain()`
   - `keeper`: Schema validation + row count sanity check
   - `reporting`: Schema validation + row count sanity check
4. Reports **RTO measurement** (time from start to verified)
5. Cleans up temporary files
6. Exits 0 on success, non-zero on failure

**Run periodically (e.g., daily via cron):**
```bash
0 3 * * * cd /opt/kora/infra/backup && node verify-restore.js --service audit-log --auto >> /var/log/kora-verify-restore.log 2>&1
```

### Scheduled GitHub Workflow (`backup-verify.yml`)

A weekly scheduled workflow runs `verify-restore --auto --json` for each of
`audit-log`, `keeper`, and `reporting` without exposing secret values in logs:

- Schedule: Mondays at 03:00 UTC (`0 3 * * 1`), plus manual `workflow_dispatch` drills.
- Each service runs `node verify-restore.ts --service <name> --auto --json`; outputs are combined into `verify-report.json`.
- The combined JSON report is uploaded as the `backup-verify-report` artifact.
- Any per-service failure fails the job with a concise `FAILED: <services>` summary.
- Permissions are minimal (`contents: read`); no secrets are printed — only pass/fail, durations, and non-secret paths.

---

## Audit Log Hash-Chain Integrity After Restore

The audit log uses a cryptographic hash chain where each record's `prevHash` links to the previous record's `recordHash`. After restore:

1. The verifier (`verifyChain`) walks the entire chain from genesis
2. Validates every `prevHash` link
3. Recomputes every `recordHash` from record contents
4. Reports the **first broken sequence** if tampering/corruption detected

**Critical:** The backup must be a *consistent snapshot*. Using `sqlite3 .backup` ensures this (it uses SQLite's online backup API which produces a page-by-page consistent copy).

---

## Monitoring & Alerting

| Metric | Alert Threshold | Action |
|--------|-----------------|--------|
| Backup job exit code ≠ 0 | Immediate | Page on-call |
| No backup file created in 2× interval | 30 min (hot), 2 hr (warm), 48 hr (cold) | Page on-call |
| Restore verification job fails | Immediate | Page on-call |
| Backup disk usage > 80% | Warning | Investigate retention cleanup |
| Restore verification RTO > 10 min | Warning | Investigate performance |

---

## Testing the Full Cycle (PR Checklist)

To validate this implementation, the PR must demonstrate:

- [ ] `node backup.js --all --tier hot` creates valid backups for all 3 services
- [ ] `node restore.js --service audit-log --tier hot --target /tmp/test.db` restores successfully
- [ ] `node verify-restore.js --service audit-log --db /tmp/test.db` passes (exit 0)
- [ ] Tampered backup is detected by verifier (exit 1)
- [ ] RTO measured and documented (< 5 min for SQLite)
- [ ] Retention cleanup works (old backups removed per policy)

---

## Security Considerations

- Backup files contain **hashed actor references only** (no raw PII per audit-log design)
- Backup directory permissions: `0700` (owner read/write/execute only)
- Encryption at rest: Recommended — mount `/backups` on encrypted volume or use `gpg`/`age` on backup files
- Access control: Only backup/restore automation and on-call engineers should have access

---

## File Index

```
infra/backup/
├── README.md                    # This file
├── config.json                  # Configuration (copy from config.json.example)
├── config.json.example          # Example configuration
├── backup.js                    # Main backup script
├── restore.js                   # Restore script
├── verify-restore.js            # Restore verification + integrity check
├── lib/
│   ├── config.ts                # Configuration loader
│   ├── sqlite.ts                # SQLite backup/restore utilities
│   ├── retention.ts             # Retention policy enforcement
│   └── verify.ts                # Verification logic per service
├── tests/
│   ├── backup.test.ts           # Unit tests for backup logic
│   ├── restore.test.ts          # Unit tests for restore logic
│   └── verify-restore.test.ts   # Unit tests for verification
└── package.json                 # Dependencies for backup tooling
```