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
  maxSizeBytesByContentType: {
    "application/json": 256 * 1024, // invoice metadata is small; cap pin quota abuse
  },
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

/**
 * Content-type allowlist enforcement for pinned invoice metadata (issue #1186).
 *
 * The declared MIME type is normalized (case, whitespace, parameters), must be
 * consistent with the filename extension, and must match the content's magic
 * bytes. This stops renamed HTML/SVG/script blobs from being pinned under an
 * allowlisted type and later served by a gateway as active content.
 */
const EXTENSIONS_BY_CONTENT_TYPE: Record<string, string[]> = {
  "application/pdf": [".pdf"],
  "application/json": [".json"],
  "image/png": [".png"],
  "image/jpeg": [".jpg", ".jpeg"],
};

const ALLOWED_CHARSETS = new Set(["utf-8", "utf8"]);

/** Lowercase the MIME type and strip parameters; reject unknown parameters. */
export function normalizeContentType(raw: string): string {
  if (typeof raw !== "string" || raw.trim() === "") {
    throw new Error("UNSUPPORTED_TYPE: missing content type");
  }
  const [base, ...params] = raw.split(";").map((part) => part.trim().toLowerCase());
  for (const param of params) {
    if (param === "") continue;
    const [name, value = ""] = param.split("=").map((part) => part.trim().replace(/^"|"$/g, ""));
    if (name !== "charset" || !ALLOWED_CHARSETS.has(value)) {
      throw new Error(`UNSUPPORTED_TYPE: ${raw}`);
    }
  }
  return base;
}

function startsWith(content: Uint8Array, signature: number[]): boolean {
  return signature.every((byte, i) => content[i] === byte);
}

/** Returns true when `content` structurally matches the normalized `contentType`. */
export function contentMatchesType(contentType: string, content: Uint8Array): boolean {
  switch (contentType) {
    case "application/pdf":
      return startsWith(content, [0x25, 0x50, 0x44, 0x46, 0x2d]); // %PDF-
    case "image/png":
      return startsWith(content, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    case "image/jpeg":
      return startsWith(content, [0xff, 0xd8, 0xff]);
    case "application/json": {
      try {
        const text = new TextDecoder("utf-8", { fatal: true }).decode(content);
        JSON.parse(text);
        return true;
      } catch {
        return false;
      }
    }
    default:
      // Unknown types have no sniffing rule; fail closed.
      return false;
  }
}

function validateConfig(config: PinningServiceConfig): void {
  if (!Array.isArray(config.allowedContentTypes) || config.allowedContentTypes.length === 0) {
    throw new Error("CONFIG_ERROR: allowedContentTypes must be a non-empty allowlist");
  }
  if (!Array.isArray(config.allowedExtensions) || config.allowedExtensions.length === 0) {
    throw new Error("CONFIG_ERROR: allowedExtensions must be a non-empty allowlist");
  }
  if (!Number.isInteger(config.maxFileSizeBytes) || config.maxFileSizeBytes <= 0) {
    throw new Error("CONFIG_ERROR: maxFileSizeBytes must be a positive integer");
  }
  for (const type of config.allowedContentTypes) {
    if (!EXTENSIONS_BY_CONTENT_TYPE[normalizeContentType(type)]) {
      throw new Error(`CONFIG_ERROR: no content sniffing rule for allowlisted type ${type}`);
    }
  }
  for (const [type, cap] of Object.entries(config.maxSizeBytesByContentType ?? {})) {
    if (!Number.isInteger(cap) || cap <= 0) {
      throw new Error(`CONFIG_ERROR: size cap for ${type} must be a positive integer`);
    }
  }
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
    validateConfig(config);
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
    const contentType = normalizeContentType(req.contentType);
    const allowedTypes = this.config.allowedContentTypes.map(normalizeContentType);
    if (!allowedTypes.includes(contentType)) {
      throw new Error(`UNSUPPORTED_TYPE: ${req.contentType}`);
    }
    const ext = extOf(req.filename);
    if (!this.config.allowedExtensions.includes(ext)) {
      throw new Error(`UNSUPPORTED_EXTENSION: ${req.filename}`);
    }
    if (!EXTENSIONS_BY_CONTENT_TYPE[contentType]?.includes(ext)) {
      throw new Error(`EXTENSION_TYPE_MISMATCH: ${req.filename} is not ${contentType}`);
    }
    const typeCap = this.config.maxSizeBytesByContentType?.[contentType];
    const maxBytes = Math.min(this.config.maxFileSizeBytes, typeCap ?? Infinity);
    if (req.content.byteLength > maxBytes) {
      throw new Error(`FILE_TOO_LARGE: ${req.content.byteLength} > ${maxBytes}`);
    }
    if (req.content.byteLength === 0) throw new Error("EMPTY_FILE");

    // Hash-verify BEFORE any network pin.
    const actual = sha256Hex(req.content);
    if (actual.toLowerCase() !== req.claimedSha256Hex.toLowerCase()) {
      throw new Error("INTEGRITY_MISMATCH: claimed sha256 does not match content");
    }
    if (!contentMatchesType(contentType, req.content)) {
      throw new Error(`CONTENT_TYPE_MISMATCH: content is not valid ${contentType}`);
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
