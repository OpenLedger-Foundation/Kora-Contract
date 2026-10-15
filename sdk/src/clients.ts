import { Keypair, scValToNative, nativeToScVal } from "@stellar/stellar-sdk";
import { BaseClient, NetworkConfig } from "./base";
import {
  EscrowDispute,
  GovernanceChoice,
  GovernanceProposal,
  GovernanceVoteRecord,
  Invoice,
  Listing,
  MarketplaceConfig,
  Pool,
  Position,
  PositionSaleOffer,
  ProposalType,
  RiskTier,
  SmeProfile,
  VotingMode,
  WrapRecord,
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

// ── DisputeEscrowClient ───────────────────────────────────────────────────────

export class DisputeEscrowClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async initialize(admin: Keypair, pool: string): Promise<void> {
    await this.invoke(
      "initialize",
      [this.addr(admin.publicKey()), this.addr(pool)],
      admin
    );
  }

  async openDispute(
    challenger: Keypair,
    payer: string,
    invoiceId: bigint,
    amount: bigint
  ): Promise<void> {
    await this.invoke(
      "open_dispute",
      [
        this.addr(challenger.publicKey()),
        this.addr(payer),
        this.u64(invoiceId),
        this.i128(amount),
      ],
      challenger
    );
  }

  async resolveDispute(
    adminOrResolver: Keypair,
    invoiceId: bigint,
    payPool: boolean
  ): Promise<void> {
    await this.invoke(
      "resolve_dispute",
      [
        this.addr(adminOrResolver.publicKey()),
        this.u64(invoiceId),
        this.bool(payPool),
      ],
      adminOrResolver
    );
  }

  async resolveTimeout(invoiceId: bigint, caller?: Keypair): Promise<void> {
    await this.invoke("resolve_timeout", [this.u64(invoiceId)], caller);
  }

  async getDispute(invoiceId: bigint): Promise<EscrowDispute | null> {
    const result = await this.invoke("get_dispute", [this.u64(invoiceId)]);
    const native = scValToNative(result);
    if (!native) return null;
    return native as EscrowDispute;
  }
}

// ── FractionalizerClient ──────────────────────────────────────────────────────

export class FractionalizerClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async initialize(
    admin: Keypair,
    financingPool: string,
    accessControl: string
  ): Promise<void> {
    await this.invoke(
      "initialize",
      [
        this.addr(admin.publicKey()),
        this.addr(financingPool),
        this.addr(accessControl),
      ],
      admin
    );
  }

  async wrap(
    investor: Keypair,
    invoiceId: bigint,
    totalShares: bigint
  ): Promise<void> {
    await this.invoke(
      "wrap",
      [
        this.addr(investor.publicKey()),
        this.u64(invoiceId),
        this.u64(totalShares),
      ],
      investor
    );
  }

  async transferShares(
    from: Keypair,
    originalInvestor: string,
    invoiceId: bigint,
    to: string,
    amount: bigint
  ): Promise<void> {
    await this.invoke(
      "transfer_shares",
      [
        this.addr(from.publicKey()),
        this.addr(originalInvestor),
        this.u64(invoiceId),
        this.addr(to),
        this.u64(amount),
      ],
      from
    );
  }

  async settle(
    invoiceId: bigint,
    originalInvestor: string,
    caller?: Keypair
  ): Promise<void> {
    await this.invoke(
      "settle",
      [this.u64(invoiceId), this.addr(originalInvestor)],
      caller
    );
  }

  async redeem(
    holder: Keypair,
    invoiceId: bigint,
    originalInvestor: string
  ): Promise<bigint> {
    const result = await this.invoke(
      "redeem",
      [
        this.addr(holder.publicKey()),
        this.u64(invoiceId),
        this.addr(originalInvestor),
      ],
      holder
    );
    return scValToNative(result) as bigint;
  }

  async unwrap(investor: Keypair, invoiceId: bigint): Promise<void> {
    await this.invoke(
      "unwrap",
      [this.addr(investor.publicKey()), this.u64(invoiceId)],
      investor
    );
  }

  async getWrap(
    invoiceId: bigint,
    originalInvestor: string
  ): Promise<WrapRecord | null> {
    const result = await this.invoke("get_wrap", [
      this.u64(invoiceId),
      this.addr(originalInvestor),
    ]);
    const native = scValToNative(result);
    if (!native) return null;
    return native as WrapRecord;
  }

  async getShareBalance(
    invoiceId: bigint,
    originalInvestor: string,
    holder: string
  ): Promise<bigint> {
    const result = await this.invoke("get_share_balance", [
      this.u64(invoiceId),
      this.addr(originalInvestor),
      this.addr(holder),
    ]);
    return scValToNative(result) as bigint;
  }
}

// ── SecondaryMarketClient ─────────────────────────────────────────────────────

