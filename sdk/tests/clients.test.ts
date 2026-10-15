import { Keypair, nativeToScVal } from "@stellar/stellar-sdk";
import {
  KoraClient,
  KoraAddresses,
  TESTNET,
  DisputeEscrowClient,
  FractionalizerClient,
  SecondaryMarketClient,
  GovernanceVotingClient,
  KoraClientError,
  mapContractError,
  EscrowDispute,
  WrapRecord,
  GovernanceProposal,
  GovernanceVoteRecord,
} from "../src";

const FIXED_ADMIN_KEYPAIR = Keypair.fromRawEd25519Seed(
  Buffer.from("1111111111111111111111111111111111111111111111111111111111111111", "hex")
);
const FIXED_USER_KEYPAIR = Keypair.fromRawEd25519Seed(
  Buffer.from("2222222222222222222222222222222222222222222222222222222222222222", "hex")
);

const VALID_CONTRACT_ADDR = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";
const VALID_CONTRACT_ADDR_2 = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";
const VALID_ACCOUNT_ADDR = FIXED_USER_KEYPAIR.publicKey();
const VALID_ACCOUNT_ADDR_2 = FIXED_ADMIN_KEYPAIR.publicKey();

const MOCK_ADDRESSES: KoraAddresses = {
  invoiceNft: VALID_CONTRACT_ADDR,
  marketplace: VALID_CONTRACT_ADDR,
  financingPool: VALID_CONTRACT_ADDR,
  treasury: VALID_CONTRACT_ADDR,
  riskRegistry: VALID_CONTRACT_ADDR,
  accessControl: VALID_CONTRACT_ADDR,
  priceOracle: VALID_CONTRACT_ADDR,
  disputeEscrow: VALID_CONTRACT_ADDR,
  fractionalizer: VALID_CONTRACT_ADDR,
  secondaryMarket: VALID_CONTRACT_ADDR,
  governanceVoting: VALID_CONTRACT_ADDR,
};

describe("KoraClientError & mapContractError", () => {
  it("creates KoraClientError with code and contract metadata", () => {
    const err = new KoraClientError("Custom error", 42, "dispute_escrow");
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(KoraClientError);
    expect(err.name).toBe("KoraClientError");
    expect(err.message).toBe("Custom error");
    expect(err.code).toBe(42);
    expect(err.contract).toBe("dispute_escrow");
  });

  it("maps dispute_escrow error codes correctly", () => {
    const err5 = mapContractError(5, "dispute_escrow");
    expect(err5.message).toBe("Already disputed");
    expect(err5.code).toBe(5);

    const err8 = mapContractError(8, "dispute_escrow");
    expect(err8.message).toBe("Timeout not reached");

    const errUnknown = mapContractError(999, "dispute_escrow");
    expect(errUnknown.message).toBe("Unknown contract error 999");
  });

  it("maps fractionalizer error codes correctly", () => {
    const err8 = mapContractError(8, "fractionalizer");
    expect(err8.message).toBe("Wrap not found");

    const err13 = mapContractError(13, "fractionalizer");
    expect(err13.message).toBe("Invalid share count");
  });

  it("maps secondary_market error codes correctly", () => {
    const err5 = mapContractError(5, "secondary_market");
    expect(err5.message).toBe("Listing not found");

    const err11 = mapContractError(11, "secondary_market");
    expect(err11.message).toBe("Buyer cannot be seller");
  });

  it("maps governance_voting error codes correctly", () => {
    const err8 = mapContractError(8, "governance_voting");
    expect(err8.message).toBe("Already voted");

    const err12 = mapContractError(12, "governance_voting");
    expect(err12.message).toBe("Circular delegation");

    const err17 = mapContractError(17, "governance_voting");
    expect(err17.message).toBe("Reentrancy detected");
  });

  it("falls back to generic error dictionary or unknown message", () => {
    const errPaused = mapContractError(4);
    expect(errPaused.message).toBe("Protocol paused");

    const errUnknown = mapContractError(9999);
    expect(errUnknown.message).toBe("Unknown contract error 9999");
  });
});

