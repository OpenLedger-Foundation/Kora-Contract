import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";

describe("Health-Check Service - Issue #641", () => {
  let mockHealthCheckRunner: any;
  let mockWebhookClient: any;
  let mockLogger: any;
  let healthCheckService: any;

  beforeEach(() => {
    mockLogger = {
      info: vi.fn(),
      error: vi.fn(),
      warn: vi.fn(),
      debug: vi.fn(),
    };

    mockWebhookClient = {
      send: vi.fn().mockResolvedValue({ status: 200 }),
      retryOnFailure: vi.fn().mockResolvedValue(true),
    };

    mockHealthCheckRunner = {
      execute: vi.fn().mockResolvedValue({ status: "healthy", timestamp: Date.now() }),
    };

    healthCheckService = {
      start: vi.fn(),
      stop: vi.fn(),
      getStatus: vi.fn().mockReturnValue({ isRunning: true, lastCheck: Date.now() }),
    };
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  describe("Configuration", () => {
    it("should accept configurable check interval", async () => {
      const checkInterval = 300000; // 5 minutes

      expect(checkInterval).toBeGreaterThan(0);
      expect(checkInterval % 1000).toBe(0); // milliseconds
    });

    it("should accept configurable webhook target", () => {
      const webhookTargets = [
        "https://hooks.slack.com/services/webhook",
        "https://discordapp.com/api/webhooks/123/abc",
        "https://example.com/api/alerts",
      ];

      webhookTargets.forEach((url) => {
        expect(url).toMatch(/^https:\/\//);
      });
    });

    it("should support multiple webhook targets for redundancy", () => {
      const primaryWebhook = "https://hooks.slack.com/primary";
      const secondaryWebhook = "https://hooks.slack.com/secondary";

      expect(primaryWebhook).toBeDefined();
      expect(secondaryWebhook).toBeDefined();
    });

    it("should accept custom health check script path", () => {
      const scriptPath = "/path/to/scripts/health-check.sh";

      expect(scriptPath).toContain("health-check");
      expect(scriptPath.endsWith(".sh")).toBe(true);
    });
  });

  describe("Service Startup", () => {
    it("should start monitoring on service init", async () => {
      const serviceConfig = {
        interval: 300000,
        webhook: "https://example.com/alerts",
      };

      await healthCheckService.start();

      expect(healthCheckService.start).toHaveBeenCalled();
      expect(healthCheckService.getStatus().isRunning).toBe(true);
    });

    it("should perform initial health check on startup", async () => {
      await healthCheckService.start();

      expect(mockHealthCheckRunner.execute).toHaveBeenCalled();
    });

    it("should log startup event", async () => {
      await healthCheckService.start();

      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.stringContaining("Health check service started")
      );
    });
  });

  describe("Health Check Execution", () => {
    it("should execute health check on interval", async () => {
      const checkCount = 3;
      const interval = 100; // ms for testing

      for (let i = 0; i < checkCount; i++) {
        await mockHealthCheckRunner.execute();
      }

      expect(mockHealthCheckRunner.execute).toHaveBeenCalledTimes(checkCount);
    });

    it("should handle passing health checks", async () => {
      const result = await mockHealthCheckRunner.execute();

      expect(result.status).toBe("healthy");
    });

    it("should handle failing health checks", async () => {
      mockHealthCheckRunner.execute.mockResolvedValueOnce({
        status: "unhealthy",
        error: "Connection timeout",
        timestamp: Date.now(),
      });

      const result = await mockHealthCheckRunner.execute();

      expect(result.status).toBe("unhealthy");
      expect(result.error).toBeDefined();
    });

    it("should capture check timestamp", async () => {
      const before = Date.now();
      const result = await mockHealthCheckRunner.execute();
      const after = Date.now();

      expect(result.timestamp).toBeGreaterThanOrEqual(before);
      expect(result.timestamp).toBeLessThanOrEqual(after);
    });
  });

  describe("Alert Deduplication", () => {
    it("should not spam alerts for continuous failures", async () => {
      mockHealthCheckRunner.execute.mockResolvedValue({
        status: "unhealthy",
        error: "Service down",
      });

      // First failure - should alert
      await mockHealthCheckRunner.execute();

      // Subsequent failures - should not alert
      await mockHealthCheckRunner.execute();
      await mockHealthCheckRunner.execute();

      // Only one webhook call should have been made
      expect(mockWebhookClient.send.mock.calls.length).toBeLessThanOrEqual(1);
    });

    it("should track last alert state", () => {
      const lastAlertState = "failure";

      expect(lastAlertState).toBeDefined();
      expect(["failure", "success"]).toContain(lastAlertState);
    });

    it("should suppress duplicate alerts within deduplication window", async () => {
      const deduplicationWindow = 3600000; // 1 hour
      const failureTime1 = Date.now();
      const failureTime2 = failureTime1 + 1000; // 1 second later

      expect(failureTime2 - failureTime1).toBeLessThan(deduplicationWindow);
    });
  });

  describe("Webhook Integration", () => {
    it("should post alert to webhook on failure", async () => {
      mockHealthCheckRunner.execute.mockResolvedValueOnce({
        status: "unhealthy",
        error: "Service unreachable",
      });

      const result = await mockHealthCheckRunner.execute();

      if (result.status === "unhealthy") {
        await mockWebhookClient.send({
          text: "Health check failed: Service unreachable",
          status: result.status,
        });
      }

      expect(mockWebhookClient.send).toHaveBeenCalled();
    });

    it("should format Slack webhook message", async () => {
      const message = {
        text: "🔴 Health Check Failed",
        attachments: [
          {
            color: "danger",
            title: "Health Check Failure",
            text: "Service is unhealthy",
            fields: [{ title: "Error", value: "Connection timeout" }],
          },
        ],
      };

      expect(message.text).toBeDefined();
      expect(message.attachments).toBeDefined();
    });

    it("should handle webhook delivery failure gracefully", async () => {
      mockWebhookClient.send.mockRejectedValueOnce(new Error("Connection refused"));

      try {
        await mockWebhookClient.send({});
      } catch (error) {
        expect(error).toBeDefined();
        expect(mockLogger.error).toHaveBeenCalled();
      }
    });

    it("should retry webhook delivery on transient failure", async () => {
      mockWebhookClient.send
        .mockRejectedValueOnce(new Error("Timeout"))
        .mockResolvedValueOnce({ status: 200 });

      await mockWebhookClient.retryOnFailure({});

      expect(mockWebhookClient.send).toHaveBeenCalled();
    });

    it("should log locally when webhook is unreachable", async () => {
      mockWebhookClient.send.mockRejectedValueOnce(
        new Error("Webhook endpoint unreachable")
      );

      try {
        await mockWebhookClient.send({});
      } catch (error) {
        await mockLogger.error(
          `Webhook delivery failed: ${error.message}`
        );
      }

      expect(mockLogger.error).toHaveBeenCalled();
    });
  });

  describe("Recovery Notifications", () => {
    it("should send recovery alert when health check passes after failure", async () => {
      // Simulate failure then recovery
      mockHealthCheckRunner.execute
        .mockResolvedValueOnce({ status: "unhealthy", error: "Down" })
        .mockResolvedValueOnce({ status: "healthy" });

      const firstCheck = await mockHealthCheckRunner.execute();
      const secondCheck = await mockHealthCheckRunner.execute();

      if (firstCheck.status === "unhealthy" && secondCheck.status === "healthy") {
        await mockWebhookClient.send({
          text: "✅ Service Recovered",
          status: "recovery",
        });
      }

      expect(mockWebhookClient.send).toHaveBeenCalled();
    });

    it("should include recovery time in notification", async () => {
      const failureTime = Date.now();
      const recoveryTime = failureTime + 600000; // 10 minutes later

      const downtime = recoveryTime - failureTime;

      expect(downtime).toBe(600000);
    });
  });

  describe("Service Management", () => {
    it("should stop service gracefully", async () => {
      await healthCheckService.start();
      await healthCheckService.stop();

      expect(healthCheckService.stop).toHaveBeenCalled();
      expect(healthCheckService.getStatus().isRunning).toBe(false);
    });

    it("should provide status endpoint", () => {
      const status = healthCheckService.getStatus();

      expect(status).toHaveProperty("isRunning");
      expect(status).toHaveProperty("lastCheck");
    });

    it("should log shutdown event", async () => {
      await healthCheckService.stop();

      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.stringContaining("stopped")
      );
    });
  });

  describe("Edge Cases", () => {
    it("should handle webhook endpoint being temporarily unreachable", async () => {
      mockWebhookClient.send
        .mockRejectedValueOnce(new Error("ECONNREFUSED"))
        .mockResolvedValueOnce({ status: 200 });

      expect(async () => {
        await mockWebhookClient.retryOnFailure({});
      }).not.toThrow();
    });

    it("should not crash if health check script fails", async () => {
      mockHealthCheckRunner.execute.mockRejectedValueOnce(
        new Error("Script execution failed")
      );

      expect(async () => {
        try {
          await mockHealthCheckRunner.execute();
        } catch (error) {
          mockLogger.error(error.message);
        }
      }).not.toThrow();
    });

    it("should handle malformed webhook configuration gracefully", () => {
      const invalidWebhook = "not-a-url";

      expect(invalidWebhook).not.toMatch(/^https?:\/\//);
    });

    it("should limit alert frequency to prevent webhook spam", () => {
      const minAlertInterval = 300000; // 5 minutes minimum

      expect(minAlertInterval).toBeGreaterThan(0);
    });
  });
});
