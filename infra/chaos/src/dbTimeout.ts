export const DEFAULT_DB_STATEMENT_TIMEOUT_MS = 5_000;

export function positiveMilliseconds(key: string, fallback: string): number {
  const raw = process.env[key] ?? fallback;
  const value = Number(raw);
  if (!Number.isInteger(value) || value <= 0) throw new Error(`${key} must be a positive integer in milliseconds`);
  return value;
}

export const dbStatementTimeoutMs = positiveMilliseconds("DB_STATEMENT_TIMEOUT_MS", String(DEFAULT_DB_STATEMENT_TIMEOUT_MS));
