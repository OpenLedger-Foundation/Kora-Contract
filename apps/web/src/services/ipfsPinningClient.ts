/**
 * REST client for the IPFS Pinning Service (Issue #1001, #1002)
 *
 * Talks to `services/ipfs-pinning` over HTTP following the surface of
 * `PinningService` (see services/ipfs-pinning/src/service.ts):
 *
 *   POST   {base}/pins                 → upload + redundant pin (PinRequest)
 *   GET    {base}/pins/:cid            → pin status (pinned/degraded/failed/repinned)
 *   GET    {base}/pins/:cid/content    → pinned bytes (for client-side hash verify)
 *   GET    {base}/pins/:cid/verify     → service-side content hash verification
 *
 * Upload metadata travels in `X-*` headers with the raw bytes as body — the
 * same convention used by the service's `HttpPinProvider` (`X-CID`).
 *
 * Failures map the service's error codes (`FILE_TOO_LARGE`, `EMPTY_FILE`,
 * `INTEGRITY_MISMATCH`, `PIN_FAILED`, …) onto `PinningClientError` carrying a
 * `retryable` flag so the wizard can render retry states without advancing to
 * the mint step (#1001). Provider failover details from `providers.ts` are
 * surfaced in `providerOutcomes` when the service returns them.
 */

const DEFAULT_BASE_URL = '';
export const DEFAULT_TIMEOUT_MS = 30_000;
export const DEFAULT_MAX_RETRIES = 1;

/** Mirrors DEFAULT_PINNING_CONFIG.maxFileSizeBytes in services/ipfs-pinning. */
export const MAX_PIN_FILE_SIZE_BYTES = 10 * 1024 * 1024;
/**
 * Allowed extensions mirroring DEFAULT_PINNING_CONFIG.allowedExtensions so the
 * user gets a pre-flight error without waiting for the service round-trip.
 */
export const ALLOWED_PIN_EXTENSIONS = ['.pdf', '.json', '.png', '.jpg', '.jpeg'];

/** Pin status values from the service (`PinStatus`). */
export type PinStatusKind = 'pinned' | 'degraded' | 'failed' | 'repinned';

/** Per-provider outcome reported by the pinning service (providers.ts failover). */
export interface ProviderOutcome {
  providerId: string;
  ok: boolean;
  error?: string;
}

/** Mirrors `PinResult` from services/ipfs-pinning/src/types.ts. */
export interface PinResult {
  cid: string;
  sizeBytes: number;
  sha256Hex: string;
  providers: ProviderOutcome[];
  degraded: boolean;
  pinnedAt: number;
}

export interface PinStatusResult {
  cid: string;
  status: PinStatusKind;
  sha256Hex?: string;
  verified?: boolean;
}

export interface PinVerificationResult {
  cid: string;
  verified: boolean;
  sha256Hex: string;
}

export class PinningClientError extends Error {
  /** Service error code, e.g. FILE_TOO_LARGE / PIN_FAILED / INTEGRITY_MISMATCH. */
  public readonly code: string;
  /** HTTP status, when one exists (0 for network/timeout). */
  public readonly status: number;
  /** True when a retry (manual via the wizard) could plausibly succeed. */
  public readonly retryable: boolean;
  /** Per-provider outcomes from ipfs-pinning providers.ts failover behavior. */
  public readonly providerOutcomes: ProviderOutcome[];

  constructor(code: string, message: string, status = 0, retryable = false, providerOutcomes: ProviderOutcome[] = []) {
    super(message);
    this.name = 'PinningClientError';
    this.code = code;
    this.status = status;
    this.retryable = retryable;
    this.providerOutcomes = providerOutcomes;
  }
}

export interface IpfsPinningClientOptions {
  /** REST base URL, e.g. https://pins.kora.finance (no trailing slash). */
  baseUrl: string;
  /** Injectable fetch (tests, proxies). Defaults to globalThis.fetch. */
  fetchFn?: typeof fetch;
  /** Per-request timeout in ms (default 30s) — slow pinning surfaces a timeout. */
  timeoutMs?: number;
  /** Extra retries on network/5xx failures (default 1) for provider failover. */
  maxRetries?: number;
}

