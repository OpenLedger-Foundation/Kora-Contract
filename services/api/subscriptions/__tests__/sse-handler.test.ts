/**
 * SSE handler authorization tests — issue #769 follow-up.
 *
 * Covers the acceptance criteria:
 *  - Unauthorized (no/invalid/expired token) cannot subscribe → 401
 *  - Authorized receives only permitted topics → 403 for unknown / cross-investor
 *  - Per-wallet rate limit → 429 with Retry-After
 *  - Authorized, permitted subscription streams events to the authenticated wallet
 *  - Edge: expired session mid-stream evicts the client with a session_expired event
 */

import { EventEmitter } from "events";
import { handleSseConnection, extractSessionToken } from "../sse-handler";
import type { SseHandlerOptions } from "../sse-handler";
import { EventBus } from "../event-bus";
import {
  createHmacSessionVerifier,
  issueSessionToken,
  SubscriptionAuthz,
} from "../authz";
import { SubscriptionRateLimiter } from "../rateLimiter";
import type { IncomingMessage, ServerResponse } from "http";
import type { EventEnvelope } from "../types";

const SECRET = "kora-test-session-secret-0123456789";

// ── Mocks ─────────────────────────────────────────────────────────────────────

interface MockRes {
  statusCode?: number;
  headers: Record<string, string>;
  chunks: string[];
  ended: boolean;
  body(): string;
}

function mockReq(urlPath: string, headers: Record<string, string> = {}): IncomingMessage {
  const req = new EventEmitter() as IncomingMessage;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (req as any).url = urlPath;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (req as any).headers = headers;
  return req;
}

function mockRes(): ServerResponse & MockRes {
  const chunks: string[] = [];
  const headers: Record<string, string> = {};
  const res = {
    statusCode: undefined as number | undefined,
    headers,
    chunks,
    ended: false,
    setHeader(k: string, v: string) {
      headers[k] = String(v);
    },
    getHeader(k: string) {
      return headers[k];
    },
    flushHeaders() {},
    write(c: string) {
      chunks.push(c);
      return true;
    },
    writeHead(status: number, h?: Record<string, string>) {
      res.statusCode = status;
      if (h) for (const k in h) headers[k] = String(h[k]);
    },
    end(c?: string) {
      if (c !== undefined) chunks.push(c);
      res.ended = true;
    },
    body() {
      return chunks.join("");
    },
  };
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return res as any;
}

// ── Setup ─────────────────────────────────────────────────────────────────────

let subscribeSpy: jest.SpyInstance;

beforeEach(() => {
  jest.useFakeTimers();
  subscribeSpy = jest
    .spyOn(EventBus.instance, "subscribe")
    .mockImplementation(() => "sub_test");
  jest.spyOn(EventBus.instance, "unsubscribe").mockImplementation(() => {});
});

afterEach(() => {
  subscribeSpy.mockRestore();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (EventBus.instance as any).unsubscribe.mockRestore?.();
  jest.useRealTimers();
});

function makeOptions(clockMs = 1_999_000_000, rateMax = 100): { clock: { ms: number }; opts: SseHandlerOptions } {
  const clock = { ms: clockMs };
  const authz = new SubscriptionAuthz(createHmacSessionVerifier(SECRET));
  const rateLimiter = new SubscriptionRateLimiter({
    maxRequestsPerWindow: rateMax,
    now: () => clock.ms,
  });
  const opts: SseHandlerOptions = {
    authz,
    rateLimiter,
    now: () => clock.ms,
  };
  return { clock, opts };
}

function tokenFor(addr: string, expSec: number): string {
  return issueSessionToken(addr, expSec, SECRET);
}

