import type {
  MarketplaceFilters,
  MarketplaceFundingStatus,
  MarketplaceListing,
  MarketplaceRiskTier,
  MarketplaceSort,
  MarketplaceSortDirection,
  MarketplaceSortField,
  MarketplaceState,
} from '../../types/marketplace';

export const MARKETPLACE_RISK_TIERS: readonly MarketplaceRiskTier[] = ['AAA', 'AA', 'A', 'B', 'C'];
export const MARKETPLACE_FUNDING_STATUSES: readonly MarketplaceFundingStatus[] = [
  'open',
  'partially_funded',
  'fully_funded',
  'closed',
];
const SORT_FIELDS: readonly MarketplaceSortField[] = [
  'createdAt',
  'expectedYield',
  'tenorDays',
  'faceValue',
  'riskScore',
  'fundingProgress',
];

export const DEFAULT_MARKETPLACE_SORT: MarketplaceSort = { field: 'createdAt', direction: 'desc' };
export const DEFAULT_PAGE = 1;
export const DEFAULT_PAGE_SIZE = 20;
const MAX_PAGE_SIZE = 100;

function parseList<T extends string>(raw: string | null, allowed: readonly T[]): T[] | undefined {
  if (!raw) return undefined;
  const values = raw
    .split(',')
    .map((v) => v.trim())
    .filter((v): v is T => (allowed as readonly string[]).includes(v));
  return values.length > 0 ? values : undefined;
}

function parseNonNegativeInt(raw: string | null): number | undefined {
  if (raw === null || raw.trim() === '') return undefined;
  const n = Number(raw);
  return Number.isInteger(n) && n >= 0 ? n : undefined;
}

/**
 * Parses marketplace state from a URL query string. Invalid values are
 * dropped and fall back to defaults. HTML-escaped separators (`&amp;`) from
 * copied links are tolerated.
 */
export function parseMarketplaceSearchParams(search: string): MarketplaceState {
  const params = new URLSearchParams(search.replace(/&amp;/g, '&'));
  const filters: MarketplaceFilters = {};

  const riskTiers = parseList(params.get('riskTier'), MARKETPLACE_RISK_TIERS);
  if (riskTiers) filters.riskTiers = riskTiers;
  const fundingStatuses = parseList(params.get('fundingStatus'), MARKETPLACE_FUNDING_STATUSES);
  if (fundingStatuses) filters.fundingStatuses = fundingStatuses;

  const numeric: Array<[keyof MarketplaceFilters, string]> = [
    ['tenorMinDays', 'tenorMinDays'],
    ['tenorMaxDays', 'tenorMaxDays'],
    ['yieldMinB0ps', 'yieldMinB0ps'],
    ['yieldMaxB0ps', 'yieldMaxB0ps'],
  ];
  for (const [key, param] of numeric) {
    const value = parseNonNegativeInt(params.get(param));
    if (value !== undefined) (filters as Record<string, number>)[key] = value;
  }

  const search_ = params.get('search')?.trim();
  if (search_) filters.search = search_;

  const field = params.get('sortField') as MarketplaceSortField | null;
  const direction = params.get('sortDirection') as MarketplaceSortDirection | null;
  const sort: MarketplaceSort = {
    field: field && SORT_FIELDS.includes(field) ? field : DEFAULT_MARKETPLACE_SORT.field,
    direction: direction === 'asc' || direction === 'desc' ? direction : DEFAULT_MARKETPLACE_SORT.direction,
  };

  const page = parseNonNegativeInt(params.get('page'));
  const pageSize = parseNonNegativeInt(params.get('pageSize'));

  return {
    filters,
    sort,
    page: page && page >= 1 ? page : DEFAULT_PAGE,
    pageSize: pageSize && pageSize >= 1 ? Math.min(pageSize, MAX_PAGE_SIZE) : DEFAULT_PAGE_SIZE,
  };
}

/** Serializes marketplace state to a query string (without leading `?`). Defaults are omitted. */
export function serializeMarketplaceState(state: MarketplaceState): string {
  const params = new URLSearchParams();
  const { filters, sort } = state;
  if (filters.riskTiers?.length) params.set('riskTier', filters.riskTiers.join(','));
  if (filters.tenorMinDays !== undefined) params.set('tenorMinDays', String(filters.tenorMinDays));
  if (filters.tenorMaxDays !== undefined) params.set('tenorMaxDays', String(filters.tenorMaxDays));
  if (filters.yieldMinB0ps !== undefined) params.set('yieldMinB0ps', String(filters.yieldMinB0ps));
  if (filters.yieldMaxB0ps !== undefined) params.set('yieldMaxB0ps', String(filters.yieldMaxB0ps));
  if (filters.fundingStatuses?.length) params.set('fundingStatus', filters.fundingStatuses.join(','));
  if (filters.search) params.set('search', filters.search);
  if (sort.field !== DEFAULT_MARKETPLACE_SORT.field) params.set('sortField', sort.field);
  if (sort.direction !== DEFAULT_MARKETPLACE_SORT.direction) params.set('sortDirection', sort.direction);
  if (state.page !== DEFAULT_PAGE) params.set('page', String(state.page));
  if (state.pageSize !== DEFAULT_PAGE_SIZE) params.set('pageSize', String(state.pageSize));
  return params.toString();
}

function sortValue(listing: MarketplaceListing, field: MarketplaceSortField): number | string {
  switch (field) {
    case 'expectedYield':
      return listing.expectedYieldB0ps;
    case 'tenorDays':
      return listing.tenorDays;
    case 'faceValue':
      return listing.faceValue;
    case 'riskScore':
      return listing.riskScore;
    case 'fundingProgress':
      return listing.fundingProgress;
    default:
      return listing.createdAt;
  }
}

/** Applies filters and sorting. Returns a new array; the input is not mutated. */
export function applyMarketplaceFilters(
  listings: MarketplaceListing[],
  filters: MarketplaceFilters,
  sort: MarketplaceSort,
): MarketplaceListing[] {
  const term = filters.search?.toLowerCase();
  const filtered = listings.filter((l) => {
    if (filters.riskTiers?.length && !filters.riskTiers.includes(l.riskTier)) return false;
    if (filters.fundingStatuses?.length && !filters.fundingStatuses.includes(l.fundingStatus)) return false;
    if (filters.tenorMinDays !== undefined && l.tenorDays < filters.tenorMinDays) return false;
    if (filters.tenorMaxDays !== undefined && l.tenorDays > filters.tenorMaxDays) return false;
    if (filters.yieldMinB0ps !== undefined && l.expectedYieldB0ps < filters.yieldMinB0ps) return false;
    if (filters.yieldMaxB0ps !== undefined && l.expectedYieldB0ps > filters.yieldMaxB0ps) return false;
    if (term && !l.invoiceNumber.toLowerCase().includes(term) && !l.debtorName.toLowerCase().includes(term)) {
      return false;
    }
    return true;
  });
  const dir = sort.direction === 'asc' ? 1 : -1;
  return filtered.sort((a, b) => {
    const av = sortValue(a, sort.field);
    const bv = sortValue(b, sort.field);
    if (av < bv) return -dir;
    if (av > bv) return dir;
    return 0;
  });
}
