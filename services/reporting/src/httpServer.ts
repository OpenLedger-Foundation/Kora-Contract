/**
 * Reporting Service — HTTP Server
 *
 * Routes:
 *   POST /exports                         Enqueue an export job
 *   GET  /exports/:id                     Poll job status
 *   GET  /exports/:id/download            Download completed output
 *   GET  /health                          Liveness probe
 *
 * Auth: API-key via X-Api-Key header (or Bearer JWT).
 * Admin scope (admin:true in token / X-Admin-Actor header) allows requesting
 * any user's export; otherwise a per_user export must match req.userId.
 *
 * Retention: a sweeper runs every REPORTING_RETENTION_CHECK_MS (default 60 s)
 * and deletes completed jobs + output files older than REPORTING_RETENTION_TTL_S
 * (default 7 days = 604800 s).
 *
 * Issue #1004
 */

import * as http from "http";
import * as fs from "fs";
import * as path from "path";
import pino from "pino";
import { ExportJobQueue } from "./exportJobQueue";
import { ExportFormat, ExportScope } from "./types";

const logger = pino({ name: "reporting:http" });

// ── Request extension ─────────────────────────────────────────────────────────

export interface ReportingRequest extends http.IncomingMessage {
  /** Set by auth middleware — raw user identifier. */
  userId?: string;
  /** Set by admin middleware — non-null means the caller has admin scope. */
  adminActor?: string;
  /** Parsed JSON body (set by bodyParser). */
  body?: unknown;
}

// ── Auth middleware ───────────────────────────────────────────────────────────

const VALID_API_KEYS = new Set<string>(
  (process.env["REPORTING_API_KEYS"] ?? "")
    .split(",")
    .map((k) => k.trim())
    .filter(Boolean)
);

const ADMIN_API_KEYS = new Set<string>(
  (process.env["REPORTING_ADMIN_KEYS"] ?? "")
    .split(",")
    .map((k) => k.trim())
    .filter(Boolean)
);

/**
 * Resolve the caller identity from the request headers.
 * Sets req.userId and req.adminActor if authenticated.
 * Returns false (and writes a 401) if not authenticated.
 */
function authenticate(
  req: ReportingRequest,
  res: http.ServerResponse
): boolean {
  const apiKey =
    req.headers["x-api-key"] as string | undefined ??
    (req.headers["authorization"] ?? "").replace(/^Bearer\s+/, "");

  if (!apiKey || (!VALID_API_KEYS.has(apiKey) && !ADMIN_API_KEYS.has(apiKey))) {
    // If no API keys are configured (dev mode), allow all
    if (VALID_API_KEYS.size === 0 && ADMIN_API_KEYS.size === 0) {
      req.userId = req.headers["x-user-id"] as string ?? "anonymous";
      req.adminActor = req.headers["x-admin-actor"] as string ?? undefined;
      return true;
    }
    res.writeHead(401, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ error: "Unauthorized" }));
    return false;
  }

  req.userId = req.headers["x-user-id"] as string ?? apiKey.slice(0, 16);

  if (ADMIN_API_KEYS.has(apiKey)) {
    req.adminActor = req.headers["x-admin-actor"] as string ?? req.userId;
  }

  return true;
}

// ── Body parser ───────────────────────────────────────────────────────────────

function parseBody(req: http.IncomingMessage): Promise<unknown> {
  return new Promise((resolve, reject) => {
    let raw = "";
    req.setEncoding("utf8");
    req.on("data", (chunk) => (raw += chunk));
    req.on("end", () => {
      try {
        resolve(raw.length > 0 ? JSON.parse(raw) : {});
      } catch {
        reject(new Error("Invalid JSON body"));
      }
    });
    req.on("error", reject);
  });
}

// ── Route helpers ─────────────────────────────────────────────────────────────

function json(
  res: http.ServerResponse,
  status: number,
  body: unknown
): void {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    "Content-Type": "application/json",
    "Content-Length": Buffer.byteLength(payload),
  });
  res.end(payload);
}

