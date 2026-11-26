/**
 * Per-wallet subscription rate limiter.
 *
 * Token-bucket per wallet address: each wallet gets a budget of new subscription
 * connections per window. Exceeding the budget returns 429 + Retry-After. This
 * mirrors the API gateway's per-key RateLimiter pattern (issue #753) but keys on
 * the authenticated wallet address rather than an API key, since subscriptions
 * are authenticated by session, not by partner API key.
 *
 * Issue: #769 — authorize invoice event subscriptions.
 */

export interface RateLimitResult {
  allowed: boolean;
  remaining: number;
  resetAfterMs: number;
}

export interface RateLimiterOptions {
  /** Max new subscriptions per window per wallet. Default: 30. */
  maxRequestsPerWindow?: number;
  /** Window length in milliseconds. Default: 60_000 (1 min). */
  windowMs?: number;
  /** Injectable clock (ms). Defaults to Date.now. */
  now?: () => number;
}

interface Bucket {
  count: number;
  windowStart: number;
}

export class SubscriptionRateLimiter {
  private buckets = new Map<string, Bucket>();
  private readonly maxRequests: number;
  private readonly windowMs: number;
  private readonly now: () => number;

  constructor(options: RateLimiterOptions = {}) {
    this.maxRequests = options.maxRequestsPerWindow ?? 30;
    this.windowMs = options.windowMs ?? 60_000;
    this.now = options.now ?? Date.now;
  }

  /** Consume one subscription slot for `key` (the wallet address). */
  consume(key: string): RateLimitResult {
    const t = this.now();
    let bucket = this.buckets.get(key);
    if (!bucket || t - bucket.windowStart >= this.windowMs) {
      bucket = { count: 0, windowStart: t };
      this.buckets.set(key, bucket);
    }
    if (bucket.count >= this.maxRequests) {
      return {
        allowed: false,
        remaining: 0,
        resetAfterMs: bucket.windowStart + this.windowMs - t,
      };
    }
    bucket.count += 1;
    return {
      allowed: true,
      remaining: this.maxRequests - bucket.count,
      resetAfterMs: bucket.windowStart + this.windowMs - t,
    };
  }

  /** Force-reset a wallet's bucket (e.g. after a manual operator action). */
  reset(key: string): void {
    this.buckets.delete(key);
  }
}

/**
 * Read the per-window subscription limit from the environment.
 * Throws on non-positive/non-integer values so misconfiguration fails at startup.
 */
export function readSubscriptionRateLimit(
  raw = process.env["SUBSCRIPTION_RATE_LIMIT_PER_MIN"],
): number {
  if (raw === undefined || raw === "") return 30;
  const n = Number(raw);
  if (!Number.isInteger(n) || n <= 0) {
    throw new Error("SUBSCRIPTION_RATE_LIMIT_PER_MIN must be a positive integer");
  }
  return n;
}
