/**
 * Yield projection model (issue #786).
 *
 * Pure functions only — no React, no I/O — so the assumptions can be reviewed
 * on their own. See docs/yield-projection-model.md for the full method.
 *
 * Summary of the method, per risk tier with allocation share a > 0:
 *
 *   r   = realized annual yield × tenor / 365   (yield per invoice cycle)
 *   d   = historical default rate per invoice   (share of matured invoices that defaulted)
 *   LGD = loss given default                    (share of principal lost on default)
 *   c   = horizon / tenor                       (number of cycles in the horizon)
 *
 *   reinvest on (principal and yield roll into new invoices):
 *     n = r − d × (r + LGD)                      expected net return per cycle per $1
 *     return = (1 + n)^c − 1
 *   reinvest off (principal rolls, yield is paid out):
 *     s = 1 − d × LGD                            principal kept per cycle
 *     return = (1 − d) × r × (1 − s^c) / (1 − s) + s^c − 1     (r × c + 0 when s = 1)
 *
 * Neither formula can lose more than the principal.
 *
 * The range uses the 90% Wilson score interval of the default rate and the
 * P10/P90 realized yield (when the analytics service provides them). Tiers with
 * fewer matured invoices than MIN_SAMPLE_LOW get no projection at all.
 *
 * Outputs are historically grounded estimates, never guarantees.
 */

export const RISK_TIERS = ['AAA', 'AA', 'A', 'B', 'C'] as const;
export type ProjectionTier = (typeof RISK_TIERS)[number];

export const TENOR_OPTIONS_DAYS = [30, 60, 90, 180] as const;

/** Sample-size thresholds (matured invoices) for confidence levels. */
export const MIN_SAMPLE_HIGH = 50;
export const MIN_SAMPLE_MEDIUM = 20;
export const MIN_SAMPLE_LOW = 5;

/** z for a two-sided 90% interval. */
const Z_90 = 1.645;

export type Confidence = 'high' | 'medium' | 'low' | 'insufficient';

/** Historical metrics for one tier (optionally one tenor bucket), from services/analytics. */
export interface TierHistory {
  tier: ProjectionTier;
  /** Tenor bucket in days; omit for all-tenor aggregates. */
  tenorDays?: number;
  /** Median realized annual investor yield, net of protocol fees, in bps. */
  realizedYieldBps: number;
  /** 10th / 90th percentile realized yield, bps (optional). */
  yieldP10Bps?: number;
  yieldP90Bps?: number;
  /** Matured invoices in the sample. */
  sampleSize: number;
  /** Matured invoices that defaulted. */
  defaults: number;
  /** Average share of principal lost on a default, bps (optional; 10_000 assumed). */
  lossGivenDefaultBps?: number;
}

export interface HistoricalMetrics {
  /** ISO date the metrics were computed. */
  asOf: string;
  /** Human-readable origin, e.g. "services/analytics". */
  source: string;
  /** Observation window covered by the data. */
  periodStart?: string;
  periodEnd?: string;
  tiers: TierHistory[];
}

export interface ProjectionInputs {
  principalUsd: number;
  /** Percent per tier, 0–100, summing to 100. Missing tiers count as 0. */
  allocations: Partial<Record<ProjectionTier, number>>;
  tenorDays: number;
  reinvest: boolean;
  /** Projection horizon in days (default 365). */
  horizonDays?: number;
}

export interface Assumption {
  id: string;
  label: string;
  value: string;
  /** Where the value comes from (data source or model default). */
  basis: string;
}

export interface Range {
  low: number;
  expected: number | null;
  high: number;
}

export interface TierProjection {
  tier: ProjectionTier;
  allocationPct: number;
  principalUsd: number;
  confidence: Confidence;
  sampleSize: number;
  /** Return as a fraction of this tier's principal over the horizon. */
  returnPct: Range | null;
  returnUsd: Range | null;
  usedTenorSpecificData: boolean;
}

