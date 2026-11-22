/**
 * Reporting Service — Fixture ReportingDataSource (dev/test mode)
 *
 * Returns deterministic fixture data when REPORTING_USE_FIXTURES=true.
 * Swap at bootstrap time — no code change needed between dev and production.
 *
 * Issue #1003
 */

import { ReportingDataSource } from "./exportJobQueue";
import {
  UserStatement,
  ProtocolStatement,
  TreasuryBalancesResponse,
  TreasuryInflowsResponse,
  TreasuryOutflowsResponse,
  TreasuryQueryOptions,
  TreasuryInflow,
  TreasuryOutflow,
} from "./types";

export class FixtureReportingDataSource implements ReportingDataSource {
  async getUserStatement(
    userAddress: string,
    fromTs: number,
    toTs: number
  ): Promise<UserStatement> {
    const mid = Math.floor((fromTs + toTs) / 2);
    return {
      userAddress,
      fromTs,
      toTs,
      funding: [
        {
          invoiceId: "fixture-inv-001",
          asset: "USDC",
          amount: 1_000_000_000n,
          timestamp: mid,
          txHash: "0xfixture_fund_001",
        },
      ],
      repayments: [
        {
          invoiceId: "fixture-inv-001",
          asset: "USDC",
          amount: 1_000_000_000n,
          timestamp: mid + 86400,
          txHash: "0xfixture_repay_001",
        },
      ],
      yields: [
        {
          invoiceId: "fixture-inv-001",
          asset: "USDC",
          principalReturned: 1_000_000_000n,
          yieldAmount: 30_000_000n,
          timestamp: mid + 86400,
          txHash: "0xfixture_yield_001",
        },
      ],
      fees: [
        {
          invoiceId: "fixture-inv-001",
          asset: "USDC",
          feeAmount: 5_000_000n,
          feeBps: 50,
          timestamp: mid + 86400,
          txHash: "0xfixture_fee_001",
        },
      ],
      summaryByAsset: [],
    };
  }

  async getProtocolStatement(
    fromTs: number,
    toTs: number
  ): Promise<ProtocolStatement> {
    return {
      fromTs,
      toTs,
      totalInvoicesFinanced: 42,
      totalInvoicesRepaid: 38,
      totalInvoicesDefaulted: 2,
      summaryByAsset: [],
    };
  }

  async getTreasuryBalances(): Promise<TreasuryBalancesResponse> {
    return {
      balances: [
        {
          asset: "USDC",
          balance: "2450000000",
          totalInflows: "3000000000",
          totalOutflows: "550000000",
          reserveBalance: "250000000",
        },
        {
          asset: "EURC",
          balance: "1200000000",
          totalInflows: "1200000000",
          totalOutflows: "0",
          reserveBalance: "100000000",
        },
      ],
      lastIndexedLedger: 51200000,
      latestChainLedger: 51200000,
      partial: false,
    };
  }

  async getTreasuryInflows(
    options: TreasuryQueryOptions = {}
  ): Promise<TreasuryInflowsResponse> {
    const all: TreasuryInflow[] = [
      {
        id: "fixture-inflow-001",
        asset: "USDC",
        amount: "1500000000",
        source: "marketplace_fee",
        invoiceId: "fixture-inv-001",
        txHash: "0xfixture_inflow_001",
        ledgerSequence: 51199800,
        timestamp: 1700000000,
      },
      {
        id: "fixture-inflow-002",
        asset: "USDC",
        amount: "1500000000",
        source: "marketplace_fee",
        invoiceId: "fixture-inv-002",
        txHash: "0xfixture_inflow_002",
        ledgerSequence: 51199900,
        timestamp: 1700086400,
      },
      {
        id: "fixture-inflow-003",
        asset: "EURC",
        amount: "1200000000",
        source: "marketplace_fee",
        invoiceId: "fixture-inv-003",
        txHash: "0xfixture_inflow_003",
        ledgerSequence: 51199950,
        timestamp: 1700172800,
      },
    ];

    const filtered = all.filter((item) => {
      if (options.asset && item.asset !== options.asset) return false;
      if (options.fromTs !== undefined && item.timestamp < options.fromTs) return false;
      if (options.toTs !== undefined && item.timestamp > options.toTs) return false;
      return true;
    });

    const offset = options.offset ?? 0;
    const limit = options.limit ?? filtered.length;
    const paged = filtered.slice(offset, offset + limit);

    return {
      inflows: paged,
      total: filtered.length,
      lastIndexedLedger: 51200000,
      latestChainLedger: 51200000,
      partial: false,
    };
  }

  async getTreasuryOutflows(
    options: TreasuryQueryOptions = {}
  ): Promise<TreasuryOutflowsResponse> {
    const all: TreasuryOutflow[] = [
      {
        id: "fixture-outflow-001",
        asset: "USDC",
        amount: "550000000",
        recipient: "GFIXTURE_TREASURY_OPS_RECIPIENT",
        reason: "withdrawal",
        txHash: "0xfixture_outflow_001",
        ledgerSequence: 51199980,
        timestamp: 1700100000,
      },
    ];

    const filtered = all.filter((item) => {
      if (options.asset && item.asset !== options.asset) return false;
      if (options.fromTs !== undefined && item.timestamp < options.fromTs) return false;
      if (options.toTs !== undefined && item.timestamp > options.toTs) return false;
      return true;
    });

    const offset = options.offset ?? 0;
    const limit = options.limit ?? filtered.length;
    const paged = filtered.slice(offset, offset + limit);

    return {
      outflows: paged,
      total: filtered.length,
      lastIndexedLedger: 51200000,
      latestChainLedger: 51200000,
      partial: false,
    };
  }
}
