/**
 * Reporting Service — Indexer-backed ReportingDataSource
 *
 * Implements the ReportingDataSource interface by querying the Kora indexer
 * service for normalized chain events (invoice, pool, treasury) and mapping
 * them into the statement-builder input types.
 *
 * Pagination: the indexer REST endpoint is paged with cursor (ledger_seq + offset).
 * Large date ranges are fetched in pages until the cursor is exhausted or events
 * fall outside the requested window.
 *
 * Finality: requests pass `finalized=true` so the adapter always uses the
 * confirmed ledger tip, avoiding reorg-impacted rows.
 *
 * Env var: INDEXER_URL (default http://localhost:8787)
 *
 * Issue #1003
 */

import pino from "pino";
import {
  ReportingDataSource,
} from "./exportJobQueue";
import {
  FundingEvent,
  RepaymentEvent,
  YieldEvent,
  FeeEvent,
  UserStatement,
  ProtocolStatement,
} from "./types";

const logger = pino({ name: "reporting:indexer-source" });

// ── Indexer REST types ────────────────────────────────────────────────────────

/**
 * One row returned by GET /events.
 * Mirrors the NormalizedEvent fields from services/indexer/src/lib.rs.
 */
interface IndexerEventRow {
  contract: string;
  event_type: string;
  ledger_sequence: number;
  event_index: number;
  decoded_payload: Record<string, unknown>;
}

/** Paginated response envelope from GET /events. */
interface IndexerPage {
  events: IndexerEventRow[];
  /** Opaque cursor for the next page. null when exhausted. */
  next_cursor: string | null;
}

// ── Indexer contracts ─────────────────────────────────────────────────────────

/** Canonical contract identifiers the indexer uses. */
const CONTRACT = {
  INVOICE_NFT: "invoice_nft",
  MARKETPLACE: "marketplace",
  FINANCING_POOL: "financing_pool",
  TREASURY: "treasury",
} as const;

// ── Payload field helpers ─────────────────────────────────────────────────────

function str(v: unknown): string {
  return v != null ? String(v) : "";
}

function bigint(v: unknown): bigint {
  try {
    return BigInt(str(v));
  } catch {
    return 0n;
  }
}

function num(v: unknown): number {
  const n = Number(v);
  return isFinite(n) ? n : 0;
}

// ── Row → domain event mappers ────────────────────────────────────────────────

function toFundingEvent(row: IndexerEventRow): FundingEvent | null {
  const p = row.decoded_payload;
  const invoiceId = str(p["invoiceId"] ?? p["invoice_id"]);
  const asset = str(p["asset"]);
  const amount = bigint(p["amount"]);
  const timestamp = num(p["timestamp"] ?? p["ledger_close_time"]);
  const txHash = str(p["txHash"] ?? p["tx_hash"]);
  if (!invoiceId || !asset || !txHash) return null;
  return { invoiceId, asset, amount, timestamp, txHash };
}

function toRepaymentEvent(row: IndexerEventRow): RepaymentEvent | null {
  const p = row.decoded_payload;
  const invoiceId = str(p["invoiceId"] ?? p["invoice_id"]);
  const asset = str(p["asset"]);
  const amount = bigint(p["amount"]);
  const timestamp = num(p["timestamp"] ?? p["ledger_close_time"]);
  const txHash = str(p["txHash"] ?? p["tx_hash"]);
  if (!invoiceId || !asset || !txHash) return null;
  return { invoiceId, asset, amount, timestamp, txHash };
}

function toYieldEvent(row: IndexerEventRow): YieldEvent | null {
  const p = row.decoded_payload;
  const invoiceId = str(p["invoiceId"] ?? p["invoice_id"]);
  const asset = str(p["asset"]);
  const principalReturned = bigint(p["principalReturned"] ?? p["principal_returned"]);
  const yieldAmount = bigint(p["yieldAmount"] ?? p["yield_amount"]);
  const timestamp = num(p["timestamp"] ?? p["ledger_close_time"]);
  const txHash = str(p["txHash"] ?? p["tx_hash"]);
  if (!invoiceId || !asset || !txHash) return null;
  return { invoiceId, asset, principalReturned, yieldAmount, timestamp, txHash };
}

