/**
 * Advisory fee hints for protocol transactions (Issue #1034).
 *
 * The Kora fee-hint endpoint (`GET /network/fee-hint`, served by the
 * fx-ingestion service) reports a surge multiplier derived from Soroban RPC
 * fee stats. These helpers apply that multiplier to the *inclusion* fee a
 * `TransactionBuilder` is created with. Soroban resource fees added by
 * `prepareTransaction` are unaffected.
 *
 * Hints are advisory: a higher fee improves the chance of inclusion during
 * surge pricing but guarantees neither inclusion nor ordering. Every failure
 * path falls back to the caller's base fee.
 */

export interface FeeHint {
  network: string;
  base_fee: number;
  surge_multiplier_bps: number;
  surge_multiplier: number;
  suggested_fee: number;
  surge_active: boolean;
  observed_at: string | null;
  latest_ledger: number | null;
  stale: boolean;
  degraded: boolean;
  advisory: true;
}

export interface FeeHintOptions {
  /** Fee-hint endpoint URL. Defaults to env `KORA_FEE_HINT_URL`. */
  url?: string;
  /** Client-side clamp on the multiplier (bps). Defaults to env `KORA_FEE_HINT_MAX_BPS` or 100_000 (10x). */
  maxMultiplierBps?: number;
  /** Request timeout (ms). Default 2_000. */
  timeoutMs?: number;
  /** Injected fetch implementation (defaults to global `fetch`). */
  fetch?: FetchLike;
}

type FetchLike = (
  url: string,
  init?: { signal?: unknown; headers?: Record<string, string> }
) => Promise<{ ok: boolean; json(): Promise<unknown> }>;

const BPS_ONE = 10_000;
const DEFAULT_MAX_MULTIPLIER_BPS = 100_000;

function env(name: string): string | undefined {
  return typeof process !== "undefined" ? process.env?.[name] : undefined;
}

/** True when env `KORA_USE_FEE_HINTS=true`. Fee hints are opt-in. */
export function feeHintsEnabled(): boolean {
  return env("KORA_USE_FEE_HINTS") === "true";
}

/**
 * Apply a hint's multiplier to `baseFee` (stroops, as string or number).
 * The multiplier is clamped to `[1.0x, maxMultiplierBps]`; the result is
 * never lower than `baseFee`. Returns a string suitable for
 * `new TransactionBuilder(account, { fee })`.
 */
export function applyFeeHint(
  baseFee: string | number,
  hint: Pick<FeeHint, "surge_multiplier_bps"> | null | undefined,
  maxMultiplierBps: number = DEFAULT_MAX_MULTIPLIER_BPS
): string {
  const n = Number(baseFee);
  const base = BigInt(Number.isFinite(n) && n > 0 ? Math.floor(n) : 0);
  const raw = hint ? Math.floor(Number(hint.surge_multiplier_bps)) : BPS_ONE;
  const cap = Math.max(BPS_ONE, Math.floor(maxMultiplierBps));
  const bps = Number.isFinite(raw) ? Math.min(Math.max(raw, BPS_ONE), cap) : BPS_ONE;
  const scaled = (base * BigInt(bps) + BigInt(BPS_ONE - 1)) / BigInt(BPS_ONE); // ceil
  return (scaled > base ? scaled : base).toString();
}

/** Fetch the current hint. Returns null on any error (network, non-2xx, malformed body). */
export async function fetchFeeHint(options: FeeHintOptions = {}): Promise<FeeHint | null> {
  const url = options.url ?? env("KORA_FEE_HINT_URL");
  const doFetch = options.fetch ?? (globalThis as { fetch?: FetchLike }).fetch;
  if (!url || !doFetch) return null;

  const AbortCtor = (globalThis as { AbortController?: new () => { signal: unknown; abort(): void } }).AbortController;
  const controller = AbortCtor ? new AbortCtor() : undefined;
  const timer = controller ? setTimeout(() => controller.abort(), options.timeoutMs ?? 2_000) : undefined;
  try {
    const res = await doFetch(url, { signal: controller?.signal, headers: { accept: "application/json" } });
    if (!res.ok) return null;
    const body = (await res.json()) as Partial<FeeHint>;
    if (typeof body?.surge_multiplier_bps !== "number") return null;
    return body as FeeHint;
  } catch {
    return null;
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/**
 * Resolve the inclusion fee for a protocol transaction.
 *
 * Returns `baseFee` unchanged unless `KORA_USE_FEE_HINTS=true` and a hint
 * endpoint is reachable; otherwise applies the (clamped) multiplier.
 */
export async function resolveFee(
  baseFee: string | number,
  options: FeeHintOptions = {}
): Promise<string> {
  if (!feeHintsEnabled()) return String(baseFee);
  const hint = await fetchFeeHint(options);
  const max =
    options.maxMultiplierBps ?? Number(env("KORA_FEE_HINT_MAX_BPS") ?? DEFAULT_MAX_MULTIPLIER_BPS);
  return applyFeeHint(baseFee, hint, Number.isFinite(max) ? max : DEFAULT_MAX_MULTIPLIER_BPS);
}
