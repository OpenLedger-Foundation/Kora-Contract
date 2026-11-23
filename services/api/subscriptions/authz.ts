/**
 * Subscription authorization — wallet/session gate for SSE invoice event streams.
 *
 * Every subscription must present a valid session token (issued by the wallet-login
 * flow) before any event is streamed. The handler fails closed: missing, malformed,
 * or expired tokens are rejected with HTTP 401. Topic authorization and per-wallet
 * rate limiting (see `rateLimiter.ts`) are enforced here so the EventBus stays a
 * pure fan-out primitive.
 *
 * Issue: #769 — authorize invoice event subscriptions.
 */

import crypto from "crypto";
import { SubscriptionFilter, SubscriptionTopic } from "./types";

/** Verified identity of a connected subscriber. */
export interface SessionClaims {
  /** Wallet address the caller authenticated as. */
  address: string;
  /** Unix seconds at which the session expires. */
  expiresAt: number;
}

/** HTTP 401 — no valid session was presented. Fail-closed. */
export class UnauthorizedError extends Error {
  readonly statusCode = 401;
  readonly code = "UNAUTHORIZED";
  constructor(message = "A valid session token is required") {
    super(message);
    this.name = "UnauthorizedError";
  }
}

/** HTTP 403 — the session is valid but the requested topic/filter is not permitted. */
export class ForbiddenError extends Error {
  readonly statusCode = 403;
  readonly code = "FORBIDDEN";
  constructor(message = "Topic not permitted for this session") {
    super(message);
    this.name = "ForbiddenError";
  }
}

/** Topics an authenticated session may subscribe to. */
export const PERMITTED_TOPICS: ReadonlySet<SubscriptionTopic> = new Set([
  "listing-funding-progress",
  "position-status-change",
  "repayment",
]);

/** A function that turns a raw token into SessionClaims, or throws UnauthorizedError. */
export type SessionVerifier = (token: string, nowSec: number) => SessionClaims;

/**
 * Default HMAC-SHA256 session verifier.
 *
 * Token format: `<payloadB64url>.<sigB64url>` where
 *   payload = base64url(JSON({ address, exp }))
 *   sig     = HMAC_SHA256(payload, SESSION_SECRET)
 *
 * Fail-closed: a missing/short SESSION_SECRET refuses to construct (the service
 * must not start in a state where sessions cannot be validated). Expired or
 * bad-signature tokens throw UnauthorizedError.
 */
export function createHmacSessionVerifier(secret: string): SessionVerifier {
  if (!secret || secret.length < 16) {
    throw new Error(
      "SESSION_SECRET must be a non-empty string of at least 16 characters",
    );
  }
  return (token: string, nowSec: number): SessionClaims => {
    const sep = token.lastIndexOf(".");
    if (sep <= 0 || sep === token.length - 1) {
      throw new UnauthorizedError("Malformed session token");
    }
    const payload = token.slice(0, sep);
    const sig = token.slice(sep + 1);

    const expected = crypto
      .createHmac("sha256", secret)
      .update(payload)
      .digest("base64url");

    const sigBuf = Buffer.from(sig);
    const expBuf = Buffer.from(expected);
    if (
      sigBuf.length !== expBuf.length ||
      !crypto.timingSafeEqual(sigBuf, expBuf)
    ) {
      throw new UnauthorizedError("Invalid session token signature");
    }

    let decoded: { address?: string; exp?: number };
    try {
      decoded = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
    } catch {
      throw new UnauthorizedError("Malformed session token payload");
    }
    if (!decoded.address || typeof decoded.exp !== "number") {
      throw new UnauthorizedError("Session token missing required claims");
    }
    if (nowSec >= decoded.exp) {
      throw new UnauthorizedError("Session token has expired");
    }
    return { address: decoded.address, expiresAt: decoded.exp };
  };
}

/**
 * Read SESSION_SECRET from the environment, validating at startup (fail closed).
 * Throws when unset so the service never starts unable to validate sessions.
 */
export function readSessionSecret(raw = process.env["SESSION_SECRET"]): string {
  if (raw === undefined || raw === "") {
    throw new Error("SESSION_SECRET environment variable is required");
  }
  return raw;
}

/**
 * Authorization gate for the subscription service. Holds a session verifier and
 * the topic-authorization policy. All checks fail closed.
 */
export class SubscriptionAuthz {
  constructor(private readonly verifier: SessionVerifier) {}

  /**
   * Validate the session token and return claims. Throws UnauthorizedError for
   * missing/invalid/expired tokens. `nowSec` is in unix seconds.
   */
  authorize(token: string | undefined, nowSec: number): SessionClaims {
    if (!token) {
      throw new UnauthorizedError("A valid session token is required");
    }
    return this.verifier(token, nowSec);
  }

  /**
   * Topic authorization: the session may only subscribe to permitted topics, and
   * `position-status-change` is scoped to the authenticated wallet — a caller may
   * not request another investor's position stream.
   */
  canSubscribe(
    claims: SessionClaims,
    topic: string,
    filter: SubscriptionFilter,
  ): void {
    if (!PERMITTED_TOPICS.has(topic as SubscriptionTopic)) {
      throw new ForbiddenError(`Topic '${topic}' is not permitted`);
    }
    if (topic === "position-status-change") {
      if (filter.investorAddress && filter.investorAddress !== claims.address) {
        throw new ForbiddenError(
          "Cannot subscribe to another investor's position stream",
        );
      }
    }
  }

  /** True once the session's expiry has passed (used for mid-stream eviction). */
  isExpired(claims: SessionClaims, nowSec: number): boolean {
    return nowSec >= claims.expiresAt;
  }
}

/**
 * Issue a session token bound to `address` expiring at `expiresAt` (unix seconds).
 * Used by the login endpoint (which verifies the wallet signature, out of scope
 * for this service) and by tests.
 */
export function issueSessionToken(
  address: string,
  expiresAt: number,
  secret: string,
): string {
  const payload = Buffer.from(
    JSON.stringify({ address, exp: expiresAt }),
    "utf8",
  ).toString("base64url");
  const sig = crypto
    .createHmac("sha256", secret)
    .update(payload)
    .digest("base64url");
  return `${payload}.${sig}`;
}
