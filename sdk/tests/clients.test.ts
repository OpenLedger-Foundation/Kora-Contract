import {
  DebtorAttestationClient,
  DisputeEscrowClient,
  FractionalizerClient,
  GovernanceVotingClient,
  SecondaryMarketClient,
  decodeProposal,
  encodeEnum,
} from "../src/clients";
import { KoraClient, KoraAddresses } from "../src/KoraClient";
import { NULL_ACCOUNT, TESTNET } from "../src/base";
import {
  CONTRACT_ERROR_MESSAGES,
  DISPUTE_ESCROW_ERROR_MESSAGES,
  FRACTIONALIZER_ERROR_MESSAGES,
  GOVERNANCE_VOTING_ERROR_MESSAGES,
  SECONDARY_MARKET_ERROR_MESSAGES,
  getErrorMessage,
} from "../src/errorMapping";
import { EscrowDispute, WrapRecord } from "../src/types";
import { Keypair, StrKey, nativeToScVal, scValToNative, xdr } from "@stellar/stellar-sdk";

const spyOnInvoke = (client: any, retval: xdr.ScVal = xdr.ScVal.scvVoid()) => {
  return jest.spyOn(client as any, "invoke").mockResolvedValue(retval);
};

describe("SDK Typed Clients - Issue #936", () => {
  const dummyContractId = StrKey.encodeContract(Buffer.alloc(32));
  const dummyToken = StrKey.encodeContract(Buffer.from("12345678901234567890123456789012"));

  describe("DisputeEscrowClient", () => {
    let client: DisputeEscrowClient;

    beforeEach(() => {
      client = new DisputeEscrowClient(dummyContractId, TESTNET);
    });

    it("initialize invokes on-chain initialize with admin and pool", async () => {
      const admin = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.initialize(admin, dummyToken);

      expect(spy).toHaveBeenCalledWith(
        "initialize",
        expect.any(Array),
        admin
      );
    });

    it("openDispute submits challenger, payer, invoiceId, amount", async () => {
      const challenger = Keypair.random();
      const payer = Keypair.random().publicKey();
      const spy = spyOnInvoke(client);

      await client.openDispute(challenger, payer, 100n, 5000n);

      expect(spy).toHaveBeenCalledWith(
        "open_dispute",
        expect.any(Array),
        challenger
      );
    });

    it("resolveDispute submits resolver, invoiceId, and boolean payPool", async () => {
      const resolver = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.resolveDispute(resolver, 100n, true);

      expect(spy).toHaveBeenCalledWith(
        "resolve_dispute",
        expect.any(Array),
        resolver
      );
    });

    it("resolveTimeout submits invoiceId with caller signature", async () => {
      const caller = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.resolveTimeout(caller, 100n);

      expect(spy).toHaveBeenCalledWith(
        "resolve_timeout",
        expect.any(Array),
        caller
      );
    });

    it("getDispute queries on-chain dispute record", async () => {
      const mockDispute: EscrowDispute = {
        invoiceId: 100n,
        challenger: Keypair.random().publicKey(),
        payer: Keypair.random().publicKey(),
        token: dummyToken,
        amount: 5000n,
        openedAt: 1700000000n,
        resolved: false,
        paidToPool: false,
      };
      const spy = jest.spyOn(client as any, "invoke").mockResolvedValue(
        nativeToScVal(mockDispute as any)
      );

      const dispute = await client.getDispute(100n);
      expect(spy).toHaveBeenCalledWith("get_dispute", expect.any(Array));
      expect(dispute).toBeDefined();
    });
  });

  describe("FractionalizerClient", () => {
    let client: FractionalizerClient;

    beforeEach(() => {
      client = new FractionalizerClient(dummyContractId, TESTNET);
    });

    it("initialize invokes with admin, financingPool, accessControl", async () => {
      const admin = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.initialize(admin, dummyToken, dummyContractId);

      expect(spy).toHaveBeenCalledWith(
        "initialize",
        expect.any(Array),
        admin
      );
    });

    it("wrap wraps position into fungible shares", async () => {
      const investor = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.wrap(investor, 42n, 1000n);

      expect(spy).toHaveBeenCalledWith(
        "wrap",
        expect.any(Array),
        investor
      );
    });

    it("transferShares transfers share balance between holders", async () => {
      const from = Keypair.random();
      const original = Keypair.random().publicKey();
      const to = Keypair.random().publicKey();
      const spy = spyOnInvoke(client);

      await client.transferShares(from, original, 42n, to, 250n);

      expect(spy).toHaveBeenCalledWith(
        "transfer_shares",
        expect.any(Array),
        from
      );
    });

    it("settle records settlement payout for wrap", async () => {
      const caller = Keypair.random();
      const original = Keypair.random().publicKey();
      const spy = spyOnInvoke(client);

      await client.settle(caller, 42n, original);

      expect(spy).toHaveBeenCalledWith(
        "settle",
        expect.any(Array),
        caller
      );
    });

    it("redeem burns shares and returns payout amount", async () => {
      const holder = Keypair.random();
      const original = Keypair.random().publicKey();
      const expectedPayout = 2500n;
      const spy = jest.spyOn(client as any, "invoke").mockResolvedValue(
        nativeToScVal(expectedPayout, { type: "i128" })
      );

      const payout = await client.redeem(holder, 42n, original);

      expect(spy).toHaveBeenCalledWith("redeem", expect.any(Array), holder);
      expect(payout).toBe(expectedPayout);
    });

    it("unwrap returns pool position to original investor", async () => {
      const investor = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.unwrap(investor, 42n);

      expect(spy).toHaveBeenCalledWith(
        "unwrap",
        expect.any(Array),
        investor
      );
    });

    it("getWrap reads wrap record", async () => {
      const original = Keypair.random().publicKey();
      const mockWrap: WrapRecord = {
        invoiceId: 42n,
        originalInvestor: original,
        totalShares: 1000n,
        outstandingShares: 1000n,
        contributed: 10000n,
        token: dummyToken,
        totalPayout: 0n,
        settled: false,
      };
      const spy = jest.spyOn(client as any, "invoke").mockResolvedValue(
        nativeToScVal(mockWrap as any)
      );

      const wrap = await client.getWrap(42n, original);
      expect(spy).toHaveBeenCalledWith("get_wrap", expect.any(Array));
      expect(wrap).toBeDefined();
    });

    it("getShareBalance queries holder balance", async () => {
      const original = Keypair.random().publicKey();
      const holder = Keypair.random().publicKey();
      const balance = 500n;
      const spy = jest.spyOn(client as any, "invoke").mockResolvedValue(
        nativeToScVal(balance, { type: "u64" })
      );

      const res = await client.getShareBalance(42n, original, holder);
      expect(spy).toHaveBeenCalledWith("get_share_balance", expect.any(Array));
      expect(res).toBe(balance);
    });
  });

  describe("SecondaryMarketClient", () => {
    let client: SecondaryMarketClient;

    beforeEach(() => {
      client = new SecondaryMarketClient(dummyContractId, TESTNET);
    });

    it("initialize configures admin, pool, treasury, and fee bps", async () => {
      const admin = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.initialize(admin, dummyToken, dummyToken, 250);

      expect(spy).toHaveBeenCalledWith(
        "initialize",
        expect.any(Array),
        admin
      );
    });

    it("pause and unpause send admin transactions", async () => {
      const admin = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.pause(admin);
      expect(spy).toHaveBeenCalledWith("pause", [], admin);

      await client.unpause(admin);
      expect(spy).toHaveBeenCalledWith("unpause", [], admin);
    });

    it("updateProtocolFeeBps updates fee in basis points", async () => {
      const admin = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.updateProtocolFeeBps(admin, 300);

      expect(spy).toHaveBeenCalledWith(
        "update_protocol_fee_bps",
        expect.any(Array),
        admin
      );
    });

    it("listPosition lists an investor position for sale", async () => {
      const seller = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.listPosition(seller, 7n, dummyToken, 5000n);

      expect(spy).toHaveBeenCalledWith(
        "list_position",
        expect.any(Array),
        seller
      );
    });

    it("buyPosition purchases a listed position", async () => {
      const buyer = Keypair.random();
      const seller = Keypair.random().publicKey();
      const spy = spyOnInvoke(client);

      await client.buyPosition(buyer, 7n, seller);

      expect(spy).toHaveBeenCalledWith(
        "buy_position",
        expect.any(Array),
        buyer
      );
    });

    it("cancelListing cancels an active listing", async () => {
      const seller = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.cancelListing(seller, 7n);

      expect(spy).toHaveBeenCalledWith(
        "cancel_listing",
        expect.any(Array),
        seller
      );
    });
  });

  describe("GovernanceVotingClient", () => {
    let client: GovernanceVotingClient;

    beforeEach(() => {
      client = new GovernanceVotingClient(dummyContractId, TESTNET);
    });

    it("setVestingContract sets vesting address", async () => {
      const admin = Keypair.random();
      const spy = spyOnInvoke(client);

      await client.setVestingContract(admin, dummyToken);

      expect(spy).toHaveBeenCalledWith("set_vesting_contract", expect.any(Array), admin);
    });

    it("getVestingContract returns address or null", async () => {
      const spy = jest.spyOn(client as any, "invoke").mockResolvedValue(
        nativeToScVal(dummyToken, { type: "address" })
      );

      const vesting = await client.getVestingContract();
      expect(spy).toHaveBeenCalledWith("get_vesting_contract", []);
      expect(vesting).toBeDefined();
    });

    it("getDelegate returns delegate address", async () => {
      const delegateAddress = Keypair.random().publicKey();
      const delegatorAddress = Keypair.random().publicKey();
      const spy = jest.spyOn(client as any, "invoke").mockResolvedValue(
        nativeToScVal(delegateAddress, { type: "address" })
      );

      const delegate = await client.getDelegate(delegatorAddress);
      expect(spy).toHaveBeenCalledWith("get_delegate", expect.any(Array));
      expect(delegate).toBe(delegateAddress);
    });
  });

  describe("KoraClient integration", () => {
    const baseAddresses: KoraAddresses = {
      invoiceNft: dummyContractId,
      marketplace: dummyContractId,
      financingPool: dummyContractId,
      treasury: dummyContractId,
      riskRegistry: dummyContractId,
      accessControl: dummyContractId,
      priceOracle: dummyContractId,
    };

    it("initializes without optional clients if not provided", () => {
      const kora = new KoraClient(baseAddresses, TESTNET);
      expect(kora.invoiceNft).toBeDefined();
      expect(kora.disputeEscrow).toBeUndefined();
      expect(kora.fractionalizer).toBeUndefined();
      expect(kora.secondaryMarket).toBeUndefined();
      expect(kora.governanceVoting).toBeUndefined();
    });

    it("initializes optional clients when addresses are provided", () => {
      const kora = new KoraClient(
        {
          ...baseAddresses,
          disputeEscrow: dummyContractId,
          fractionalizer: dummyContractId,
          secondaryMarket: dummyContractId,
          governanceVoting: dummyContractId,
        },
        TESTNET
      );

      expect(kora.disputeEscrow).toBeInstanceOf(DisputeEscrowClient);
      expect(kora.fractionalizer).toBeInstanceOf(FractionalizerClient);
      expect(kora.secondaryMarket).toBeInstanceOf(SecondaryMarketClient);
      expect(kora.governanceVoting).toBeInstanceOf(GovernanceVotingClient);
    });
  });

  describe("encodeEnum helper", () => {
    it("encodes variant as a single-element ScVal vector containing a symbol", () => {
      const scVal = encodeEnum("Standard");
      expect(scVal.switch()).toBe(xdr.ScValType.scvVec());
      const vec = scVal.vec();
      expect(vec).toBeDefined();
      expect(vec?.length).toBe(1);
      expect(scValToNative(scVal)).toEqual(["Standard"]);
    });
  });

  describe("Error Mappings", () => {
    it("dispute escrow errors map properly", () => {
      expect(DISPUTE_ESCROW_ERROR_MESSAGES[1]).toBe("Already initialized");
      expect(DISPUTE_ESCROW_ERROR_MESSAGES[5]).toBe("Already disputed");
      expect(DISPUTE_ESCROW_ERROR_MESSAGES[8]).toBe("Timeout not reached");
      expect(DISPUTE_ESCROW_ERROR_MESSAGES[9]).toBe("Timeout expired");
      expect(getErrorMessage(5, "dispute_escrow")).toBe("Already disputed");
    });

    it("fractionalizer errors map properly", () => {
      expect(FRACTIONALIZER_ERROR_MESSAGES[8]).toBe("Wrap not found");
      expect(FRACTIONALIZER_ERROR_MESSAGES[9]).toBe("Shares outstanding");
      expect(FRACTIONALIZER_ERROR_MESSAGES[10]).toBe("Pool not closed");
      expect(FRACTIONALIZER_ERROR_MESSAGES[11]).toBe("Already claimed");
      expect(getErrorMessage(8, "fractionalizer")).toBe("Wrap not found");
    });

    it("secondary market errors map properly", () => {
      expect(SECONDARY_MARKET_ERROR_MESSAGES[4]).toBe("Invalid fee rate");
      expect(SECONDARY_MARKET_ERROR_MESSAGES[5]).toBe("Listing not found");
      expect(SECONDARY_MARKET_ERROR_MESSAGES[11]).toBe("Same address");
      expect(getErrorMessage(11, "secondary_market")).toBe("Same address");
    });

    it("governance voting errors map properly", () => {
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[8]).toBe("Already voted");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[12]).toBe("Circular delegation");
      expect(GOVERNANCE_VOTING_ERROR_MESSAGES[15]).toBe("Quadratic voting not allowed");
      expect(getErrorMessage(12, "governance_voting")).toBe("Circular delegation");
    });

    it("fallback to contract error messages", () => {
      expect(CONTRACT_ERROR_MESSAGES[4]).toBe("Protocol paused");
      expect(CONTRACT_ERROR_MESSAGES[14]).toBe("Invalid amount");
      expect(CONTRACT_ERROR_MESSAGES[90]).toBe("Arithmetic overflow");
      expect(getErrorMessage(90)).toBe("Arithmetic overflow");
    });
  });
});

