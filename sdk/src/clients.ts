import { Keypair, scValToNative } from "@stellar/stellar-sdk";
import { BaseClient, NetworkConfig } from "./base";
import {
  Invoice,
  Listing,
  MarketplaceConfig,
  Pool,
  Position,
  RiskTier,
  SmeProfile,
} from "./types";

// ── InvoiceNftClient ──────────────────────────────────────────────────────────

export class InvoiceNftClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async mintInvoice(
    sme: Keypair,
    debtorHash: Buffer,
    amount: bigint,
    currency: string,
    dueDate: bigint,
    ipfsCid: string,
    riskScore: number
  ): Promise<bigint> {
    const result = await this.invoke(
      "mint_invoice",
      [
        this.addr(sme.publicKey()),
        this.bytes(debtorHash),
        this.i128(amount),
        this.sym(currency),
        this.u64(dueDate),
        this.str(ipfsCid),
        this.u32(riskScore),
      ],
      sme
    );
    return scValToNative(result) as bigint;
  }

  async getInvoice(invoiceId: bigint): Promise<Invoice> {
    const result = await this.invoke("get_invoice", [this.u64(invoiceId)]);
    return scValToNative(result) as Invoice;
  }

  async nextId(): Promise<bigint> {
    const result = await this.invoke("next_id", []);
    return scValToNative(result) as bigint;
  }

  async setAuthorizedCallers(
    admin: Keypair,
    marketplace: string,
    financingPool: string
  ): Promise<void> {
    await this.invoke(
      "set_authorized_callers",
      [this.addr(admin.publicKey()), this.addr(marketplace), this.addr(financingPool)],
      admin
    );
  }
}

// ── MarketplaceClient ─────────────────────────────────────────────────────────

export class MarketplaceClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async listInvoice(
    seller: Keypair,
    invoiceId: bigint,
    askingPrice: bigint,
    faceValue: bigint,
    token: string,
    fundingDeadline: bigint
  ): Promise<void> {
    await this.invoke(
      "list_invoice",
      [
        this.addr(seller.publicKey()),
        this.u64(invoiceId),
        this.i128(askingPrice),
        this.i128(faceValue),
        this.addr(token),
        this.u64(fundingDeadline),
      ],
      seller
    );
  }

  async fundInvoice(investor: Keypair, invoiceId: bigint, amount: bigint): Promise<void> {
    await this.invoke(
      "fund_invoice",
      [this.addr(investor.publicKey()), this.u64(invoiceId), this.i128(amount)],
      investor
    );
  }

  async cancelListing(caller: Keypair, invoiceId: bigint): Promise<void> {
    await this.invoke(
      "cancel_listing",
      [this.addr(caller.publicKey()), this.u64(invoiceId)],
      caller
    );
  }

  async getListing(invoiceId: bigint): Promise<Listing> {
    const result = await this.invoke("get_listing", [this.u64(invoiceId)]);
    return scValToNative(result) as Listing;
  }

  async getConfig(): Promise<MarketplaceConfig> {
    const result = await this.invoke("get_config", []);
    return scValToNative(result) as MarketplaceConfig;
  }

  async getFeeBps(): Promise<number> {
    const result = await this.invoke("get_fee_bps", []);
    return scValToNative(result) as number;
  }

  async setTierFeeBps(admin: Keypair, tier: RiskTier, feeBps: number): Promise<void> {
    await this.invoke(
      "set_tier_fee_bps",
      [this.addr(admin.publicKey()), this.sym(tier), this.u32(feeBps)],
      admin
    );
  }

  async getTierFeeBps(tier: RiskTier): Promise<number> {
    const result = await this.invoke("get_tier_fee_bps", [this.sym(tier)]);
    return scValToNative(result) as number;
  }
}

// ── FinancingPoolClient ───────────────────────────────────────────────────────

export class FinancingPoolClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async repay(sme: Keypair, invoiceId: bigint, token: string, amount: bigint): Promise<void> {
    await this.invoke(
      "repay",
      [this.addr(sme.publicKey()), this.u64(invoiceId), this.addr(token), this.i128(amount)],
      sme
    );
  }

  async getPool(invoiceId: bigint): Promise<Pool> {
    const result = await this.invoke("get_pool", [this.u64(invoiceId)]);
    return scValToNative(result) as Pool;
  }

  async getPosition(invoiceId: bigint, investor: string): Promise<Position> {
    const result = await this.invoke("get_position", [this.u64(invoiceId), this.addr(investor)]);
    return scValToNative(result) as Position;
  }
}

// ── TreasuryClient ────────────────────────────────────────────────────────────

export class TreasuryClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async getFeeBps(): Promise<number> {
    const result = await this.invoke("get_fee_bps", []);
    return scValToNative(result) as number;
  }

  async getBalance(token: string): Promise<bigint> {
    const result = await this.invoke("get_balance", [this.addr(token)]);
    return scValToNative(result) as bigint;
  }

  async getCollected(token: string): Promise<bigint> {
    const result = await this.invoke("get_collected", [this.addr(token)]);
    return scValToNative(result) as bigint;
  }

  async withdraw(admin: Keypair, token: string, recipient: string, amount: bigint): Promise<void> {
    await this.invoke(
      "withdraw",
      [this.addr(admin.publicKey()), this.addr(token), this.addr(recipient), this.i128(amount)],
      admin
    );
  }
}

// ── RiskRegistryClient ────────────────────────────────────────────────────────

export class RiskRegistryClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async registerSme(verifier: Keypair, sme: string, riskScore: number): Promise<void> {
    await this.invoke(
      "register_sme",
      [this.addr(verifier.publicKey()), this.addr(sme), this.u32(riskScore)],
      verifier
    );
  }

  async getSmeProfile(sme: string): Promise<SmeProfile> {
    const result = await this.invoke("get_sme_profile", [this.addr(sme)]);
    return scValToNative(result) as SmeProfile;
  }

  async isVerifiedSme(sme: string): Promise<boolean> {
    const result = await this.invoke("is_verified_sme", [this.addr(sme)]);
    return scValToNative(result) as boolean;
  }

  async addVerifier(admin: Keypair, verifier: string): Promise<void> {
    await this.invoke(
      "add_verifier",
      [this.addr(admin.publicKey()), this.addr(verifier)],
      admin
    );
  }
}

// ── AccessControlClient ───────────────────────────────────────────────────────

export class AccessControlClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async isPaused(): Promise<boolean> {
    const result = await this.invoke("is_paused", []);
    return scValToNative(result) as boolean;
  }

  async pause(admin: Keypair): Promise<void> {
    await this.invoke("pause", [this.addr(admin.publicKey())], admin);
  }

  async unpause(admin: Keypair): Promise<void> {
    await this.invoke("unpause", [this.addr(admin.publicKey())], admin);
  }

  async getAdmin(): Promise<string> {
    const result = await this.invoke("get_admin", []);
    return scValToNative(result) as string;
  }
}

// ── PriceOracleClient ─────────────────────────────────────────────────────────

export class PriceOracleClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async getPrice(asset: string): Promise<bigint> {
    const result = await this.invoke("get_price", [this.sym(asset)]);
    return scValToNative(result) as bigint;
  }

  async setPrice(admin: Keypair, asset: string, price: bigint): Promise<void> {
    await this.invoke(
      "set_price",
      [this.addr(admin.publicKey()), this.sym(asset), this.i128(price)],
      admin
    );
  }
}
