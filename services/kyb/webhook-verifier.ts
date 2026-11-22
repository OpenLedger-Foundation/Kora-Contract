import { createHmac, timingSafeEqual } from "node:crypto";

export interface WebhookReplayCache {
  /** Atomically records a signature key unless it has already been accepted. */
  addIfAbsent(key: string, expiresAtSeconds: number, nowSeconds: number): boolean;
}

export interface KybWebhookInput {
  timestamp: string;
  signature: string;
  rawBody: Buffer | string;
}

export interface KybWebhookVerificationConfig {
  secret: string | Buffer;
  replayCache: WebhookReplayCache;
  nowSeconds?: () => number;
  maxClockSkewSeconds?: number;
}

/** Bounded in-process replay cache for a single-instance deployment. */
export class InMemoryWebhookReplayCache implements WebhookReplayCache {
  private readonly entries = new Map<string, number>();

  constructor(private readonly maxEntries = 10_000) {
    if (!Number.isInteger(maxEntries) || maxEntries < 1) {
      throw new Error("Replay cache maxEntries must be a positive integer");
    }
  }

  addIfAbsent(key: string, expiresAtSeconds: number, nowSeconds: number): boolean {
    for (const [entry, expiresAt] of this.entries) {
      if (expiresAt < nowSeconds) this.entries.delete(entry);
    }
    if (this.entries.has(key)) return false;
    if (this.entries.size >= this.maxEntries) {
      throw new Error("Webhook replay cache capacity reached; rejecting webhook");
    }
    this.entries.set(key, expiresAtSeconds);
    return true;
  }
}

/**
 * Verifies the provider's HMAC-SHA256 over `${timestamp}.${rawBody}` and
 * rejects stale, future-dated, malformed, or previously accepted deliveries.
 */
export function verifyKybWebhook(
  input: KybWebhookInput,
  config: KybWebhookVerificationConfig,
): void {
  const secret = Buffer.isBuffer(config.secret) ? config.secret : Buffer.from(config.secret, "utf8");
  if (secret.byteLength === 0) throw new Error("KYB webhook signing secret is required");

  const maxClockSkewSeconds = config.maxClockSkewSeconds ?? 300;
  if (!Number.isInteger(maxClockSkewSeconds) || maxClockSkewSeconds < 0) {
    throw new Error("maxClockSkewSeconds must be a non-negative integer");
  }
  if (!/^\d{1,12}$/.test(input.timestamp)) {
    throw new Error("Invalid KYB webhook timestamp");
  }
  const timestamp = Number(input.timestamp);
  const nowSeconds = (config.nowSeconds ?? (() => Math.floor(Date.now() / 1000)))();
  if (Math.abs(nowSeconds - timestamp) > maxClockSkewSeconds) {
    throw new Error("KYB webhook timestamp outside allowed clock skew");
  }

  const receivedHex = input.signature.startsWith("sha256=")
    ? input.signature.slice("sha256=".length)
    : input.signature;
  if (!/^[a-fA-F0-9]{64}$/.test(receivedHex)) {
    throw new Error("Malformed KYB webhook signature");
  }
  const rawBody = Buffer.isBuffer(input.rawBody) ? input.rawBody : Buffer.from(input.rawBody, "utf8");
  const expected = createHmac("sha256", secret)
    .update(input.timestamp, "utf8")
    .update(".", "utf8")
    .update(rawBody)
    .digest();
  const received = Buffer.from(receivedHex, "hex");
  if (received.byteLength !== expected.byteLength || !timingSafeEqual(received, expected)) {
    throw new Error("Invalid KYB webhook signature");
  }

  const replayKey = `${input.timestamp}:${receivedHex.toLowerCase()}`;
  if (!config.replayCache.addIfAbsent(replayKey, timestamp + maxClockSkewSeconds, nowSeconds)) {
    throw new Error("KYB webhook replay detected");
  }
}
