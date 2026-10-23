/**
 * Issue #786 — yield calculator and return projection tool.
 */
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import {
  HistoricalMetrics,
  ProjectionInputs,
  annualize,
  confidenceFor,
  cycleTerms,
  historyFor,
  horizonReturn,
  netReturnPerCycle,
  projectReturns,
  validateInputs,
  wilsonInterval,
} from '../src/yieldProjection/model';
import {
  AnalyticsService,
  DEV_SAMPLE_METRICS,
  HISTORICAL_YIELDS_PATH,
  parseHistoricalMetrics,
} from '../src/services/analyticsService';
import {
  DISCLAIMER,
  ProjectionSummary,
  YieldCalculator,
  YieldCalculatorView,
} from '../src/components/yield/YieldCalculator';
import { ErrorPanel } from '../src/components/common/KoraDataState';
import { mapSdkError } from '../src/sdk/errors';

const metrics: HistoricalMetrics = {
  asOf: '2026-09-30',
  source: 'test analytics',
  periodStart: '2025-10-01',
  periodEnd: '2026-09-30',
  tiers: [
    { tier: 'AAA', realizedYieldBps: 800, yieldP10Bps: 700, yieldP90Bps: 900, sampleSize: 200, defaults: 0, lossGivenDefaultBps: 5_000 },
    { tier: 'A', realizedYieldBps: 1_200, sampleSize: 100, defaults: 2 },
    { tier: 'A', tenorDays: 90, realizedYieldBps: 1_300, sampleSize: 60, defaults: 1, lossGivenDefaultBps: 5_000 },
    { tier: 'B', realizedYieldBps: 1_600, sampleSize: 30, defaults: 3 },
    { tier: 'C', realizedYieldBps: 2_200, sampleSize: 8, defaults: 2 },
    { tier: 'AA', realizedYieldBps: 1_000, sampleSize: 2, defaults: 0 },
  ],
};

function inputs(overrides: Partial<ProjectionInputs> = {}): ProjectionInputs {
  return { principalUsd: 10_000, allocations: { AAA: 100 }, tenorDays: 60, reinvest: false, ...overrides };
}

function jsonResponse(status: number, body: unknown): Response {
  return { status, ok: status >= 200 && status < 300, headers: { get: () => null }, json: async () => body } as unknown as Response;
}

