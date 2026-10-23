/**
 * Maps SDK, Soroban RPC and gateway failures to messages the UI can show.
 *
 * Every failure reaching a component goes through `mapSdkError`, so the
 * funding and risk views never render raw RPC strings or stack traces.
 */
import { getErrorMessage } from '@kora-protocol/sdk';

export type KoraErrorCode =
  | 'RATE_LIMITED'
  | 'NETWORK'
  | 'CONTRACT'
  | 'NOT_FOUND'
  | 'STALE_MANIFEST'
  | 'NOT_CONFIGURED'
  | 'INVALID_CONFIG'
  | 'WALLET_NOT_CONNECTED'
  | 'UNKNOWN';

export class KoraUserError extends Error {
  public readonly code: KoraErrorCode;
  /** Short, user-visible explanation. */
  public readonly userMessage: string;
  /** True when trying again later may succeed. */
  public readonly retryable: boolean;
  /** Suggested wait before retrying (rate limits). */
  public readonly retryAfterMs?: number;
  /** Contract error code when the failure came from a contract. */
  public readonly contractCode?: number;

  constructor(
    code: KoraErrorCode,
    userMessage: string,
    options: { retryable?: boolean; retryAfterMs?: number; contractCode?: number; cause?: unknown } = {},
  ) {
    super(userMessage);
    this.name = 'KoraUserError';
    this.code = code;
    this.userMessage = userMessage;
    this.retryable = options.retryable ?? false;
    this.retryAfterMs = options.retryAfterMs;
    this.contractCode = options.contractCode;
  }
}

/** Thrown by the gateway client when the API keeps answering 429. */
export class GatewayRateLimitError extends Error {
  constructor(public readonly retryAfterMs: number) {
    super(`RATE_LIMITED: retry after ${retryAfterMs}ms`);
    this.name = 'GatewayRateLimitError';
  }
}

/** Contract error codes that mean "the record does not exist". */
const NOT_FOUND_CODES = new Set([10, 20, 30, 34, 50]);

const CONTRACT_ERROR_RE = /Error\(Contract, #(\d+)\)/;
const STALE_RE = /MissingValue|non-?existent contract|contract (?:not found|does not exist)|wasm.*not found/i;
const RATE_RE = /\b429\b|too many requests|rate.?limit/i;
const NETWORK_RE = /fetch failed|network ?error|ECONNREFUSED|ECONNRESET|ENOTFOUND|ETIMEDOUT|timed? ?out|socket hang up/i;

function retryAfterSeconds(ms: number): string {
  const s = Math.max(1, Math.ceil(ms / 1000));
  return `${s} second${s === 1 ? '' : 's'}`;
}

/**
 * Converts any thrown value into a `KoraUserError`.
 *
 * @param contract  Contract name used to pick the right error table
 *                  (e.g. `financing_pool`, `governance_voting`).
 */
export function mapSdkError(error: unknown, contract?: string): KoraUserError {
  if (error instanceof KoraUserError) return error;

  if (error instanceof GatewayRateLimitError) {
    return new KoraUserError(
      'RATE_LIMITED',
      `Too many requests right now. Please try again in ${retryAfterSeconds(error.retryAfterMs)}.`,
      { retryable: true, retryAfterMs: error.retryAfterMs, cause: error },
    );
  }

  const message = error instanceof Error ? error.message : String(error ?? '');
  const status = (error as { response?: { status?: number }; status?: number } | null)?.response?.status
    ?? (error as { status?: number } | null)?.status;

  const contractMatch = CONTRACT_ERROR_RE.exec(message);
  if (contractMatch) {
    const code = Number(contractMatch[1]);
    const known = getErrorMessage(code, contract);
    if (NOT_FOUND_CODES.has(code) && (!contract || !/governance|voting|dispute|escrow|fractional|secondary/i.test(contract))) {
      return new KoraUserError('NOT_FOUND', known ? `${known}.` : 'The requested record was not found.', {
        contractCode: code,
        cause: error,
      });
    }
    return new KoraUserError(
      'CONTRACT',
      known ? `The contract rejected this request: ${known}.` : `The contract rejected this request (code ${code}).`,
      { contractCode: code, cause: error },
    );
  }

  if (status === 429 || RATE_RE.test(message)) {
    const retryAfterMs = 5_000;
    return new KoraUserError(
      'RATE_LIMITED',
      `Too many requests right now. Please try again in ${retryAfterSeconds(retryAfterMs)}.`,
      { retryable: true, retryAfterMs, cause: error },
    );
  }

  if (STALE_RE.test(message)) {
    return new KoraUserError(
      'STALE_MANIFEST',
      'The configured contract addresses look out of date. The deployment manifest may need to be refreshed.',
      { cause: error },
    );
  }

  if (NETWORK_RE.test(message) || (typeof status === 'number' && status >= 500)) {
    return new KoraUserError('NETWORK', 'We could not reach the Stellar network. Check your connection and try again.', {
      retryable: true,
      cause: error,
    });
  }

  return new KoraUserError('UNKNOWN', 'Something went wrong while loading on-chain data. Please try again.', {
    retryable: true,
    cause: error,
  });
}

/**
 * Runs `fn`, retrying rate-limited and network failures with backoff.
 * Non-retryable errors are rethrown immediately as `KoraUserError`.
 */
export async function withRetry<T>(
  fn: () => Promise<T>,
  options: { retries?: number; baseDelayMs?: number; contract?: string; sleep?: (ms: number) => Promise<void> } = {},
): Promise<T> {
  const retries = options.retries ?? 2;
  const baseDelayMs = options.baseDelayMs ?? 500;
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  let attempt = 0;
  for (;;) {
    try {
      return await fn();
    } catch (error) {
      const mapped = mapSdkError(error, options.contract);
      if (!mapped.retryable || mapped.code === 'UNKNOWN' || attempt >= retries) throw mapped;
      const backoff = baseDelayMs * 2 ** attempt;
      await sleep(Math.min(mapped.retryAfterMs ?? backoff, Math.max(backoff, 5_000)));
      attempt += 1;
    }
  }
}