function toFeeEvent(row: IndexerEventRow): FeeEvent | null {
  const p = row.decoded_payload;
  const invoiceId = str(p["invoiceId"] ?? p["invoice_id"]);
  const asset = str(p["asset"]);
  const feeAmount = bigint(p["feeAmount"] ?? p["fee_amount"]);
  const feeBps = num(p["feeBps"] ?? p["fee_bps"]);
  const timestamp = num(p["timestamp"] ?? p["ledger_close_time"]);
  const txHash = str(p["txHash"] ?? p["tx_hash"]);
  if (!invoiceId || !asset || !txHash) return null;
  return { invoiceId, asset, feeAmount, feeBps, timestamp, txHash };
}

// ── IndexerReportingDataSource ────────────────────────────────────────────────

export interface IndexerDataSourceOptions {
  /** Base URL of the indexer REST service. Default: INDEXER_URL env or http://localhost:8787 */
  indexerUrl?: string;
  /** Maximum rows per page. Default: 200 */
  pageSize?: number;
  /** Inject a custom fetch implementation for testing. Default: global fetch */
  fetchFn?: typeof fetch;
}

/**
 * Production ReportingDataSource backed by the Kora indexer service.
 *
 * Queries the indexer REST API for on-chain events, paginates through all
 * results for the requested date range, and maps rows into the domain types
 * expected by StatementBuilder.
 */
export class IndexerReportingDataSource implements ReportingDataSource {
  private readonly indexerUrl: string;
  private readonly pageSize: number;
  private readonly fetchFn: typeof fetch;

  constructor(opts: IndexerDataSourceOptions = {}) {
    this.indexerUrl = (
      opts.indexerUrl ??
      process.env["INDEXER_URL"] ??
      "http://localhost:8787"
    ).replace(/\/$/, "");
    this.pageSize = opts.pageSize ?? 200;
    this.fetchFn = opts.fetchFn ?? globalThis.fetch;
  }

  // ── ReportingDataSource implementation ────────────────────────────────────

  async getUserStatement(
    userAddress: string,
    fromTs: number,
    toTs: number
  ): Promise<UserStatement> {
    const rows = await this._fetchAllEvents(fromTs, toTs);

    // Filter to events involving this user address
    const userRows = rows.filter((r) => {
      const p = r.decoded_payload;
      return (
        str(p["investor"] ?? p["userAddress"] ?? p["user_address"] ?? p["sender"]) ===
        userAddress
      );
    });

    const funding = this._collectFunding(userRows);
    const repayments = this._collectRepayments(userRows);
    const yields = this._collectYields(userRows);
    const fees = this._collectFees(userRows);

    logger.info(
      {
        userAddress,
        fromTs,
        toTs,
        funding: funding.length,
        repayments: repayments.length,
        yields: yields.length,
        fees: fees.length,
      },
      "Indexer: user statement assembled"
    );

    return {
      userAddress,
      fromTs,
      toTs,
      funding,
      repayments,
      yields,
      fees,
      summaryByAsset: [], // StatementBuilder recomputes this
    };
  }

  async getProtocolStatement(
    fromTs: number,
    toTs: number
  ): Promise<ProtocolStatement> {
    const rows = await this._fetchAllEvents(fromTs, toTs);

    const financing = rows.filter((r) => r.contract === CONTRACT.FINANCING_POOL);
    const invoiceFinanced = financing.filter((r) =>
      r.event_type === "INV_FUNDED" || r.event_type === "invoice_funded"
    ).length;
    const invoiceRepaid = financing.filter((r) =>
      r.event_type === "INV_REPAID" || r.event_type === "invoice_repaid"
    ).length;
    const invoiceDefaulted = financing.filter((r) =>
      r.event_type === "INV_DEFAULT" || r.event_type === "invoice_defaulted"
    ).length;

    logger.info(
      { fromTs, toTs, invoiceFinanced, invoiceRepaid, invoiceDefaulted },
      "Indexer: protocol statement assembled"
    );

    return {
      fromTs,
      toTs,
      totalInvoicesFinanced: invoiceFinanced,
      totalInvoicesRepaid: invoiceRepaid,
      totalInvoicesDefaulted: invoiceDefaulted,
      summaryByAsset: [], // StatementBuilder recomputes this
    };
  }

