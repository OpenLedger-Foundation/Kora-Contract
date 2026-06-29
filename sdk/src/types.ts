import { xdr } from "@stellar/stellar-sdk";

// ── Enums ─────────────────────────────────────────────────────────────────────

export type InvoiceStatus = "Created" | "Listed" | "Funded" | "Repaid" | "Defaulted";
export type RiskTier = "AAA" | "AA" | "A" | "B" | "C";

// ── Structs ───────────────────────────────────────────────────────────────────

export interface Invoice {
  id: bigint;
  sme: string;
  debtorHash: Buffer;
  amount: bigint;
  currency: string;
  dueDate: bigint;
  ipfsCid: string;
  riskScore: number;
  riskTier: RiskTier;
  status: InvoiceStatus;
  createdAt: bigint;
  fundedAt: bigint | null;
  repaidAt: bigint | null;
}

export interface Listing {
  invoiceId: bigint;
  seller: string;
  askingPrice: bigint;
  faceValue: bigint;
  token: string;
  fundedAmount: bigint;
  fundingDeadline: bigint;
  isActive: boolean;
}

export interface Pool {
  invoiceId: bigint;
  token: string;
  totalFunded: bigint;
  faceValue: bigint;
  repaidAmount: bigint;
  isClosed: boolean;
  latePenaltyBps: number;
  totalOwed: bigint;
  penaltyApplied: boolean;
}

export interface Position {
  investor: string;
  invoiceId: bigint;
  contributed: bigint;
  shareBps: number;
  yieldClaimed: bigint;
}

export interface SmeProfile {
  address: string;
  verified: boolean;
  verifier: string;
  riskScore: number;
  totalInvoices: number;
  defaults: number;
  registeredAt: bigint;
}

export interface MarketplaceConfig {
  admin: string;
  invoiceNft: string;
  financingPool: string;
  treasury: string;
  accessControl: string;
  feeBps: number;
}
