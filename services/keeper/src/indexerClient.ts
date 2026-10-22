/**
 * Keeper Service — Indexer Deadline Ingestion Client
 *
 * Polls the indexer for active listings with upcoming funding deadlines
 * (plus invoice due dates / grace-period ends where exposed) and maps rows
 * to {@link Deadline} records with stable dedup keys (`${kind}:${invoiceId}`).
 *
 * Design notes:
 *  - Idempotent: enqueue path relies on JobStore `INSERT OR IGNORE` on
 *    `dedup_key`, so restarts never duplicate jobs.
 *  - Outage-safe: indexer 503s, timeouts, and network errors resolve to an
 *    empty batch (logged + counted) and never corrupt SQLite job state.
 *  - Already-passed deadlines are returned as-is; the scheduler promotes
 *    them to `ready` immediately.
 *
 * Issue #1022
 */

import pino from "pino";
import { Deadline, DeadlineKind } from "./types";

const logger = pino({ name: "keeper:indexer-client" });

/** Minimal row shape served by the indexer deadline export view/endpoint. */
export interface IndexerDeadlineRow {
  invoice_id: string | number | bigint;
  kind?: string;
  funding_deadline?: string | number | null;
  due_date?: string | number | null;
  grace_period_end?: string | number | null;
  status?: string;
}

const KIND_BY_FIELD: Array<{ field: keyof IndexerDeadlineRow; kind: DeadlineKind }> = [
  { field: "funding_deadline", kind: "funding_expiry" },
  { field: "due_date", kind: "due_date" },
  { field: "grace_period_end", kind: "grace_period_end" },
];

export interface IndexerDeadlineClientOptions {
  /** Base indexer URL, e.g. https://indexer/api/v1/deadlines */
  indexerUrl: string;
  /** Optional bearer credential for the indexer endpoint. */
  apiKey?: string;
  /** Only ingest deadlines within now + horizonSec (already-passed always included). */
  horizonSec: number;
  /** Contract address stamped onto produced deadlines. */
  contractAddress: string;
  /** Per-request timeout in ms. Default: 10_000. */
  timeoutMs?: number;
  /** Injectable fetch for unit tests. Defaults to global fetch. */
  fetchFn?: typeof fetch;
}

function toNumber(value: string | number | bigint | null | undefined): number | null {
  if (value === null || value === undefined) return null;
  const n = typeof value === "bigint" ? Number(value) : Number(value);
  return Number.isFinite(n) ? n : null;
}

export class IndexerDeadlineClient {
  private consecutiveFailures = 0;

  constructor(private readonly options: IndexerDeadlineClientOptions) {}

  get degraded(): boolean {
    return this.consecutiveFailures > 0;
  }

  /** Fetch + map the current deadline batch. Never throws: outages yield []. */
  async fetchDeadlines(nowSec: number = Math.floor(Date.now() / 1000)): Promise<Deadline[]> {
    const { indexerUrl, apiKey, horizonSec, contractAddress, timeoutMs = 10_000 } = this.options;
    if (!indexerUrl) {
      logger.debug("Indexer URL unset; skipping ingestion tick (manual seed path only)");
      return [];
    }

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const url = `${indexerUrl}?horizon_sec=${horizonSec}`;
      const res = await (this.options.fetchFn ?? fetch)(url, {
        headers: {
          Accept: "application/json",
          ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
        },
        signal: controller.signal,
      });
      if (!res.ok) {
        throw new Error(`indexer responded HTTP ${res.status}`);
      }
      const body = (await res.json()) as { deadlines?: IndexerDeadlineRow[] } | IndexerDeadlineRow[];
      const rows = Array.isArray(body) ? body : body.deadlines ?? [];
      this.consecutiveFailures = 0;
      return this.mapRows(rows, nowSec, horizonSec, contractAddress);
    } catch (err) {
      this.consecutiveFailures += 1;
      logger.warn(
        { err: String(err), failures: this.consecutiveFailures },
        "Indexer unreachable; scheduler keeps dispatching existing jobs"
      );
      return [];
    } finally {
      clearTimeout(timer);
    }
  }

  /** Backoff with jitter for the caller poll loop after failures. */
  backoffMs(baseMs = 5_000, maxMs = 60_000): number {
    const exp = Math.min(maxMs, baseMs * 2 ** Math.min(this.consecutiveFailures, 5));
    return Math.floor(exp * (0.5 + Math.random() * 0.5));
  }

  private mapRows(
    rows: IndexerDeadlineRow[],
    nowSec: number,
    horizonSec: number,
    contractAddress: string
  ): Deadline[] {
    const out: Deadline[] = [];
    for (const row of rows) {
      if (row.status === "cancelled" || row.status === "fully_funded") continue;
      const invoiceId = BigInt(row.invoice_id as string | number);
      const fields =
        row.kind !== undefined
          ? KIND_BY_FIELD.filter((k) => k.kind === row.kind)
          : KIND_BY_FIELD;
      for (const { field, kind } of fields) {
        const ts = toNumber(row[field] as string | number | null | undefined);
        if (ts === null) continue;
        // Already-passed deadlines enqueue as immediately ready; future ones
        // only when inside the horizon window; empty horizon writes nothing.
        if (ts > nowSec + horizonSec) continue;
        out.push({
          dedupKey: `${kind}:${invoiceId.toString()}`,
          kind,
          invoiceId,
          deadlineTs: ts,
          contractAddress,
        });
      }
    }
    return out;
  }
}
