/**
 * Keeper Service — TTL-bump Dry-Run Analyzer
 *
 * Rehearses the TTL-bump decision WITHOUT writing anything. It scans the
 * marketplace contract's persistent `Listing` entries together with the
 * contract *instance* entry, classifies each as instance vs persistent, and
 * reports which are below the configured TTL threshold (i.e. need a bump).
 *
 * It performs read-only `getLedgerEntries` calls only — it never enqueues,
 * dispatches, or submits a transaction — so operators can run it safely before
 * paying for a mass extend. Removing the wrong key or extending an already-live
 * key costs fees; the dry-run shows exactly what the live keeper WOULD bump.
 *
 * Issue: #<keeper-dry-run>
 */

import { SorobanRpc, xdr } from "@stellar/stellar-sdk";
import {
  buildInstanceLedgerKey,
  buildListingLedgerKey,
  HttpListingSource,
  type ListingSource,
  type TtlRpc,
} from "./ttlKeeper";
import { KeeperConfig } from "./types";

export type KeyDurability = "instance" | "persistent";

export interface DryRunEntry {
  /** Human-readable key identifier: "instance" or "listing:<invoiceId>". */
  key: string;
  durability: KeyDurability;
  /** Invoice id for listing entries; undefined for the contract instance. */
  invoiceId?: bigint;
  liveUntilLedgerSeq: number;
  remainingLedgers: number;
  needsBump: boolean;
  /** True when the entry was absent from the ledger (archived/expired/never written). */
  missing: boolean;
}

export interface DryRunSummary {
  total: number;
  instance: number;
  persistent: number;
  needsBump: number;
  healthy: number;
  missing: number;
}

export interface DryRunReport {
  contractAddress: string;
  latestLedger: number;
  thresholdLedgers: number;
  entries: DryRunEntry[];
  summary: DryRunSummary;
}

/** Typed configuration failure (fail-closed). */
export class DryRunConfigError extends Error {
  readonly code = "DRY_RUN_CONFIG";
}

interface PlannedKey {
  xdrKey: xdr.LedgerKey;
  durability: KeyDurability;
  label: string;
  invoiceId?: bigint;
}

export class TtlDryRun {
  private readonly rpc: TtlRpc;
  private readonly source: ListingSource;

  constructor(
    private readonly config: KeeperConfig,
    rpc?: TtlRpc,
    source?: ListingSource,
  ) {
    if (!config.marketplaceContractAddress) {
      throw new DryRunConfigError(
        "MARKETPLACE_CONTRACT_ID is required for a dry run",
      );
    }
    if (
      !Number.isSafeInteger(config.ttlThresholdLedgers) ||
      config.ttlThresholdLedgers <= 0
    ) {
      throw new DryRunConfigError(
        "KEEPER_TTL_THRESHOLD_LEDGERS must be a positive integer for a dry run",
      );
    }
    this.rpc = rpc ?? (new SorobanRpc.Server(config.rpcUrl) as unknown as TtlRpc);
    this.source =
      source ?? new HttpListingSource(config.activeListingsUrl, config.indexerApiKey);
  }

  /**
   * Run a read-only TTL rehearsal. Classifies each watched key as instance or
   * persistent and reports which need a TTL bump. Never enqueues, dispatches,
   * or submits a transaction.
   */
  async analyze(): Promise<DryRunReport> {
    const planned: PlannedKey[] = [];

    // 1. The contract instance entry (instance durability). This is the key
    //    shared by all of the contract's instance storage; bumping it is cheap
    //    and covers the whole instance.
    planned.push({
      xdrKey: buildInstanceLedgerKey(this.config.marketplaceContractAddress),
      durability: "instance",
      label: "instance",
    });

    // 2. Active persistent listing entries — one per listing.
    const listings = await this.source.getActiveListings();
    for (const listing of listings) {
      if (!/^\d+$/.test(listing.invoiceId)) continue;
      planned.push({
        xdrKey: buildListingLedgerKey(
          this.config.marketplaceContractAddress,
          listing.invoiceId,
        ),
        durability: "persistent",
        label: `listing:${listing.invoiceId}`,
        invoiceId: BigInt(listing.invoiceId),
      });
    }

    const entries: DryRunEntry[] = [];
    let latestLedger = 0;

    for (let offset = 0; offset < planned.length; offset += 100) {
      const batch = planned.slice(offset, offset + 100);
      const response = await this.rpc.getLedgerEntries(
        ...batch.map((p) => p.xdrKey),
      );
      latestLedger = response.latestLedger;
      const byXdr = new Map(
        response.entries.map((e) => [e.key.toXDR("base64"), e]),
      );

      for (const p of batch) {
        const found = byXdr.get(p.xdrKey.toXDR("base64"));
        if (!found) {
          // Absent → archived/expired (or never written); needs restore (treat as
          // needing a bump) so operators see it rather than silently skip it.
          entries.push({
            key: p.label,
            durability: p.durability,
            invoiceId: p.invoiceId,
            liveUntilLedgerSeq: 0,
            remainingLedgers: 0,
            needsBump: true,
            missing: true,
          });
          continue;
        }
        const remaining = found.liveUntilLedgerSeq - response.latestLedger;
        entries.push({
          key: p.label,
          durability: p.durability,
          invoiceId: p.invoiceId,
          liveUntilLedgerSeq: found.liveUntilLedgerSeq,
          remainingLedgers: remaining,
          needsBump: remaining < this.config.ttlThresholdLedgers,
          missing: false,
        });
      }
    }

    return {
      contractAddress: this.config.marketplaceContractAddress,
      latestLedger,
      thresholdLedgers: this.config.ttlThresholdLedgers,
      entries,
      summary: summarize(entries),
    };
  }
}

function summarize(entries: DryRunEntry[]): DryRunSummary {
  let instance = 0;
  let persistent = 0;
  let needsBump = 0;
  let healthy = 0;
  let missing = 0;
  for (const e of entries) {
    if (e.durability === "instance") instance++;
    else persistent++;
    if (e.missing) missing++;
    if (e.needsBump) needsBump++;
    else healthy++;
  }
  return { total: entries.length, instance, persistent, needsBump, healthy, missing };
}

/** JSON serializer helper: bigint → string for HTTP/CLI output. */
export function dryRunReplacer(_k: string, v: unknown): unknown {
  return typeof v === "bigint" ? v.toString() : v;
}
