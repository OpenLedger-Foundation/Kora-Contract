/**
 * Reporting Service — Types
 *
 * Domain types for data export and regulatory reporting.
 * Issue #770: Build Data Export Service for Regulatory Reporting.
 */

/** Supported export formats. */
export type ExportFormat = "csv" | "pdf";

/** Scope of the export. */
export type ExportScope = "per_user" | "protocol_wide";

/** Status of an asynchronous export job. */
export type ExportStatus = "pending" | "processing" | "ready" | "failed";

/** A funding event: investor funded an invoice. */
export interface FundingEvent {
  invoiceId: string;
  asset: string;           // e.g. "USDC", "EURC"
  amount: bigint;          // in stroops (7 decimal places)
  timestamp: number;       // Unix seconds
  txHash: string;
}

/** A repayment event: SME repaid an invoice. */
export interface RepaymentEvent {
  invoiceId: string;
  asset: string;
  amount: bigint;
  timestamp: number;
  txHash: string;
}

/** A yield distribution event: investor received yield. */
export interface YieldEvent {
  invoiceId: string;
  asset: string;
  principalReturned: bigint;
  yieldAmount: bigint;
  timestamp: number;
  txHash: string;
}

/** A fee event: protocol fee collected. */
export interface FeeEvent {
  invoiceId: string;
  asset: string;
  feeAmount: bigint;
  feeBps: number;
  timestamp: number;
  txHash: string;
}

/** Per-asset totals — avoids conflating different settlement assets. */
export interface AssetSummary {
  asset: string;
  totalFunded: bigint;
  totalRepaid: bigint;
  totalYield: bigint;
  totalFees: bigint;
}

/** All event data for one user over a date range. */
export interface UserStatement {
  userAddress: string;
  fromTs: number;
  toTs: number;
  funding: FundingEvent[];
  repayments: RepaymentEvent[];
  yields: YieldEvent[];
  fees: FeeEvent[];
  /** Totals broken down by asset — never conflates USDC with EURC etc. */
  summaryByAsset: AssetSummary[];
}

/** Protocol-wide aggregate export. */
export interface ProtocolStatement {
  fromTs: number;
  toTs: number;
  totalInvoicesFinanced: number;
  totalInvoicesRepaid: number;
  totalInvoicesDefaulted: number;
  summaryByAsset: AssetSummary[];
}

/** An asynchronous export job record. */
export interface ExportJob {
  jobId: string;
  scope: ExportScope;
  format: ExportFormat;
  userAddress?: string;    // set for per_user scope
  fromTs: number;
  toTs: number;
  status: ExportStatus;
  outputPath?: string;     // local file path when ready
  errorMessage?: string;
  requestedAt: number;
  completedAt?: number;
}

// ── Treasury Transparency Types (Issue #1099) ─────────────────────────────────

/** A treasury fee/inflow event. */
export interface TreasuryInflow {
  id: string;
  asset: string;
  amount: string;          // stroops as decimal string (7 decimal places)
  source: string;
  invoiceId?: string;
  txHash: string;
  ledgerSequence: number;
  timestamp: number;       // Unix seconds
}

/** A treasury withdrawal/outflow event. */
export interface TreasuryOutflow {
  id: string;
  asset: string;
  amount: string;          // stroops as decimal string
  recipient: string;
  reason?: string;
  txHash: string;
  ledgerSequence: number;
  timestamp: number;       // Unix seconds
}

/** Per-token treasury balance summary. */
export interface TreasuryBalance {
  asset: string;
  balance: string;         // stroops as decimal string
  totalInflows: string;
  totalOutflows: string;
  reserveBalance?: string;
}

/** Indexer sync status for treasury transparency responses. */
export interface TreasuryIndexerStatus {
  lastIndexedLedger: number;
  latestChainLedger?: number;
  partial: boolean;
  lagWarning?: string;
}

/** Query options for treasury inflows/outflows endpoints. */
export interface TreasuryQueryOptions {
  asset?: string;
  fromTs?: number;
  toTs?: number;
  limit?: number;
  offset?: number;
}

/** Response envelope for GET /treasury/balances. */
export interface TreasuryBalancesResponse extends TreasuryIndexerStatus {
  balances: TreasuryBalance[];
  /**
   * Asset codes present in `balances`, sorted (issue #1184). Added by the HTTP
   * layer; data sources may omit it.
   */
  assets?: string[];
}

/** Response envelope for GET /treasury/inflows. */
export interface TreasuryInflowsResponse extends TreasuryIndexerStatus {
  inflows: TreasuryInflow[];
  total: number;
}

/** Response envelope for GET /treasury/outflows. */
export interface TreasuryOutflowsResponse extends TreasuryIndexerStatus {
  outflows: TreasuryOutflow[];
  total: number;
}
