import { KoraClient, KoraAddresses } from "../src/KoraClient";
import { GovernanceVotingClient } from "../src/clients";
import { TESTNET } from "../src/base";
import { GOVERNANCE_VOTING_ERROR_MESSAGES, getErrorMessage } from "../src/errorMapping";
import { Proposal, VoteRecord } from "../src/types";
import { Keypair, StrKey, xdr, nativeToScVal } from "@stellar/stellar-sdk";

const spyOn = (target: any, method: any) => {
  if (typeof jest !== "undefined" && jest.spyOn) {
    return jest.spyOn(target, method);
  }
  return (globalThis as any).vi.spyOn(target, method);
};

describe("Governance Proposal Lifecycle - Issue #936", () => {
  let kora: KoraClient;
  let addresses: KoraAddresses;
  let proposerKeypair: Keypair;
  let voterKeypair: Keypair;
  let adminKeypair: Keypair;
  const dummyContract = StrKey.encodeContract(Buffer.alloc(32));

  beforeEach(() => {
    proposerKeypair = Keypair.random();
    voterKeypair = Keypair.random();
    adminKeypair = Keypair.random();

    addresses = {
      invoiceNft: dummyContract,
      marketplace: dummyContract,
      financingPool: dummyContract,
      treasury: dummyContract,
      riskRegistry: dummyContract,
      accessControl: dummyContract,
      priceOracle: dummyContract,
      governanceVoting: dummyContract,
    };

    kora = new KoraClient(addresses, TESTNET);
  });

  describe("GovernanceVotingClient instantiation", () => {
    it("should instantiate governanceVoting on KoraClient when address provided", () => {
      expect(kora.governanceVoting).toBeDefined();
      expect(kora.governanceVoting).toBeInstanceOf(GovernanceVotingClient);
    });

    it("should allow direct GovernanceVotingClient instantiation", () => {
      const client = new GovernanceVotingClient(addresses.governanceVoting!, TESTNET);
      expect(client).toBeDefined();
    });
  });

  describe("createProposal", () => {
    it("should create a financial proposal using standard voting mode", async () => {
      const mockProposalId = BigInt(1);
      const spy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        nativeToScVal(mockProposalId, { type: "u64" })
      );

      const proposalId = await kora.governanceVoting!.createProposal(
        proposerKeypair,
        "Treasury Grant",
        "Allocate 50000 USDC",
        "Financial",
        "Standard",
        BigInt(100000),
        BigInt(604800)
      );

      expect(proposalId).toBe(mockProposalId);
      expect(spy).toHaveBeenCalledWith(
        "create_proposal",
        expect.any(Array),
        proposerKeypair
      );
    });

    it("should create a sentiment proposal using quadratic voting mode", async () => {
      const mockProposalId = BigInt(2);
      const spy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        nativeToScVal(mockProposalId, { type: "u64" })
      );

      const proposalId = await kora.governanceVoting!.createProposal(
        proposerKeypair,
        "Community Sentiment",
        "Protocol roadmap direction",
        "Sentiment",
        "Quadratic",
        BigInt(50000),
        BigInt(86400)
      );

      expect(proposalId).toBe(mockProposalId);
      expect(spy).toHaveBeenCalled();
    });
  });

  describe("vote", () => {
    it("should cast standard vote for proposal", async () => {
      const spy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        xdr.ScVal.scvVoid()
      );

      await kora.governanceVoting!.vote(
        voterKeypair,
        BigInt(1),
        "For"
      );

      expect(spy).toHaveBeenCalledWith(
        "vote",
        expect.any(Array),
        voterKeypair
      );
    });

    it("should cast quadratic vote with voice credits", async () => {
      const spy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        xdr.ScVal.scvVoid()
      );

      await kora.governanceVoting!.vote(
        voterKeypair,
        BigInt(2),
        "Against",
        BigInt(100)
      );

      expect(spy).toHaveBeenCalledWith(
        "vote",
        expect.any(Array),
        voterKeypair
      );
    });
  });

  describe("executeProposal", () => {
    it("should execute proposal after quorum reached", async () => {
      const spy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        xdr.ScVal.scvVoid()
      );

      await kora.governanceVoting!.executeProposal(adminKeypair, BigInt(1));

      expect(spy).toHaveBeenCalledWith(
        "execute_proposal",
        expect.any(Array),
        adminKeypair
      );
    });
  });

  describe("Voting Weight & Delegation", () => {
    it("should set and retrieve voting weight", async () => {
      const weight = BigInt(5000);
      const targetUser = Keypair.random().publicKey();
      const setSpy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        xdr.ScVal.scvVoid()
      );

      await kora.governanceVoting!.setVotingWeight(adminKeypair, targetUser, weight);
      expect(setSpy).toHaveBeenCalledWith("set_voting_weight", expect.any(Array), adminKeypair);

      setSpy.mockResolvedValueOnce(nativeToScVal(weight, { type: "i128" }));
      const retrieved = await kora.governanceVoting!.getVotingWeight(targetUser);
      expect(retrieved).toBe(weight);
    });

    it("should delegate voting weight to another address", async () => {
      const delegatee = Keypair.random().publicKey();
      const delegateSpy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        xdr.ScVal.scvVoid()
      );

      await kora.governanceVoting!.delegate(proposerKeypair, delegatee);
      expect(delegateSpy).toHaveBeenCalledWith("delegate", expect.any(Array), proposerKeypair);
    });

    it("should revoke delegation", async () => {
      const revokeSpy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        xdr.ScVal.scvVoid()
      );

      await kora.governanceVoting!.revokeDelegation(proposerKeypair);
      expect(revokeSpy).toHaveBeenCalledWith("revoke_delegation", expect.any(Array), proposerKeypair);
    });
  });

  describe("Read Helpers", () => {
    it("getProposal should retrieve proposal details", async () => {
      const proposalId = BigInt(1);
      const mockProposal: Proposal = {
        id: proposalId,
        proposer: Keypair.random().publicKey(),
        title: "Treasury Grant",
        description: "Allocate USDC",
        proposalType: "Financial",
        votingMode: "Standard",
        votesFor: BigInt(1000),
        votesAgainst: BigInt(0),
        votesAbstain: BigInt(0),
        createdAt: BigInt(1000000),
        expiresAt: BigInt(1604800),
        executed: false,
        quorumRequired: BigInt(500),
      };

      const spy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        nativeToScVal(mockProposal as any)
      );

      const proposal = await kora.governanceVoting!.getProposal(proposalId);
      expect(proposal).toBeDefined();
    });

    it("getVoteRecord should retrieve voter choice and weight", async () => {
      const proposalId = BigInt(1);
      const voterAddress = voterKeypair.publicKey();
      const mockRecord: VoteRecord = {
        voter: voterAddress,
        choice: "For",
        weight: BigInt(500),
        voiceCreditsSpent: BigInt(0),
        votedAt: BigInt(1000500),
      };

      const spy = spyOn(kora.governanceVoting as any, "invoke").mockResolvedValue(
        nativeToScVal(mockRecord as any)
      );

      const record = await kora.governanceVoting!.getVoteRecord(proposalId, voterAddress);
      expect(record).toBeDefined();
    });
  });

  describe("Error Mapping & Codes", () => {
    it("should map governance voting error codes to human-readable messages", () => {
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[1]).toBe("Already initialized");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[5]).toBe("Proposal not found");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[6]).toBe("Proposal expired");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[7]).toBe("Proposal already executed");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[8]).toBe("Already voted");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[9]).toBe("Insufficient voting weight");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[12]).toBe("Circular delegation");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[15]).toBe("Quadratic voting not allowed");
    });

    it("getErrorMessage should resolve governance contract errors", () => {
      expect(getErrorMessage(8, "governance_voting")).toBe("Already voted");
      expect(getErrorMessage(15, "governance")).toBe("Quadratic voting not allowed");
    });
  });
});
