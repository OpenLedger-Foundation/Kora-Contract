/**
 * Prometheus text-format metrics for the reconciler.
 *
 * Exports three metrics:
 *
 *   kora_reconciler_gaps_detected_total — counter; incremented once per
 *     ledger gap entry enqueued.  Monotonically non-decreasing.
 *
 *   kora_reconciler_alerts_delivered_total — counter; incremented once per
 *     successful alert sink acknowledgement.
 *
 *   kora_reconciler_pending_alerts — gauge; current depth of the persistent
 *     alert queue.  Zero means all detected gaps have been acknowledged by
 *     the sink.
 *
 *   kora_reconciler_oldest_gap_age_seconds — gauge; age in seconds of the
 *     oldest undelivered alert (created_at → now).  Zero when queue is empty.
 *     This is the primary SLO signal: if it exceeds the lag budget, an alert
 *     has been stuck for longer than expected.
 *
 * All metric values are passed in at render time so this module has no side
 * effects and is straightforward to test.
 */

export interface ReconcilerMetricsSnapshot {
  gapsDetectedTotal: number;
  alertsDeliveredTotal: number;
  pendingAlerts: number;
  oldestGapAgeSeconds: number;
}

/**
 * Render a Prometheus 0.0.4 text-format exposition from the given snapshot.
 * Always emits HELP/TYPE lines even when counters are zero.
 */
export function renderMetrics(snap: ReconcilerMetricsSnapshot): string {
  const lines: string[] = [];

  lines.push("# HELP kora_reconciler_gaps_detected_total Total ledger gaps detected since startup.");
  lines.push("# TYPE kora_reconciler_gaps_detected_total counter");
  lines.push(`kora_reconciler_gaps_detected_total ${snap.gapsDetectedTotal}`);

  lines.push("# HELP kora_reconciler_alerts_delivered_total Total alerts successfully acknowledged by the sink since startup.");
  lines.push("# TYPE kora_reconciler_alerts_delivered_total counter");
  lines.push(`kora_reconciler_alerts_delivered_total ${snap.alertsDeliveredTotal}`);

  lines.push("# HELP kora_reconciler_pending_alerts Number of undelivered alerts currently in the persistent queue.");
  lines.push("# TYPE kora_reconciler_pending_alerts gauge");
  lines.push(`kora_reconciler_pending_alerts ${snap.pendingAlerts}`);

  lines.push("# HELP kora_reconciler_oldest_gap_age_seconds Age in seconds of the oldest undelivered alert. Zero when queue is empty.");
  lines.push("# TYPE kora_reconciler_oldest_gap_age_seconds gauge");
  lines.push(`kora_reconciler_oldest_gap_age_seconds ${snap.oldestGapAgeSeconds}`);

  lines.push("");
  return lines.join("\n");
}