export interface ProjectionResult {
  valid: boolean;
  horizonDays: number;
  /** Input validation problems; projection fields are null when present. */
  errors: string[];
  confidence: Confidence;
  /** Tiers with allocation > 0 and too little history. */
  lowConfidenceTiers: ProjectionTier[];
  /** Portfolio return over the horizon (fraction of principal); null if any allocated tier is insufficient. */
  returnPct: Range | null;
  returnUsd: Range | null;
  /** Annualized equivalent of `returnPct`. */
  annualizedPct: Range | null;
  tiers: TierProjection[];
  assumptions: Assumption[];
  warnings: string[];
}

export function confidenceFor(sampleSize: number): Confidence {
  if (sampleSize >= MIN_SAMPLE_HIGH) return 'high';
  if (sampleSize >= MIN_SAMPLE_MEDIUM) return 'medium';
  if (sampleSize >= MIN_SAMPLE_LOW) return 'low';
  return 'insufficient';
}

const RANK: Record<Confidence, number> = { high: 3, medium: 2, low: 1, insufficient: 0 };

function worst(a: Confidence, b: Confidence): Confidence {
  return RANK[a] <= RANK[b] ? a : b;
}

/** 90% Wilson score interval for a proportion; [0, 1] when n = 0. */
export function wilsonInterval(successes: number, n: number, z = Z_90): { low: number; high: number } {
  if (n <= 0) return { low: 0, high: 1 };
  const p = successes / n;
  const z2 = z * z;
  const denom = 1 + z2 / n;
  const centre = (p + z2 / (2 * n)) / denom;
  const margin = (z * Math.sqrt((p * (1 - p)) / n + z2 / (4 * n * n))) / denom;
  return { low: Math.max(0, centre - margin), high: Math.min(1, centre + margin) };
}

/** Per-cycle terms for one tier (see file header). */
export interface CycleTerms {
  /** Yield per cycle. */
  r: number;
  /** Default rate per invoice. */
  d: number;
  /** Loss given default. */
  lgd: number;
}

export function cycleTerms(annualYield: number, tenorDays: number, defaultRate: number, lgd: number): CycleTerms {
  return { r: annualYield * (tenorDays / 365), d: defaultRate, lgd };
}

/** Expected net return per cycle per $1 when everything is reinvested. */
export function netReturnPerCycle({ r, d, lgd }: CycleTerms): number {
  return r - d * (r + lgd);
}

/** Return over the horizon, as a fraction of principal (≥ −1). */
export function horizonReturn(terms: CycleTerms, cycles: number, reinvest: boolean): number {
  if (reinvest) {
    return Math.pow(Math.max(0, 1 + netReturnPerCycle(terms)), cycles) - 1;
  }
  const s = 1 - terms.d * terms.lgd;
  const paidOutPerUnit = (1 - terms.d) * terms.r;
  const sumOfPrincipal = s === 1 ? cycles : (1 - Math.pow(s, cycles)) / (1 - s);
  return paidOutPerUnit * sumOfPrincipal + Math.pow(s, cycles) - 1;
}

export function annualize(horizonReturnPct: number, horizonDays: number): number {
  if (horizonDays === 365) return horizonReturnPct;
  return Math.pow(Math.max(0, 1 + horizonReturnPct), 365 / horizonDays) - 1;
}

function pct(n: number, digits = 2): string {
  return `${(n * 100).toFixed(digits)}%`;
}

export function validateInputs(inputs: ProjectionInputs): string[] {
  const errors: string[] = [];
  if (!Number.isFinite(inputs.principalUsd) || inputs.principalUsd <= 0) {
    errors.push('Enter an amount greater than zero.');
  }
  let sum = 0;
  for (const tier of RISK_TIERS) {
    const a = inputs.allocations[tier] ?? 0;
    if (!Number.isFinite(a) || a < 0 || a > 100) errors.push(`Allocation for ${tier} must be between 0% and 100%.`);
    else sum += a;
  }
  if (Math.abs(sum - 100) > 0.01) errors.push(`Allocations must add up to 100% (currently ${Math.round(sum * 100) / 100}%).`);
  if (!Number.isFinite(inputs.tenorDays) || inputs.tenorDays <= 0) errors.push('Tenor must be a positive number of days.');
  const horizon = inputs.horizonDays ?? 365;
  if (!Number.isFinite(horizon) || horizon <= 0) errors.push('Horizon must be a positive number of days.');
  return errors;
}

