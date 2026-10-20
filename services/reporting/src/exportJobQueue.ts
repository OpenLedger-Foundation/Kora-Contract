/**
 * Reporting Service — Export Job Queue
 *
 * Manages async export jobs with a download-when-ready flow.
 * Large exports are generated in the background; callers poll /status/:jobId.
 *
 * Persistence: SQLite so jobs survive service restarts.
 *
 * Issue #770
 */

import * as path from "path";
import * as fs from "fs";
import Database, { Database as DB } from "better-sqlite3";
import { v4 as uuidv4 } from "uuid";
import {
  ExportJob,
  ExportFormat,
  ExportScope,
  ExportStatus,
  UserStatement,
  ProtocolStatement,
} from "./types";
import { StatementBuilder } from "./statementBuilder";
import { CsvRenderer } from "./csvRenderer";
import { PdfRenderer } from "./pdfRenderer";
import pino from "pino";

const logger = pino({ name: "reporting:queue" });

export interface ExportRequest {
  scope: ExportScope;
  format: ExportFormat;
  userAddress?: string;
  fromTs: number;
  toTs: number;
}

/**
 * Data source interface — injected so the queue is testable without a real
 * indexer or analytics service.
 */
export interface ReportingDataSource {
  getUserStatement(
    userAddress: string,
    fromTs: number,
    toTs: number
  ): Promise<UserStatement>;

  getProtocolStatement(fromTs: number, toTs: number): Promise<ProtocolStatement>;
}

export class ExportJobQueue {
  private db: DB;
  private readonly builder = new StatementBuilder();
  private readonly csvRenderer = new CsvRenderer();
  private readonly pdfRenderer = new PdfRenderer();

  constructor(
    dbPath: string,
    private readonly dataSource: ReportingDataSource,
    private readonly outputDir: string
  ) {
    this.db = new Database(dbPath);
    this._migrate();
    fs.mkdirSync(outputDir, { recursive: true });
  }

  // ---------------------------------------------------------------------------
  // Schema
  // ---------------------------------------------------------------------------

