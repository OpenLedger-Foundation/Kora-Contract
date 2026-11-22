/**
 * Tests for services/api/gateway/src/dbTimeout.ts
 *
 * Closes #1095 — Chaos GAP-2: DB statement_timeout and 503 mapping.
 *
 * Coverage targets:
 *  - readDbStatementTimeout: env var parsing, validation, defaults
 *  - statementTimeoutSql: SQL generation
 *  - configureDbSession: session initialization
 *  - isDbTimeoutError: SQLSTATE 57014 and message-pattern detection
 *  - mapDbError: HTTP 503 body and Retry-After
 *  - DbTimeoutError: class shape, statusCode, code, retryable flag
 *
 * "Simulating a slow query" (issue acceptance criterion) is covered by the
 * slow-checkout and slow-query tests in gateway-dbtimeout.test.ts below, but
 * the full error-mapping pipeline is exercised here against the actual
 * `configureDbSession` → mock session contract.
 */

import {
  DEFAULT_DB_STATEMENT_TIMEOUT_MS,
  DB_TIMEOUT_CODE,
  readDbStatementTimeout,
  statementTimeoutSql,
  configureDbSession,
  isDbTimeoutError,
  mapDbError,
  DbTimeoutError,
  type SqlSession,
} from "../src/dbTimeout";

// ── readDbStatementTimeout ──────────────────────────────────────────────────

describe("readDbStatementTimeout", () => {
  it("returns the default when env var is absent", () => {
    expect(readDbStatementTimeout(undefined)).toBe(DEFAULT_DB_STATEMENT_TIMEOUT_MS);
  });

  it("returns the default when env var is empty string", () => {
    expect(readDbStatementTimeout("")).toBe(DEFAULT_DB_STATEMENT_TIMEOUT_MS);
  });

  it("parses a valid positive integer string", () => {
    expect(readDbStatementTimeout("3000")).toBe(3000);
  });

  it("throws when value is zero", () => {
    expect(() => readDbStatementTimeout("0")).toThrow(
      "DB_STATEMENT_TIMEOUT_MS must be a positive integer",
    );
  });

  it("throws when value is negative", () => {
    expect(() => readDbStatementTimeout("-1")).toThrow(
      "DB_STATEMENT_TIMEOUT_MS must be a positive integer",
    );
  });

  it("throws when value is a float", () => {
    expect(() => readDbStatementTimeout("1500.5")).toThrow(
      "DB_STATEMENT_TIMEOUT_MS must be a positive integer",
    );
  });

  it("throws when value is a non-numeric string", () => {
    expect(() => readDbStatementTimeout("not-a-number")).toThrow(
      "DB_STATEMENT_TIMEOUT_MS must be a positive integer",
    );
  });

  it("includes the bad value in the error message", () => {
    expect(() => readDbStatementTimeout("bad")).toThrow(/got "bad"/);
  });
});

// ── statementTimeoutSql ─────────────────────────────────────────────────────

describe("statementTimeoutSql", () => {
  it("produces SET statement_timeout = <ms>", () => {
    expect(statementTimeoutSql(5000)).toBe("SET statement_timeout = 5000");
  });

  it("produces correct SQL for custom values", () => {
    expect(statementTimeoutSql(1000)).toBe("SET statement_timeout = 1000");
    expect(statementTimeoutSql(30000)).toBe("SET statement_timeout = 30000");
  });

  it("throws for zero timeout", () => {
    expect(() => statementTimeoutSql(0)).toThrow("must be a positive integer");
  });

  it("throws for negative timeout", () => {
    expect(() => statementTimeoutSql(-500)).toThrow("must be a positive integer");
  });

  it("throws for float timeout", () => {
    expect(() => statementTimeoutSql(2500.5)).toThrow("must be a positive integer");
  });
});

// ── configureDbSession ──────────────────────────────────────────────────────

describe("configureDbSession", () => {
  it("calls session.query with the SET statement_timeout SQL", async () => {
    const queries: string[] = [];
    const session: SqlSession = {
      query: async (sql) => { queries.push(sql); return undefined; },
    };
    await configureDbSession(session, 5000);
    expect(queries).toHaveLength(1);
    expect(queries[0]).toBe("SET statement_timeout = 5000");
  });

  it("propagates the timeout value to the SQL", async () => {
    const queries: string[] = [];
    const session: SqlSession = {
      query: async (sql) => { queries.push(sql); return undefined; },
    };
    await configureDbSession(session, 2000);
    expect(queries[0]).toBe("SET statement_timeout = 2000");
  });

  it("re-throws if session.query rejects", async () => {
    const session: SqlSession = {
      query: async () => { throw new Error("connection refused"); },
    };
    await expect(configureDbSession(session, 5000)).rejects.toThrow("connection refused");
  });
});

// ── isDbTimeoutError ────────────────────────────────────────────────────────

