/**
 * Helpers for values returned by SDK getters.
 *
 * The SDK getters return `scValToNative` output cast to camelCase types, but
 * Soroban structs decode with their Rust snake_case field names and unit enums
 * decode as `["Variant"]`. These helpers accept both shapes.
 */

function snake(key: string): string {
  return key.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
}

/** Reads `key` (camelCase) or its snake_case twin from a decoded struct. */
export function field<T = unknown>(obj: unknown, key: string): T | undefined {
  if (!obj || typeof obj !== 'object') return undefined;
  const record = obj as Record<string, unknown>;
  return (key in record ? record[key] : record[snake(key)]) as T | undefined;
}

export function toBigInt(value: unknown, fallback = 0n): bigint {
  if (typeof value === 'bigint') return value;
  if (typeof value === 'number' && Number.isFinite(value)) return BigInt(Math.trunc(value));
  if (typeof value === 'string' && /^-?\d+$/.test(value)) return BigInt(value);
  return fallback;
}

export function toNumber(value: unknown, fallback = 0): number {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'bigint') return Number(value);
  if (typeof value === 'string' && value.trim() !== '' && Number.isFinite(Number(value))) return Number(value);
  return fallback;
}

/** `["Funded"]` or `"Funded"` → `"Funded"`. */
export function enumName(value: unknown): string | undefined {
  if (Array.isArray(value)) return value.length > 0 ? String(value[0]) : undefined;
  return value === undefined || value === null ? undefined : String(value);
}

export function toHex(value: unknown): string {
  if (value instanceof Uint8Array) return Array.from(value, (b) => b.toString(16).padStart(2, '0')).join('');
  return typeof value === 'string' ? value : '';
}

/** Token base units → whole units (e.g. 7-decimal USDC stroops → dollars). */
export function fromBaseUnits(amount: bigint, decimals: number): number {
  const scale = 10n ** BigInt(decimals);
  const whole = amount / scale;
  const frac = amount % scale;
  return Number(whole) + Number(frac) / Number(scale);
}

/** Accepts `101`, `"101"` or display ids such as `"INV-2026-101"`. */
export function parseInvoiceId(id: string | number | bigint): bigint | null {
  if (typeof id === 'bigint') return id >= 0n ? id : null;
  if (typeof id === 'number') return Number.isInteger(id) && id >= 0 ? BigInt(id) : null;
  const match = /^(?:[A-Za-z]+-(?:\d+-)?)?(\d+)$/.exec(id.trim());
  return match ? BigInt(match[1]) : null;
}
