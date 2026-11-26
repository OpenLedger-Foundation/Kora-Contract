/**
 * Subscription authorization + rate limiter tests — issue #769 follow-up.
 *
 * Covers:
 *  - HMAC session verifier: valid token accepted, signature/structure/expiry rejected
 *  - Fail-closed construction (missing/short SESSION_SECRET)
 *  - SubscriptionAuthz.authorize fail-closed (no token, invalid, expired)
 *  - Topic authorization: permitted topics only; position-status-change scoped to wallet
 *  - Mid-stream expiry (isExpired)
 *  - readSessionSecret / readSubscriptionRateLimit env validation
 *  - SubscriptionRateLimiter: allow until budget, then 429; window reset; per-key isolation
 */

import {
  createHmacSessionVerifier,
  ForbiddenError,
  issueSessionToken,
  readSessionSecret,
  SubscriptionAuthz,
  UnauthorizedError,
} from "../authz";
import { SubscriptionRateLimiter, readSubscriptionRateLimit } from "../rateLimiter";
import crypto from "crypto";

const SECRET = "kora-test-session-secret-0123456789";

function makeAuthz(secret = SECRET): SubscriptionAuthz {
  return new SubscriptionAuthz(createHmacSessionVerifier(secret));
}

describe("createHmacSessionVerifier", () => {
  it("accepts a freshly issued, non-expired token", () => {
    const now = 1_000_000;
    const token = issueSessionToken("WALLET_A", now + 600, SECRET);
    const claims = createHmacSessionVerifier(SECRET)(token, now);
    expect(claims).toEqual({ address: "WALLET_A", expiresAt: now + 600 });
  });

  it("rejects a token signed with a different secret", () => {
    const now = 1_000_000;
    const token = issueSessionToken("WALLET_A", now + 600, SECRET);
    expect(() =>
      createHmacSessionVerifier("a-different-secret-1234567890")(token, now),
    ).toThrow(UnauthorizedError);
  });

  it("rejects an expired token", () => {
    const exp = 1_000_000;
    const token = issueSessionToken("WALLET_A", exp, SECRET);
    expect(() => createHmacSessionVerifier(SECRET)(token, exp + 1)).toThrow(
      "expired",
    );
  });

  it("rejects a token whose expiry equals now (boundary)", () => {
    const exp = 1_000_000;
    const token = issueSessionToken("WALLET_A", exp, SECRET);
    expect(() => createHmacSessionVerifier(SECRET)(token, exp)).toThrow(
      "expired",
    );
  });

  it("rejects a tampered payload", () => {
    const token = issueSessionToken("WALLET_A", 2_000_000, SECRET);
    const [payload, sig] = token.split(".");
    // Flip the address in the payload but keep the old signature.
    const tamperedPayload = Buffer.from(
      JSON.stringify({ address: "WALLET_B", exp: 2_000_000 }),
      "utf8",
    ).toString("base64url");
    expect(() =>
      createHmacSessionVerifier(SECRET)(`${tamperedPayload}.${sig}`, 1_000_000),
    ).toThrow(UnauthorizedError);
  });

  it("rejects malformed tokens (no separator, empty parts)", () => {
    const v = createHmacSessionVerifier(SECRET);
    expect(() => v("noseparator", 1_000_000)).toThrow(UnauthorizedError);
    expect(() => v(".sigonly", 1_000_000)).toThrow(UnauthorizedError);
    expect(() => v("payload.", 1_000_000)).toThrow(UnauthorizedError);
  });

  it("refuses to construct without a sufficiently long secret (fail closed)", () => {
    expect(() => createHmacSessionVerifier("")).toThrow("SESSION_SECRET");
    expect(() => createHmacSessionVerifier("short")).toThrow("SESSION_SECRET");
  });

  it("rejects a token with a valid signature but non-JSON payload", () => {
    const payload = Buffer.from("not-json", "utf8").toString("base64url");
    const sig = crypto
      .createHmac("sha256", SECRET)
      .update(payload)
      .digest("base64url");
    expect(() =>
      createHmacSessionVerifier(SECRET)(`${payload}.${sig}`, 1_000_000),
    ).toThrow("Malformed session token payload");
  });

  it("rejects a token with a valid signature but missing required claims", () => {
    const payload = Buffer.from(JSON.stringify({}), "utf8").toString(
      "base64url",
    );
    const sig = crypto
      .createHmac("sha256", SECRET)
      .update(payload)
      .digest("base64url");
    expect(() =>
      createHmacSessionVerifier(SECRET)(`${payload}.${sig}`, 1_000_000),
    ).toThrow("missing required claims");
  });
});