  private _migrate(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS export_jobs (
        job_id       TEXT    PRIMARY KEY,
        scope        TEXT    NOT NULL,
        format       TEXT    NOT NULL,
        user_address TEXT,
        from_ts      INTEGER NOT NULL,
        to_ts        INTEGER NOT NULL,
        status       TEXT    NOT NULL DEFAULT 'pending',
        output_path  TEXT,
        error_msg    TEXT,
        requested_at INTEGER NOT NULL,
        completed_at INTEGER
      );
    `);
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  /**
   * Enqueue an export request and immediately start processing in the
   * background.  Returns the jobId for polling.
   */
  enqueue(req: ExportRequest): string {
    if (req.scope === "per_user" && !req.userAddress) {
      throw new Error("userAddress is required for per_user scope");
    }

    const jobId = uuidv4();
    const now = Math.floor(Date.now() / 1000);

    this.db
      .prepare(
        `INSERT INTO export_jobs
           (job_id, scope, format, user_address, from_ts, to_ts, status, requested_at)
         VALUES
           (@jobId, @scope, @format, @userAddress, @fromTs, @toTs, 'pending', @now)`
      )
      .run({
        jobId,
        scope: req.scope,
        format: req.format,
        userAddress: req.userAddress ?? null,
        fromTs: req.fromTs,
        toTs: req.toTs,
        now,
      });

    logger.info({ jobId, scope: req.scope, format: req.format }, "Export job enqueued");

    // Start async processing (fire-and-forget — caller polls status)
    void this._process(jobId);

    return jobId;
  }

  /** Get current job status. */
  getJob(jobId: string): ExportJob | null {
    const row = this.db
      .prepare(`SELECT * FROM export_jobs WHERE job_id = ?`)
      .get(jobId) as RawRow | undefined;
    return row ? rowToJob(row) : null;
  }

  /** List all jobs, most recent first. */
  listJobs(limit = 100): ExportJob[] {
    const rows = this.db
      .prepare(`SELECT * FROM export_jobs ORDER BY requested_at DESC LIMIT ?`)
      .all(limit) as RawRow[];
    return rows.map(rowToJob);
  }

  /** Delete a job record (used by the retention sweeper). */
  deleteJob(jobId: string): void {
    this.db.prepare(`DELETE FROM export_jobs WHERE job_id = ?`).run(jobId);
    logger.info({ jobId }, "Export job deleted by retention sweeper");
  }

  close(): void {
    this.db.close();
  }

  // ---------------------------------------------------------------------------
  // Processing
  // ---------------------------------------------------------------------------

  private async _process(jobId: string): Promise<void> {
    this._updateStatus(jobId, "processing");

    const job = this.getJob(jobId);
    if (!job) return;

    try {
      let content: Buffer | string;

      if (job.scope === "per_user") {
        const rawStmt = await this.dataSource.getUserStatement(
          job.userAddress!,
          job.fromTs,
          job.toTs
        );
        const stmt = this.builder.buildUserStatement(
          rawStmt.userAddress,
          rawStmt.fromTs,
          rawStmt.toTs,
          rawStmt.funding,
          rawStmt.repayments,
          rawStmt.yields,
          rawStmt.fees
        );
        content =
          job.format === "csv"
            ? this.csvRenderer.renderUserStatement(stmt)
            : await this.pdfRenderer.renderUserStatement(stmt);
      } else {
        const rawStmt = await this.dataSource.getProtocolStatement(
          job.fromTs,
          job.toTs
        );
        const stmt = this.builder.buildProtocolStatement(
          rawStmt.fromTs,
          rawStmt.toTs,
          rawStmt.totalInvoicesFinanced,
          rawStmt.totalInvoicesRepaid,
          rawStmt.totalInvoicesDefaulted,
          rawStmt.summaryByAsset.flatMap(() => []),
          [],
          [],
          []
        );
        content =
          job.format === "csv"
            ? this.csvRenderer.renderProtocolStatement(stmt)
            : await this.pdfRenderer.renderProtocolStatement(stmt);
      }

      const ext = job.format === "csv" ? "csv" : "pdf";
      const outputPath = path.join(this.outputDir, `${jobId}.${ext}`);
      fs.writeFileSync(outputPath, typeof content === "string" ? content : content);

      const now = Math.floor(Date.now() / 1000);
      this.db
        .prepare(
          `UPDATE export_jobs
           SET status = 'ready', output_path = @outputPath, completed_at = @now
           WHERE job_id = @jobId`
        )
        .run({ outputPath, now, jobId });

      logger.info({ jobId, outputPath }, "Export job completed");
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      logger.error({ jobId, msg }, "Export job failed");
      const now = Math.floor(Date.now() / 1000);
      this.db
        .prepare(
          `UPDATE export_jobs
           SET status = 'failed', error_msg = @msg, completed_at = @now
           WHERE job_id = @jobId`
        )
        .run({ msg, now, jobId });
    }
  }

  private _updateStatus(jobId: string, status: ExportStatus): void {
    this.db
      .prepare(`UPDATE export_jobs SET status = ? WHERE job_id = ?`)
      .run(status, jobId);
  }
}

// ---------------------------------------------------------------------------
// SQLite row mapping
// ---------------------------------------------------------------------------

interface RawRow {
  job_id: string;
  scope: string;
  format: string;
  user_address: string | null;
  from_ts: number;
  to_ts: number;
  status: string;
  output_path: string | null;
  error_msg: string | null;
  requested_at: number;
  completed_at: number | null;
}

function rowToJob(r: RawRow): ExportJob {
  return {
    jobId: r.job_id,
    scope: r.scope as ExportScope,
    format: r.format as ExportFormat,
    userAddress: r.user_address ?? undefined,
    fromTs: r.from_ts,
    toTs: r.to_ts,
    status: r.status as ExportStatus,
    outputPath: r.output_path ?? undefined,
    errorMessage: r.error_msg ?? undefined,
    requestedAt: r.requested_at,
    completedAt: r.completed_at ?? undefined,
  };
}
