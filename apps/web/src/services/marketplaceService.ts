import type { MarketplaceListing, MarketplacePage, MarketplaceState } from '../types/marketplace';
import { applyMarketplaceFilters } from '../components/marketplace/marketplaceFilters';

/** Filters, sorts and paginates marketplace listings for the browsing view. */
export class MarketplaceService {
  public query(listings: MarketplaceListing[], state: MarketplaceState): MarketplacePage {
    const matched = applyMarketplaceFilters(listings, state.filters, state.sort);
    const totalPages = Math.max(1, Math.ceil(matched.length / state.pageSize));
    const page = Math.min(Math.max(1, state.page), totalPages);
    const start = (page - 1) * state.pageSize;
    return {
      items: matched.slice(start, start + state.pageSize),
      total: matched.length,
      page,
      pageSize: state.pageSize,
      totalPages,
    };
  }
}

export const marketplaceService = new MarketplaceService();