describe('projection model maths', () => {
  it('nets defaults out of the per-cycle yield', () => {
    // 12% p.a. over 73 days = 2.4% per cycle; 5% default rate losing 100% → 0.024 - 0.05 × 1.024
    expect(cycleTerms(0.12, 73, 0.05, 1).r).toBeCloseTo(0.024, 10);
    expect(netReturnPerCycle(cycleTerms(0.12, 73, 0.05, 1))).toBeCloseTo(0.024 - 0.0512, 10);
    expect(netReturnPerCycle(cycleTerms(0.1, 365, 0, 1))).toBeCloseTo(0.1);
  });

  it('compounds only when reinvesting', () => {
    const t = { r: 0.02, d: 0, lgd: 1 };
    expect(horizonReturn(t, 6, false)).toBeCloseTo(0.12);
    expect(horizonReturn(t, 6, true)).toBeCloseTo(1.02 ** 6 - 1);
  });

  it('without reinvestment, defaults shrink the principal that earns yield', () => {
    // s = 1 - 0.1 × 0.5 = 0.95; two cycles: paid 0.9×0.02×(1+0.95) + 0.95² − 1
    const t = { r: 0.02, d: 0.1, lgd: 0.5 };
    expect(horizonReturn(t, 2, false)).toBeCloseTo(0.9 * 0.02 * 1.95 + 0.9025 - 1, 10);
  });

  it('never loses more than the principal', () => {
    const awful = { r: 0.01, d: 0.9, lgd: 1 };
    for (const reinvest of [true, false]) {
      for (const cycles of [1, 6, 24]) expect(horizonReturn(awful, cycles, reinvest)).toBeGreaterThanOrEqual(-1);
    }
    expect(horizonReturn({ r: -5, d: 0, lgd: 1 }, 3, true)).toBe(-1);
  });

  it('annualizes non-year horizons', () => {
    expect(annualize(0.1, 365)).toBe(0.1);
    expect(annualize(0.05, 182.5)).toBeCloseTo(1.05 ** 2 - 1);
    expect(annualize(-1.5, 100)).toBe(-1);
  });

  it('computes Wilson intervals that contain the observed rate', () => {
    const w = wilsonInterval(2, 100);
    expect(w.low).toBeLessThan(0.02);
    expect(w.high).toBeGreaterThan(0.02);
    expect(wilsonInterval(0, 200).low).toBe(0);
    expect(wilsonInterval(5, 5).high).toBe(1);
    expect(wilsonInterval(0, 0)).toEqual({ low: 0, high: 1 });
  });

  it('maps sample size to confidence', () => {
    expect(confidenceFor(50)).toBe('high');
    expect(confidenceFor(20)).toBe('medium');
    expect(confidenceFor(5)).toBe('low');
    expect(confidenceFor(4)).toBe('insufficient');
  });

  it('prefers tenor-specific history and falls back to all-tenor data', () => {
    expect(historyFor(metrics, 'A', 90)).toMatchObject({ tenorSpecific: true, history: { tenorDays: 90 } });
    const fallback = historyFor(metrics, 'A', 30);
    expect(fallback.tenorSpecific).toBe(false);
    expect(fallback.history?.tenorDays).toBeUndefined();
    expect(fallback.history?.realizedYieldBps).toBe(1_200);
    expect(historyFor({ ...metrics, tiers: [] }, 'A', 30)).toEqual({ history: null, tenorSpecific: false });
  });
});

