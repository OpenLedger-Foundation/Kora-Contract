import React, { useState } from 'react';
import { RiskBreakdown } from '../../types/risk';
import { RiskService, riskService as defaultRiskService } from '../../services/riskService';
import { useAsyncResource } from '../../hooks/useAsyncResource';
import { DataNotice, ErrorPanel, LoadingPanel } from '../common/KoraDataState';

interface RiskScoreVisualizationProps {
  /** Pre-loaded breakdown; skips the fetch. */
  breakdown?: RiskBreakdown;
  invoiceId?: string;
  /** Injected service (tests). Defaults to the env-configured singleton. */
  service?: RiskService;
}

/** Loads the risk breakdown via the SDK (registry + pipeline) with loading and error states. */
export const RiskScoreVisualization: React.FC<RiskScoreVisualizationProps> = ({
  breakdown: initialBreakdown,
  invoiceId = 'INV-2026-101',
  service = defaultRiskService,
}) => {
  const { state, reload } = useAsyncResource(
    () => service.loadRiskBreakdown(invoiceId),
    [invoiceId, service],
    initialBreakdown,
  );
  if (state.status === 'loading') return <LoadingPanel label="Loading risk data from the registry…" />;
  if (state.status === 'error') {
    return <ErrorPanel error={state.error} onRetry={reload} emptyTitle="No risk data for this invoice" />;
  }
  return <RiskScoreCard breakdown={state.data} />;
};