function trimBase(baseUrl: string): string {
  return baseUrl.replace(/\/+$/, '');
}

/** Error codes for which a manual retry can plausibly succeed. */
const RETRYABLE_CODES = new Set(['TIMEOUT', 'NETWORK', 'PIN_FAILED', 'SERVER', 'REQUEST']);

/** Service error codes surfaced verbatim to the wizard for user messaging. */
export const SERVICE_ERROR_CODES = [
  'FILE_TOO_LARGE',
  'EMPTY_FILE',
  'UNSUPPORTED_TYPE',
  'UNSUPPORTED_EXTENSION',
  'INTEGRITY_MISMATCH',
  'PII_NOT_HANDLED',
] as const;

export class IpfsPinningClient {
  private baseUrl: string;
  private fetchFn: typeof fetch;
  private timeoutMs: number;
  private maxRetries: number;

  constructor(options: IpfsPinningClientOptions) {
    this.baseUrl = trimBase(options.baseUrl);
    this.fetchFn = options.fetchFn ?? (globalThis.fetch.bind(globalThis) as typeof fetch);
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.maxRetries = options.maxRetries ?? DEFAULT_MAX_RETRIES;
  }

  /**
   * Upload + pin a document. Throws PinningClientError on any failure.
   * Duplicate pins of the same content hash are idempotent — the service
   * derives the CID from the SHA-256 digest and upserts the pin record.
   */
  public async pinDocument(request: {
    filename: string;
    content: Uint8Array;
    contentType: string;
    claimedSha256Hex: string;
    piiHandled: boolean;
    onProgress?: (percent: number) => void;
  }): Promise<PinResult> {
    request.onProgress?.(10); // hashing done by caller before this call

    const response = await this.request('/pins', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/octet-stream',
        'X-Filename': request.filename,
        'X-SHA256': request.claimedSha256Hex.toLowerCase(),
        'X-Content-Type': request.contentType,
        'X-PII-Handled': request.piiHandled ? 'true' : 'false',
      },
      body: new Uint8Array(request.content) as unknown as BodyInit,
    });

    request.onProgress?.(75); // bytes transmitted; waiting on provider pins
    const result = (await this.parseJson(response)) as unknown as PinResult;
    if (!result || typeof result.cid !== 'string' || result.cid.length === 0) {
      throw new PinningClientError('MALFORMED_RESPONSE', 'Pinning service returned no CID.');
    }
    request.onProgress?.(100);
    return result;
  }

  /** Poll pin status for a CID (idempotent; safe to call repeatedly). */
  public async getPinStatus(cid: string): Promise<PinStatusResult> {
    const response = await this.request(`/pins/${encodeURIComponent(cid)}`, { method: 'GET' });
    return (await this.parseJson(response)) as unknown as PinStatusResult;
  }

  /** Download the pinned bytes for gateway-alteration checks ("fails closed"). */
  public async fetchPinnedContent(cid: string): Promise<Uint8Array> {
    const response = await this.request(`/pins/${encodeURIComponent(cid)}/content`, { method: 'GET' });
    if (!response.ok) {
      throw await this.buildError(cid, response);
    }
    try {
      return new Uint8Array(await response.arrayBuffer());
    } catch {
      throw new PinningClientError('MALFORMED_RESPONSE', `Could not read pinned content for ${cid}.`);
    }
  }

  /** Service-side verification: SHA-256 over pinned content vs pin record. */
  public async verifyPinned(cid: string): Promise<PinVerificationResult> {
    const response = await this.request(`/pins/${encodeURIComponent(cid)}/verify`, { method: 'GET' });
    if (!response.ok) {
      throw await this.buildError(cid, response);
    }
    return (await this.parseJson(response)) as unknown as PinVerificationResult;
  }

  private async request(path: string, init: RequestInit): Promise<Response> {
    let lastError: PinningClientError | null = null;

    for (let attempt = 0; attempt <= this.maxRetries; attempt += 1) {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), this.timeoutMs);
      try {
        const response = await this.fetchFn(`${this.baseUrl}${path}`, { ...init, signal: controller.signal });
        if (response.ok || (response.status >= 400 && response.status < 500)) {
          clearTimeout(timer);
          return response;
        }
        // 5xx → provider-side failure (failover headroom); retry then surface.
        lastError = await this.buildError(path, response);
      } catch (err) {
        if (err instanceof Error && (err as any).name === 'AbortError') {
          lastError = new PinningClientError(
            'TIMEOUT',
            `Pinning timed out after ${this.timeoutMs}ms — pinning may be slow; please retry.`,
          );
        } else {
          lastError = new PinningClientError(
            'NETWORK',
            err instanceof Error ? err.message : 'Network error contacting pinning service',
          );
        }
      } finally {
        clearTimeout(timer);
      }
    }

    throw lastError ?? new PinningClientError('NETWORK', 'Unknown pinning service failure');
  }

  private async parseJson(response: Response): Promise<Record<string, unknown>> {
    if (!response.ok) {
      throw await this.buildError('', response);
    }
    try {
      return (await response.json()) as Record<string, unknown>;
    } catch {
      throw new PinningClientError('MALFORMED_RESPONSE', 'Pinning service returned invalid JSON.');
    }
  }

  /** Builds a PinningClientError from a non-2xx response (service JSON or fallback). */
  private async buildError(resource: string, response: Response): Promise<PinningClientError> {
    const baseMessage = `Pinning service error for ${resource || 'request'} (HTTP ${response.status})`;
    const fallback = new PinningClientError(
      response.status >= 500 ? 'SERVER' : 'REQUEST',
      baseMessage,
      response.status,
      RETRYABLE_CODES.has(response.status >= 500 ? 'SERVER' : 'REQUEST'),
    );
    try {
      const body = (await response.json()) as {
        error?: string;
        message?: string;
        providers?: ProviderOutcome[];
      };
      const code = body.error ? String(body.error).split(' ')[0].replace(/[^A-Z_]/g, '') : fallback.code;
      const message = body.message || body.error || baseMessage;
      return new PinningClientError(code || fallback.code, message, response.status, RETRYABLE_CODES.has(code), body.providers ?? []);
    } catch {
      return fallback;
    }
  }
}