const EXP_SEC = 2_000_000; // expiry in seconds
const NOW_MS = 1_999_000_000; // before expiry

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("extractSessionToken", () => {
  it("reads a Bearer token from the Authorization header", () => {
    const req = mockReq("/sse", { authorization: "Bearer abc.def" });
    expect(extractSessionToken(req, new URL("/sse", "http://localhost"))).toBe(
      "abc.def",
    );
  });
  it("falls back to the ?session= query param", () => {
    const req = mockReq("/sse?session=xyz.uvw");
    expect(extractSessionToken(req, new URL("/sse?session=xyz.uvw", "http://localhost"))).toBe(
      "xyz.uvw",
    );
  });
  it("returns undefined when neither is present", () => {
    const req = mockReq("/sse");
    expect(
      extractSessionToken(req, new URL("/sse", "http://localhost")),
    ).toBeUndefined();
  });
  it("ignores a non-Bearer Authorization header and falls back to the query param", () => {
    const req = mockReq("/sse?session=xyz.uvw", { authorization: "Basic abc" });
    expect(
      extractSessionToken(req, new URL("/sse?session=xyz.uvw", "http://localhost")),
    ).toBe("xyz.uvw");
  });
});

describe("handleSseConnection — unauthorized cannot subscribe", () => {
  it("rejects a missing token with 401", () => {
    const { opts } = makeOptions();
    const req = mockReq("/sse?topic=repayment");
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBe(401);
    expect(res.body()).toContain("UNAUTHORIZED");
    expect(subscribeSpy).not.toHaveBeenCalled();
  });

  it("rejects an invalid (bad-signature) token with 401", () => {
    const { opts } = makeOptions();
    const req = mockReq("/sse?topic=repayment&session=notvalid.notvalidsig");
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBe(401);
  });

  it("rejects an expired token with 401", () => {
    const { opts, clock } = makeOptions();
    // Issue a token already expired relative to the clock.
    const expiredToken = tokenFor("WALLET_A", 1_000_000);
    clock.ms = 2_000_001_000; // past expiry
    const req = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(expiredToken)}`);
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBe(401);
  });
});

describe("handleSseConnection — authorized receives only permitted topics", () => {
  it("rejects an unknown topic with 403", () => {
    const { opts } = makeOptions();
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(`/sse?topic=bogus&session=${encodeURIComponent(t)}`);
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBe(403);
    expect(res.body()).toContain("FORBIDDEN");
    expect(subscribeSpy).not.toHaveBeenCalled();
  });

  it("rejects subscribing to another investor's position stream with 403", () => {
    const { opts } = makeOptions();
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(
      `/sse?topic=position-status-change&investorAddress=WALLET_B&session=${encodeURIComponent(t)}`,
    );
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBe(403);
    expect(res.body()).toContain("another investor");
    expect(subscribeSpy).not.toHaveBeenCalled();
  });

  it("accepts a permitted topic for an authenticated wallet and subscribes", () => {
    const { opts } = makeOptions();
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(t)}`);
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBeUndefined(); // 200 implied by flushHeaders
    expect(res.headers["Content-Type"]).toBe("text/event-stream");
    expect(subscribeSpy).toHaveBeenCalledTimes(1);
    // The authorized wallet address (not a caller-supplied one) is passed through.
    expect(subscribeSpy.mock.calls[0][2]).toBe("WALLET_A");
    // Clean up the keep-alive interval.
    req.emit("close");
  });

  it("accepts position-status-change scoped to the authenticated wallet", () => {
    const { opts } = makeOptions();
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(
      `/sse?topic=position-status-change&investorAddress=WALLET_A&session=${encodeURIComponent(t)}`,
    );
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBeUndefined();
    expect(subscribeSpy).toHaveBeenCalledTimes(1);
    req.emit("close");
  });

  it("returns 403 when the EventBus itself rejects the subscription", () => {
    const { opts } = makeOptions();
    subscribeSpy.mockImplementation(() => {
      throw new Error("Cannot subscribe to another investor's position updates");
    });
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(
      `/sse?topic=position-status-change&session=${encodeURIComponent(t)}`,
    );
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBe(403);
    expect(res.body()).toContain("FORBIDDEN");
  });

  it("returns a generic FORBIDDEN when the EventBus throws a non-Error", () => {
    const { opts } = makeOptions();
    subscribeSpy.mockImplementation(() => {
      throw "string error";
    });
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(t)}`);
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(res.statusCode).toBe(403);
    expect(res.body()).toContain("Subscription error");
  });

  it("passes the invoiceId filter parsed from the query string", () => {
    const { opts } = makeOptions();
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(
      `/sse?topic=repayment&invoiceId=5&session=${encodeURIComponent(t)}`,
    );
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(subscribeSpy).toHaveBeenCalledTimes(1);
    expect(subscribeSpy.mock.calls[0][1]).toEqual({ invoiceId: 5n });
    req.emit("close");
  });

  it("streams delivered events to the authorized subscriber", () => {
    const { opts } = makeOptions();
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(t)}`);
    const res = mockRes();
    handleSseConnection(req, res, opts);

    // The subscribe callback is the 4th argument.
    const send = subscribeSpy.mock.calls[0][3] as (env: EventEnvelope) => void;
    const env: EventEnvelope = {
      cursor: "Y3Vyc29y",
      event: {
        topic: "repayment",
        invoiceId: 42n,
        payer: "SME_A",
        amount: 1000n,
        ledgerTimestamp: 123,
      },
    };
    send(env);
    expect(res.body()).toContain("id: Y3Vyc29y");
    expect(res.body()).toContain('"payer":"SME_A"');
    // bigint serialized as string
    expect(res.body()).toContain('"amount":"1000"');
    req.emit("close");
  });
});

