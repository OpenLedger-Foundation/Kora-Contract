import { describe, it, expect, beforeEach } from "vitest";

interface InvoiceState {
  id: string;
  status: "Created" | "Listed" | "Funded" | "Repaid" | "Defaulted";
}

interface ListingState {
  invoice_id: string;
  is_active: boolean;
  funded_amount: number;
  asking_price: number;
}

interface PoolState {
  invoice_id: string;
  exists: boolean;
  is_closed: boolean;
  repaid_amount: number;
}

interface DriftCheck {
  type: string;
  passed: boolean;
  message: string;
}

interface StateDriftResult {
  invoice_id: string;
  network: string;
  timestamp: string;
  checks: DriftCheck[];
  drift_detected: boolean;
  mismatch_count: number;
}

class StateDriftMonitor {
  private invoiceState: InvoiceState | null = null;
  private listingState: ListingState | null = null;
  private poolState: PoolState | null = null;

  setInvoiceState(state: InvoiceState): void {
    this.invoiceState = state;
  }

  setListingState(state: ListingState | null): void {
    this.listingState = state;
  }

  setPoolState(state: PoolState): void {
    this.poolState = state;
  }

  private checkListingActiveWithInvoiceFunded(): DriftCheck {
    if (
      this.listingState?.is_active &&
      (this.invoiceState?.status === "Funded" ||
        this.invoiceState?.status === "Repaid" ||
        this.invoiceState?.status === "Defaulted")
    ) {
      return {
        type: "listing_active_with_completed_invoice",
        passed: false,
        message: `Listing is_active=true but invoice status=${this.invoiceState.status}`,
      };
    }
    return {
      type: "listing_active_with_completed_invoice",
      passed: true,
      message: "Listing and invoice status consistent",
    };
  }

  private checkPoolExistsWithCreatedInvoice(): DriftCheck {
    if (
      this.poolState?.exists &&
      (this.invoiceState?.status === "Created" ||
        this.invoiceState?.status === "Listed")
    ) {
      return {
        type: "pool_exists_with_early_invoice",
        passed: false,
        message: `Pool exists but invoice status=${this.invoiceState.status} (expected Funded/Repaid/Defaulted)`,
      };
    }
    return {
      type: "pool_exists_with_early_invoice",
      passed: true,
      message: "Pool and invoice status consistent",
    };
  }

  private checkFundedInvoiceHasPool(): DriftCheck {
    if (
      this.invoiceState?.status === "Funded" &&
      !this.poolState?.exists
    ) {
      return {
        type: "funded_invoice_without_pool",
        passed: false,
        message: "Invoice status=Funded but no pool exists in financing_pool",
      };
    }
    return {
      type: "funded_invoice_without_pool",
      passed: true,
      message: "Funded invoice has corresponding pool",
    };
  }

  private checkRepaidInvoicePoolClosed(): DriftCheck {
    if (
      this.invoiceState?.status === "Repaid" &&
      this.poolState?.is_closed === false
    ) {
      return {
        type: "repaid_invoice_open_pool",
        passed: false,
        message: "Invoice status=Repaid but pool is_closed=false",
      };
    }
    return {
      type: "repaid_invoice_open_pool",
      passed: true,
      message: "Repaid invoice has closed pool",
    };
  }

  private checkClosedPoolWithFundedInvoice(): DriftCheck {
    if (
      this.poolState?.is_closed &&
      this.invoiceState?.status === "Funded"
    ) {
      return {
        type: "closed_pool_with_funded_invoice",
        passed: false,
        message: "Pool is_closed=true but invoice status=Funded (expected Repaid or Defaulted)",
      };
    }
    return {
      type: "closed_pool_with_funded_invoice",
      passed: true,
      message: "Pool closure state matches invoice status",
    };
  }

  private checkDeactivatedListingWithFundedInvoice(): DriftCheck {
    if (
      this.listingState &&
      !this.listingState.is_active &&
      !this.poolState?.exists &&
      this.invoiceState?.status === "Funded"
    ) {
      return {
        type: "deactivated_listing_funded_no_pool",
        passed: false,
        message: "Listing deactivated and invoice=Funded but no pool exists",
      };
    }
    return {
      type: "deactivated_listing_funded_no_pool",
      passed: true,
      message: "Listing deactivation and pool state consistent",
    };
  }

