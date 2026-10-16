/**
 * Risk Pipeline — Entry point (internal tool/API surface)
 *
 * Exposes the pipeline for verifier tooling. Verifiers review the
 * `SuggestedScore` (score + confidence + breakdown) and then submit their own
 * judgment to `risk_registry` — the pipeline never submits on-chain itself.
 *
 * Issue #755, #944
 */

export * from "./types";
export * from "./factors";
export * from "./formulas";
export * from "./scorer";
export * from "./metrics";

import { RiskPipeline } from "./scorer";
import { riskPipelineMetrics } from "./metrics";

/** Convenience helper for verifier tooling: score with defaults and record telemetry. */
export function suggestScore(input: import("./types").ScoringInput, formulaVersion?: string) {
  try {
    const result = new RiskPipeline().score(input, formulaVersion);
    riskPipelineMetrics.recordSuccess();
    return result;
  } catch (err) {
    riskPipelineMetrics.recordFailure();
    throw err;
  }
}