describe("DebtorAttestationClient and read-only simulation (#788, #938)", () => {
  const contractId = StrKey.encodeContract(Buffer.alloc(32, 7));

  it("attestViaRelay sends relay, debtor, invoice, amount, expiry, nonce and sig", async () => {
    const client = new DebtorAttestationClient(contractId, TESTNET);
    const relay = Keypair.random();
    const debtor = Keypair.random().publicKey();
    const spy = jest.spyOn(client as any, "invoke").mockResolvedValue(xdr.ScVal.scvVoid());
    await client.attestViaRelay(relay, debtor, 7n, 1000n, 2000n, 3n);
    expect(spy).toHaveBeenCalledWith("attest_via_relay", expect.any(Array), relay);
    const args = spy.mock.calls[0][1] as xdr.ScVal[];
    expect(args).toHaveLength(7);
    expect(scValToNative(args[5])).toBe(3n);
  });

  it("reject is signed by the caller", async () => {
    const client = new DebtorAttestationClient(contractId, TESTNET);
    const caller = Keypair.random();
    const spy = jest.spyOn(client as any, "invoke").mockResolvedValue(xdr.ScVal.scvVoid());
    await client.reject(caller, 9n);
    expect(spy).toHaveBeenCalledWith("reject", expect.any(Array), caller);
  });

  it("decodes status enums, nonces and confirmation flags", async () => {
    const client = new DebtorAttestationClient(contractId, TESTNET);
    const spy = jest.spyOn(client as any, "invoke");
    spy.mockResolvedValueOnce(xdr.ScVal.scvVec([nativeToScVal("Confirmed", { type: "symbol" })]));
    await expect(client.attestationStatus(1n)).resolves.toBe("Confirmed");
    spy.mockResolvedValueOnce(nativeToScVal(4n, { type: "u64" }));
    await expect(client.nonceOf(Keypair.random().publicKey())).resolves.toBe(4n);
    spy.mockResolvedValueOnce(xdr.ScVal.scvBool(true));
    await expect(client.isConfirmed(1n, 10n)).resolves.toBe(true);
  });

  it("KoraClient wires debtorAttestation when the address is provided", () => {
    const addresses: KoraAddresses = {
      invoiceNft: contractId, marketplace: contractId, financingPool: contractId, treasury: contractId,
      riskRegistry: contractId, accessControl: contractId, priceOracle: contractId, debtorAttestation: contractId,
    };
    expect(new KoraClient(addresses, TESTNET).debtorAttestation).toBeInstanceOf(DebtorAttestationClient);
  });

  it("read-only calls simulate from the null account instead of loading the contract as an account", async () => {
    const client = new DebtorAttestationClient(contractId, TESTNET);
    const server = (client as any).server;
    const getAccount = jest.spyOn(server, "getAccount");
    const simulate = jest.spyOn(server, "simulateTransaction").mockResolvedValue({
      id: "1", latestLedger: 1, events: [], _parsed: true, transactionData: undefined,
      minResourceFee: "0", cost: { cpuInsns: "0", memBytes: "0" },
      result: { auth: [], retval: xdr.ScVal.scvBool(false) },
    } as any);
    await expect(client.isConfirmed(1n, 1n)).resolves.toBe(false);
    expect(getAccount).not.toHaveBeenCalled();
    const tx = simulate.mock.calls[0][0] as any;
    expect(tx.source).toBe(NULL_ACCOUNT);
  });
});

