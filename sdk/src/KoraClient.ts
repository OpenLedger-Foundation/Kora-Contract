import { NetworkConfig, TESTNET } from "./base";
import {
  AccessControlClient,
  FinancingPoolClient,
  InvoiceNftClient,
  MarketplaceClient,
  PriceOracleClient,
  RiskRegistryClient,
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
}

/**
 * Unified facade over all 7 Kora Protocol contracts.
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

  constructor(addresses: KoraAddresses, network: NetworkConfig = TESTNET) {
    this.invoiceNft    = new InvoiceNftClient(addresses.invoiceNft, network);
    this.marketplace   = new MarketplaceClient(addresses.marketplace, network);
    this.financingPool = new FinancingPoolClient(addresses.financingPool, network);
    this.treasury      = new TreasuryClient(addresses.treasury, network);
    this.riskRegistry  = new RiskRegistryClient(addresses.riskRegistry, network);
    this.accessControl = new AccessControlClient(addresses.accessControl, network);
    this.priceOracle   = new PriceOracleClient(addresses.priceOracle, network);
  }
}