// ── Route handlers ────────────────────────────────────────────────────────────

/**
 * POST /exports
 * Enqueue an export job and return the jobId immediately.
 *
 * Body: { scope, format, userAddress?, fromTs, toTs }
 */
async function handleEnqueue(
  queue: ExportJobQueue,
  req: ReportingRequest,
  res: http.ServerResponse
): Promise<void> {
  const body = req.body as Record<string, unknown>;

  const scope = body["scope"] as ExportScope | undefined;
  const format = body["format"] as ExportFormat | undefined;
  const fromTs = Number(body["fromTs"] ?? body["from_ts"]);
  const toTs = Number(body["toTs"] ?? body["to_ts"]);
  let userAddress = (body["userAddress"] ?? body["user_address"]) as string | undefined;

  // Validate required fields
  if (!scope || !["per_user", "protocol_wide"].includes(scope)) {
    return json(res, 400, { error: "scope must be 'per_user' or 'protocol_wide'" });
  }
  if (!format || !["csv", "pdf"].includes(format)) {
    return json(res, 400, { error: "format must be 'csv' or 'pdf'" });
  }
  if (!isFinite(fromTs) || !isFinite(toTs) || fromTs >= toTs) {
    return json(res, 400, { error: "fromTs and toTs must be valid Unix timestamps with fromTs < toTs" });
  }

  // Authorization: per_user exports must match the authenticated user (unless admin)
  if (scope === "per_user") {
    if (!userAddress) {
      // Default to the authenticated user
      userAddress = req.userId;
    }
    if (!req.adminActor && userAddress !== req.userId) {
      return json(res, 403, {
        error: "Per-user export must match authenticated user; admin scope required to export for others",
      });
    }
  }

  try {
    const jobId = queue.enqueue({ scope, format, userAddress, fromTs, toTs });
    logger.info({ jobId, scope, format, userId: req.userId }, "Export job enqueued via HTTP");
    return json(res, 202, { jobId });
  } catch (err) {
    const message = err instanceof Error ? err.message : "Unknown error";
    return json(res, 400, { error: message });
  }
}

/**
 * GET /exports/:id
 * Return current job status.
 */
function handleGetJob(
  queue: ExportJobQueue,
  req: ReportingRequest,
  res: http.ServerResponse,
  jobId: string
): void {
  const job = queue.getJob(jobId);
  if (!job) {
    return json(res, 404, { error: "Job not found" });
  }
  // Non-admin users can only view their own jobs
  if (!req.adminActor && job.userAddress && job.userAddress !== req.userId) {
    return json(res, 403, { error: "Forbidden" });
  }
  // Omit local outputPath from the API response (internal detail)
  const { outputPath: _omit, ...safeJob } = job as typeof job & { outputPath?: string };
  return json(res, 200, safeJob);
}

/**
 * GET /exports/:id/download
 * Stream the generated file to the client.
 */
function handleDownload(
  queue: ExportJobQueue,
  req: ReportingRequest,
  res: http.ServerResponse,
  jobId: string
): void {
  const job = queue.getJob(jobId);
  if (!job) {
    return json(res, 404, { error: "Job not found" });
  }
  if (!req.adminActor && job.userAddress && job.userAddress !== req.userId) {
    return json(res, 403, { error: "Forbidden" });
  }
  if (job.status !== "ready" || !job.outputPath) {
    if (job.status === "failed") {
      return json(res, 500, { error: "Export failed", details: job.errorMessage });
    }
    return json(res, 409, { error: `Job is not ready (status: ${job.status})` });
  }
  if (!fs.existsSync(job.outputPath)) {
    return json(res, 404, { error: "Output file not found (may have been purged by retention sweeper)" });
  }
  const contentType =
    job.format === "csv" ? "text/csv" : "application/pdf";
  const filename = path.basename(job.outputPath);
  res.writeHead(200, {
    "Content-Type": contentType,
    "Content-Disposition": `attachment; filename="${filename}"`,
  });
  fs.createReadStream(job.outputPath).pipe(res);
}