export const RiskScoreCard: React.FC<{ breakdown: RiskBreakdown }> = ({ breakdown }) => {
  const [expanded, setExpanded] = useState<boolean>(false);
  const riskService = defaultRiskService;
  const registry = breakdown.registry;

  const meta = riskService.getRiskTierMetadata(breakdown.tier);

  return (
    <div
      className="risk-visualization-card bg-white border border-gray-200 rounded-2xl p-4 sm:p-6 shadow-sm"
      role="region"
      aria-label={`Risk score assessment for invoice ${breakdown.invoiceId}`}
    >
      <DataNotice notice={breakdown.notice} devFixture={breakdown.source === 'dev-fixture'} />

      {/* Header & Main Tier Indicator */}
      <div className="flex items-center justify-between flex-wrap gap-3 mb-4">
        <div>
          <h3 className="text-sm font-semibold text-gray-700">Composite Risk Score</h3>
          <p className="text-xs text-gray-500">Invoice #{breakdown.invoiceId}</p>
        </div>

        <div className="flex items-center gap-2">
          {breakdown.status === 'UNDER_REVIEW' && (
            <span
              className="inline-flex items-center px-2.5 py-1 text-xs font-semibold rounded-full bg-amber-100 text-amber-900 border border-amber-300"
              role="status"
              aria-label="Status: Under Dispute Review"
            >
              ⚠️ UNDER REVIEW
            </span>
          )}

          <span
            className={`inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-bold rounded-full border ${meta.badgeClass}`}
            role="status"
            aria-label={meta.ariaLabel}
          >
            <span>{meta.icon}</span>
            <span>{meta.label} ({breakdown.compositeScore}/100)</span>
          </span>
        </div>
      </div>

      {/* Under Review Notice Banner if applicable */}
      {(breakdown.status === 'UNDER_REVIEW' || breakdown.status === 'DISPUTED') && (
        <div
          className="mb-4 p-3 bg-amber-50 border border-amber-200 rounded-xl text-xs text-amber-800"
          role="alert"
        >
          <strong>Notice:</strong> {breakdown.disputeNotice || 'This score is undergoing dispute review.'}
        </div>
      )}

      {/* Score Meter & Confidence Summary */}
      <div className="space-y-2 mb-4">
        <div className="flex justify-between text-xs font-medium text-gray-600">
          <span>Score Breakdown</span>
          <span>{breakdown.compositeScore} / 100</span>
        </div>

        {/* Accessible Progress Bar */}
        <div
          className="w-full bg-gray-100 rounded-full h-2.5 overflow-hidden"
          role="progressbar"
          aria-valuenow={breakdown.compositeScore}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-label={`Composite risk score: ${breakdown.compositeScore} out of 100`}
        >
          <div
            className={`h-2.5 rounded-full transition-all duration-300 ${
              breakdown.tier === 'LOW'
                ? 'bg-emerald-500'
                : breakdown.tier === 'MEDIUM'
                ? 'bg-amber-500'
                : breakdown.tier === 'HIGH'
                ? 'bg-orange-500'
                : 'bg-rose-500'
            }`}
            style={{ width: `${breakdown.compositeScore}%` }}
          />
        </div>

        <div className="flex items-center justify-between text-xs text-gray-500 pt-1">
          <span>Verifier Consensus: <strong>{breakdown.verifierCount} Nodes</strong></span>
          <span>
            Confidence:{' '}
            <strong>{breakdown.confidencePercentage === null ? 'n/a' : `${breakdown.confidencePercentage}%`}</strong>
          </span>
        </div>
      </div>

      {breakdown.source === 'sdk' && (
        <dl
          className="mb-4 grid grid-cols-2 gap-2 p-3 bg-gray-50 border border-gray-200 rounded-xl text-xs"
          aria-label="Risk registry fields"
        >
          <dt className="text-gray-500">On-chain tier</dt>
          <dd className="text-gray-900 font-semibold">{breakdown.onChainTier ?? 'n/a'}</dd>
          {registry ? (
            <>
              <dt className="text-gray-500">SME verified</dt>
              <dd className="text-gray-900 font-semibold">{registry.verified ? 'Yes' : 'No'}</dd>
              <dt className="text-gray-500">Registry SME score</dt>
              <dd className="text-gray-900 font-semibold">{registry.smeRiskScore}/100</dd>
              <dt className="text-gray-500">Invoices / defaults</dt>
              <dd className="text-gray-900 font-semibold">
                {registry.totalInvoices} / {registry.defaults}
              </dd>
            </>
          ) : (
            <>
              <dt className="text-gray-500">Risk registry</dt>
              <dd className="text-gray-900 font-semibold">SME not registered</dd>
            </>
          )}
        </dl>
      )}

      {breakdown.pipelineNotice && (
        <p role="note" className="mb-4 text-xs text-gray-600">
          {breakdown.pipelineNotice}
        </p>
      )}

      {/* Expandable Factor Breakdown Toggle */}
      <button
        type="button"
        onClick={() => setExpanded(!expanded)}
        aria-expanded={expanded}
        aria-controls={`risk-factors-${breakdown.invoiceId}`}
        className="w-full py-2 px-3 bg-gray-50 hover:bg-gray-100 border border-gray-200 rounded-xl text-xs font-semibold text-gray-700 flex items-center justify-between transition-colors focus:ring-2 focus:ring-indigo-500 focus:outline-none"
      >
        <span>{expanded ? 'Hide Contributing Factors' : 'View Contributing Factors Breakdown'}</span>
        <span className="text-gray-400 font-bold">{expanded ? '▲' : '▼'}</span>
      </button>

      {/* Expandable Detailed Breakdown Section */}
      {expanded && (
        <div
          id={`risk-factors-${breakdown.invoiceId}`}
          className="mt-4 pt-4 border-t border-gray-100 space-y-3"
          role="region"
          aria-label="Contributing Risk Factors Detail"
        >
          <h4 className="text-xs font-bold text-gray-800 uppercase tracking-wider">
            Risk Factor Pipeline Breakdown
          </h4>

          {breakdown.factors.length === 0 && (
            <p className="text-xs text-gray-500">No factor breakdown available.</p>
          )}
          {breakdown.factors.map((factor) => (
            <div key={factor.id} className="p-3 bg-gray-50 border border-gray-200 rounded-xl text-xs space-y-1">
              <div className="flex items-center justify-between font-semibold text-gray-900">
                <span>{factor.name} ({factor.category})</span>
                <span className="text-gray-600">Weight: {factor.weightPercentage}% | Score: {factor.score}/100</span>
              </div>
              <p className="text-gray-600">{factor.description}</p>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};