describe('projectReturns', () => {
  it('100% in one high-confidence tier gives a full range with a middle estimate', () => {
    const r = projectReturns(metrics, inputs());
    expect(r.valid).toBe(true);
    expect(r.confidence).toBe('high');
    expect(r.returnPct!.low).toBeLessThan(r.returnPct!.expected!);
    expect(r.returnPct!.expected!).toBeLessThan(r.returnPct!.high);
    // no defaults observed: middle = 8% × 60/365 × 365/60 = 8%
    expect(r.returnPct!.expected!).toBeCloseTo(0.08, 6);
    expect(r.returnUsd!.expected!).toBeCloseTo(800, 2);
    expect(r.annualizedPct!.expected!).toBeCloseTo(0.08, 6);
    expect(r.tiers).toHaveLength(1);
    expect(r.horizonDays).toBe(365);
  });

  it('ignores 0% tiers even when they have no history', () => {
    const r = projectReturns(metrics, inputs({ allocations: { AAA: 100, AA: 0, C: 0 } }));
    expect(r.tiers.map((t) => t.tier)).toEqual(['AAA']);
    expect(r.lowConfidenceTiers).toEqual([]);
  });

  it('reinvestment increases the middle estimate for positive yields', () => {
    const off = projectReturns(metrics, inputs({ reinvest: false }));
    const on = projectReturns(metrics, inputs({ reinvest: true }));
    expect(on.returnPct!.expected!).toBeGreaterThan(off.returnPct!.expected!);
    expect(on.assumptions.find((a) => a.id === 'reinvest')!.value).toMatch(/compounding/);
    expect(off.assumptions.find((a) => a.id === 'reinvest')!.value).toMatch(/no compounding/);
  });

  it('uses tenor-specific data and warns when falling back', () => {
    const tenor90 = projectReturns(metrics, inputs({ allocations: { A: 100 }, tenorDays: 90 }));
    expect(tenor90.tiers[0].usedTenorSpecificData).toBe(true);
    expect(tenor90.warnings).toEqual([]);
    const tenor30 = projectReturns(metrics, inputs({ allocations: { A: 100 }, tenorDays: 30 }));
    expect(tenor30.tiers[0].usedTenorSpecificData).toBe(false);
    expect(tenor30.warnings[0]).toMatch(/all-tenor data/);
    expect(tenor30.assumptions.find((a) => a.id === 'tier-A')!.value).toMatch(/no recovery assumed/);
  });

  it('flags low-confidence tiers and withholds a single number', () => {
    const r = projectReturns(metrics, inputs({ allocations: { AAA: 50, C: 50 } }));
    expect(r.confidence).toBe('low');
    expect(r.lowConfidenceTiers).toEqual(['C']);
    expect(r.returnPct!.expected).toBeNull();
    expect(r.annualizedPct!.expected).toBeNull();
    expect(r.tiers.find((t) => t.tier === 'C')!.returnPct!.expected).toBeNull();
    expect(r.tiers.find((t) => t.tier === 'AAA')!.returnPct!.expected).not.toBeNull();
    expect(r.warnings[0]).toMatch(/Low confidence/);
  });

  it('refuses to project when an allocated tier has insufficient or no history', () => {
    const thin = projectReturns(metrics, inputs({ allocations: { AAA: 90, AA: 10 } }));
    expect(thin.valid).toBe(true);
    expect(thin.confidence).toBe('insufficient');
    expect(thin.returnPct).toBeNull();
    expect(thin.returnUsd).toBeNull();
    expect(thin.tiers.find((t) => t.tier === 'AA')!.returnPct).toBeNull();
    expect(thin.assumptions.find((a) => a.id === 'tier-AA')!.value).toMatch(/Only 2 matured invoices/);
    const missing = projectReturns({ ...metrics, tiers: [] }, inputs());
    expect(missing.confidence).toBe('insufficient');
    expect(missing.assumptions.find((a) => a.id === 'tier-AAA')!.value).toMatch(/Only 0 matured invoices/);
    const one = projectReturns({ ...metrics, tiers: [{ tier: 'AAA', realizedYieldBps: 1, sampleSize: 1, defaults: 0 }] }, inputs());
    expect(one.assumptions.find((a) => a.id === 'tier-AAA')!.value).toMatch(/Only 1 matured invoice on/);
  });

  it('medium confidence keeps the middle estimate', () => {
    const r = projectReturns(metrics, inputs({ allocations: { B: 100 } }));
    expect(r.confidence).toBe('medium');
    expect(r.returnPct!.expected).not.toBeNull();
  });

  it('handles non-year horizons', () => {
    const r = projectReturns(metrics, inputs({ horizonDays: 180, tenorDays: 90 }));
    expect(r.horizonDays).toBe(180);
    expect(r.annualizedPct!.expected!).toBeGreaterThan(r.returnPct!.expected!);
  });

  it('validates boundary inputs', () => {
    expect(validateInputs(inputs())).toEqual([]);
    expect(validateInputs(inputs({ allocations: { AAA: 99.995 } }))).toEqual([]);
    expect(validateInputs(inputs({ allocations: { AAA: 100.5 } }))).toHaveLength(2);
    expect(validateInputs(inputs({ allocations: { AAA: 60, A: 30 } }))[0]).toMatch(/currently 90%/);
    expect(validateInputs(inputs({ allocations: { AAA: -10, A: 110 } })).length).toBeGreaterThanOrEqual(2);
    expect(validateInputs(inputs({ principalUsd: 0 }))[0]).toMatch(/greater than zero/);
    expect(validateInputs(inputs({ tenorDays: 0 }))).toContain('Tenor must be a positive number of days.');
    expect(validateInputs(inputs({ horizonDays: -1 }))).toContain('Horizon must be a positive number of days.');
    const r = projectReturns(metrics, inputs({ allocations: {} }));
    expect(r.valid).toBe(false);
    expect(r.returnPct).toBeNull();
    expect(r.assumptions.length).toBeGreaterThan(0);
  });

  it('exposes every assumption used', () => {
    const r = projectReturns(metrics, inputs({ allocations: { AAA: 50, A: 50 } }));
    expect(r.assumptions.map((a) => a.id)).toEqual(
      expect.arrayContaining(['data-source', 'method', 'reinvest', 'horizon', 'tenor', 'range', 'tier-AAA', 'tier-A']),
    );
    expect(r.assumptions.find((a) => a.id === 'data-source')!.value).toContain('2025-10-01 to 2026-09-30');
    const noPeriod = projectReturns({ ...metrics, periodStart: undefined }, inputs());
    expect(noPeriod.assumptions[0].value).toBe('test analytics, as of 2026-09-30');
  });
});

