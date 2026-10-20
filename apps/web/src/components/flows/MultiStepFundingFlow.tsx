import React, { useState } from 'react';
import { ConcentrationCapCheck, FundingStep, ListingFundingState, YieldPreview } from '../../types/funding';
import { fundingService } from '../../services/fundingService';
import { useTransactionSimulation } from '../../context/SimulationContext';
import { KillSwitchGate } from '../common/KillSwitchGate';

interface MultiStepFundingFlowProps {
  listingId?: number;
  investorTotalPortfolioUsd?: number;
  investorDebtorExposureUsd?: number;
}

export const MultiStepFundingFlow: React.FC<MultiStepFundingFlowProps> = ({
  listingId = 101,
  investorTotalPortfolioUsd = 10000,
  investorDebtorExposureUsd = 2000,
}) => {
  const { runSimulation } = useTransactionSimulation();
  const [listing] = useState<ListingFundingState>(fundingService.getListingState(listingId));

  const [step, setStep] = useState<FundingStep>('AMOUNT_SELECTION');
  const [amountUsd, setAmountUsd] = useState<number>(1000);
  const [capCheck, setCapCheck] = useState<ConcentrationCapCheck | null>(null);
  const [yieldPreview, setYieldPreview] = useState<YieldPreview | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Handle amount change & live cap checking
  const handleAmountChange = (newAmount: number) => {
    setAmountUsd(newAmount);
    setErrorMsg(null);

    const check = fundingService.checkConcentrationCap(
      listing.debtorName,
      investorDebtorExposureUsd,
      newAmount,
      listing.maxConcentrationCapUsd,
      investorTotalPortfolioUsd
    );
    setCapCheck(check);

    const preview = fundingService.calculateYieldPreview(
      newAmount,
      listing.annualYieldPercentage,
      listing.tenorDays
    );
    setYieldPreview(preview);
  };

  const handleProceedToPreview = (e: React.FormEvent) => {
    e.preventDefault();

    if (listing.isFullyFunded || listing.remainingFundingUsd <= 0) {
      setErrorMsg('Notice: This listing has been fully funded by other investors.');
      return;
    }

    if (amountUsd > listing.remainingFundingUsd) {
      setErrorMsg(`Amount cannot exceed remaining funding needed ($${listing.remainingFundingUsd.toLocaleString()})`);
      return;
    }

    const check = fundingService.checkConcentrationCap(
      listing.debtorName,
      investorDebtorExposureUsd,
      amountUsd,
      listing.maxConcentrationCapUsd,
      investorTotalPortfolioUsd
    );

    if (check.willBreach) {
      setErrorMsg(check.breachReason || 'Contribution exceeds concentration cap limit.');
      return;
    }

    const preview = fundingService.calculateYieldPreview(
      amountUsd,
      listing.annualYieldPercentage,
      listing.tenorDays
    );
    setYieldPreview(preview);
    setStep('YIELD_PREVIEW');
  };

  const handleSimulateAndSign = async () => {
    setStep('SIMULATION_AND_SIGN');
    await runSimulation({
      flowType: 'FUNDING',
      contractAddress: 'C_FINANCING_POOL_001',
      methodName: 'fund_listing_partial',
      args: {
        listingId: listing.listingId,
        amountUsd,
        debtor: listing.debtorName,
      },
    });
  };

  return (
    <KillSwitchGate flagName="multi-asset-pool">
    <div
      className="multi-step-funding-flow bg-white border border-gray-200 rounded-2xl p-4 sm:p-6 shadow-sm max-w-xl mx-auto"
      role="region"
      aria-label="Multi-step partial contribution funding flow"
    >
      {/* Wizard Header / Steps Progress */}
      <div className="mb-6">
        <h2 className="text-lg font-bold text-gray-900">Fund Invoice #{listing.listingId}</h2>
        <p className="text-xs text-gray-500">Debtor: {listing.debtorName}</p>

        <div className="flex items-center justify-between mt-4 text-xs font-semibold">
          <span className={step === 'AMOUNT_SELECTION' ? 'text-indigo-600 font-bold' : 'text-gray-400'}>
            1. Amount & Caps
          </span>
          <span className="text-gray-300">→</span>
          <span className={step === 'YIELD_PREVIEW' ? 'text-indigo-600 font-bold' : 'text-gray-400'}>
            2. Yield & Fee Preview
          </span>
          <span className="text-gray-300">→</span>
          <span className={step === 'SIMULATION_AND_SIGN' ? 'text-indigo-600 font-bold' : 'text-gray-400'}>
            3. Simulation & Sign
          </span>
        </div>
      </div>

      {/* Listing Status Overview */}
      <div className="mb-6 p-3 bg-gray-50 border border-gray-200 rounded-xl text-xs space-y-1">
        <div className="flex justify-between text-gray-700">
          <span>Remaining Funding Needed:</span>
          <strong className="text-gray-900">${listing.remainingFundingUsd.toLocaleString()} USD</strong>
        </div>
        <div className="flex justify-between text-gray-700">
          <span>Annual Target Yield:</span>
          <strong className="text-emerald-700">{listing.annualYieldPercentage}% p.a. ({listing.tenorDays}d Tenor)</strong>
        </div>
      </div>

      {/* Error Message Banner */}
      {errorMsg && (
        <div
          className="mb-4 p-3 bg-rose-50 border border-rose-200 text-rose-800 rounded-xl text-xs"
          role="alert"
        >
          <strong>Validation Warning:</strong> {errorMsg}
        </div>
      )}

      {/* STEP 1: Amount Selection */}
      {step === 'AMOUNT_SELECTION' && (
        <form onSubmit={handleProceedToPreview} className="space-y-4">
          <div>
            <label className="block text-xs font-semibold text-gray-700 mb-1">
              Contribution Amount (USDC)
            </label>
            <input
              type="number"
              value={amountUsd}
              onChange={(e) => handleAmountChange(Number(e.target.value))}
              min={listing.minContributionUsd}
              max={listing.remainingFundingUsd}
              className="w-full px-3 py-2 border border-gray-300 rounded-xl text-sm focus:ring-2 focus:ring-indigo-500 focus:outline-none"
              required
            />
          </div>

          {/* Concentration Cap Pre-submission Feedback */}
          {capCheck && (
            <div
              className={`p-3 rounded-xl text-xs border ${
                capCheck.willBreach
                  ? 'bg-rose-50 border-rose-300 text-rose-800'
                  : 'bg-emerald-50 border-emerald-300 text-emerald-800'
              }`}
            >
              <div className="font-semibold mb-0.5">
                {capCheck.willBreach ? '⚠️ Concentration Cap Exceeded' : '✓ Within Concentration Limit'}
              </div>
              <p>
                Projected exposure for {capCheck.debtorName}: {capCheck.projectedPercentage}% of portfolio (Max cap: ${capCheck.maxCapUsd.toLocaleString()})
              </p>
            </div>
          )}

          <button
            type="submit"
            disabled={Boolean(capCheck?.willBreach)}
            className="w-full py-3 bg-indigo-600 text-white font-semibold text-sm rounded-xl hover:bg-indigo-700 disabled:opacity-50 transition-colors shadow-sm"
          >
            Preview Yield & Fees →
          </button>
        </form>
      )}

      {/* STEP 2: Yield & Fee Preview */}
      {step === 'YIELD_PREVIEW' && yieldPreview && (
        <div className="space-y-4">
          <div className="p-4 bg-gray-50 border border-gray-200 rounded-xl space-y-2 text-xs">
            <h4 className="font-bold text-gray-900 text-sm mb-2">Contribution & Yield Breakdown</h4>

            <div className="flex justify-between text-gray-700">
              <span>Gross Contribution:</span>
              <strong className="text-gray-900">${yieldPreview.contributionAmountUsd.toLocaleString()} USD</strong>
            </div>
            <div className="flex justify-between text-gray-700">
              <span>Platform Service Fee (0.5%):</span>
              <strong className="text-rose-600">-${yieldPreview.platformFeeUsd.toLocaleString()} USD</strong>
            </div>
            <div className="flex justify-between text-gray-700 border-t border-gray-200 pt-2">
              <span>Estimated Net Interest Yield ({listing.tenorDays} days):</span>
              <strong className="text-emerald-700">+${yieldPreview.expectedNetYieldUsd.toLocaleString()} USD</strong>
            </div>
            <div className="flex justify-between text-gray-900 font-bold border-t border-gray-200 pt-2 text-sm">
              <span>Expected Net ROI:</span>
              <span className="text-emerald-600">+{yieldPreview.estimatedReturnOnInvestmentPct}%</span>
            </div>
          </div>

          <div className="flex gap-3">
            <button
              type="button"
              onClick={() => setStep('AMOUNT_SELECTION')}
              className="w-1/2 py-3 bg-gray-100 text-gray-700 font-semibold text-sm rounded-xl hover:bg-gray-200 transition-colors"
            >
              ← Edit Amount
            </button>
            <button
              type="button"
              onClick={handleSimulateAndSign}
              className="w-1/2 py-3 bg-indigo-600 text-white font-semibold text-sm rounded-xl hover:bg-indigo-700 transition-colors shadow-sm"
            >
              Simulate & Sign →
            </button>
          </div>
        </div>
      )}

      {/* STEP 3: Simulation & Sign */}
      {step === 'SIMULATION_AND_SIGN' && (
        <div className="text-center py-6 space-y-3">
          <div className="text-emerald-600 text-3xl font-bold">✓</div>
          <h3 className="text-base font-bold text-gray-900">Pre-flight Simulation Triggered</h3>
          <p className="text-xs text-gray-500">
            Reviewing on-chain state transition preview in the simulation modal.
          </p>
          <button
            type="button"
            onClick={() => setStep('AMOUNT_SELECTION')}
            className="mt-4 px-4 py-2 bg-gray-100 text-gray-700 text-xs font-semibold rounded-xl hover:bg-gray-200"
          >
            Start New Contribution
          </button>
        </div>
      )}
    </div>
    </KillSwitchGate>
  );
};
