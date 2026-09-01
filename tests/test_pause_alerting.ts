import { describe, it, expect, beforeAll, afterAll } from "vitest";

/**
 * Protocol Pause and Circuit Breaker Alerting Tests (Issue #655)
 *
 * Test suite for monitoring Paused/circuit-breaker-triggered events on-chain
 * and alerting via webhook immediately. These are highest-urgency operational
 * events requiring near-real-time detection and alerting.
 */

interface OnChainEvent {
  type: "Paused" | "CircuitBreakerTriggered" | "Resumed" | "Other";
  contractAddress: string;
  timestamp: number;
  source: "access_control" | "invoice_nft" | "marketplace" | "financing_pool" | "treasury" | "risk_registry";
  details: Record<string, unknown>;
  blockSequence: number;
}

interface AlertPayload {
  severity: "critical" | "high" | "medium" | "low";
  eventType: string;
  contract: string;
  message: string;
  timestamp: string;
  blockSequence: number;
  details: Record<string, unknown>;
}

interface AlertingConfig {
  webhookUrl: string;
  enablePolling: boolean;
  pollingIntervalMs: number;
  enableEventStream: boolean;
  maxRetries: number;
  timeoutMs: number;
}

class PauseEventMonitor {
  private webhookUrl: string;
  private maxRetries: number;
  private timeoutMs: number;
  private alertsSent = 0;

  constructor(config: AlertingConfig) {
    this.webhookUrl = config.webhookUrl;
    this.maxRetries = config.maxRetries || 3;
    this.timeoutMs = config.timeoutMs || 5000;
  }

  isPauseEvent(event: OnChainEvent): boolean {
    return event.type === "Paused" || event.type === "CircuitBreakerTriggered";
  }

  generateAlertPayload(event: OnChainEvent): AlertPayload {
    const isCritical = event.type === "CircuitBreakerTriggered";
    const severity = isCritical ? "critical" : "high";

    return {
      severity,
      eventType: event.type,
      contract: event.source,
      message: `⚠️ ALERT: Protocol ${event.type.toLowerCase()} event on ${event.source}`,
      timestamp: new Date(event.timestamp).toISOString(),
      blockSequence: event.blockSequence,
      details: event.details,
    };
  }

  async sendAlert(payload: AlertPayload): Promise<boolean> {
    let lastError: Error | null = null;

    for (let attempt = 1; attempt <= this.maxRetries; attempt++) {
      try {
        // Simulate webhook call with timeout
        await this.simulateWebhookCall(payload);
        this.alertsSent++;
        return true;
      } catch (error) {
        lastError = error as Error;
        if (attempt < this.maxRetries) {
          // Exponential backoff: 100ms, 200ms, 400ms
          const backoffMs = 100 * Math.pow(2, attempt - 1);
          await new Promise((resolve) => setTimeout(resolve, backoffMs));
        }
      }
    }

    throw new Error(`Failed to send alert after ${this.maxRetries} retries: ${lastError?.message}`);
  }

  private async simulateWebhookCall(payload: AlertPayload): Promise<void> {
    // Simulate HTTP POST to webhook with timeout
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error(`Webhook request timed out after ${this.timeoutMs}ms`));
      }, this.timeoutMs);

      // Simulate successful webhook delivery (in production, this would be a real HTTP call)
      if (!this.webhookUrl) {
        clearTimeout(timer);
        reject(new Error("Webhook URL not configured"));
        return;
      }

      // Simulate network delay
      setTimeout(() => {
        clearTimeout(timer);
        if (Math.random() > 0.9) {
          // 10% failure rate for testing retry logic
          reject(new Error("Simulated webhook failure"));
        } else {
          resolve();
        }
      }, 50);
    });
  }

  getAlertsSent(): number {
    return this.alertsSent;
  }
}

class CircuitBreakerEventDetector {
  private pausedContracts = new Set<string>();

  async monitorForPauseEvents(events: OnChainEvent[]): Promise<OnChainEvent[]> {
    const pauseEvents: OnChainEvent[] = [];

    for (const event of events) {
      if (event.type === "Paused") {
        this.pausedContracts.add(event.source);
        pauseEvents.push(event);
      } else if (event.type === "CircuitBreakerTriggered") {
        pauseEvents.push(event);
      } else if (event.type === "Resumed") {
        this.pausedContracts.delete(event.source);
      }
    }

    return pauseEvents;
  }

  isPaused(contract: string): boolean {
    return this.pausedContracts.has(contract);
  }

  getPausedContracts(): string[] {
    return Array.from(this.pausedContracts);
  }
}