describe('AnalyticsService', () => {
  it('fetches and validates historical yields from the analytics service', async () => {
    const fetchFn = jest.fn(async () =>
      jsonResponse(200, {
        asOf: '2026-10-01',
        source: 'services/analytics',
        tiers: [
          metrics.tiers[0],
          { tier: 'ZZZ', realizedYieldBps: 1, sampleSize: 1, defaults: 0 },
          { tier: 'A', realizedYieldBps: -1, sampleSize: 1, defaults: 0 },
          { tier: 'A', realizedYieldBps: 1, sampleSize: 1, defaults: 2 },
          { tier: 'B', realizedYieldBps: 1, sampleSize: 3.7, defaults: 0, lossGivenDefaultBps: 20_000, tenorDays: 30 },
          null,
        ],
      }),
    );
    const svc = new AnalyticsService({ env: { KORA_ANALYTICS_URL: 'https://analytics.example' }, fetchFn });
    const { metrics: m, sample } = await svc.loadHistoricalMetrics();
    expect(fetchFn).toHaveBeenCalledWith(`https://analytics.example${HISTORICAL_YIELDS_PATH}`, expect.anything());
    expect(sample).toBe(false);
    expect(m.tiers.map((t) => t.tier)).toEqual(['AAA', 'B']);
    expect(m.tiers[1]).toMatchObject({ sampleSize: 3, lossGivenDefaultBps: undefined, tenorDays: 30 });
  });

  it('fills envelope defaults and rejects unusable bodies', () => {
    expect(parseHistoricalMetrics({ tiers: [] })).toMatchObject({ asOf: 'unknown date', source: 'services/analytics' });
    expect(() => parseHistoricalMetrics({})).toThrow(/unexpected format/);
    expect(() => parseHistoricalMetrics(null)).toThrow(/unexpected format/);
  });

  it('maps 404, rate limits and missing config', async () => {
    const notFound = new AnalyticsService({ env: { KORA_ANALYTICS_URL: 'https://a' }, fetchFn: async () => jsonResponse(404, {}) });
    await expect(notFound.loadHistoricalMetrics()).rejects.toMatchObject({ code: 'NOT_FOUND' });
    const limited = new AnalyticsService({
      env: { KORA_ANALYTICS_URL: 'https://a' },
      fetchFn: async () => jsonResponse(429, {}),
      sleep: async () => undefined,
    });
    await expect(limited.loadHistoricalMetrics()).rejects.toMatchObject({ code: 'RATE_LIMITED' });
    await expect(new AnalyticsService({ env: { NODE_ENV: 'production' } }).loadHistoricalMetrics()).rejects.toMatchObject({
      code: 'NOT_CONFIGURED',
    });
    await expect(new AnalyticsService({ env: {} }).loadHistoricalMetrics()).resolves.toEqual({
      metrics: DEV_SAMPLE_METRICS,
      sample: true,
    });
  });
});

