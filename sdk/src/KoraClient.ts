import { NetworkConfig, TESTNET } from "./base";
import {
  AccessControlClient,
  DisputeEscrowClient,
  FinancingPoolClient,
  FractionalizerClient,
  GovernanceVotingClient,
  InvoiceNftClient,
  MarketplaceClient,
  PriceOracleClient,
  RiskRegistryClient,
  SecondaryMarketClient,
  TreasuryClient,
} from "./clients";

export interface KoraAddresses {
  invoiceNft: string;
  marketplace: string;
  financingPool: string;
  treasury: string;
  riskRegistry: string;
  accessControl: string;
  priceOracle: string;
  disputeEscrow?: string;
  fractionalizer?: string;
  secondaryMarket?: string;
  governanceVoting?: string;
}

/**
 * Unified facade over Kora Protocol contracts.
 *
 * @example
 * ```ts
 * const kora = new KoraClient(addresses, KoraClient.TESTNET);
 * const id = await kora.invoiceNft.mintInvoice(smeKeypair, ...);
 * await kora.marketplace.listInvoice(smeKeypair, id, ...);
 * await kora.marketplace.fundInvoice(investorKeypair, id, amount);
 * await kora.financingPool.repay(smeKeypair, id, token, faceValue);
 * ```
 */
export class KoraClient {
  static readonly TESTNET = TESTNET;

  readonly invoiceNft: InvoiceNftClient;
  readonly marketplace: MarketplaceClient;
  readonly financingPool: FinancingPoolClient;
  readonly treasury: TreasuryClient;
  readonly riskRegistry: RiskRegistryClient;
  readonly accessControl: AccessControlClient;
  readonly priceOracle: PriceOracleClient;
  readonly disputeEscrow?: DisputeEscrowClient;
  readonly fractionalizer?: FractionalizerClient;
  readonly secondaryMarket?: SecondaryMarketClient;
  readonly governanceVoting?: GovernanceVotingClient;

  constructor(addresses: KoraAddresses, network: NetworkConfig = TESTNET) {
    this.invoiceNft    = new InvoiceNftClient(addresses.invoiceNft, network);
    this.marketplace   = new MarketplaceClient(addresses.marketplace, network);
    this.financingPool = new FinancingPoolClient(addresses.financingPool, network);
    this.treasury      = new TreasuryClient(addresses.treasury, network);
    this.riskRegistry  = new RiskRegistryClient(addresses.riskRegistry, network);
    this.accessControl = new AccessControlClient(addresses.accessControl, network);
    this.priceOracle   = new PriceOracleClient(addresses.priceOracle, network);
    if (addresses.disputeEscrow) {
      this.disputeEscrow = new DisputeEscrowClient(addresses.disputeEscrow, network);
    }
    if (addresses.fractionalizer) {
      this.fractionalizer = new FractionalizerClient(addresses.fractionalizer, network);
    }
    if (addresses.secondaryMarket) {
      this.secondaryMarket = new SecondaryMarketClient(addresses.secondaryMarket, network);
    }
    if (addresses.governanceVoting) {
      this.governanceVoting = new GovernanceVotingClient(addresses.governanceVoting, network);
    }
  }

  async batchGetInvoices(invoiceIds: bigint[]) {
    return Promise.all(invoiceIds.map((invoiceId) => this.invoiceNft.getInvoice(invoiceId)));
  }

  async reconcileTransaction(hash: string) {
    return this.invoiceNft.reconcileTransaction(hash);
  }

  async batchGetListings(invoiceIds: bigint[]) {
    return Promise.all(invoiceIds.map((invoiceId) => this.marketplace.getListing(invoiceId)));
  }

  paginate<T>(items: T[], page = 1, pageSize = 20) {
    const start = Math.max(0, (page - 1) * pageSize);
    return items.slice(start, start + pageSize);
  }
}