describe("SubscriptionAuthz.authorize", () => {
  it("returns claims for a valid token", () => {
    const authz = makeAuthz();
    const now = 1_000_000;
    const token = issueSessionToken("WALLET_A", now + 600, SECRET);
    expect(authz.authorize(token, now)).toEqual({
      address: "WALLET_A",
      expiresAt: now + 600,
    });
  });

  it("throws UnauthorizedError when no token is presented", () => {
    const authz = makeAuthz();
    expect(() => authz.authorize(undefined, 1_000_000)).toThrow(
      UnauthorizedError,
    );
    expect(() => authz.authorize("", 1_000_000)).toThrow(UnauthorizedError);
  });

  it("throws UnauthorizedError for an expired token", () => {
    const authz = makeAuthz();
    const exp = 1_000_000;
    const token = issueSessionToken("WALLET_A", exp, SECRET);
    expect(() => authz.authorize(token, exp + 1)).toThrow("expired");
  });

  it("exposes statusCode 401 on UnauthorizedError", () => {
    const authz = makeAuthz();
    try {
      authz.authorize(undefined, 1_000_000);
      fail("expected throw");
    } catch (err) {
      expect((err as UnauthorizedError).statusCode).toBe(401);
    }
  });
});

describe("SubscriptionAuthz.canSubscribe", () => {
  const authz = makeAuthz();
  const claims = { address: "WALLET_A", expiresAt: 2_000_000 };

  it("permits listing-funding-progress and repayment to any authenticated wallet", () => {
    expect(() =>
      authz.canSubscribe(claims, "listing-funding-progress", {}),
    ).not.toThrow();
    expect(() => authz.canSubscribe(claims, "repayment", {})).not.toThrow();
  });

  it("permits position-status-change with no explicit investor filter (auto-scoped)", () => {
    expect(() =>
      authz.canSubscribe(claims, "position-status-change", {}),
    ).not.toThrow();
  });

  it("permits position-status-change scoped to the authenticated wallet", () => {
    expect(() =>
      authz.canSubscribe(claims, "position-status-change", {
        investorAddress: "WALLET_A",
      }),
    ).not.toThrow();
  });

  it("forbids subscribing to another investor's position stream", () => {
    expect(() =>
      authz.canSubscribe(claims, "position-status-change", {
        investorAddress: "WALLET_B",
      }),
    ).toThrow(ForbiddenError);
  });

  it("forbids unknown topics", () => {
    expect(() => authz.canSubscribe(claims, "unknown_topic", {})).toThrow(
      ForbiddenError,
    );
    expect(() => authz.canSubscribe(claims, "", {})).toThrow(ForbiddenError);
  });

  it("exposes statusCode 403 on ForbiddenError", () => {
    try {
      authz.canSubscribe(claims, "unknown_topic", {});
      fail("expected throw");
    } catch (err) {
      expect((err as ForbiddenError).statusCode).toBe(403);
    }
  });
});

describe("SubscriptionAuthz.isExpired (mid-stream eviction)", () => {
  it("returns false before expiry and true at/after expiry", () => {
    const authz = makeAuthz();
    const claims = { address: "WALLET_A", expiresAt: 1_000_000 };
    expect(authz.isExpired(claims, 999_999)).toBe(false);
    expect(authz.isExpired(claims, 1_000_000)).toBe(true);
    expect(authz.isExpired(claims, 1_000_001)).toBe(true);
  });
});

describe("readSessionSecret", () => {
  afterEach(() => {
    delete process.env.SESSION_SECRET;
  });

  it("returns the env value when set", () => {
    process.env.SESSION_SECRET = "env-secret-0123456789abcdef";
    expect(readSessionSecret()).toBe("env-secret-0123456789abcdef");
  });

  it("throws when unset (fail closed)", () => {
    delete process.env.SESSION_SECRET;
    expect(() => readSessionSecret()).toThrow("SESSION_SECRET");
  });

  it("throws when empty", () => {
    process.env.SESSION_SECRET = "";
    expect(() => readSessionSecret()).toThrow("SESSION_SECRET");
  });
});

