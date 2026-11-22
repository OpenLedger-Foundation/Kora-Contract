/**
 * IPFS Pinning Service — Core upload pipeline
 *
 * 1. Validate format/size (allowlists + configured cap).
 * 2. Require `piiHandled` (PII must be hashed/encrypted before pinning).
 * 3. Hash-verify content integrity (SHA-256 vs claimed digest) BEFORE pinning.
 * 4. Pin redundantly across all configured providers (≥2 for redundancy).
 * 5. Persist a pin-status record for the periodic verification job.
 *
 * Partial failure: success requires ≥ `minSuccesses` (default 1). A single
 * provider failure degrades (recorded, returned with `degraded: true`) rather
 * than failing the upload; total failure throws.
 *
 * Issue #754
 */

import { createHash } from "crypto";
import { PinProvider, PinProviderError } from "./providers";
import { PinStore } from "./store";
import { PinDeadLetter, PinRequest, PinResult, PinningServiceConfig } from "./types";

export const DEFAULT_PINNING_CONFIG: PinningServiceConfig = {
  maxFileSizeBytes: 10 * 1024 * 1024, // 10 MiB
  allowedContentTypes: ["application/pdf", "application/json", "image/png", "image/jpeg"],
  allowedExtensions: [".pdf", ".json", ".png", ".jpg", ".jpeg"],
  minSuccesses: 1,
  pinRetryAttempts: 3,
  pinRetryBackoffMs: 250,
  pinRetryMaxBackoffMs: 5_000,
};

export interface PinningMetrics {
  pinAttempts: number;
  pinRetries: number;
  permanentFailures: number;
  deadLetterEntries: number;
  deadLetterReplayAttempts: number;
  deadLetterReplaySuccesses: number;
}

type Wait = (ms: number) => Promise<void>;

const defaultWait: Wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export function sha256Hex(content: Uint8Array): string {
  return createHash("sha256").update(content).digest("hex");
}

function extOf(filename: string): string {
  const i = filename.lastIndexOf(".");
  return i >= 0 ? filename.slice(i).toLowerCase() : "";
}

export class PinningService {
  private contentCache = new Map<string, Uint8Array>();
  private readonly metrics: PinningMetrics = {
    pinAttempts: 0,
    pinRetries: 0,
    permanentFailures: 0,
    deadLetterEntries: 0,
    deadLetterReplayAttempts: 0,
    deadLetterReplaySuccesses: 0,
  };

  constructor(
    private providers: PinProvider[],
    private store: PinStore,
    private config: PinningServiceConfig = DEFAULT_PINNING_CONFIG,
    private readonly wait: Wait = defaultWait,
  ) {
    if (providers.length < 2) {
      throw new Error("At least two independent pinning providers are required for redundancy");
    }
    if (!Number.isInteger(config.pinRetryAttempts ?? 3) || (config.pinRetryAttempts ?? 3) < 1) {
      throw new Error("pinRetryAttempts must be a positive integer");
    }
    if ((config.pinRetryBackoffMs ?? 250) < 0 || (config.pinRetryMaxBackoffMs ?? 5_000) < 0) {
      throw new Error("Pin retry backoff values must be non-negative");
    }
  }

  getMetrics(): PinningMetrics & { pendingDeadLetters: number } {
    return {
      ...this.metrics,
      pendingDeadLetters: this.store.listDeadLetters("pending").length,
    };
  }

  metricsText(): string {
    const metrics = this.getMetrics();
    return [
      "# TYPE kora_ipfs_pin_attempts_total counter",
      `kora_ipfs_pin_attempts_total ${metrics.pinAttempts}`,
      "# TYPE kora_ipfs_pin_retries_total counter",
      `kora_ipfs_pin_retries_total ${metrics.pinRetries}`,
      "# TYPE kora_ipfs_pin_permanent_failures_total counter",
      `kora_ipfs_pin_permanent_failures_total ${metrics.permanentFailures}`,
      "# TYPE kora_ipfs_pin_dead_letter_entries_total counter",
      `kora_ipfs_pin_dead_letter_entries_total ${metrics.deadLetterEntries}`,
      "# TYPE kora_ipfs_pin_dead_letter_replay_attempts_total counter",
      `kora_ipfs_pin_dead_letter_replay_attempts_total ${metrics.deadLetterReplayAttempts}`,
      "# TYPE kora_ipfs_pin_dead_letter_replay_successes_total counter",
      `kora_ipfs_pin_dead_letter_replay_successes_total ${metrics.deadLetterReplaySuccesses}`,
      "# TYPE kora_ipfs_pin_dead_letter_pending gauge",
      `kora_ipfs_pin_dead_letter_pending ${metrics.pendingDeadLetters}`,
    ].join("\n");
  }

