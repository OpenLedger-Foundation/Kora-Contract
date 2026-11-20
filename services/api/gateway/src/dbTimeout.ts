/**
 * Database timeout policy for API adapters.
 *
 * PostgreSQL SQLSTATE 57014 is emitted when statement_timeout cancels a
 * statement. Drivers may also surface the same condition by message, so the
 * mapper keeps the transport contract stable without exposing SQL text.
 */

export const DEFAULT_DB_STATEMENT_TIMEOUT_MS = 5_000;
export const DB_TIMEOUT_CODE = "DB_TIMEOUT";

export interface SqlSession {
  query(sql: string): Promise<unknown>;
}

export interface HttpErrorBody {
  status: number;
  body: { code: string; message: string; retryable: boolean };
  retryAfterSeconds: number;
}

export function readDbStatementTimeout(raw = process.env.DB_STATEMENT_TIMEOUT_MS): number {
  const value = raw === undefined || raw === "" ? DEFAULT_DB_STATEMENT_TIMEOUT_MS : Number(raw);
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error("DB_STATEMENT_TIMEOUT_MS must be a positive integer in milliseconds");
  }
  return value;
}

export function statementTimeoutSql(timeoutMs = readDbStatementTimeout()): string {
  if (!Number.isInteger(timeoutMs) || timeoutMs <= 0) {
    throw new Error("Database statement timeout must be positive");
  }
  return `SET statement_timeout = ${timeoutMs}`;
}

/** Run once after a pooled connection is acquired or during session init. */
export async function configureDbSession(session: SqlSession, timeoutMs = readDbStatementTimeout()): Promise<void> {
  await session.query(statementTimeoutSql(timeoutMs));
}

export function isDbTimeoutError(error: unknown): boolean {
  const candidate = error as { code?: string; message?: string } | null;
  return candidate?.code === "57014" || /statement timeout|canceling statement due to user request|query timed out/i.test(candidate?.message ?? "");
}

/** Convert a driver timeout into the public API error contract. */
export function mapDbError(error: unknown): HttpErrorBody | null {
  if (!isDbTimeoutError(error)) return null;
  return {
    status: 503,
    body: { code: DB_TIMEOUT_CODE, message: "Database operation timed out; retry the request", retryable: true },
    retryAfterSeconds: Math.max(1, Math.ceil(readDbStatementTimeout() / 1_000)),
  };
}
