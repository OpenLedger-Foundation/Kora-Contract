import React, { useState, useEffect, useCallback } from 'react';
import { RiskBreakdown } from '../../types/risk';
import { riskService } from '../../services/riskService';

interface RiskScoreVisualizationProps {
  breakdown?: RiskBreakdown;
  invoiceId?: string;
  isUnderReview?: boolean;
  smeAddress?: string;
}

export const RiskScoreVisualization: React.FC<RiskScoreVisualizationProps> = ({
  breakdown: initialBreakdown,
  invoiceId = 'INV-2026-101',
  isUnderReview = false,
  smeAddress,
}) => {
  const [breakdown, setBreakdown] = useState<RiskBreakdown>(() =>
    initialBreakdown || riskService.getRiskBreakdown(invoiceId, { isUnderReview })
  );
  const [expanded, setExpanded] = useState<boolean>(false);
  const [isLoading, setIsLoading] = useState<boolean>(false);
  const [fetchError, setFetchError] = useState<string | null>(null);

  const reloadRisk = useCallback(async () => {
    if (initialBreakdown) {
      setBreakdown(initialBreakdown);
      return;
    }
    setIsLoading(true);
    setFetchError(null);
    try {
      const data = await riskService.fetchRiskBreakdown(invoiceId, {
        isUnderReview,
        smeAddress,
      });
      setBreakdown(data);
    } catch (err: any) {
      setFetchError(err?.message || 'Failed to fetch live risk score from gateway/registry.');
    } finally {
      setIsLoading(false);
    }
  }, [initialBreakdown, invoiceId, isUnderReview, smeAddress]);

  useEffect(() => {
    reloadRisk();
  }, [reloadRisk]);

  const meta = riskService.getRiskTierMetadata(breakdown.tier);

  return (
    <div
      className="risk-visualization-card bg-white border border-gray-200 rounded-2xl p-4 sm:p-6 shadow-sm"
      role="region"
      aria-label={`Risk score assessment for invoice ${breakdown.invoiceId}`}
    >
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

      {/* Network / Gateway Loading & Error States */}
      {isLoading && (
        <div
          className="mb-4 p-3 bg-indigo-50 border border-indigo-200 text-indigo-700 rounded-xl text-xs flex items-center justify-between animate-pulse"
          role="status"
          aria-live="polite"
        >
          <span>⏳ Fetching verified risk profile from on-chain RiskRegistry...</span>
        </div>
      )}

      {fetchError && (
        <div
          className="mb-4 p-3 bg-amber-50 border border-amber-200 text-amber-800 rounded-xl text-xs flex items-center justify-between"
          role="alert"
        >
          <span>⚠️ {fetchError}</span>
          <button
            type="button"
            onClick={reloadRisk}
            className="ml-2 px-2 py-1 bg-amber-200 hover:bg-amber-300 rounded text-amber-900 font-semibold text-xs"
          >
            Retry
          </button>
        </div>
      )}

      {/* Under Review Notice Banner if applicable */}
      {breakdown.status === 'UNDER_REVIEW' && (
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
          <span>Confidence: <strong>{breakdown.confidencePercentage}%</strong></span>
        </div>
      </div>

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