describe('yield calculator UI', () => {
  const GUARANTEE = /guarantee/i;

  it('renders inputs, range, confidence and the disclaimer', () => {
    const html = renderToStaticMarkup(<YieldCalculatorView metrics={metrics} initialInputs={inputs()} />);
    expect(html).toContain('Yield Calculator');
    expect(html).toContain('AAA allocation percent');
    expect(html).toContain('Reinvest returns');
    expect(html).toContain('data-confidence="high"');
    expect(html).toContain('middle estimate');
    expect(html).toContain('over one year');
    expect(html).toContain('not a promise of future returns');
    expect(html).not.toMatch(GUARANTEE);
    expect(html).not.toContain('Development sample data');
  });

  it('shows assumptions in an expandable section', () => {
    const html = renderToStaticMarkup(<YieldCalculatorView metrics={metrics} initialInputs={inputs()} />);
    expect(html).toContain('<details');
    expect(html).toContain('How this estimate is made (assumptions)');
    for (const id of ['data-source', 'method', 'reinvest', 'horizon', 'tenor', 'range', 'tier-AAA']) {
      expect(html).toContain(`data-assumption="${id}"`);
    }
  });

  it('flags low-confidence and insufficient projections without precise numbers', () => {
    const low = renderToStaticMarkup(
      <YieldCalculatorView metrics={metrics} initialInputs={inputs({ allocations: { AAA: 50, C: 50 } })} />,
    );
    expect(low).toContain('data-confidence="low"');
    expect(low).toContain('No single estimate is shown');
    expect(low.split('Projection by risk tier')[0]).not.toContain('middle estimate');
    expect(low).toContain('data-tier-confidence="low"');
    const none = renderToStaticMarkup(
      <YieldCalculatorView metrics={metrics} sample initialInputs={inputs({ allocations: { AA: 100 }, horizonDays: 90, tenorDays: 30 })} />,
    );
    expect(none).toContain('data-confidence="insufficient"');
    expect(none).toContain('We cannot project returns for this mix: AA');
    expect(none).toContain('Not enough history');
    expect(none).toContain('Development sample data');
  });

  it('shows validation errors for bad allocations', () => {
    const result = projectReturns(metrics, inputs({ allocations: { AAA: 40 } }));
    const html = renderToStaticMarkup(<ProjectionSummary result={result} />);
    expect(html).toContain('Allocations must add up to 100%');
    expect(html).toContain(DISCLAIMER.slice(0, 40));
  });

  it('formats losses and non-year horizons', () => {
    const lossy: HistoricalMetrics = {
      ...metrics,
      tiers: [{ tier: 'C', realizedYieldBps: 500, sampleSize: 100, defaults: 40 }],
    };
    const html = renderToStaticMarkup(
      <YieldCalculatorView metrics={lossy} initialInputs={inputs({ allocations: { C: 100 }, horizonDays: 180 })} />,
    );
    expect(html).toContain('−$');
    expect(html).toContain('over 180 days');
    // Losses are bounded by the principal.
    const result = projectReturns(lossy, inputs({ allocations: { C: 100 }, horizonDays: 180 }));
    expect(result.returnPct!.low).toBeGreaterThanOrEqual(-1);
  });

  it('shows loading and error states around analytics loads', () => {
    expect(renderToStaticMarkup(<YieldCalculator service={new AnalyticsService({ env: {} })} />)).toContain(
      'Loading historical protocol data',
    );
    const err = renderToStaticMarkup(
      <ErrorPanel error={mapSdkError(new Error('Error(Contract, #10)'))} emptyTitle="No historical data yet" />,
    );
    expect(err).toContain('No historical data yet');
  });

  it('the dev sample covers every tier and includes a thin-data tier', () => {
    const tiers = new Set(DEV_SAMPLE_METRICS.tiers.map((t) => t.tier));
    expect(tiers.size).toBe(5);
    expect(DEV_SAMPLE_METRICS.tiers.some((t) => confidenceFor(t.sampleSize) === 'insufficient')).toBe(true);
  });
});