describe("KoraClient Facade Namespace Extensions", () => {
  it("initializes all 4 new clients when addresses are provided", () => {
    const kora = new KoraClient(MOCK_ADDRESSES, TESTNET);
    expect(kora.disputeEscrow).toBeInstanceOf(DisputeEscrowClient);
    expect(kora.fractionalizer).toBeInstanceOf(FractionalizerClient);
    expect(kora.secondaryMarket).toBeInstanceOf(SecondaryMarketClient);
    expect(kora.governanceVoting).toBeInstanceOf(GovernanceVotingClient);
  });

  it("leaves optional client properties undefined when addresses are omitted", () => {
    const minimalAddresses: KoraAddresses = {
      invoiceNft: VALID_CONTRACT_ADDR,
      marketplace: VALID_CONTRACT_ADDR,
      financingPool: VALID_CONTRACT_ADDR,
      treasury: VALID_CONTRACT_ADDR,
      riskRegistry: VALID_CONTRACT_ADDR,
      accessControl: VALID_CONTRACT_ADDR,
      priceOracle: VALID_CONTRACT_ADDR,
    };
    const kora = new KoraClient(minimalAddresses, TESTNET);
    expect(kora.disputeEscrow).toBeUndefined();
    expect(kora.fractionalizer).toBeUndefined();
    expect(kora.secondaryMarket).toBeUndefined();
    expect(kora.governanceVoting).toBeUndefined();
  });
});

describe("DisputeEscrowClient Contract Methods", () => {
  let client: DisputeEscrowClient;
  let invokeSpy: jest.SpyInstance;

  beforeEach(() => {
    client = new DisputeEscrowClient(MOCK_ADDRESSES.disputeEscrow!, TESTNET);
    invokeSpy = jest.spyOn(client as any, "invoke").mockResolvedValue(nativeToScVal(null, { type: "null" }));
  });

  it("calls initialize with admin and pool", async () => {
    await client.initialize(FIXED_ADMIN_KEYPAIR, VALID_CONTRACT_ADDR_2);
    expect(invokeSpy).toHaveBeenCalledWith(
      "initialize",
      expect.any(Array),
      FIXED_ADMIN_KEYPAIR
    );
  });

  it("calls openDispute with challenger, payer, invoiceId, amount", async () => {
    await client.openDispute(FIXED_USER_KEYPAIR, VALID_ACCOUNT_ADDR_2, 100n, 5000000n);
    expect(invokeSpy).toHaveBeenCalledWith(
      "open_dispute",
      expect.any(Array),
      FIXED_USER_KEYPAIR
    );
  });

  it("calls resolveDispute with resolver, invoiceId, payPool", async () => {
    await client.resolveDispute(FIXED_ADMIN_KEYPAIR, 100n, true);
    expect(invokeSpy).toHaveBeenCalledWith(
      "resolve_dispute",
      expect.any(Array),
      FIXED_ADMIN_KEYPAIR
    );
  });

  it("calls resolveTimeout with invoiceId and caller", async () => {
    await client.resolveTimeout(100n, FIXED_USER_KEYPAIR);
    expect(invokeSpy).toHaveBeenCalledWith(
      "resolve_timeout",
      expect.any(Array),
      FIXED_USER_KEYPAIR
    );
  });

  it("reads getDispute returning typed EscrowDispute", async () => {
    const mockDispute: EscrowDispute = {
      invoiceId: 100n,
      challenger: FIXED_USER_KEYPAIR.publicKey(),
      payer: VALID_ACCOUNT_ADDR_2,
      token: VALID_CONTRACT_ADDR_2,
      amount: 5000000n,
      openedAt: 123456n,
      resolved: false,
      paidToPool: false,
    };
    invokeSpy.mockResolvedValueOnce(nativeToScVal(mockDispute));

    const result = await client.getDispute(100n);
    expect(result).toMatchObject({
      invoiceId: 100n,
      resolved: false,
    });
  });
});

