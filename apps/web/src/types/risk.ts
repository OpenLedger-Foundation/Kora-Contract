export type RiskTier = 'LOW' | 'MEDIUM' | 'HIGH' | 'CRITICAL';
export type RiskStatus = 'ACTIVE' | 'UNDER_REVIEW' | 'DISPUTED';

export interface RiskFactor {
  id: string;
  name: string;
  category: 'FINANCIAL' | 'REPAYMENT_HISTORY' | 'CONCENTRATION' | 'INDUSTRY' | 'TENOR';
  score: number; // 0 - 100
  weightPercentage: number; // e.g. 30 for 30%
  description: string;
}

export interface RiskBreakdown {
  invoiceId: string;
  compositeScore: number; // 0 - 100
  tier: RiskTier;
  status: RiskStatus;
  /** e.g. 92 (%). `null` when no risk-pipeline confidence is available. */
  confidencePercentage: number | null;
  verifierCount: number;
  factors: RiskFactor[];
  lastUpdated: string;
  disputeNotice?: string;
  /** `sdk` = invoice_nft + risk_registry (+ risk pipeline); `dev-fixture` = sample data. */
  source?: 'sdk' | 'dev-fixture';
  /** On-chain tier from invoice_nft (AAA, AA, A, B, C). */
  onChainTier?: string;
  /** SME profile from risk_registry, when registered. */
  registry?: RegistryRiskFields | null;
  /** Shown when the risk-pipeline breakdown could not be loaded. */
  pipelineNotice?: string;
  /** Non-fatal warning (e.g. stale deployment manifest). */
  notice?: string;
}

export interface RegistryRiskFields {
  smeAddress: string;
  verified: boolean;
  smeRiskScore: number;
  totalInvoices: number;
  defaults: number;
  registeredAt?: string;
}

/** `SuggestedScore` shape from services/risk-pipeline, as served by the gateway. */
export interface PipelineSuggestion {
  score: number;
  confidence: number;
  formulaVersion: string;
  breakdown: Array<{
    factorId: 'payment_history' | 'invoice_frequency' | 'debtor_concentration' | 'macro_trade' | string;
    score: number;
    appliedWeight: number;
    confidence: number;
    missing: boolean;
    rationale: string;
  }>;
  warnings: string[];
  computedAt: number;
}