describe("GovernanceVotingClient tally reads (#933)", () => {
  const contractId = StrKey.encodeContract(Buffer.alloc(32, 9));
  const snakeProposal = () =>
    nativeToScVal({
      id: nativeToScVal(3n, { type: "u64" }),
      proposer: Keypair.random().publicKey(),
      title: "Grant",
      description: "d",
      proposal_type: xdr.ScVal.scvVec([nativeToScVal("Financial", { type: "symbol" })]),
      voting_mode: xdr.ScVal.scvVec([nativeToScVal("Standard", { type: "symbol" })]),
      votes_for: nativeToScVal(700n, { type: "i128" }),
      votes_against: nativeToScVal(200n, { type: "i128" }),
      votes_abstain: nativeToScVal(100n, { type: "i128" }),
      created_at: nativeToScVal(10n, { type: "u64" }),
      expires_at: nativeToScVal(20n, { type: "u64" }),
      executed: false,
      quorum_required: nativeToScVal(500n, { type: "i128" }),
    });

  it("decodes snake_case proposal structs into the camelCase Proposal type", async () => {
    const client = new GovernanceVotingClient(contractId, TESTNET);
    jest.spyOn(client as any, "invoke").mockResolvedValue(snakeProposal());
    const proposal = await client.getProposal(3n);
    expect(proposal).toMatchObject({ id: 3n, votesFor: 700n, votingMode: "Standard", proposalType: "Financial", quorumRequired: 500n });
  });

  it("getProposalTally returns on-chain counts or null", async () => {
    const client = new GovernanceVotingClient(contractId, TESTNET);
    const spy = jest.spyOn(client as any, "invoke").mockResolvedValueOnce(snakeProposal());
    await expect(client.getProposalTally(3n)).resolves.toEqual({
      proposalId: 3n, votesFor: 700n, votesAgainst: 200n, votesAbstain: 100n, quorumRequired: 500n,
      createdAt: 10n, expiresAt: 20n, executed: false, votingMode: "Standard",
    });
    spy.mockResolvedValueOnce(xdr.ScVal.scvVoid());
    await expect(client.getProposalTally(4n)).resolves.toBeNull();
  });

  it("decodeProposal accepts camelCase input and rejects non-objects", () => {
    expect(decodeProposal(null)).toBeNull();
    expect(decodeProposal({ id: 1, votesFor: "5", votingMode: "Quadratic", executed: true })).toMatchObject({
      id: 1n, votesFor: 5n, votesAgainst: 0n, votingMode: "Quadratic", executed: true,
    });
  });
});