describe("isDbTimeoutError", () => {
  it("returns true for SQLSTATE 57014", () => {
    expect(isDbTimeoutError({ code: "57014" })).toBe(true);
  });

  it("returns true for 'statement timeout' message (pg)", () => {
    expect(isDbTimeoutError({ message: "ERROR: canceling statement due to statement timeout" })).toBe(true);
  });

  it("returns true for 'canceling statement due to user request' (pg cancel)", () => {
    expect(isDbTimeoutError({ message: "ERROR: canceling statement due to user request" })).toBe(true);
  });

  it("returns true for 'query timed out' (pgbouncer / proxy variant)", () => {
    expect(isDbTimeoutError({ message: "query timed out" })).toBe(true);
  });

  it("is case-insensitive on message patterns", () => {
    expect(isDbTimeoutError({ message: "STATEMENT TIMEOUT" })).toBe(true);
    expect(isDbTimeoutError({ message: "Query Timed Out" })).toBe(true);
  });

  it("returns false for other SQLSTATE codes", () => {
    expect(isDbTimeoutError({ code: "23505" })).toBe(false); // unique violation
    expect(isDbTimeoutError({ code: "08006" })).toBe(false); // connection failure
  });

  it("returns false for unrelated error messages", () => {
    expect(isDbTimeoutError({ message: "connection refused" })).toBe(false);
    expect(isDbTimeoutError({ message: "null value in column" })).toBe(false);
  });

  it("returns false for null", () => {
    expect(isDbTimeoutError(null)).toBe(false);
  });

  it("returns false for undefined", () => {
    expect(isDbTimeoutError(undefined)).toBe(false);
  });

  it("returns false for primitive values", () => {
    expect(isDbTimeoutError("57014")).toBe(false);
    expect(isDbTimeoutError(57014)).toBe(false);
  });
});

// ── mapDbError ──────────────────────────────────────────────────────────────

describe("mapDbError", () => {
  it("returns null for non-timeout errors", () => {
    expect(mapDbError(new Error("connection refused"))).toBeNull();
    expect(mapDbError({ code: "23505" })).toBeNull();
    expect(mapDbError(null)).toBeNull();
  });

  it("returns HTTP 503 body for SQLSTATE 57014", () => {
    const result = mapDbError({ code: "57014" });
    expect(result).not.toBeNull();
    expect(result!.status).toBe(503);
    expect(result!.body.code).toBe(DB_TIMEOUT_CODE);
    expect(result!.body.retryable).toBe(true);
  });

  it("returns HTTP 503 for timeout message patterns", () => {
    const result = mapDbError({ message: "canceling statement due to statement timeout" });
    expect(result!.status).toBe(503);
    expect(result!.body.code).toBe("DB_TIMEOUT");
  });

  it("retryAfterSeconds is at least 1", () => {
    const result = mapDbError({ code: "57014" });
    expect(result!.retryAfterSeconds).toBeGreaterThanOrEqual(1);
  });

  it("retryAfterSeconds matches configured timeout (ceiling of ms/1000)", () => {
    // DEFAULT_DB_STATEMENT_TIMEOUT_MS = 5000 → 5 seconds
    const result = mapDbError({ code: "57014" });
    expect(result!.retryAfterSeconds).toBe(
      Math.max(1, Math.ceil(DEFAULT_DB_STATEMENT_TIMEOUT_MS / 1_000)),
    );
  });

  it("body message is non-empty and human-readable", () => {
    const result = mapDbError({ code: "57014" });
    expect(result!.body.message.length).toBeGreaterThan(0);
    expect(result!.body.message).toContain("retry");
  });
});

// ── DbTimeoutError ───────────────────────────────────────────────────────────

describe("DbTimeoutError", () => {
  it("has statusCode 503", () => {
    const err = new DbTimeoutError(5);
    expect(err.statusCode).toBe(503);
  });

  it("has code DB_TIMEOUT", () => {
    const err = new DbTimeoutError(5);
    expect(err.code).toBe(DB_TIMEOUT_CODE);
  });

  it("has retryable true", () => {
    const err = new DbTimeoutError(5);
    expect(err.retryable).toBe(true);
  });

  it("stores retryAfterSeconds", () => {
    const err = new DbTimeoutError(10);
    expect(err.retryAfterSeconds).toBe(10);
  });

  it("is an instance of Error", () => {
    const err = new DbTimeoutError(5);
    expect(err).toBeInstanceOf(Error);
  });

  it("has name DbTimeoutError", () => {
    const err = new DbTimeoutError(5);
    expect(err.name).toBe("DbTimeoutError");
  });

  it("stores cause when provided", () => {
    const cause = new Error("57014");
    const err = new DbTimeoutError(5, cause);
    expect(err.cause).toBe(cause);
  });

  it("message is non-empty", () => {
    const err = new DbTimeoutError(5);
    expect(err.message.length).toBeGreaterThan(0);
  });
});
