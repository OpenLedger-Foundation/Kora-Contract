/**
 * Yield calculator and return projection tool (issue #786).
 *
 * Presentation only — all maths lives in `yieldProjection/model.ts`.
 * Projection/education tool: it never builds or submits a portfolio.
 */
import React, { useMemo, useState } from 'react';
import {
  Confidence,
  HistoricalMetrics,
  ProjectionInputs,
  ProjectionResult,
  ProjectionTier,
  RISK_TIERS,
  Range,
  TENOR_OPTIONS_DAYS,
  projectReturns,
} from '../../yieldProjection/model';
import { AnalyticsService, analyticsService as defaultAnalytics } from '../../services/analyticsService';
import { useAsyncResource } from '../../hooks/useAsyncResource';
import { ErrorPanel, LoadingPanel } from '../common/KoraDataState';

export const DISCLAIMER =
  'These figures are estimates based on past protocol results. They are not a promise of future returns, ' +
  'and you can lose some or all of the money you invest.';

const CONFIDENCE_LABEL: Record<Confidence, string> = {
  high: 'Good amount of history',
  medium: 'Moderate history',
  low: 'Low confidence — limited history',
  insufficient: 'Not enough history to project',
};

const CONFIDENCE_CLASS: Record<Confidence, string> = {
  high: 'bg-emerald-50 text-emerald-800 border-emerald-300',
  medium: 'bg-sky-50 text-sky-800 border-sky-300',
  low: 'bg-amber-50 text-amber-900 border-amber-300',
  insufficient: 'bg-rose-50 text-rose-800 border-rose-300',
};

export const DEFAULT_INPUTS: ProjectionInputs = {
  principalUsd: 10_000,
  allocations: { AAA: 20, AA: 30, A: 30, B: 20, C: 0 },
  tenorDays: 60,
  reinvest: true,
  horizonDays: 365,
};

function usd(n: number): string {
  const sign = n < 0 ? '−' : '';
  return `${sign}$${Math.abs(n).toLocaleString('en-US', { maximumFractionDigits: 0 })}`;
}

function pct(n: number): string {
  const sign = n < 0 ? '−' : '';
  return `${sign}${Math.abs(n * 100).toFixed(1)}%`;
}

function RangeText({ range, format }: { range: Range; format: (n: number) => string }) {
  return (
    <span>
      {format(range.low)} to {format(range.high)}
      {range.expected !== null && <span className="text-gray-500"> (middle estimate {format(range.expected)})</span>}
    </span>
  );
}

