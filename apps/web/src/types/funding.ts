export type FundingStep = 'AMOUNT_SELECTION' | 'YIELD_PREVIEW' | 'SIMULATION_AND_SIGN';

export interface FeeBreakdown {
  grossAmountUsd: number;
  platformFeePercentage: number;
  platformFeeUsd: number;
  netContributionUsd: number;
}

export interface YieldPreview {
  contributionAmountUsd: number;
  annualYieldPercentage: number;
  tenorDays: number;
  expectedGrossYieldUsd: number;
  platformFeeUsd: number;
  expectedNetYieldUsd: number;
  estimatedReturnOnInvestmentPct: number;
}

export interface ConcentrationCapCheck {
  debtorName: string;
  currentExposureUsd: number;
  proposedContributionUsd: number;
  maxCapUsd: number;
  currentPercentage: number;
  projectedPercentage: number;
  willBreach: boolean;
  breachReason?: string;
}

export interface ListingFundingState {
  listingId: number;
  invoiceId: string;
  debtorName: string;
  targetFundingUsd: number;
  currentFundingUsd: number;
  remainingFundingUsd: number;
  minContributionUsd: number;
  maxConcentrationCapUsd: number;
  tenorDays: number;
  annualYieldPercentage: number;
  isFullyFunded: boolean;
  /** False when the listing is cancelled, expired or the invoice left `Listed`. */
  isOpenForFunding?: boolean;
  /** `sdk` = read from contracts; `dev-fixture` = local sample data (never in production). */
  source?: 'sdk' | 'dev-fixture';
  /** Marketplace fee from `get_fee_bps`, as a percentage. */
  platformFeePercentage?: number;
  /** ISO timestamp of the listing's funding deadline. */
  fundingDeadline?: string;
  /** Non-fatal warning to show with the data (e.g. stale deployment manifest). */
  notice?: string;
}