  /** Replay a pending permanent failure with caller-supplied, hash-verified content. */
  async replayDeadLetter(id: number, content: Uint8Array): Promise<PinDeadLetter> {
    const deadLetter = this.store.getDeadLetter(id);
    if (!deadLetter || deadLetter.status !== "pending") {
      throw new Error(`Pending IPFS dead-letter entry not found: ${id}`);
    }
    if (sha256Hex(content) !== deadLetter.cid) {
      throw new Error("INTEGRITY_MISMATCH: replay content does not match dead-letter CID");
    }
    const provider = this.providers.find((candidate) => candidate.id === deadLetter.providerId);
    if (!provider) throw new Error(`IPFS provider is not configured: ${deadLetter.providerId}`);

    this.metrics.deadLetterReplayAttempts += 1;
    try {
      await this.pinWithRetry(provider, deadLetter.cid, content);
    } catch (error) {
      this.store.recordDeadLetterAttempt(id, errorMessage(error), Math.floor(Date.now() / 1000));
      throw error;
    }

    const now = Math.floor(Date.now() / 1000);
    this.store.recordDeadLetterAttempt(id, deadLetter.error, now);
    this.store.resolveDeadLetter(id, now);
    this.metrics.deadLetterReplaySuccesses += 1;
    return this.store.getDeadLetter(id)!;
  }

  /** Content cache accessor for the verification job's re-pin path. */
  getCachedContent(cid: string): Uint8Array | undefined {
    return this.contentCache.get(cid);
  }

  async upload(req: PinRequest): Promise<PinResult> {
    if (!req.piiHandled) {
      throw new Error("PII_NOT_HANDLED: debtor PII must be hashed/encrypted before pinning");
    }
    if (!this.config.allowedContentTypes.includes(req.contentType)) {
      throw new Error(`UNSUPPORTED_TYPE: ${req.contentType}`);
    }
    if (!this.config.allowedExtensions.includes(extOf(req.filename))) {
      throw new Error(`UNSUPPORTED_EXTENSION: ${req.filename}`);
    }
    if (req.content.byteLength > this.config.maxFileSizeBytes) {
      throw new Error(
        `FILE_TOO_LARGE: ${req.content.byteLength} > ${this.config.maxFileSizeBytes}`,
      );
    }
    if (req.content.byteLength === 0) throw new Error("EMPTY_FILE");

    // Hash-verify BEFORE any network pin.
    const actual = sha256Hex(req.content);
    if (actual.toLowerCase() !== req.claimedSha256Hex.toLowerCase()) {
      throw new Error("INTEGRITY_MISMATCH: claimed sha256 does not match content");
    }

    // Content-derived CID (hex digest; gateway maps to CIDv0/v1 on read).
    const cid = actual;
    const now = Math.floor(Date.now() / 1000);

    const outcomes: Array<{ providerId: string; ok: boolean; error?: string }> = [];
    for (const p of this.providers) {
      try {
        await this.pinWithRetry(p, cid, req.content);
        outcomes.push({ providerId: p.id, ok: true });
      } catch (err) {
        if (err instanceof PinProviderError && !err.retryable) {
          this.metrics.permanentFailures += 1;
          this.store.recordDeadLetter({
            cid,
            providerId: p.id,
            error: err.message,
            createdAt: now,
            updatedAt: now,
          });
          this.metrics.deadLetterEntries += 1;
        }
        outcomes.push({
          providerId: p.id,
          ok: false,
          error: errorMessage(err),
        });
      }
    }

    const successes = outcomes.filter((o) => o.ok).length;
    const minSuccesses = this.config.minSuccesses ?? 1;
    if (successes < minSuccesses) {
      this.store.upsert({
        cid,
        filename: req.filename,
        sizeBytes: req.content.byteLength,
        sha256Hex: actual,
        status: "failed",
        providers: JSON.stringify(outcomes),
        createdAt: now,
        updatedAt: now,
        lastVerifiedAt: null,
        consecutiveFailures: 1,
      });
      throw new Error(`PIN_FAILED: only ${successes}/${this.providers.length} providers succeeded`);
    }

    const degraded = successes < this.providers.length;
    this.contentCache.set(cid, req.content);
    this.store.upsert({
      cid,
      filename: req.filename,
      sizeBytes: req.content.byteLength,
      sha256Hex: actual,
      status: degraded ? "degraded" : "pinned",
      providers: JSON.stringify(outcomes),
      createdAt: now,
      updatedAt: now,
      lastVerifiedAt: null,
      consecutiveFailures: 0,
    });

    return {
      cid,
      sizeBytes: req.content.byteLength,
      sha256Hex: actual,
      providers: outcomes,
      degraded,
      pinnedAt: now,
    };
  }

  private async pinWithRetry(provider: PinProvider, cid: string, content: Uint8Array): Promise<void> {
    const attempts = this.config.pinRetryAttempts ?? 3;
    const baseDelay = this.config.pinRetryBackoffMs ?? 250;
    const maxDelay = this.config.pinRetryMaxBackoffMs ?? 5_000;
    let lastError: unknown;

    for (let attempt = 0; attempt < attempts; attempt++) {
      this.metrics.pinAttempts += 1;
      try {
        await provider.pin(cid, content);
        return;
      } catch (error) {
        lastError = error;
        if (error instanceof PinProviderError && !error.retryable) throw error;
        if (attempt + 1 < attempts) {
          this.metrics.pinRetries += 1;
          const delay = Math.min(baseDelay * (2 ** attempt), maxDelay);
          await this.wait(delay);
        }
      }
    }
    throw lastError;
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