describe("FractionalizerClient Contract Methods", () => {
  let client: FractionalizerClient;
  let invokeSpy: jest.SpyInstance;

  beforeEach(() => {
    client = new FractionalizerClient(MOCK_ADDRESSES.fractionalizer!, TESTNET);
    invokeSpy = jest.spyOn(client as any, "invoke").mockResolvedValue(nativeToScVal(null, { type: "null" }));
  });

  it("calls initialize with admin, financingPool, accessControl", async () => {
    await client.initialize(FIXED_ADMIN_KEYPAIR, VALID_CONTRACT_ADDR, VALID_CONTRACT_ADDR_2);
    expect(invokeSpy).toHaveBeenCalledWith(
      "initialize",
      expect.any(Array),
      FIXED_ADMIN_KEYPAIR
    );
  });

  it("calls wrap with investor, invoiceId, totalShares", async () => {
    await client.wrap(FIXED_USER_KEYPAIR, 200n, 1000n);
    expect(invokeSpy).toHaveBeenCalledWith(
      "wrap",
      expect.any(Array),
      FIXED_USER_KEYPAIR
    );
  });

  it("calls transferShares with from, originalInvestor, invoiceId, to, amount", async () => {
    await client.transferShares(FIXED_USER_KEYPAIR, VALID_ACCOUNT_ADDR_2, 200n, VALID_ACCOUNT_ADDR, 100n);
    expect(invokeSpy).toHaveBeenCalledWith(
      "transfer_shares",
      expect.any(Array),
      FIXED_USER_KEYPAIR
    );
  });

  it("calls settle with invoiceId and originalInvestor", async () => {
    await client.settle(200n, VALID_ACCOUNT_ADDR_2, FIXED_USER_KEYPAIR);
    expect(invokeSpy).toHaveBeenCalledWith(
      "settle",
      expect.any(Array),
      FIXED_USER_KEYPAIR
    );
  });

  it("calls redeem and returns payout amount", async () => {
    invokeSpy.mockResolvedValueOnce(nativeToScVal(500000n, { type: "i128" }));
    const payout = await client.redeem(FIXED_USER_KEYPAIR, 200n, VALID_ACCOUNT_ADDR_2);
    expect(payout).toBe(500000n);
  });

  it("calls unwrap with investor and invoiceId", async () => {
    await client.unwrap(FIXED_USER_KEYPAIR, 200n);
    expect(invokeSpy).toHaveBeenCalledWith(
      "unwrap",
      expect.any(Array),
      FIXED_USER_KEYPAIR
    );
  });

  it("reads getWrap returning typed WrapRecord", async () => {
    const mockWrap: WrapRecord = {
      invoiceId: 200n,
      originalInvestor: FIXED_USER_KEYPAIR.publicKey(),
      totalShares: 1000n,
      outstandingShares: 1000n,
      contributed: 10000000n,
      token: VALID_CONTRACT_ADDR,
      totalPayout: 0n,
      settled: false,
    };
    invokeSpy.mockResolvedValueOnce(nativeToScVal(mockWrap));
    const result = await client.getWrap(200n, FIXED_USER_KEYPAIR.publicKey());
    expect(result).toMatchObject({
      invoiceId: 200n,
      totalShares: 1000n,
    });
  });

  it("reads getShareBalance returning balance", async () => {
    invokeSpy.mockResolvedValueOnce(nativeToScVal(250n, { type: "u64" }));
    const bal = await client.getShareBalance(200n, VALID_ACCOUNT_ADDR_2, FIXED_USER_KEYPAIR.publicKey());
    expect(bal).toBe(250n);
  });
});

describe("SecondaryMarketClient Contract Methods", () => {
  let client: SecondaryMarketClient;
  let invokeSpy: jest.SpyInstance;

  beforeEach(() => {
    client = new SecondaryMarketClient(MOCK_ADDRESSES.secondaryMarket!, TESTNET);
    invokeSpy = jest.spyOn(client as any, "invoke").mockResolvedValue(nativeToScVal(null, { type: "null" }));
  });

  it("calls initialize with admin, financingPool, treasury, protocolFeeBps", async () => {
    await client.initialize(FIXED_ADMIN_KEYPAIR, VALID_CONTRACT_ADDR, VALID_CONTRACT_ADDR_2, 250);
    expect(invokeSpy).toHaveBeenCalledWith(
      "initialize",
      expect.any(Array),
      FIXED_ADMIN_KEYPAIR
    );
  });

  it("calls pause, unpause, and updateProtocolFeeBps", async () => {
    await client.pause(FIXED_ADMIN_KEYPAIR);
    expect(invokeSpy).toHaveBeenCalledWith("pause", [], FIXED_ADMIN_KEYPAIR);

    await client.unpause(FIXED_ADMIN_KEYPAIR);
    expect(invokeSpy).toHaveBeenCalledWith("unpause", [], FIXED_ADMIN_KEYPAIR);

    await client.updateProtocolFeeBps(FIXED_ADMIN_KEYPAIR, 300);
    expect(invokeSpy).toHaveBeenCalledWith(
      "update_protocol_fee_bps",
      expect.any(Array),
      FIXED_ADMIN_KEYPAIR
    );
  });

  it("calls listPosition, buyPosition, cancelListing", async () => {
    await client.listPosition(FIXED_USER_KEYPAIR, 300n, 9500000n);
    expect(invokeSpy).toHaveBeenCalledWith("list_position", expect.any(Array), FIXED_USER_KEYPAIR);

    await client.buyPosition(FIXED_ADMIN_KEYPAIR, 300n, FIXED_USER_KEYPAIR.publicKey());
    expect(invokeSpy).toHaveBeenCalledWith("buy_position", expect.any(Array), FIXED_ADMIN_KEYPAIR);

    await client.cancelListing(FIXED_USER_KEYPAIR, 300n);
    expect(invokeSpy).toHaveBeenCalledWith("cancel_listing", expect.any(Array), FIXED_USER_KEYPAIR);
  });
});