  // ── Pagination ────────────────────────────────────────────────────────────

  /**
   * Fetch all event rows for the given timestamp window.
   * Pages through cursor results until exhausted or events leave the window.
   *
   * Uses `finalized=true` to skip non-confirmed ledger rows (reorg safety).
   */
  private async _fetchAllEvents(fromTs: number, toTs: number): Promise<IndexerEventRow[]> {
    const all: IndexerEventRow[] = [];
    let cursor: string | null = null;
    let page = 0;
    const MAX_PAGES = 1000; // safety cap — ~200k rows

    do {
      const url = this._buildUrl(fromTs, toTs, cursor);
      logger.debug({ url, page }, "Fetching indexer page");

      const resp = await this.fetchFn(url, {
        headers: { Accept: "application/json" },
      });

      if (!resp.ok) {
        const body = await resp.text().catch(() => "");
        throw new Error(
          `Indexer HTTP ${resp.status}: ${body.slice(0, 200)}`
        );
      }

      const json = (await resp.json()) as IndexerPage;
      all.push(...json.events);
      cursor = json.next_cursor ?? null;
      page++;

      if (page >= MAX_PAGES) {
        logger.warn({ fromTs, toTs, totalRows: all.length }, "Indexer page cap reached — partial result");
        break;
      }
    } while (cursor !== null);

    logger.info({ fromTs, toTs, totalRows: all.length, pages: page }, "Indexer fetch complete");
    return all;
  }

  private _buildUrl(fromTs: number, toTs: number, cursor: string | null): string {
    const params = new URLSearchParams({
      from_ts: String(fromTs),
      to_ts: String(toTs),
      limit: String(this.pageSize),
      finalized: "true",
    });
    if (cursor) params.set("cursor", cursor);
    return `${this.indexerUrl}/events?${params.toString()}`;
  }

  // ── Event collection helpers ──────────────────────────────────────────────

  private _collectFunding(rows: IndexerEventRow[]): FundingEvent[] {
    return rows
      .filter(
        (r) =>
          r.event_type === "INV_FUNDED" ||
          r.event_type === "invoice_funded" ||
          r.event_type === "fund_invoice"
      )
      .map(toFundingEvent)
      .filter((e): e is FundingEvent => e !== null);
  }

  private _collectRepayments(rows: IndexerEventRow[]): RepaymentEvent[] {
    return rows
      .filter(
        (r) =>
          r.event_type === "INV_REPAID" ||
          r.event_type === "invoice_repaid" ||
          r.event_type === "repay_invoice"
      )
      .map(toRepaymentEvent)
      .filter((e): e is RepaymentEvent => e !== null);
  }

  private _collectYields(rows: IndexerEventRow[]): YieldEvent[] {
    return rows
      .filter(
        (r) =>
          r.event_type === "YIELD_DIST" ||
          r.event_type === "yield_distributed" ||
          r.event_type === "distribute_yield"
      )
      .map(toYieldEvent)
      .filter((e): e is YieldEvent => e !== null);
  }

  private _collectFees(rows: IndexerEventRow[]): FeeEvent[] {
    return rows
      .filter(
        (r) =>
          r.event_type === "FEE_COLL" ||
          r.event_type === "fee_collected" ||
          r.event_type === "collect_fee"
      )
      .map(toFeeEvent)
      .filter((e): e is FeeEvent => e !== null);
  }
}
