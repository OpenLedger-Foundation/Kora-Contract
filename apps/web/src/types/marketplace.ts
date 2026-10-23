/**
 * Investor marketplace browsing types (#773).
 *
 * Basis-point fields keep the `B0ps` spelling used by the marketplace
 * filters and URL parameters.
 */

/** On-chain risk tiers (mirrors `RiskTier` in `@kora-protocol/sdk`). */
export type MarketplaceRiskTier = 'AAA' | 'AA' | 'A' | 'B' | 'C';

export type MarketplaceFundingStatus = 'open' | 'partially_funded' | 'fully_funded' | 'closed';

export type MarketplaceSortField =
  | 'createdAt'
  | 'expectedYield'
  | 'tenorDays'
  | 'faceValue'
  | 'riskScore'
  | 'fundingProgress';

export type MarketplaceSortDirection = 'asc' | 'desc';

export interface MarketplaceListing {
  id: string;
  invoiceNumber: string;
  debtorName: string;
  debtorCountry: string;
  currency: string;
  faceValue: number;
  discountRateB0ps: number;
  expectedYieldB0ps: number;
  tenorDays: number;
  riskTier: MarketplaceRiskTier;
  riskScore: number;
  fundingStatus: MarketplaceFundingStatus;
  fundedAmount: number;
  targetAmount: number;
  /** 0–100 */
  fundingProgress: number;
  maturityDate: string;
  createdAt: string;
  updatedAt: string;
}

export interface MarketplaceFilters {
  riskTiers?: MarketplaceRiskTier[];
  tenorMinDays?: number;
  tenorMaxDays?: number;
  yieldMinB0ps?: number;
  yieldMaxB0ps?: number;
  fundingStatuses?: MarketplaceFundingStatus[];
  search?: string;
}

export interface MarketplaceSort {
  field: MarketplaceSortField;
  direction: MarketplaceSortDirection;
}

export interface MarketplaceState {
  filters: MarketplaceFilters;
  sort: MarketplaceSort;
  page: number;
  pageSize: number;
}

export interface MarketplacePage {
  items: MarketplaceListing[];
  total: number;
  page: number;
  pageSize: number;
  totalPages: number;
}
