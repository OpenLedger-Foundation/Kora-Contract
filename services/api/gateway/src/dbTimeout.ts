/**
 * Database timeout policy for API adapters.
 *
 * Closes #1095 — Chaos GAP-2: DB statement_timeout and 503 mapping.
 *
 * PostgreSQL SQLSTATE 57014 is emitted when statement_timeout cancels a
 * statement.  Drivers may also surface the same condition by message text, so
 * the mapper keeps the transport contract stable without exposing SQL text or
 * driver internals.
 *
 * Usage in a pool adapter:
 *
 *   // After acquiring a connection from the pool:
 *   await configureDbSession(connection, readDbStatementTimeout());
 *
 *   // In a request handler's catch block:
 *   const mapped = mapDbError(err);
 *   if (mapped) {
 *     res.setHeader("Retry-After", String(mapped.retryAfterSeconds));
 *     res.writeHead(mapped.status);
 *     res.end(JSON.stringify(mapped.body));
 *     return;
 *   }
 *   throw err; // re-throw non-timeout errors
 */

export const DEFAULT_DB_STATEMENT_TIMEOUT_MS = 5_000;
export const DB_TIMEOUT_CODE = "DB_TIMEOUT";

/** Minimal interface satisfied by pg, postgres.js, knex raw, etc. */
export interface SqlSession {
  query(sql: string): Promise<unknown>;
}

/** Shape of the structured HTTP error that transport adapters should emit. */
export interface HttpErrorBody {
  status: number;
  body: { code: string; message: string; retryable: boolean };
  retryAfterSeconds: number;
}

/**
 * Read and validate `DB_STATEMENT_TIMEOUT_MS` from the environment.
 *
 * Throws at startup if the value is zero, negative, or non-integer — this
 * surfaces misconfiguration before the first request rather than silently
 * running without a timeout.
 *
 * @param raw - raw string value (defaults to `process.env.DB_STATEMENT_TIMEOUT_MS`)
 */
export function readDbStatementTimeout(raw = process.env["DB_STATEMENT_TIMEOUT_MS"]): number {
  const value =
    raw === undefined || raw === ""
      ? DEFAULT_DB_STATEMENT_TIMEOUT_MS
      : Number(raw);
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(
      "DB_STATEMENT_TIMEOUT_MS must be a positive integer in milliseconds" +
        ` (got ${JSON.stringify(raw)})`,
    );
  }
  return value;
}

/**
 * Build the `SET statement_timeout = <ms>` command for a PostgreSQL session.
 *
 * The timeout fires SQLSTATE 57014 on any statement that exceeds it,
 * regardless of network or query complexity.  Pool adapters should issue this
 * once per acquired connection (or in a `beforeQuery` hook if the driver
 * supports it) so every statement in the request lifecycle is bounded.
 */
export function statementTimeoutSql(timeoutMs = readDbStatementTimeout()): string {
  if (!Number.isInteger(timeoutMs) || timeoutMs <= 0) {
    throw new Error("Database statement timeout must be a positive integer");
  }
  return `SET statement_timeout = ${timeoutMs}`;
}

/**
 * Run once after a pooled connection is acquired or during session init.
 *
 * ```ts
 * pool.on("connect", async (client) => {
 *   await configureDbSession(client, readDbStatementTimeout());
 * });
 * ```
 */
export async function configureDbSession(
  session: SqlSession,
  timeoutMs = readDbStatementTimeout(),
): Promise<void> {
  await session.query(statementTimeoutSql(timeoutMs));
}

/**
 * Returns true when the error is a PostgreSQL statement-timeout cancellation.
 *
 * Covers:
 * - SQLSTATE 57014  (`code === "57014"` — pg, postgres.js)
 * - message patterns from pg, pgbouncer, AWS Aurora Proxy, and common wrappers
 */
export function isDbTimeoutError(error: unknown): boolean {
  const candidate = error as { code?: string; message?: string } | null;
  if (!candidate) return false;
  if (candidate.code === "57014") return true;
  return /statement timeout|canceling statement due to user request|query timed out/i.test(
    candidate.message ?? "",
  );
}

/**
 * Convert a driver timeout error into the structured HTTP error contract.
 *
 * Returns `null` for non-timeout errors so callers can re-throw them.
 *
 * Response contract:
 * - HTTP 503 (Service Unavailable)
 * - `Retry-After: <seconds>` header using `retryAfterSeconds`
 * - Body: `{ code: "DB_TIMEOUT", message: "...", retryable: true }`
 */
export function mapDbError(error: unknown): HttpErrorBody | null {
  if (!isDbTimeoutError(error)) return null;
  const retryAfterSeconds = Math.max(1, Math.ceil(readDbStatementTimeout() / 1_000));
  return {
    status: 503,
    body: {
      code: DB_TIMEOUT_CODE,
      message: "Database operation timed out; retry the request",
      retryable: true,
    },
    retryAfterSeconds,
  };
}

/**
 * Error class thrown by the gateway when a DB statement timeout is detected.
 *
 * Transport adapters catch this and emit HTTP 503 + Retry-After, distinct from
 * the generic `ApiUnavailableError` so clients can apply targeted retry logic.
 *
 * ```ts
 * try {
 *   return gateway.queryInvoices(filter, page, opts);
 * } catch (err) {
 *   if (err instanceof DbTimeoutError) {
 *     res.setHeader("Retry-After", String(err.retryAfterSeconds));
 *     res.status(503).json({ code: err.code, message: err.message, retryable: true });
 *     return;
 *   }
 *   throw err;
 * }
 * ```
 */
export class DbTimeoutError extends Error {
  readonly statusCode = 503;
  readonly code = DB_TIMEOUT_CODE;
  readonly retryable = true;
  readonly retryAfterSeconds: number;

  constructor(retryAfterSeconds: number, cause?: unknown) {
    super("Database operation timed out; retry the request");
    this.name = "DbTimeoutError";
    this.retryAfterSeconds = retryAfterSeconds;
    if (cause !== undefined) this.cause = cause;
  }
}