describe("SubscriptionRateLimiter", () => {
  it("allows subscriptions up to the budget then blocks", () => {
    let now = 0;
    const limiter = new SubscriptionRateLimiter({
      maxRequestsPerWindow: 3,
      windowMs: 60_000,
      now: () => now,
    });
    expect(limiter.consume("WALLET_A").allowed).toBe(true);
    expect(limiter.consume("WALLET_A").allowed).toBe(true);
    expect(limiter.consume("WALLET_A").allowed).toBe(true);
    const blocked = limiter.consume("WALLET_A");
    expect(blocked.allowed).toBe(false);
    expect(blocked.remaining).toBe(0);
    expect(blocked.resetAfterMs).toBeGreaterThan(0);
  });

  it("resets the budget after the window elapses", () => {
    let now = 0;
    const limiter = new SubscriptionRateLimiter({
      maxRequestsPerWindow: 1,
      windowMs: 1_000,
      now: () => now,
    });
    expect(limiter.consume("WALLET_A").allowed).toBe(true);
    expect(limiter.consume("WALLET_A").allowed).toBe(false);
    now += 1_000;
    expect(limiter.consume("WALLET_A").allowed).toBe(true);
  });

  it("isolates budgets per wallet", () => {
    let now = 0;
    const limiter = new SubscriptionRateLimiter({
      maxRequestsPerWindow: 1,
      now: () => now,
    });
    expect(limiter.consume("WALLET_A").allowed).toBe(true);
    expect(limiter.consume("WALLET_A").allowed).toBe(false);
    expect(limiter.consume("WALLET_B").allowed).toBe(true);
  });

  it("reset() clears a wallet's bucket", () => {
    let now = 0;
    const limiter = new SubscriptionRateLimiter({
      maxRequestsPerWindow: 1,
      now: () => now,
    });
    expect(limiter.consume("WALLET_A").allowed).toBe(true);
    expect(limiter.consume("WALLET_A").allowed).toBe(false);
    limiter.reset("WALLET_A");
    expect(limiter.consume("WALLET_A").allowed).toBe(true);
  });

  it("counts down remaining", () => {
    let now = 0;
    const limiter = new SubscriptionRateLimiter({
      maxRequestsPerWindow: 3,
      now: () => now,
    });
    expect(limiter.consume("W").remaining).toBe(2);
    expect(limiter.consume("W").remaining).toBe(1);
    expect(limiter.consume("W").remaining).toBe(0);
  });

  it("uses the default budget/window when constructed without options", () => {
    // Exercises the `?? 30` / `?? 60_000` / `?? Date.now` default branches.
    const limiter = new SubscriptionRateLimiter();
    for (let i = 0; i < 30; i++) {
      expect(limiter.consume("WALLET_DEFAULT").allowed).toBe(true);
    }
    expect(limiter.consume("WALLET_DEFAULT").allowed).toBe(false);
  });
});

describe("readSubscriptionRateLimit", () => {
  afterEach(() => {
    delete process.env.SUBSCRIPTION_RATE_LIMIT_PER_MIN;
  });

  it("defaults to 30 when unset", () => {
    delete process.env.SUBSCRIPTION_RATE_LIMIT_PER_MIN;
    expect(readSubscriptionRateLimit()).toBe(30);
  });

  it("returns the configured value", () => {
    process.env.SUBSCRIPTION_RATE_LIMIT_PER_MIN = "120";
    expect(readSubscriptionRateLimit()).toBe(120);
  });

  it("throws on non-positive or non-integer values", () => {
    process.env.SUBSCRIPTION_RATE_LIMIT_PER_MIN = "0";
    expect(() => readSubscriptionRateLimit()).toThrow("positive integer");
    process.env.SUBSCRIPTION_RATE_LIMIT_PER_MIN = "-5";
    expect(() => readSubscriptionRateLimit()).toThrow("positive integer");
    process.env.SUBSCRIPTION_RATE_LIMIT_PER_MIN = "1.5";
    expect(() => readSubscriptionRateLimit()).toThrow("positive integer");
  });
});