describe("GovernanceVotingClient Contract Methods", () => {
  let client: GovernanceVotingClient;
  let invokeSpy: jest.SpyInstance;

  beforeEach(() => {
    client = new GovernanceVotingClient(MOCK_ADDRESSES.governanceVoting!, TESTNET);
    invokeSpy = jest.spyOn(client as any, "invoke").mockResolvedValue(nativeToScVal(null, { type: "null" }));
  });

  it("calls initialize and setVotingWeight", async () => {
    await client.initialize(FIXED_ADMIN_KEYPAIR, VALID_CONTRACT_ADDR_2);
    expect(invokeSpy).toHaveBeenCalledWith("initialize", expect.any(Array), FIXED_ADMIN_KEYPAIR);

    await client.setVotingWeight(FIXED_ADMIN_KEYPAIR, VALID_ACCOUNT_ADDR, 1000n);
    expect(invokeSpy).toHaveBeenCalledWith("set_voting_weight", expect.any(Array), FIXED_ADMIN_KEYPAIR);
  });

  it("reads getVotingWeight and getDelegate", async () => {
    invokeSpy.mockResolvedValueOnce(nativeToScVal(1000n, { type: "i128" }));
    const weight = await client.getVotingWeight(VALID_ACCOUNT_ADDR);
    expect(weight).toBe(1000n);

    invokeSpy.mockResolvedValueOnce(nativeToScVal(VALID_ACCOUNT_ADDR_2, { type: "string" }));
    const delegate = await client.getDelegate(VALID_ACCOUNT_ADDR);
    expect(delegate).toBe(VALID_ACCOUNT_ADDR_2);
  });

  it("calls delegate and revokeDelegation", async () => {
    await client.delegate(FIXED_USER_KEYPAIR, VALID_ACCOUNT_ADDR_2);
    expect(invokeSpy).toHaveBeenCalledWith("delegate", expect.any(Array), FIXED_USER_KEYPAIR);

    await client.revokeDelegation(FIXED_USER_KEYPAIR);
    expect(invokeSpy).toHaveBeenCalledWith("revoke_delegation", expect.any(Array), FIXED_USER_KEYPAIR);
  });

  it("calls createProposal, vote, and executeProposal", async () => {
    invokeSpy.mockResolvedValueOnce(nativeToScVal(1n, { type: "u64" }));
    const proposalId = await client.createProposal(
      FIXED_USER_KEYPAIR,
      "Upgrade Pool",
      "Upgrade financing pool contract",
      "Upgrade",
      "Standard",
      86400n
    );
    expect(proposalId).toBe(1n);

    await client.vote(FIXED_USER_KEYPAIR, 1n, "For", 100n);
    expect(invokeSpy).toHaveBeenCalledWith("vote", expect.any(Array), FIXED_USER_KEYPAIR);

    await client.executeProposal(FIXED_ADMIN_KEYPAIR, 1n);
    expect(invokeSpy).toHaveBeenCalledWith("execute_proposal", expect.any(Array), FIXED_ADMIN_KEYPAIR);
  });

  it("reads getProposal and getVoteRecord", async () => {
    const mockProposal: GovernanceProposal = {
      id: 1n,
      proposer: FIXED_USER_KEYPAIR.publicKey(),
      title: "Test",
      description: "Desc",
      proposalType: "Financial",
      votingMode: "Standard",
      votesFor: 100n,
      votesAgainst: 10n,
      votesAbstain: 5n,
      createdAt: 1000n,
      expiresAt: 2000n,
      executed: false,
      quorumRequired: 50n,
    };
    invokeSpy.mockResolvedValueOnce(nativeToScVal(mockProposal));
    const proposal = await client.getProposal(1n);
    expect(proposal).toMatchObject({
      id: 1n,
      title: "Test",
    });

    const mockVoteRecord: GovernanceVoteRecord = {
      voter: FIXED_USER_KEYPAIR.publicKey(),
      choice: "For",
      weight: 100n,
      voiceCreditsSpent: 0n,
      votedAt: 1500n,
    };
    invokeSpy.mockResolvedValueOnce(nativeToScVal(mockVoteRecord));
    const record = await client.getVoteRecord(1n, FIXED_USER_KEYPAIR.publicKey());
    expect(record).toMatchObject({
      choice: "For",
      weight: 100n,
    });
  });
});
