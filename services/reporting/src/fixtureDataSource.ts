/**
 * Reporting Service — Fixture ReportingDataSource (dev/test mode)
 *
 * Returns deterministic fixture data when REPORTING_USE_FIXTURES=true.
 * Swap at bootstrap time — no code change needed between dev and production.
 *
 * Issue #1003
 */

import { ReportingDataSource } from "./exportJobQueue";
import { UserStatement, ProtocolStatement } from "./types";

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
}