export class SecondaryMarketClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async initialize(
    admin: Keypair,
    financingPool: string,
    treasury: string,
    protocolFeeBps: number
  ): Promise<void> {
    await this.invoke(
      "initialize",
      [
        this.addr(admin.publicKey()),
        this.addr(financingPool),
        this.addr(treasury),
        this.u32(protocolFeeBps),
      ],
      admin
    );
  }

  async pause(admin: Keypair): Promise<void> {
    await this.invoke("pause", [], admin);
  }

  async unpause(admin: Keypair): Promise<void> {
    await this.invoke("unpause", [], admin);
  }

  async updateProtocolFeeBps(admin: Keypair, feeBps: number): Promise<void> {
    await this.invoke("update_protocol_fee_bps", [this.u32(feeBps)], admin);
  }

  async listPosition(
    seller: Keypair,
    invoiceId: bigint,
    price: bigint
  ): Promise<void> {
    await this.invoke(
      "list_position",
      [this.addr(seller.publicKey()), this.u64(invoiceId), this.i128(price)],
      seller
    );
  }

  async buyPosition(
    buyer: Keypair,
    invoiceId: bigint,
    seller: string
  ): Promise<void> {
    await this.invoke(
      "buy_position",
      [this.addr(buyer.publicKey()), this.u64(invoiceId), this.addr(seller)],
      buyer
    );
  }

  async cancelListing(seller: Keypair, invoiceId: bigint): Promise<void> {
    await this.invoke(
      "cancel_listing",
      [this.addr(seller.publicKey()), this.u64(invoiceId)],
      seller
    );
  }
}

// ── GovernanceVotingClient ────────────────────────────────────────────────────

export class GovernanceVotingClient extends BaseClient {
  constructor(contractId: string, network: NetworkConfig) {
    super(contractId, network);
  }

  async initialize(admin: Keypair, accessControl: string): Promise<void> {
    await this.invoke(
      "initialize",
      [this.addr(admin.publicKey()), this.addr(accessControl)],
      admin
    );
  }

  async setVotingWeight(
    admin: Keypair,
    address: string,
    weight: bigint
  ): Promise<void> {
    await this.invoke(
      "set_voting_weight",
      [this.addr(admin.publicKey()), this.addr(address), this.i128(weight)],
      admin
    );
  }

  async getVotingWeight(address: string): Promise<bigint> {
    const result = await this.invoke("get_voting_weight", [this.addr(address)]);
    return scValToNative(result) as bigint;
  }

  async delegate(delegator: Keypair, delegate: string): Promise<void> {
    await this.invoke(
      "delegate",
      [this.addr(delegator.publicKey()), this.addr(delegate)],
      delegator
    );
  }

  async revokeDelegation(delegator: Keypair): Promise<void> {
    await this.invoke(
      "revoke_delegation",
      [this.addr(delegator.publicKey())],
      delegator
    );
  }

  async getDelegate(delegator: string): Promise<string | null> {
    const result = await this.invoke("get_delegate", [this.addr(delegator)]);
    const native = scValToNative(result);
    if (!native) return null;
    return native as string;
  }

  async createProposal(
    proposer: Keypair,
    title: string,
    description: string,
    proposalType: ProposalType,
    votingMode: VotingMode,
    durationSeconds?: bigint
  ): Promise<bigint> {
    const durationScVal = durationSeconds
      ? this.u64(durationSeconds)
      : nativeToScVal(null, { type: "null" });

    const result = await this.invoke(
      "create_proposal",
      [
        this.addr(proposer.publicKey()),
        this.str(title),
        this.str(description),
        this.sym(proposalType),
        this.sym(votingMode),
        durationScVal,
      ],
      proposer
    );
    return scValToNative(result) as bigint;
  }

  async vote(
    voter: Keypair,
    proposalId: bigint,
    choice: GovernanceChoice,
    voiceCredits?: bigint
  ): Promise<void> {
    const voiceCreditsScVal = voiceCredits
      ? this.i128(voiceCredits)
      : nativeToScVal(null, { type: "null" });

    await this.invoke(
      "vote",
      [
        this.addr(voter.publicKey()),
        this.u64(proposalId),
        this.sym(choice),
        voiceCreditsScVal,
      ],
      voter
    );
  }

  async getProposal(proposalId: bigint): Promise<GovernanceProposal | null> {
    const result = await this.invoke("get_proposal", [this.u64(proposalId)]);
    const native = scValToNative(result);
    if (!native) return null;
    return native as GovernanceProposal;
  }

  async getVoteRecord(
    proposalId: bigint,
    voter: string
  ): Promise<GovernanceVoteRecord | null> {
    const result = await this.invoke("get_vote_record", [
      this.u64(proposalId),
      this.addr(voter),
    ]);
    const native = scValToNative(result);
    if (!native) return null;
    return native as GovernanceVoteRecord;
  }

  async executeProposal(executor: Keypair, proposalId: bigint): Promise<void> {
    await this.invoke(
      "execute_proposal",
      [this.addr(executor.publicKey()), this.u64(proposalId)],
      executor
    );
  }
}