describe("handleSseConnection — per-wallet rate limit", () => {
  it("returns 429 with Retry-After once the wallet exceeds its budget", () => {
    const { opts } = makeOptions(NOW_MS, 1);
    const t = tokenFor("WALLET_A", EXP_SEC);
    // First connection consumes the single-slot budget.
    const req1 = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(t)}`);
    handleSseConnection(req1, mockRes(), opts);
    expect(subscribeSpy).toHaveBeenCalledTimes(1);
    // Second connection is rate-limited.
    const req2 = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(t)}`);
    const res2 = mockRes();
    handleSseConnection(req2, res2, opts);
    expect(res2.statusCode).toBe(429);
    expect(res2.headers["Retry-After"]).toBeDefined();
    expect(res2.body()).toContain("RATE_LIMITED");
    expect(subscribeSpy).toHaveBeenCalledTimes(1); // not subscribed
    req1.emit("close");
  });

  it("does not rate-limit a different wallet", () => {
    const { opts } = makeOptions(NOW_MS, 1);
    const tA = tokenFor("WALLET_A", EXP_SEC);
    const tB = tokenFor("WALLET_B", EXP_SEC);
    const reqA = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(tA)}`);
    handleSseConnection(reqA, mockRes(), opts);
    const reqB = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(tB)}`);
    handleSseConnection(reqB, mockRes(), opts);
    expect(subscribeSpy).toHaveBeenCalledTimes(2);
    reqA.emit("close");
    reqB.emit("close");
  });
});

describe("handleSseConnection — expired session mid-stream", () => {
  it("evicts the stream with a session_expired event once the session expires", () => {
    const { opts, clock } = makeOptions();
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(t)}`);
    const res = mockRes();
    handleSseConnection(req, res, opts);
    expect(subscribeSpy).toHaveBeenCalledTimes(1);

    // Advance the clock past expiry, then fire the keep-alive ping.
    clock.ms = (EXP_SEC + 1) * 1000;
    jest.advanceTimersByTime(15_000);

    expect(res.ended).toBe(true);
    expect(res.body()).toContain("session_expired");
    expect(res.body()).toContain("re-authenticate");
    expect(EventBus.instance.unsubscribe).toHaveBeenCalledWith("sub_test");
  });

  it("closes the stream as degraded when the indexer health probe fails", () => {
    const { opts } = makeOptions();
    opts.health = () => false;
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(t)}`);
    const res = mockRes();
    handleSseConnection(req, res, opts);
    jest.advanceTimersByTime(15_000);
    expect(res.ended).toBe(true);
    expect(res.body()).toContain("degraded");
  });

  it("sends a keep-alive comment when healthy and not expired", () => {
    const { opts } = makeOptions();
    opts.health = () => true;
    const t = tokenFor("WALLET_A", EXP_SEC);
    const req = mockReq(`/sse?topic=repayment&session=${encodeURIComponent(t)}`);
    const res = mockRes();
    handleSseConnection(req, res, opts);
    jest.advanceTimersByTime(15_000);
    expect(res.body()).toContain(": ping");
    expect(res.ended).toBe(false);
    req.emit("close");
  });
});