/** Picks tenor-specific history when present, otherwise the all-tenor aggregate. */
export function historyFor(
  metrics: HistoricalMetrics,
  tier: ProjectionTier,
  tenorDays: number,
): { history: TierHistory | null; tenorSpecific: boolean } {
  const specific = metrics.tiers.find((t) => t.tier === tier && t.tenorDays === tenorDays);
  if (specific) return { history: specific, tenorSpecific: true };
  const aggregate = metrics.tiers.find((t) => t.tier === tier && t.tenorDays === undefined);
  return { history: aggregate ?? null, tenorSpecific: false };
}

function scale(range: Range, factor: number): Range {
  return {
    low: range.low * factor,
    expected: range.expected === null ? null : range.expected * factor,
    high: range.high * factor,
  };
}

/**
 * Projects portfolio returns. Never throws: invalid inputs yield
 * `valid: false` with `errors`.
 */
export function projectReturns(metrics: HistoricalMetrics, inputs: ProjectionInputs): ProjectionResult {
  const horizonDays = inputs.horizonDays ?? 365;
  const errors = validateInputs(inputs);
  const assumptions: Assumption[] = [
    {
      id: 'data-source',
      label: 'Historical data',
      value: `${metrics.source}, as of ${metrics.asOf}`
        + (metrics.periodStart && metrics.periodEnd ? ` (${metrics.periodStart} to ${metrics.periodEnd})` : ''),
      basis: 'Analytics service',
    },
    {
      id: 'method',
      label: 'Method',
      value: 'Per cycle: yield × tenor/365, minus yield and principal lost on the historical share of defaulted invoices',
      basis: 'Model (docs/yield-projection-model.md)',
    },
    {
      id: 'reinvest',
      label: 'Reinvestment',
      value: inputs.reinvest
        ? 'On: principal and yield roll into new invoices each cycle (compounding)'
        : 'Off: principal rolls into new invoices, yield is paid out (no compounding)',
      basis: 'Your input',
    },
    { id: 'horizon', label: 'Horizon', value: `${horizonDays} days`, basis: 'Model default' },
    {
      id: 'tenor',
      label: 'Invoice tenor',
      value: `${inputs.tenorDays} days (${Math.round((horizonDays / inputs.tenorDays) * 100) / 100} cycles in the horizon)`,
      basis: 'Your input',
    },
    {
      id: 'range',
      label: 'Range',
      value: 'Low/high use the 90% interval of the default rate and P10/P90 realized yield; capital is assumed fully deployed',
      basis: 'Model',
    },
  ];
  const warnings: string[] = [];

  if (errors.length > 0) {
    return {
      valid: false,
      horizonDays,
      errors,
      confidence: 'insufficient',
      lowConfidenceTiers: [],
      returnPct: null,
      returnUsd: null,
      annualizedPct: null,
      tiers: [],
      assumptions,
      warnings,
    };
  }

  const cycles = horizonDays / inputs.tenorDays;
  let overall: Confidence = 'high';
  const lowConfidenceTiers: ProjectionTier[] = [];
  const tiers: TierProjection[] = [];
  let totalLow = 0;
  let totalExpected = 0;
  let totalHigh = 0;
  let projectable = true;

  for (const tier of RISK_TIERS) {
    const allocationPct = inputs.allocations[tier] ?? 0;
    if (allocationPct <= 0) continue;
    const principalUsd = (inputs.principalUsd * allocationPct) / 100;
    const { history, tenorSpecific } = historyFor(metrics, tier, inputs.tenorDays);
    const sampleSize = history?.sampleSize ?? 0;
    const confidence = confidenceFor(sampleSize);
    overall = worst(overall, confidence);
    if (confidence === 'low' || confidence === 'insufficient') lowConfidenceTiers.push(tier);

    if (!history || confidence === 'insufficient') {
      projectable = false;
      tiers.push({
        tier, allocationPct, principalUsd, confidence, sampleSize,
        returnPct: null, returnUsd: null, usedTenorSpecificData: tenorSpecific,
      });
      assumptions.push({
        id: `tier-${tier}`,
        label: `${tier} tier`,
        value: `Only ${sampleSize} matured invoice${sampleSize === 1 ? '' : 's'} on record — no projection`,
        basis: 'Analytics service',
      });
      continue;
    }

    const lgd = (history.lossGivenDefaultBps ?? 10_000) / 10_000;
    const yieldMid = history.realizedYieldBps / 10_000;
    const yieldLow = (history.yieldP10Bps ?? history.realizedYieldBps) / 10_000;
    const yieldHigh = (history.yieldP90Bps ?? history.realizedYieldBps) / 10_000;
    const defaultRate = history.defaults / sampleSize;
    const interval = wilsonInterval(history.defaults, sampleSize);

    const low = horizonReturn(cycleTerms(yieldLow, inputs.tenorDays, interval.high, lgd), cycles, inputs.reinvest);
    const mid = horizonReturn(cycleTerms(yieldMid, inputs.tenorDays, defaultRate, lgd), cycles, inputs.reinvest);
    const high = horizonReturn(cycleTerms(yieldHigh, inputs.tenorDays, interval.low, lgd), cycles, inputs.reinvest);
    // Low-confidence tiers get a range only — no single number.
    const expected = confidence === 'low' ? null : mid;
    const returnPct: Range = { low, expected, high };

    tiers.push({
      tier, allocationPct, principalUsd, confidence, sampleSize,
      returnPct, returnUsd: scale(returnPct, principalUsd), usedTenorSpecificData: tenorSpecific,
    });
    totalLow += low * principalUsd;
    totalExpected += mid * principalUsd;
    totalHigh += high * principalUsd;

    assumptions.push({
      id: `tier-${tier}`,
      label: `${tier} tier`,
      value:
        `Realized yield ${pct(yieldMid)} p.a. (P10 ${pct(yieldLow)}, P90 ${pct(yieldHigh)}); ` +
        `default rate ${pct(defaultRate)} (${history.defaults}/${sampleSize}, 90% range ${pct(interval.low)}–${pct(interval.high)}); ` +
        `loss on default ${pct(lgd, 0)}` +
        (history.lossGivenDefaultBps === undefined ? ' (no recovery assumed)' : ''),
      basis: tenorSpecific
        ? `Analytics service, ${inputs.tenorDays}-day invoices`
        : 'Analytics service, all tenors (no tenor-specific history)',
    });
    if (!tenorSpecific) {
      warnings.push(`${tier}: no ${inputs.tenorDays}-day history, so all-tenor data is used.`);
    }
  }

  if (lowConfidenceTiers.length > 0) {
    warnings.unshift(
      `Low confidence: limited history for ${lowConfidenceTiers.join(', ')}. ` +
        'Treat these figures as rough indications only.',
    );
  }

  if (!projectable) {
    return {
      valid: true, horizonDays, errors: [], confidence: 'insufficient', lowConfidenceTiers,
      returnPct: null, returnUsd: null, annualizedPct: null, tiers, assumptions, warnings,
    };
  }

  const P = inputs.principalUsd;
  const showExpected = overall === 'high' || overall === 'medium';
  const returnPct: Range = { low: totalLow / P, expected: showExpected ? totalExpected / P : null, high: totalHigh / P };
  return {
    valid: true,
    horizonDays,
    errors: [],
    confidence: overall,
    lowConfidenceTiers,
    returnPct,
    returnUsd: scale(returnPct, P),
    annualizedPct: {
      low: annualize(returnPct.low, horizonDays),
      expected: returnPct.expected === null ? null : annualize(returnPct.expected, horizonDays),
      high: annualize(returnPct.high, horizonDays),
    },
    tiers,
    assumptions,
    warnings,
  };
}