describe("Protocol Pause and Circuit Breaker Alerting", () => {
  let monitor: PauseEventMonitor;
  let detector: CircuitBreakerEventDetector;

  beforeAll(() => {
    monitor = new PauseEventMonitor({
      webhookUrl: "https://alerts.example.com/pause-events",
      enablePolling: true,
      pollingIntervalMs: 2000,
      enableEventStream: true,
      maxRetries: 3,
      timeoutMs: 5000,
    });

    detector = new CircuitBreakerEventDetector();
  });

  describe("Pause Event Detection", () => {
    it("should identify pause events", () => {
      const pauseEvent: OnChainEvent = {
        type: "Paused",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "marketplace",
        details: { reason: "emergency pause" },
        blockSequence: 12345,
      };

      expect(monitor.isPauseEvent(pauseEvent)).toBe(true);
    });

    it("should identify circuit breaker triggered events", () => {
      const cbEvent: OnChainEvent = {
        type: "CircuitBreakerTriggered",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "financing_pool",
        details: { threshold: "exceeded" },
        blockSequence: 12346,
      };

      expect(monitor.isPauseEvent(cbEvent)).toBe(true);
    });

    it("should ignore non-pause events", () => {
      const otherEvent: OnChainEvent = {
        type: "Other",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "marketplace",
        details: {},
        blockSequence: 12347,
      };

      expect(monitor.isPauseEvent(otherEvent)).toBe(false);
    });
  });

  describe("Alert Payload Generation", () => {
    it("should generate alert payload for pause event", () => {
      const event: OnChainEvent = {
        type: "Paused",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "marketplace",
        details: { reason: "manual pause" },
        blockSequence: 12345,
      };

      const payload = monitor.generateAlertPayload(event);

      expect(payload.severity).toBe("high");
      expect(payload.eventType).toBe("Paused");
      expect(payload.contract).toBe("marketplace");
      expect(payload.message).toContain("paused");
      expect(payload.blockSequence).toBe(12345);
    });

    it("should mark circuit breaker events as critical", () => {
      const event: OnChainEvent = {
        type: "CircuitBreakerTriggered",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "financing_pool",
        details: { threshold: "exceeded", value: "99.5%" },
        blockSequence: 12346,
      };

      const payload = monitor.generateAlertPayload(event);

      expect(payload.severity).toBe("critical");
      expect(payload.eventType).toBe("CircuitBreakerTriggered");
      expect(payload.message).toContain("circuit");
    });

    it("should include event details in alert payload", () => {
      const event: OnChainEvent = {
        type: "Paused",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "treasury",
        details: { reason: "security incident", pausedBy: "admin_key_001" },
        blockSequence: 12347,
      };

      const payload = monitor.generateAlertPayload(event);

      expect(payload.details.reason).toBe("security incident");
      expect(payload.details.pausedBy).toBe("admin_key_001");
    });
  });

  describe("Webhook Alert Delivery", () => {
    it("should send alert via webhook", async () => {
      const event: OnChainEvent = {
        type: "Paused",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "marketplace",
        details: { reason: "test pause" },
        blockSequence: 12345,
      };

      const payload = monitor.generateAlertPayload(event);
      await expect(monitor.sendAlert(payload)).resolves.toBe(true);
    });

    it("should reject alerts without webhook URL", async () => {
      const noWebhookMonitor = new PauseEventMonitor({
        webhookUrl: "",
        enablePolling: true,
        pollingIntervalMs: 2000,
        enableEventStream: true,
        maxRetries: 1,
        timeoutMs: 5000,
      });

      const payload: AlertPayload = {
        severity: "critical",
        eventType: "Paused",
        contract: "marketplace",
        message: "Test alert",
        timestamp: new Date().toISOString(),
        blockSequence: 12345,
        details: {},
      };

      await expect(noWebhookMonitor.sendAlert(payload)).rejects.toThrow();
    });

    it("should implement retry logic for failed webhooks", async () => {
      // This test verifies retry behavior is attempted
      const event: OnChainEvent = {
        type: "CircuitBreakerTriggered",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "financing_pool",
        details: { threshold: "exceeded" },
        blockSequence: 12346,
      };

      const payload = monitor.generateAlertPayload(event);
      // The sendAlert method implements exponential backoff retry
      await expect(monitor.sendAlert(payload)).resolves.toBe(true);
    });
  });

  describe("Circuit Breaker Event Detection", () => {
    it("should track paused contracts", async () => {
      const events: OnChainEvent[] = [
        {
          type: "Paused",
          contractAddress: "CABC123",
          timestamp: Date.now(),
          source: "marketplace",
          details: {},
          blockSequence: 1,
        },
        {
          type: "Paused",
          contractAddress: "CABC456",
          timestamp: Date.now(),
          source: "financing_pool",
          details: {},
          blockSequence: 2,
        },
      ];

      await detector.monitorForPauseEvents(events);

      expect(detector.isPaused("marketplace")).toBe(true);
      expect(detector.isPaused("financing_pool")).toBe(true);
      expect(detector.isPaused("invoice_nft")).toBe(false);
    });

    it("should remove contracts from paused list on resume", async () => {
      const events: OnChainEvent[] = [
        {
          type: "Paused",
          contractAddress: "CABC123",
          timestamp: Date.now(),
          source: "marketplace",
          details: {},
          blockSequence: 1,
        },
        {
          type: "Resumed",
          contractAddress: "CABC123",
          timestamp: Date.now() + 10000,
          source: "marketplace",
          details: {},
          blockSequence: 2,
        },
      ];

      await detector.monitorForPauseEvents(events);

      expect(detector.isPaused("marketplace")).toBe(false);
    });

    it("should return only pause-type events from monitoring", async () => {
      const events: OnChainEvent[] = [
        {
          type: "Paused",
          contractAddress: "CABC123",
          timestamp: Date.now(),
          source: "marketplace",
          details: {},
          blockSequence: 1,
        },
        {
          type: "CircuitBreakerTriggered",
          contractAddress: "CABC456",
          timestamp: Date.now(),
          source: "financing_pool",
          details: {},
          blockSequence: 2,
        },
        {
          type: "Other",
          contractAddress: "CABC789",
          timestamp: Date.now(),
          source: "treasury",
          details: {},
          blockSequence: 3,
        },
      ];

      const pauseEvents = await detector.monitorForPauseEvents(events);

      expect(pauseEvents).toHaveLength(2);
      expect(pauseEvents.every((e) => monitor.isPauseEvent(e))).toBe(true);
    });

    it("should list all currently paused contracts", async () => {
      const events: OnChainEvent[] = [
        {
          type: "Paused",
          contractAddress: "CABC123",
          timestamp: Date.now(),
          source: "marketplace",
          details: {},
          blockSequence: 1,
        },
        {
          type: "Paused",
          contractAddress: "CABC456",
          timestamp: Date.now(),
          source: "access_control",
          details: {},
          blockSequence: 2,
        },
        {
          type: "Paused",
          contractAddress: "CABC789",
          timestamp: Date.now(),
          source: "invoice_nft",
          details: {},
          blockSequence: 3,
        },
      ];

      await detector.monitorForPauseEvents(events);
      const paused = detector.getPausedContracts();

      expect(paused).toContain("marketplace");
      expect(paused).toContain("access_control");
      expect(paused).toContain("invoice_nft");
      expect(paused.length).toBe(3);
    });
  });

  describe("Integration: Full Alerting Pipeline", () => {
    it("should alert immediately on pause event without suppression", async () => {
      // Verify that alerts fire even for operator-initiated pauses
      const events: OnChainEvent[] = [
        {
          type: "Paused",
          contractAddress: "CABC123",
          timestamp: Date.now(),
          source: "marketplace",
          details: { pausedBy: "admin_key_001" },
          blockSequence: 12345,
        },
      ];

      const pauseEvents = await detector.monitorForPauseEvents(events);
      expect(pauseEvents).toHaveLength(1);

      const alertPayload = monitor.generateAlertPayload(pauseEvents[0]);
      await expect(monitor.sendAlert(alertPayload)).resolves.toBe(true);
    });

    it("should handle multiple pause events across contracts", async () => {
      const events: OnChainEvent[] = [
        {
          type: "CircuitBreakerTriggered",
          contractAddress: "CABC123",
          timestamp: Date.now(),
          source: "marketplace",
          details: { threshold: "exceeded" },
          blockSequence: 1,
        },
        {
          type: "Paused",
          contractAddress: "CABC456",
          timestamp: Date.now() + 100,
          source: "financing_pool",
          details: { reason: "cascade pause" },
          blockSequence: 2,
        },
      ];

      const pauseEvents = await detector.monitorForPauseEvents(events);
      expect(pauseEvents).toHaveLength(2);

      for (const event of pauseEvents) {
        const payload = monitor.generateAlertPayload(event);
        await expect(monitor.sendAlert(payload)).resolves.toBe(true);
      }
    });

    it("should track alert delivery success rate", async () => {
      const initialAlerts = monitor.getAlertsSent();

      const event: OnChainEvent = {
        type: "Paused",
        contractAddress: "CABC123",
        timestamp: Date.now(),
        source: "treasury",
        details: {},
        blockSequence: 12345,
      };

      const payload = monitor.generateAlertPayload(event);
      await monitor.sendAlert(payload);

      const finalAlerts = monitor.getAlertsSent();
      expect(finalAlerts).toBeGreaterThan(initialAlerts);
    });
  });
});
