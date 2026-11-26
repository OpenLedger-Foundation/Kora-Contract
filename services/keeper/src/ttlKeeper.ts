/** Active-listing discovery and Soroban persistent-entry TTL checks. */

import { Address, SorobanRpc, xdr } from "@stellar/stellar-sdk";
import { KeeperConfig, Deadline } from "./types";

export interface ActiveListing {
  invoiceId: string;
  isActive: boolean;
}

interface ListingPage {
  data: ActiveListing[];
  nextCursor: string | null;
}

export interface LedgerEntry {
  key: xdr.LedgerKey;
  liveUntilLedgerSeq: number;
}

export interface TtlRpc {
  getLedgerEntries(...keys: xdr.LedgerKey[]): Promise<{
    latestLedger: number;
    entries: LedgerEntry[];
  }>;
}

export interface ListingSource {
  getActiveListings(): Promise<ActiveListing[]>;
}

export class HttpListingSource implements ListingSource {
  constructor(
    private readonly endpoint: string,
    private readonly apiKey: string
  ) {}

  async getActiveListings(): Promise<ActiveListing[]> {
    const listings: ActiveListing[] = [];
    const seenCursors = new Set<string>();
    let cursor: string | null = null;

    do {
      const url = new URL(this.endpoint);
      url.searchParams.set("isActive", "true");
      url.searchParams.set("limit", "100");
      if (cursor) url.searchParams.set("cursor", cursor);

      const response = await fetch(url, {
        headers: this.apiKey ? { Authorization: `Bearer ${this.apiKey}` } : {},
      });
      if (!response.ok) {
        throw new Error(`Active-listings endpoint returned HTTP ${response.status}`);
      }

      const page = (await response.json()) as ListingPage;
      if (!Array.isArray(page.data)) {
        throw new Error("Active-listings endpoint response must contain a data array");
      }
      listings.push(...page.data.filter((listing) => listing.isActive));

      cursor = page.nextCursor;
      if (cursor && seenCursors.has(cursor)) {
        throw new Error("Active-listings endpoint repeated a pagination cursor");
      }
      if (cursor) seenCursors.add(cursor);
    } while (cursor);

    return listings;
  }
}

/**
 * Build the persistent ledger key for a marketplace `Listing(invoiceId)` entry.
 * Shared by the live keeper and the dry-run analyzer so both reason about the
 * exact same on-chain key.
 */
export function buildListingLedgerKey(
  contractAddress: string,
  invoiceId: string
): xdr.LedgerKey {
  const dataKey = xdr.ScVal.scvVec([
    xdr.ScVal.scvSymbol("Listing"),
    xdr.ScVal.scvU64(xdr.Uint64.fromString(invoiceId)),
  ]);
  return xdr.LedgerKey.contractData(
    new xdr.LedgerKeyContractData({
      contract: new Address(contractAddress).toScAddress(),
      key: dataKey,
      durability: xdr.ContractDataDurability.persistent(),
    })
  );
}

/**
 * Build the ledger key for the marketplace contract *instance* entry (the
 * contract's instance storage / Wasm reference). This is the key Soroban's
 * bump/restore footprint helpers use for the contract instance itself.
 */
export function buildInstanceLedgerKey(contractAddress: string): xdr.LedgerKey {
  return xdr.LedgerKey.contractData(
    new xdr.LedgerKeyContractData({
      contract: new Address(contractAddress).toScAddress(),
      key: xdr.ScVal.scvLedgerKeyContractInstance(),
      durability: xdr.ContractDataDurability.persistent(),
    })
  );
}

export class ListingTtlKeeper {
  private readonly rpc: TtlRpc;
  private readonly source: ListingSource;
  private scannedTotal = 0;
  private atRiskTotal = 0;
  private scanErrorsTotal = 0;

  constructor(
    private readonly config: KeeperConfig,
    rpc?: TtlRpc,
    source?: ListingSource
  ) {
    this.rpc = rpc ?? (new SorobanRpc.Server(config.rpcUrl) as unknown as TtlRpc);
    this.source = source ?? new HttpListingSource(config.activeListingsUrl, config.indexerApiKey);
  }

  async scan(): Promise<Deadline[]> {
    try {
      const listings = await this.source.getActiveListings();
      this.scannedTotal += listings.length;
      const validListings = listings.filter((listing) => /^\d+$/.test(listing.invoiceId));
      const bumps: Deadline[] = [];

      for (let offset = 0; offset < validListings.length; offset += 100) {
        const batch = validListings.slice(offset, offset + 100);
        const keyedListings = batch.map((listing) => ({
          invoiceId: listing.invoiceId,
          key: this._listingLedgerKey(listing.invoiceId),
        }));
        const response = await this.rpc.getLedgerEntries(
          ...keyedListings.map(({ key }) => key)
        );
        const entries = new Map(
          response.entries.map((entry) => [entry.key.toXDR("base64"), entry])
        );

        for (const { invoiceId, key } of keyedListings) {
          const entry = entries.get(key.toXDR("base64"));
          if (!entry || entry.liveUntilLedgerSeq - response.latestLedger >= this.config.ttlThresholdLedgers) {
            continue;
          }

          this.atRiskTotal++;
          bumps.push({
            dedupKey: `listing_ttl:${invoiceId}:${entry.liveUntilLedgerSeq}`,
            kind: "listing_ttl",
            invoiceId: BigInt(invoiceId),
            deadlineTs: Math.floor(Date.now() / 1000),
            contractAddress: this.config.marketplaceContractAddress,
          });
        }
      }

      return bumps;
    } catch (error) {
      this.scanErrorsTotal++;
      throw error;
    }
  }

  getMetrics(): { scanned: number; atRisk: number; errors: number } {
    return {
      scanned: this.scannedTotal,
      atRisk: this.atRiskTotal,
      errors: this.scanErrorsTotal,
    };
  }

  private _listingLedgerKey(invoiceId: string): xdr.LedgerKey {
    return buildListingLedgerKey(this.config.marketplaceContractAddress, invoiceId);
  }
}