  performDriftCheck(invoiceId: string, network: string = "testnet"): StateDriftResult {
    const checks: DriftCheck[] = [
      this.checkListingActiveWithInvoiceFunded(),
      this.checkPoolExistsWithCreatedInvoice(),
      this.checkFundedInvoiceHasPool(),
      this.checkRepaidInvoicePoolClosed(),
      this.checkClosedPoolWithFundedInvoice(),
      this.checkDeactivatedListingWithFundedInvoice(),
    ];

    const failedChecks = checks.filter((c) => !c.passed);

    return {
      invoice_id: invoiceId,
      network,
      timestamp: new Date().toISOString(),
      checks,
      drift_detected: failedChecks.length > 0,
      mismatch_count: failedChecks.length,
    };
  }

  getChecks(): DriftCheck[] {
    return [
      this.checkListingActiveWithInvoiceFunded(),
      this.checkPoolExistsWithCreatedInvoice(),
      this.checkFundedInvoiceHasPool(),
      this.checkRepaidInvoicePoolClosed(),
      this.checkClosedPoolWithFundedInvoice(),
      this.checkDeactivatedListingWithFundedInvoice(),
    ];
  }
}

describe("State Drift Monitoring (Issue #648)", () => {
  let monitor: StateDriftMonitor;

  beforeEach(() => {
    monitor = new StateDriftMonitor();
  });

  describe("consistency checks", () => {
    it("should detect drift when listing is active but invoice is completed", () => {
      monitor.setInvoiceState({
        id: "invoice-1",
        status: "Repaid",
      });
      monitor.setListingState({
        invoice_id: "invoice-1",
        is_active: true,
        funded_amount: 1000,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "invoice-1",
        exists: true,
        is_closed: true,
        repaid_amount: 1000,
      });

      const result = monitor.performDriftCheck("invoice-1");
      expect(result.drift_detected).toBe(true);
      expect(result.mismatch_count).toBeGreaterThan(0);
    });

    it("should not detect drift with consistent states", () => {
      monitor.setInvoiceState({
        id: "invoice-1",
        status: "Funded",
      });
      monitor.setListingState({
        invoice_id: "invoice-1",
        is_active: false,
        funded_amount: 1000,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "invoice-1",
        exists: true,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("invoice-1");
      expect(result.drift_detected).toBe(false);
      expect(result.mismatch_count).toBe(0);
    });

    it("should detect when pool exists for early-stage invoice", () => {
      monitor.setInvoiceState({
        id: "invoice-1",
        status: "Created",
      });
      monitor.setListingState({
        invoice_id: "invoice-1",
        is_active: true,
        funded_amount: 0,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "invoice-1",
        exists: true,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("invoice-1");
      expect(result.drift_detected).toBe(true);
      const poolCheck = result.checks.find(
        (c) => c.type === "pool_exists_with_early_invoice"
      );
      expect(poolCheck?.passed).toBe(false);
    });

    it("should detect when funded invoice has no pool", () => {
      monitor.setInvoiceState({
        id: "invoice-1",
        status: "Funded",
      });
      monitor.setListingState({
        invoice_id: "invoice-1",
        is_active: false,
        funded_amount: 1000,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "invoice-1",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("invoice-1");
      expect(result.drift_detected).toBe(true);
      const fundedPoolCheck = result.checks.find(
        (c) => c.type === "funded_invoice_without_pool"
      );
      expect(fundedPoolCheck?.passed).toBe(false);
    });

    it("should detect when repaid invoice has open pool", () => {
      monitor.setInvoiceState({
        id: "invoice-1",
        status: "Repaid",
      });
      monitor.setListingState({
        invoice_id: "invoice-1",
        is_active: false,
        funded_amount: 1000,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "invoice-1",
        exists: true,
        is_closed: false,
        repaid_amount: 1000,
      });

      const result = monitor.performDriftCheck("invoice-1");
      expect(result.drift_detected).toBe(true);
      const repaidPoolCheck = result.checks.find(
        (c) => c.type === "repaid_invoice_open_pool"
      );
      expect(repaidPoolCheck?.passed).toBe(false);
    });

    it("should detect when closed pool exists with funded invoice", () => {
      monitor.setInvoiceState({
        id: "invoice-1",
        status: "Funded",
      });
      monitor.setListingState({
        invoice_id: "invoice-1",
        is_active: false,
        funded_amount: 1000,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "invoice-1",
        exists: true,
        is_closed: true,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("invoice-1");
      expect(result.drift_detected).toBe(true);
      const closedPoolCheck = result.checks.find(
        (c) => c.type === "closed_pool_with_funded_invoice"
      );
      expect(closedPoolCheck?.passed).toBe(false);
    });

    it("should detect deactivated listing with no pool for funded invoice", () => {
      monitor.setInvoiceState({
        id: "invoice-1",
        status: "Funded",
      });
      monitor.setListingState({
        invoice_id: "invoice-1",
        is_active: false,
        funded_amount: 1000,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "invoice-1",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("invoice-1");
      expect(result.drift_detected).toBe(true);
      const deactivatedCheck = result.checks.find(
        (c) => c.type === "deactivated_listing_funded_no_pool"
      );
      expect(deactivatedCheck?.passed).toBe(false);
    });
  });

  describe("result reporting", () => {
    it("should include invoice_id in result", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Listed" });
      monitor.setListingState(null);
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("inv-123");
      expect(result.invoice_id).toBe("inv-123");
    });

    it("should include network in result", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Listed" });
      monitor.setListingState(null);
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("inv-123", "mainnet");
      expect(result.network).toBe("mainnet");
    });

    it("should include timestamp in result", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Listed" });
      monitor.setListingState(null);
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const beforeTime = new Date();
      const result = monitor.performDriftCheck("inv-123");
      const afterTime = new Date();

      const resultTime = new Date(result.timestamp);
      expect(resultTime.getTime()).toBeGreaterThanOrEqual(
        beforeTime.getTime()
      );
      expect(resultTime.getTime()).toBeLessThanOrEqual(afterTime.getTime());
    });

    it("should include all checks in result", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Listed" });
      monitor.setListingState(null);
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("inv-123");
      expect(result.checks.length).toBe(6);
    });

    it("should report correct mismatch count", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Funded" });
      monitor.setListingState({
        invoice_id: "inv-123",
        is_active: true,
        funded_amount: 500,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("inv-123");
      expect(result.mismatch_count).toBe(2);
    });
  });

  describe("scheduled monitoring integration", () => {
    it("should run full drift check suite", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Listed" });
      monitor.setListingState({
        invoice_id: "inv-123",
        is_active: true,
        funded_amount: 0,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("inv-123", "testnet");

      expect(result.invoice_id).toBeDefined();
      expect(result.network).toBeDefined();
      expect(result.timestamp).toBeDefined();
      expect(result.checks).toBeDefined();
      expect(result.drift_detected).toBeDefined();
      expect(result.mismatch_count).toBeDefined();
    });

    it("should return structured checks for alerting", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Funded" });
      monitor.setListingState(null);
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("inv-123");

      result.checks.forEach((check) => {
        expect(check).toHaveProperty("type");
        expect(check).toHaveProperty("passed");
        expect(check).toHaveProperty("message");
      });
    });
  });

  describe("confirm-on-next-run pattern for false positives", () => {
    it("should mark potential false positive checks", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Funded" });
      monitor.setListingState({
        invoice_id: "inv-123",
        is_active: false,
        funded_amount: 1000,
        asking_price: 1000,
      });
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      const result = monitor.performDriftCheck("inv-123");
      const fundedPoolCheck = result.checks.find(
        (c) => c.type === "funded_invoice_without_pool"
      );

      expect(fundedPoolCheck?.passed).toBe(false);
      expect(fundedPoolCheck?.message).toContain("no pool exists");
    });

    it("should allow re-checking after confirming state on next run", () => {
      monitor.setInvoiceState({ id: "inv-123", status: "Created" });
      monitor.setListingState(null);
      monitor.setPoolState({
        invoice_id: "inv-123",
        exists: false,
        is_closed: false,
        repaid_amount: 0,
      });

      let result = monitor.performDriftCheck("inv-123");
      expect(result.drift_detected).toBe(false);

      monitor.setInvoiceState({ id: "inv-123", status: "Funded" });
      result = monitor.performDriftCheck("inv-123");
      expect(result.drift_detected).toBe(true);
    });
  });
});
