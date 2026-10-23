/**
 * Minimal JSON client for the read-only API gateway (services/api/gateway).
 *
 * Handles HTTP 429 by honouring `Retry-After` (seconds or HTTP date) for a
 * bounded number of retries, then throws `GatewayRateLimitError` so the UI
 * can show "try again in N seconds" instead of a generic failure.
 */
import { GatewayRateLimitError } from './errors';

export interface GatewayClientOptions {
  baseUrl: string;
  fetchFn?: typeof fetch;
  /** Retries after a 429 before giving up (default 2). */
  maxRetries?: number;
  /** Longest single wait we accept before surfacing the 429 (default 5s). */
  maxWaitMs?: number;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
}

export function parseRetryAfter(header: string | null, now: number): number {
  if (!header) return 1_000;
  const seconds = Number(header);
  if (Number.isFinite(seconds) && seconds >= 0) return Math.ceil(seconds * 1000);
  const date = Date.parse(header);
  return Number.isFinite(date) ? Math.max(0, date - now) : 1_000;
}

export class GatewayClient {
  private readonly baseUrl: string;
  private readonly fetchFn: typeof fetch;
  private readonly maxRetries: number;
  private readonly maxWaitMs: number;
  private readonly sleep: (ms: number) => Promise<void>;
  private readonly now: () => number;

  constructor(options: GatewayClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.fetchFn = options.fetchFn ?? ((...args) => fetch(...args));
    this.maxRetries = options.maxRetries ?? 2;
    this.maxWaitMs = options.maxWaitMs ?? 5_000;
    this.sleep = options.sleep ?? ((ms) => new Promise<void>((r) => setTimeout(r, ms)));
    this.now = options.now ?? Date.now;
  }

  /** GETs `path` and parses JSON. Resolves `null` on 404. */
  async getJson<T>(path: string): Promise<T | null> {
    for (let attempt = 0; ; attempt += 1) {
      const response = await this.fetchFn(`${this.baseUrl}${path}`, { headers: { Accept: 'application/json' } });
      if (response.status === 429) {
        const waitMs = parseRetryAfter(response.headers.get('Retry-After'), this.now());
        if (attempt >= this.maxRetries || waitMs > this.maxWaitMs) throw new GatewayRateLimitError(waitMs);
        await this.sleep(waitMs);
        continue;
      }
      if (response.status === 404) return null;
      if (!response.ok) {
        const error = new Error(`Gateway request failed with HTTP ${response.status}`) as Error & { status: number };
        error.status = response.status;
        throw error;
      }
      return (await response.json()) as T;
    }
  }
}
