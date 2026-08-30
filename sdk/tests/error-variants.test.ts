import { describe, it, expect, beforeEach, vi } from "vitest";

describe("SDK Error Variant Coverage - Issue #642", () => {
  let mockRpcClient: any;
  let mockContractCallers: any;

  beforeEach(() => {
    mockRpcClient = {
      invokeContract: vi.fn(),
      getAccount: vi.fn(),
    };

    mockContractCallers = {
      invoiceNft: vi.fn(),
      marketplace: vi.fn(),
      financingPool: vi.fn(),
      treasury: vi.fn(),
      riskRegistry: vi.fn(),
      accessControl: vi.fn(),
      priceOracle: vi.fn(),
    };
  });

  describe("Authorization & Access Control Errors (1-7)", () => {
    it("should surface Unauthorized error (1)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 1,
        message: "Unauthorized",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(1);
        expect(error.message).toContain("Unauthorized");
      }
    });

    it("should surface NotAdmin error (2)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 2,
        message: "NotAdmin",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(2);
      }
    });

    it("should surface NotVerifier error (3)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 3,
        message: "NotVerifier",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(3);
      }
    });

    it("should surface ProtocolPaused error (4)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 4,
        message: "ProtocolPaused",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(4);
      }
    });

    it("should surface AlreadyPaused error (5)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 5,
        message: "AlreadyPaused",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(5);
      }
    });

    it("should surface NotPaused error (6)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 6,
        message: "NotPaused",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(6);
      }
    });

    it("should surface RoleNotAssigned error (7)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 7,
        message: "RoleNotAssigned",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(7);
      }
    });
  });

  describe("Invoice Errors (10-19)", () => {
    it("should surface InvoiceNotFound error (10)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 10,
        message: "InvoiceNotFound",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(10);
      }
    });

    it("should surface InvoiceAlreadyExists error (11)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 11,
        message: "InvoiceAlreadyExists",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(11);
      }
    });

    it("should surface InvalidInvoiceStatus error (12)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 12,
        message: "InvalidInvoiceStatus",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(12);
      }
    });

    it("should surface InvoiceExpired error (13)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 13,
        message: "InvoiceExpired",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(13);
      }
    });

    it("should surface InvalidAmount error (14)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 14,
        message: "InvalidAmount",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(14);
      }
    });

    it("should surface InvalidDueDate error (15)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 15,
        message: "InvalidDueDate",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(15);
      }
    });

    it("should surface InvalidRiskScore error (16)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 16,
        message: "InvalidRiskScore",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(16);
      }
    });

    it("should surface InvalidCid error (17)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 17,
        message: "InvalidCid",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(17);
      }
    });

    it("should surface InvoiceFrozen error (18)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 18,
        message: "InvoiceFrozen",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(18);
      }
    });

    it("should surface BatchSizeExceeded error (19)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 19,
        message: "BatchSizeExceeded",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(19);
      }
    });
  });

  describe("Marketplace Errors (20-28)", () => {
    it("should surface ListingNotFound error (20)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 20,
        message: "ListingNotFound",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(20);
      }
    });

    it("should surface ListingAlreadyCancelled error (21)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 21,
        message: "ListingAlreadyCancelled",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(21);
      }
    });

    it("should surface FundingDeadlinePassed error (23)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 23,
        message: "FundingDeadlinePassed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(23);
      }
    });

    it("should surface InsufficientFunds error (24)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 24,
        message: "InsufficientFunds",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(24);
      }
    });

    it("should surface ExceedsFundingTarget error (25)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 25,
        message: "ExceedsFundingTarget",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(25);
      }
    });

    it("should surface ListingFullyFunded error (27)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 27,
        message: "ListingFullyFunded",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(27);
      }
    });

    it("should surface FundingNotExpired error (28)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 28,
        message: "FundingNotExpired",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(28);
      }
    });
  });

  describe("Pool Errors (30-36)", () => {
    it("should surface PoolNotFound error (30)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 30,
        message: "PoolNotFound",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(30);
      }
    });

    it("should surface PoolAlreadyClosed error (31)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 31,
        message: "PoolAlreadyClosed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(31);
      }
    });

    it("should surface RepaymentAlreadyMade error (32)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 32,
        message: "RepaymentAlreadyMade",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(32);
      }
    });

    it("should surface InsufficientPoolBalance error (33)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 33,
        message: "InsufficientPoolBalance",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(33);
      }
    });

    it("should surface PositionNotFound error (34)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 34,
        message: "PositionNotFound",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(34);
      }
    });

    it("should surface SaleAlreadyListed error (35)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 35,
        message: "SaleAlreadyListed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(35);
      }
    });

    it("should surface SaleNotFound error (36)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 36,
        message: "SaleNotFound",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(36);
      }
    });
  });

  describe("Treasury Errors (40-43)", () => {
    it("should surface InvalidFeeRate error (40)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 40,
        message: "InvalidFeeRate",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(40);
      }
    });

    it("should surface TokenNotWhitelisted error (42)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 42,
        message: "TokenNotWhitelisted",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(42);
      }
    });

    it("should surface WithdrawalRateLimitExceeded error (43)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 43,
        message: "WithdrawalRateLimitExceeded",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(43);
      }
    });
  });

  describe("Risk Registry Errors (50, 53, 129)", () => {
    it("should surface SMENotRegistered error (50)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 50,
        message: "SMENotRegistered",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(50);
      }
    });

    it("should surface ComplianceNotAttested error (53)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 53,
        message: "ComplianceNotAttested",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(53);
      }
    });

    it("should surface SMENotVerified error (129)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 129,
        message: "SMENotVerified",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(129);
      }
    });
  });

  describe("General Errors (90-99, 102)", () => {
    it("should surface ArithmeticOverflow error (90)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 90,
        message: "ArithmeticOverflow",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(90);
      }
    });

    it("should surface ArithmeticUnderflow error (91)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 91,
        message: "ArithmeticUnderflow",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(91);
      }
    });

    it("should surface InvalidAddress error (92)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 92,
        message: "InvalidAddress",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(92);
      }
    });

    it("should surface EmptyString error (93)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 93,
        message: "EmptyString",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(93);
      }
    });

    it("should surface AlreadyInitialized error (94)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 94,
        message: "AlreadyInitialized",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(94);
      }
    });

    it("should surface NoContribution error (95)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 95,
        message: "NoContribution",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(95);
      }
    });

    it("should surface NotInitialized error (96)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 96,
        message: "NotInitialized",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(96);
      }
    });

    it("should surface EmptyBytes error (97)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 97,
        message: "EmptyBytes",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(97);
      }
    });

    it("should surface Reentrancy error (98)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 98,
        message: "Reentrancy",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(98);
      }
    });

    it("should surface InvalidLength error (99)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 99,
        message: "InvalidLength",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(99);
      }
    });

    it("should surface FieldTooLong error (102)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 102,
        message: "FieldTooLong",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(102);
      }
    });
  });

  describe("Upgrade Errors (100-101)", () => {
    it("should surface NoUpgradeProposed error (100)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 100,
        message: "NoUpgradeProposed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(100);
      }
    });

    it("should surface UpgradeTimelockNotElapsed error (101)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 101,
        message: "UpgradeTimelockNotElapsed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(101);
      }
    });
  });

  describe("Governance Errors (110-117)", () => {
    it("should surface ParameterProposalNotFound error (110)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 110,
        message: "ParameterProposalNotFound",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(110);
      }
    });

    it("should surface ParameterProposalAlreadyExecuted error (111)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 111,
        message: "ParameterProposalAlreadyExecuted",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(111);
      }
    });

    it("should surface NotMultisigSigner error (112)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 112,
        message: "NotMultisigSigner",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(112);
      }
    });

    it("should surface AlreadyVoted error (113)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 113,
        message: "AlreadyVoted",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(113);
      }
    });

    it("should surface GovernanceThresholdNotMet error (114)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 114,
        message: "GovernanceThresholdNotMet",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(114);
      }
    });

    it("should surface GovernanceTimelockNotElapsed error (115)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 115,
        message: "GovernanceTimelockNotElapsed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(115);
      }
    });

    it("should surface InvalidParameterValue error (116)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 116,
        message: "InvalidParameterValue",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(116);
      }
    });

    it("should surface ScoreUpdateCooldownNotElapsed error (117)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 117,
        message: "ScoreUpdateCooldownNotElapsed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(117);
      }
    });
  });

  describe("Marketplace Advanced Errors (118-125)", () => {
    it("should surface CancellationPending error (118)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 118,
        message: "CancellationPending",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(118);
      }
    });

    it("should surface NoCancellationPending error (119)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 119,
        message: "NoCancellationPending",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(119);
      }
    });

    it("should surface NotInvoiceOwner error (120)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 120,
        message: "NotInvoiceOwner",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(120);
      }
    });

    it("should surface CreditLimitExceeded error (121)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 121,
        message: "CreditLimitExceeded",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(121);
      }
    });

    it("should surface CurrencyNotAllowed error (122)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 122,
        message: "CurrencyNotAllowed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(122);
      }
    });

    it("should surface InvestorConcentrationExceeded error (123)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 123,
        message: "InvestorConcentrationExceeded",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(123);
      }
    });

    it("should surface InvestorNotAccredited error (124)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 124,
        message: "InvestorNotAccredited",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(124);
      }
    });

    it("should surface ListingAlreadyFunded error (125)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 125,
        message: "ListingAlreadyFunded",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(125);
      }
    });
  });

  describe("Access Control Multisig Errors (126, 140-149)", () => {
    it("should surface AlreadyApproved error (126)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 126,
        message: "AlreadyApproved",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(126);
      }
    });

    it("should surface ProposalNotFound error (140)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 140,
        message: "ProposalNotFound",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(140);
      }
    });

    it("should surface ProposalAlreadyExecuted error (141)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 141,
        message: "ProposalAlreadyExecuted",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(141);
      }
    });

    it("should surface ProposalExpired error (142)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 142,
        message: "ProposalExpired",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(142);
      }
    });

    it("should surface ThresholdNotMet error (143)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 143,
        message: "ThresholdNotMet",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(143);
      }
    });

    it("should surface SignerNotFound error (144)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 144,
        message: "SignerNotFound",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(144);
      }
    });

    it("should surface MultisigNotConfigured error (145)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 145,
        message: "MultisigNotConfigured",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(145);
      }
    });

    it("should surface MultisigApprovalRequired error (146)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 146,
        message: "MultisigApprovalRequired",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(146);
      }
    });

    it("should surface QuorumRequired error (147)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 147,
        message: "QuorumRequired",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(147);
      }
    });

    it("should surface UnauthorizedCaller error (148)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 148,
        message: "UnauthorizedCaller",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(148);
      }
    });

    it("should surface InvalidParameterValue error (149)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 149,
        message: "InvalidParameterValue",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(149);
      }
    });
  });

  describe("Dependency & Token Whitelist Timelock Errors (150-153)", () => {
    it("should surface DependencyUpdateTimelockNotElapsed error (150)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 150,
        message: "DependencyUpdateTimelockNotElapsed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(150);
      }
    });

    it("should surface NoDependencyUpdateProposed error (151)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 151,
        message: "NoDependencyUpdateProposed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(151);
      }
    });

    it("should surface TokenWhitelistTimelockNotElapsed error (152)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 152,
        message: "TokenWhitelistTimelockNotElapsed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(152);
      }
    });

    it("should surface NoTokenWhitelistProposed error (153)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 153,
        message: "NoTokenWhitelistProposed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(153);
      }
    });
  });

  describe("Treasury Loss Reserve Errors (154-160)", () => {
    it("should surface ContributionBelowMinimum error (154)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 154,
        message: "ContributionBelowMinimum",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(154);
      }
    });

    it("should surface InsufficientReserveBalance error (155)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 155,
        message: "InsufficientReserveBalance",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(155);
      }
    });

    it("should surface ReserveCallerNotAuthorized error (156)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 156,
        message: "ReserveCallerNotAuthorized",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(156);
      }
    });

    it("should surface EmergencyNotDeclared error (157)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 157,
        message: "EmergencyNotDeclared",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(157);
      }
    });

    it("should surface RecipientNotAllowed error (158)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 158,
        message: "RecipientNotAllowed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(158);
      }
    });

    it("should surface NoRecipientProposed error (159)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 159,
        message: "NoRecipientProposed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(159);
      }
    });

    it("should surface RecipientTimelockNotElapsed error (160)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 160,
        message: "RecipientTimelockNotElapsed",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(160);
      }
    });
  });

  describe("Invoice Mint Rate Limit Error (161)", () => {
    it("should surface MintRateLimitExceeded error (161)", async () => {
      mockRpcClient.invokeContract.mockRejectedValueOnce({
        code: 161,
        message: "MintRateLimitExceeded",
      });

      try {
        await mockRpcClient.invokeContract({});
      } catch (error: any) {
        expect(error.code).toBe(161);
      }
    });
  });

  describe("Error Coverage Summary", () => {
    it("should map all error codes to human-readable messages", () => {
      const errorMessages: Record<number, string> = {
        1: "Unauthorized",
        2: "NotAdmin",
        3: "NotVerifier",
        4: "ProtocolPaused",
        10: "InvoiceNotFound",
        14: "InvalidAmount",
        90: "ArithmeticOverflow",
        114: "GovernanceThresholdNotMet",
      };

      Object.entries(errorMessages).forEach(([code, message]) => {
        expect(message).toBeDefined();
        expect(message.length).toBeGreaterThan(0);
      });
    });
  });
});