/** Reads the configured pinning service base URL (unset in dev → empty string). */
export function getPinBaseUrl(): string {
  return process.env.VITE_IPFS_PINNING_URL || DEFAULT_BASE_URL;
}

/**
 * Builds a client when `VITE_IPFS_PINNING_URL` is configured, else null so the
 * wizard can fall back to the local dev mock. When the env var is set, no mock
 * CID prefix may ever enter the production path.
 */
export function createPinningClient(): IpfsPinningClient | null {
  const base = getPinBaseUrl();
  if (!base) return null;
  return new IpfsPinningClient({ baseUrl: base });
}

/** Pre-flight validation matching the pinning service allowlists + size cap. */
export function validateFileForPinning(fileName: string, sizeBytes: number, contentByteLength: number): string | null {
  if (contentByteLength === 0) return 'EMPTY_FILE';
  const dot = fileName.lastIndexOf('.');
  const ext = dot >= 0 ? fileName.slice(dot).toLowerCase() : '';
  if (!ALLOWED_PIN_EXTENSIONS.includes(ext)) {
    return `UNSUPPORTED_EXTENSION: ${fileName}`;
  }
  if (contentByteLength > MAX_PIN_FILE_SIZE_BYTES) {
    return `FILE_TOO_LARGE: content exceeds ${MAX_PIN_FILE_SIZE_BYTES} bytes limit`;
  }
  return null;
}

export { RETRYABLE_CODES };
