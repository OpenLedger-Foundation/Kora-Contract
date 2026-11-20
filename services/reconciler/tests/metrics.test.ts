/**
 * Unit tests for the Prometheus metrics renderer.
 *
 * Validates format, label correctness, and zero-value behaviour.
 */

import { renderMetrics } from "../src/metrics";

describe("renderMetrics", () => {
  it("emits all four metric families", () => {
    const output = renderMetrics({
      gapsDetectedTotal: 0,
      alertsDeliveredTotal: 0,
      pendingAlerts: 0,
      oldestGapAgeSeconds: 0,
    });
    expect(output).toContain("kora_reconciler_gaps_detected_total");
    expect(output).toContain("kora_reconciler_alerts_delivered_total");
    expect(output).toContain("kora_reconciler_pending_alerts");
    expect(output).toContain("kora_reconciler_oldest_gap_age_seconds");
  });

  it("includes HELP and TYPE lines for every metric", () => {
    const output = renderMetrics({
      gapsDetectedTotal: 1,
      alertsDeliveredTotal: 1,
      pendingAlerts: 1,
      oldestGapAgeSeconds: 60,
    });
    const helpCount = (output.match(/^# HELP /gm) ?? []).length;
    const typeCount = (output.match(/^# TYPE /gm) ?? []).length;
    expect(helpCount).toBe(4);
    expect(typeCount).toBe(4);
  });

  it("renders correct numeric values", () => {
    const output = renderMetrics({
      gapsDetectedTotal: 42,
      alertsDeliveredTotal: 37,
      pendingAlerts: 5,
      oldestGapAgeSeconds: 300,
    });
    expect(output).toContain("kora_reconciler_gaps_detected_total 42");
    expect(output).toContain("kora_reconciler_alerts_delivered_total 37");
    expect(output).toContain("kora_reconciler_pending_alerts 5");
    expect(output).toContain("kora_reconciler_oldest_gap_age_seconds 300");
  });

  it("types are counter / counter / gauge / gauge in that order", () => {
    const output = renderMetrics({
      gapsDetectedTotal: 0,
      alertsDeliveredTotal: 0,
      pendingAlerts: 0,
      oldestGapAgeSeconds: 0,
    });
    const typeLines = output
      .split("\n")
      .filter((l) => l.startsWith("# TYPE "));
    expect(typeLines[0]).toContain("counter");
    expect(typeLines[1]).toContain("counter");
    expect(typeLines[2]).toContain("gauge");
    expect(typeLines[3]).toContain("gauge");
  });

  it("zero values are emitted without omitting metric lines", () => {
    const output = renderMetrics({
      gapsDetectedTotal: 0,
      alertsDeliveredTotal: 0,
      pendingAlerts: 0,
      oldestGapAgeSeconds: 0,
    });
    expect(output).toContain("kora_reconciler_gaps_detected_total 0");
    expect(output).toContain("kora_reconciler_oldest_gap_age_seconds 0");
  });

  it("ends with a trailing newline (Prometheus convention)", () => {
    const output = renderMetrics({
      gapsDetectedTotal: 1,
      alertsDeliveredTotal: 1,
      pendingAlerts: 0,
      oldestGapAgeSeconds: 0,
    });
    expect(output.endsWith("\n")).toBe(true);
  });
});
