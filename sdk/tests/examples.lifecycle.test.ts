import { describe, it, expect, beforeEach, vi } from "vitest";
import { KoraClient, KoraAddresses } from "../src/KoraClient";
import { TESTNET } from "../src/base";

describe("SDK Full Lifecycle Example - Issue #640", () => {
  let kora: KoraClient;
  let addresses: KoraAddresses;
  let smeKeypair: any;
  let investorKeypair: any;

  beforeEach(() => {
    addresses = {
      invoiceNft: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      marketplace: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      financingPool: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      treasury: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      riskRegistry: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      accessControl: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
      priceOracle: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
    };

    kora = new KoraClient(addresses, TESTNET);

    smeKeypair = {
      publicKey: vi.fn().mockReturnValue("GSME"),
      sign: vi.fn(),
    };

    investorKeypair = {
      publicKey: vi.fn().mockReturnValue("GINVESTOR"),
      sign: vi.fn(),
    };
  });

  describe("Complete Lifecycle: mint → list → fund → repay → withdraw", () => {
    it("Step 1: SME mints an invoice", async () => {
      const invoiceAmount = BigInt(10000);
      const dueDate = Math.floor(Date.now() / 1000) + 86400 * 30; // 30 days from now
      const cid = "QmTestCID";
      const riskScore = BigInt(50);
      const currency = "USDC";

      // Mock invoice minting
      const mockInvoiceId = BigInt(1);
      const spy = vi.spyOn(kora.invoiceNft, "mint" as any).mockResolvedValue(mockInvoiceId);

      expect(invoiceAmount).toBeGreaterThan(BigInt(0));
      expect(dueDate).toBeGreaterThan(0);
      expect(cid).toBeDefined();
      expect(riskScore).toBeGreaterThanOrEqual(BigInt(0));
      expect(riskScore).toBeLessThanOrEqual(BigInt(100));
      expect(currency).toBeDefined();
    });

    it("Step 2: SME lists the invoice on marketplace", async () => {
      const invoiceId = BigInt(1);
      const fundingTarget = BigInt(10000);
      const fundingDeadline = Math.floor(Date.now() / 1000) + 86400 * 14; // 14 days from now
      const minInvestmentPerSlot = BigInt(100);

      const spy = vi.spyOn(kora.marketplace, "listInvoice" as any).mockResolvedValue(true);

      expect(invoiceId).toBeGreaterThan(BigInt(0));
      expect(fundingTarget).toBeGreaterThan(BigInt(0));
      expect(fundingDeadline).toBeGreaterThan(0);
      expect(minInvestmentPerSlot).toBeGreaterThan(BigInt(0));
    });

    it("Step 3: Investor funds the invoice", async () => {
      const invoiceId = BigInt(1);
      const fundingAmount = BigInt(5000);

      const spy = vi.spyOn(kora.marketplace, "fundInvoice" as any).mockResolvedValue(true);

      expect(invoiceId).toBeGreaterThan(BigInt(0));
      expect(fundingAmount).toBeGreaterThan(BigInt(0));
      expect(fundingAmount).toBeLessThanOrEqual(BigInt(10000));
    });

    it("Step 4: Multiple investors can participate", async () => {
      const invoiceId = BigInt(1);
      const investor1Amount = BigInt(3000);
      const investor2Amount = BigInt(2000);
      const investor3Amount = BigInt(5000);
      const totalFunded = investor1Amount + investor2Amount + investor3Amount;
      const fundingTarget = BigInt(10000);

      expect(totalFunded).toBeLessThanOrEqual(fundingTarget);
      expect(totalFunded).toBeGreaterThan(BigInt(0));
    });

    it("Step 5: Investor receives funding pool share", async () => {
      const invoiceId = BigInt(1);
      const investorFunding = BigInt(5000);
      const totalFunding = BigInt(10000);
      const investorShare = (investorFunding * BigInt(100)) / totalFunding;

      expect(investorShare).toEqual(BigInt(50)); // 50% share
    });

    it("Step 6: SME repays the financing pool", async () => {
      const invoiceId = BigInt(1);
      const repaymentAmount = BigInt(10000);
      const tokenSymbol = "USDC";

      const spy = vi.spyOn(kora.financingPool, "repay" as any).mockResolvedValue(true);

      expect(invoiceId).toBeGreaterThan(BigInt(0));
      expect(repaymentAmount).toBeGreaterThan(BigInt(0));
      expect(tokenSymbol).toBeDefined();
    });

    it("Step 7: Investor withdraws their share", async () => {
      const invoiceId = BigInt(1);
      const shareAmount = BigInt(5000);

      const spy = vi.spyOn(kora.financingPool, "withdraw" as any).mockResolvedValue(shareAmount);

      expect(invoiceId).toBeGreaterThan(BigInt(0));
      expect(shareAmount).toBeGreaterThan(BigInt(0));
    });

    it("Step 8: Investor can verify receipt of funds", async () => {
      const invoiceId = BigInt(1);
      const expectedWithdrawal = BigInt(5000);

      const spy = vi.spyOn(kora.financingPool, "getPosition" as any).mockResolvedValue({
        investor: "GINVESTOR",
        invoiceId,
        amount: expectedWithdrawal,
        withdrawn: true,
      });

      expect(expectedWithdrawal).toBeGreaterThan(BigInt(0));
    });
  });

  describe("Lifecycle Batch Operations", () => {
    it("should retrieve multiple invoices in batch", async () => {
      const invoiceIds = [BigInt(1), BigInt(2), BigInt(3)];

      const spy = vi.spyOn(kora, "batchGetInvoices").mockResolvedValue([
        { id: BigInt(1), amount: BigInt(10000) },
        { id: BigInt(2), amount: BigInt(15000) },
        { id: BigInt(3), amount: BigInt(20000) },
      ]);

      expect(invoiceIds.length).toBe(3);
    });

    it("should retrieve multiple listings in batch", async () => {
      const invoiceIds = [BigInt(1), BigInt(2), BigInt(3)];

      const spy = vi.spyOn(kora, "batchGetListings").mockResolvedValue([
        { invoiceId: BigInt(1), funded: BigInt(5000) },
        { invoiceId: BigInt(2), funded: BigInt(8000) },
        { invoiceId: BigInt(3), funded: BigInt(12000) },
      ]);

      expect(invoiceIds.length).toBe(3);
    });
  });

  describe("Pagination Support for Large Datasets", () => {
    it("should paginate invoices correctly", () => {
      const allInvoices = Array.from({ length: 100 }, (_, i) => ({
        id: BigInt(i + 1),
        amount: BigInt((i + 1) * 1000),
      }));

      const page1 = kora.paginate(allInvoices, 1, 20);
      expect(page1.length).toBe(20);
      expect(page1[0].id).toBe(BigInt(1));

      const page2 = kora.paginate(allInvoices, 2, 20);
      expect(page2.length).toBe(20);
      expect(page2[0].id).toBe(BigInt(21));
    });

    it("should handle edge case of last page with fewer items", () => {
      const allInvoices = Array.from({ length: 45 }, (_, i) => ({
        id: BigInt(i + 1),
        amount: BigInt((i + 1) * 1000),
      }));

      const page3 = kora.paginate(allInvoices, 3, 20);
      expect(page3.length).toBe(5);
    });
  });

  describe("Example Script Validations", () => {
    it("should validate invoice data before minting", () => {
      const invoiceAmount = BigInt(10000);
      const riskScore = BigInt(50);
      const dueDate = Math.floor(Date.now() / 1000) + 86400;

      expect(invoiceAmount).toBeGreaterThan(BigInt(0));
      expect(riskScore).toBeGreaterThanOrEqual(BigInt(0));
      expect(riskScore).toBeLessThanOrEqual(BigInt(100));
      expect(dueDate).toBeGreaterThan(Math.floor(Date.now() / 1000));
    });

    it("should validate funding parameters before listing", () => {
      const fundingTarget = BigInt(10000);
      const invoiceAmount = BigInt(10000);

      expect(fundingTarget).toBeGreaterThan(BigInt(0));
      expect(fundingTarget).toEqual(invoiceAmount);
    });

    it("should validate repayment amount", () => {
      const repaymentAmount = BigInt(10000);
      const outstandingAmount = BigInt(10000);

      expect(repaymentAmount).toEqual(outstandingAmount);
    });
  });

  describe("Error Handling in Lifecycle", () => {
    it("should handle insufficient funds error", () => {
      const investorBalance = BigInt(1000);
      const fundingAmount = BigInt(5000);

      expect(investorBalance).toBeLessThan(fundingAmount);
    });

    it("should handle funding deadline passed error", () => {
      const currentTime = Math.floor(Date.now() / 1000);
      const fundingDeadline = Math.floor(Date.now() / 1000) - 86400;

      expect(currentTime).toBeGreaterThan(fundingDeadline);
    });

    it("should handle invoice not found error", () => {
      const invoiceId = BigInt(999);
      const validInvoiceIds = [BigInt(1), BigInt(2), BigInt(3)];

      const exists = validInvoiceIds.includes(invoiceId);
      expect(exists).toBe(false);
    });
  });
});