// ── Retention sweeper ─────────────────────────────────────────────────────────

const RETENTION_TTL_S = Number(
  process.env["REPORTING_RETENTION_TTL_S"] ?? 604_800
); // 7 days
const RETENTION_CHECK_MS = Number(
  process.env["REPORTING_RETENTION_CHECK_MS"] ?? 60_000
); // 1 minute

/**
 * Periodically purge completed (ready/failed) jobs older than the retention TTL.
 * Deletes both the DB record and the output file on disk.
 */
export function startRetentionSweeper(
  queue: ExportJobQueue,
  outputDir: string
): NodeJS.Timeout {
  const sweep = () => {
    const cutoff = Math.floor(Date.now() / 1000) - RETENTION_TTL_S;
    const jobs = queue.listJobs(10_000);
    let purged = 0;
    for (const job of jobs) {
      const doneAt = job.completedAt ?? job.requestedAt;
      if (
        (job.status === "ready" || job.status === "failed") &&
        doneAt < cutoff
      ) {
        // Delete output file if present
        if (job.outputPath && fs.existsSync(job.outputPath)) {
          try {
            fs.unlinkSync(job.outputPath);
          } catch (err) {
            logger.error({ err, jobId: job.jobId }, "Retention: failed to delete output file");
          }
        }
        queue.deleteJob(job.jobId);
        purged++;
      }
    }
    if (purged > 0) {
      logger.info({ purged, cutoff }, "Retention sweep completed");
    }
  };

  sweep(); // Run immediately on start
  return setInterval(sweep, RETENTION_CHECK_MS);
}

// ── HTTP server factory ───────────────────────────────────────────────────────

/**
 * Create and return the HTTP server.
 * Does not call listen() — caller decides port and binding.
 */
export function createReportingServer(
  queue: ExportJobQueue,
  outputDir: string
): http.Server {
  const server = http.createServer(async (rawReq, res) => {
    const req = rawReq as ReportingRequest;

    // Liveness check (no auth)
    if (req.method === "GET" && req.url === "/health") {
      return json(res, 200, { ok: true });
    }

    // Auth gate for all other routes
    if (!authenticate(req, res)) return;

    // Parse body for POST/PUT
    if (req.method === "POST" || req.method === "PUT") {
      try {
        req.body = await parseBody(req);
      } catch {
        return json(res, 400, { error: "Invalid JSON body" });
      }
    }

    const url = req.url ?? "/";

    // POST /exports
    if (req.method === "POST" && url === "/exports") {
      return handleEnqueue(queue, req, res);
    }

    // GET /exports/:id/download
    const downloadMatch = url.match(/^\/exports\/([^/]+)\/download$/);
    if (req.method === "GET" && downloadMatch) {
      return handleDownload(queue, req, res, downloadMatch[1]!);
    }

    // GET /exports/:id
    const jobMatch = url.match(/^\/exports\/([^/]+)$/);
    if (req.method === "GET" && jobMatch) {
      return handleGetJob(queue, req, res, jobMatch[1]!);
    }

    json(res, 404, { error: "Not found" });
  });

  return server;
}

/**
 * Bootstrap the reporting HTTP service from environment.
 * Exported for use by the main entry point.
 */
export async function startReportingServer(
  queue: ExportJobQueue,
  outputDir: string
): Promise<http.Server> {
  const port = Number(process.env["REPORTING_PORT"] ?? 3040);
  const host = process.env["REPORTING_HOST"] ?? "127.0.0.1";

  const server = createReportingServer(queue, outputDir);
  startRetentionSweeper(queue, outputDir);

  await new Promise<void>((resolve) =>
    server.listen(port, host, () => resolve())
  );

  logger.info({ port, host }, "Reporting HTTP server started");
  return server;
}
