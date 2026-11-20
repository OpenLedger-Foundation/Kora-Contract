import { xdr } from "@stellar/stellar-sdk";

// ── Enums ─────────────────────────────────────────────────────────────────────

export type InvoiceStatus = "Created" | "Listed" | "Funded" | "Repaid" | "Defaulted";
export type RiskTier = "AAA" | "AA" | "A" | "B" | "C";

// ── Structs ───────────────────────────────────────────────────────────────────

/**
 * On-chain Invoice type — schema v2.
 *
 * v1 → v2 migration (invoice_nft::migrate) added:
 *   - `metadataHash`: 32-byte SHA-256 content commitment of the off-chain
 *     metadata document.  Empty `Buffer` (`Bytes` of length 0) on pre-v2
 *     records until `commit_metadata_hash` is called.
 *   - `notes`: optional free-text memo.  `null` on pre-v2 records.
 *
 * dApps must handle both old (pre-migration) and new records:
 *   - Check `metadataHash.length === 32` before treating it as a committed hash.
 *   - Treat `notes === null` as "no memo" rather than an error.
 */
export interface Invoice {
  id: bigint;
  sme: string;
  debtorHash: Buffer;
  amount: bigint;
  currency: string;
  dueDate: bigint;
  ipfsCid: string;
  /** SHA-256 content commitment (32 bytes) committed via `commit_metadata_hash`.
   *  Empty Buffer on unmigrated or pre-commitment records. */
  metadataHash: Buffer;
  riskScore: number;
  riskTier: RiskTier;
  status: InvoiceStatus;
  createdAt: bigint;
  fundedAt: bigint | null;
  repaidAt: bigint | null;
  /** Optional free-text memo set at minting time. Added in schema v2.
   *  `null` on records minted before the v2 migration. */
  notes: string | null;
}

/**
 * Decode a Soroban `ScVal` map returned by `get_invoice` into the typed
 * `Invoice` interface.  Handles both v1 (pre-migration) and v2 records so
 * dApps never silently drop `metadataHash` or `notes`.
 *
 * @param val - Raw `ScVal` as returned by the RPC `simulateTransaction`.
 * @returns Decoded `Invoice` with all v2 fields populated (empty / null for
 *          unmigrated records).
 */
export function decodeInvoice(val: xdr.ScVal): Invoice {
  if (val.switch() !== xdr.ScValType.scvMap()) {
    throw new Error("decodeInvoice: expected ScvMap, got " + val.switch().name);
  }
  const map = new Map<string, xdr.ScVal>();
  for (const entry of val.map()!) {
    map.set(entry.key().sym().toString(), entry.val());
  }

  function reqU64(key: string): bigint {
    const v = map.get(key);
    if (!v) throw new Error(`decodeInvoice: missing field "${key}"`);
    return BigInt(v.u64().toString());
  }
  function optU64(key: string): bigint | null {
    const v = map.get(key);
    if (!v) return null;
    // Option<u64> is encoded as ScvVec([]) for None or ScvVec([u64]) for Some.
    if (v.switch() === xdr.ScValType.scvVec()) {
      const inner = v.vec();
      if (!inner || inner.length === 0) return null;
      return BigInt(inner[0].u64().toString());
    }
    return BigInt(v.u64().toString());
  }
  function reqI128(key: string): bigint {
    const v = map.get(key);
    if (!v) throw new Error(`decodeInvoice: missing field "${key}"`);
    const parts = v.i128();
    return (BigInt(parts.hi().toString()) << 64n) | BigInt(parts.lo().toString());
  }
  function reqStr(key: string): string {
    const v = map.get(key);
    if (!v) throw new Error(`decodeInvoice: missing field "${key}"`);
    return v.str().toString();
  }
  function reqSym(key: string): string {
    const v = map.get(key);
    if (!v) throw new Error(`decodeInvoice: missing field "${key}"`);
    return v.sym().toString();
  }
  function reqBytes(key: string): Buffer {
    const v = map.get(key);
    if (!v) return Buffer.alloc(0);
    if (v.switch() === xdr.ScValType.scvBytes()) return Buffer.from(v.bytes());
    return Buffer.alloc(0);
  }
  function optStr(key: string): string | null {
    const v = map.get(key);
    if (!v) return null;
    if (v.switch() === xdr.ScValType.scvVec()) {
      const inner = v.vec();
      if (!inner || inner.length === 0) return null;
      return inner[0].str().toString();
    }
    return null;
  }
  function reqAddr(key: string): string {
    const v = map.get(key);
    if (!v) throw new Error(`decodeInvoice: missing field "${key}"`);
    return v.address().accountId().ed25519().toString("hex");
  }
  function reqEnum(key: string): string {
    const v = map.get(key);
    if (!v) throw new Error(`decodeInvoice: missing field "${key}"`);
    if (v.switch() === xdr.ScValType.scvVec()) {
      const inner = v.vec();
      if (inner && inner.length > 0) return inner[0].sym().toString();
    }
    return v.sym().toString();
  }
  function reqU32(key: string): number {
    const v = map.get(key);
    if (!v) throw new Error(`decodeInvoice: missing field "${key}"`);
    return v.u32();
  }

  return {
    id: reqU64("id"),
    sme: reqAddr("sme"),
    debtorHash: reqBytes("debtor_hash"),
    amount: reqI128("amount"),
    currency: reqSym("currency"),
    dueDate: reqU64("due_date"),
    ipfsCid: reqStr("ipfs_cid"),
    metadataHash: reqBytes("metadata_hash"),
    riskScore: reqU32("risk_score"),
    riskTier: reqEnum("risk_tier") as RiskTier,
    status: reqEnum("status") as InvoiceStatus,
    createdAt: reqU64("created_at"),
    fundedAt: optU64("funded_at"),
    repaidAt: optU64("repaid_at"),
    notes: optStr("notes"),
  };
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

// ── Governance Enums ──────────────────────────────────────────────────────────

export type VotingMode = "Standard" | "Quadratic";
export type ProposalType = "Financial" | "Sentiment" | "Upgrade" | "ParameterChange";
export type VoteChoice = "For" | "Against" | "Abstain";

// ── Dispute Escrow Structs ────────────────────────────────────────────────────

export interface EscrowDispute {
  invoiceId: bigint;
  challenger: string;
  payer: string;
  token: string;
  amount: bigint;
  openedAt: bigint;
  resolved: boolean;
  paidToPool: boolean;
}

// ── Fractionalizer Structs ────────────────────────────────────────────────────

export interface WrapRecord {
  invoiceId: bigint;
  originalInvestor: string;
  totalShares: bigint;
  outstandingShares: bigint;
  contributed: bigint;
  token: string;
  totalPayout: bigint;
  settled: boolean;
}

// ── Secondary Market Structs ──────────────────────────────────────────────────

export interface PositionSaleOffer {
  seller: string;
  invoiceId: bigint;
  token: string;
  price: bigint;
}

// ── Governance Structs ────────────────────────────────────────────────────────

export interface Proposal {
  id: bigint;
  proposer: string;
  title: string;
  description: string;
  proposalType: ProposalType;
  votingMode: VotingMode;
  votesFor: bigint;
  votesAgainst: bigint;
  votesAbstain: bigint;
  createdAt: bigint;
  expiresAt: bigint;
  executed: boolean;
  quorumRequired: bigint;
}

export interface VoteRecord {
  voter: string;
  choice: VoteChoice;
  weight: bigint;
  voiceCreditsSpent: bigint;
  votedAt: bigint;
}

export interface DelegationRecord {
  delegator: string;
  delegate: string;
  delegatedAt: bigint;
}