/** Results panel: range, confidence, warnings and the expandable assumptions. */
export const ProjectionSummary: React.FC<{ result: ProjectionResult; sample?: boolean }> = ({ result, sample }) => (
  <section aria-label="Projected return range" className="space-y-3">
    {sample && (
      <p className="rounded-lg border border-dashed border-gray-300 px-2 py-1 text-[11px] text-gray-500">
        Development sample data — set KORA_ANALYTICS_URL to use real protocol history.
      </p>
    )}
    {!result.valid ? (
      <div role="alert" className="rounded-xl border border-rose-200 bg-rose-50 p-3 text-xs text-rose-800">
        <ul className="list-disc pl-4">
          {result.errors.map((e) => (
            <li key={e}>{e}</li>
          ))}
        </ul>
      </div>
    ) : (
      <>
        <span
          role="status"
          data-confidence={result.confidence}
          className={`inline-block rounded-full border px-3 py-1 text-xs font-semibold ${CONFIDENCE_CLASS[result.confidence]}`}
        >
          {CONFIDENCE_LABEL[result.confidence]}
        </span>
        {result.returnUsd && result.annualizedPct ? (
          <div className="rounded-xl border border-gray-200 bg-gray-50 p-4 text-sm">
            <p className="text-xs text-gray-500">
              Estimated return over {result.horizonDays === 365 ? 'one year' : `${result.horizonDays} days`}, based on
              history
            </p>
            <p className="mt-1 text-lg font-bold text-gray-900">
              <RangeText range={result.returnUsd} format={usd} />
            </p>
            <p className="text-xs text-gray-600">
              Per year: <RangeText range={result.annualizedPct} format={pct} />
            </p>
            {result.returnUsd.expected === null && (
              <p className="mt-2 text-xs text-amber-800">
                No single estimate is shown because some tiers have limited history.
              </p>
            )}
          </div>
        ) : (
          <div role="alert" className="rounded-xl border border-rose-200 bg-rose-50 p-3 text-xs text-rose-800">
            We cannot project returns for this mix: {result.lowConfidenceTiers.join(', ')} has too little history.
            Lower or remove that allocation to see an estimate.
          </div>
        )}
        {result.warnings.length > 0 && (
          <ul className="space-y-1 text-xs text-amber-900">
            {result.warnings.map((w) => (
              <li key={w}>⚠ {w}</li>
            ))}
          </ul>
        )}
        {result.tiers.length > 0 && (
          <table className="w-full text-xs" aria-label="Projection by risk tier">
            <thead>
              <tr className="text-left text-gray-500">
                <th className="py-1">Tier</th>
                <th>Share</th>
                <th>History</th>
                <th>Estimated return</th>
              </tr>
            </thead>
            <tbody>
              {result.tiers.map((t) => (
                <tr key={t.tier} className="border-t border-gray-100" data-tier-confidence={t.confidence}>
                  <td className="py-1 font-semibold">{t.tier}</td>
                  <td>{t.allocationPct}%</td>
                  <td>
                    {t.sampleSize} invoices{t.confidence === 'low' || t.confidence === 'insufficient' ? ' ⚠' : ''}
                  </td>
                  <td>{t.returnUsd ? <RangeText range={t.returnUsd} format={usd} /> : 'Not enough history'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </>
    )}
    <details className="rounded-xl border border-gray-200 p-3 text-xs">
      <summary className="cursor-pointer font-semibold text-gray-800">How this estimate is made (assumptions)</summary>
      <dl className="mt-2 space-y-2">
        {result.assumptions.map((a) => (
          <div key={a.id} data-assumption={a.id}>
            <dt className="font-semibold text-gray-700">{a.label}</dt>
            <dd className="text-gray-600">
              {a.value} <span className="text-gray-400">— {a.basis}</span>
            </dd>
          </div>
        ))}
      </dl>
    </details>
    <p className="text-[11px] text-gray-500" role="note">
      {DISCLAIMER}
    </p>
  </section>
);

export interface YieldCalculatorViewProps {
  metrics: HistoricalMetrics;
  sample?: boolean;
  initialInputs?: ProjectionInputs;
}

export const YieldCalculatorView: React.FC<YieldCalculatorViewProps> = ({
  metrics,
  sample = false,
  initialInputs = DEFAULT_INPUTS,
}) => {
  const [inputs, setInputs] = useState<ProjectionInputs>(initialInputs);
  const result = useMemo(() => projectReturns(metrics, inputs), [metrics, inputs]);
  const setAllocation = (tier: ProjectionTier, value: number) =>
    setInputs((prev) => ({ ...prev, allocations: { ...prev.allocations, [tier]: value } }));

  return (
    <div
      className="yield-calculator mx-auto max-w-2xl space-y-4 rounded-2xl border border-gray-200 bg-white p-4 shadow-sm sm:p-6"
      role="region"
      aria-label="Yield calculator"
    >
      <header>
        <h2 className="text-lg font-bold text-gray-900">Yield Calculator</h2>
        <p className="text-xs text-gray-500">
          See a range of possible returns based on how past invoices on Kora performed.
        </p>
      </header>

      <form className="space-y-3 text-xs" onSubmit={(e) => e.preventDefault()}>
        <label className="block">
          <span className="font-semibold text-gray-700">Amount to invest (USDC)</span>
          <input
            type="number"
            min={0}
            value={inputs.principalUsd}
            onChange={(e) => setInputs((p) => ({ ...p, principalUsd: Number(e.target.value) }))}
            className="mt-1 w-full rounded-xl border border-gray-300 px-3 py-2 text-sm"
          />
        </label>

        <fieldset>
          <legend className="font-semibold text-gray-700">Split across risk tiers (must add up to 100%)</legend>
          <div className="mt-1 grid grid-cols-5 gap-2">
            {RISK_TIERS.map((tier) => (
              <label key={tier} className="block text-center">
                <span className="block font-semibold">{tier}</span>
                <input
                  type="number"
                  min={0}
                  max={100}
                  aria-label={`${tier} allocation percent`}
                  value={inputs.allocations[tier] ?? 0}
                  onChange={(e) => setAllocation(tier, Number(e.target.value))}
                  className="w-full rounded-lg border border-gray-300 px-1 py-1 text-center"
                />
              </label>
            ))}
          </div>
        </fieldset>

        <div className="flex flex-wrap gap-4">
          <label className="block">
            <span className="font-semibold text-gray-700">Invoice length</span>
            <select
              value={inputs.tenorDays}
              onChange={(e) => setInputs((p) => ({ ...p, tenorDays: Number(e.target.value) }))}
              className="mt-1 block rounded-xl border border-gray-300 px-3 py-2"
            >
              {TENOR_OPTIONS_DAYS.map((d) => (
                <option key={d} value={d}>
                  {d} days
                </option>
              ))}
            </select>
          </label>
          <label className="flex items-center gap-2 pt-5">
            <input
              type="checkbox"
              checked={inputs.reinvest}
              onChange={(e) => setInputs((p) => ({ ...p, reinvest: e.target.checked }))}
            />
            <span className="font-semibold text-gray-700">Reinvest returns</span>
          </label>
        </div>
      </form>

      <ProjectionSummary result={result} sample={sample} />
    </div>
  );
};

/** Loads analytics history, then renders the calculator. */
export const YieldCalculator: React.FC<{ service?: AnalyticsService; initialInputs?: ProjectionInputs }> = ({
  service = defaultAnalytics,
  initialInputs,
}) => {
  const { state, reload } = useAsyncResource(() => service.loadHistoricalMetrics(), [service]);
  if (state.status === 'loading') return <LoadingPanel label="Loading historical protocol data…" />;
  if (state.status === 'error') {
    return <ErrorPanel error={state.error} onRetry={reload} emptyTitle="No historical data yet" />;
  }
  return <YieldCalculatorView metrics={state.data.metrics} sample={state.data.sample} initialInputs={initialInputs} />;
};
