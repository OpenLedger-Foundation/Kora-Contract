import React, { useState } from 'react';
import { useI18n } from '../../context/LocaleContext';
import { useTransactionSimulation } from '../../context/SimulationContext';
import { FormattedAmount } from '../localization/FormattedAmount';
import { KillSwitchGate } from '../common/KillSwitchGate';

interface FundingFlowProps {
  listingId?: number;
  defaultAmountUsd?: number;
}

export const FundingFlow: React.FC<FundingFlowProps> = ({
  listingId = 101,
  defaultAmountUsd = 1000,
}) => {
  const { t } = useI18n();
  const { runSimulation } = useTransactionSimulation();

  const [id, setId] = useState<number>(listingId);
  const [contributionAmountUsd, setContributionAmountUsd] = useState<number>(defaultAmountUsd);

  const handleFundSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    await runSimulation({
      flowType: 'FUNDING',
      contractAddress: 'C_MARKETPLACE_001',
      methodName: 'fund_invoice',
      args: {
        listingId: id,
        amount: contributionAmountUsd,
        currency: 'USDC',
      },
    });
  };

  return (
    <KillSwitchGate flagName="secondary-market">
      <div className="flow-card max-w-xl mx-auto bg-white border border-gray-200 rounded-2xl p-4 sm:p-6 shadow-sm">
        <h2 className="text-lg font-bold text-gray-900 mb-1">{t('flow.fund_invoice')}</h2>
        <p className="text-xs text-gray-500 mb-6">
          Provide liquidity to SME invoice #{id}. Soroban pre-flight simulation will preview your state change before signing.
        </p>

        <form onSubmit={handleFundSubmit} className="space-y-4">
          <div>
            <label className="block text-xs font-semibold text-gray-700 mb-1">Listing ID</label>
            <input
              type="number"
              value={id}
              onChange={(e) => setId(Number(e.target.value))}
              className="w-full px-3 py-2 border border-gray-300 rounded-xl text-sm focus:ring-2 focus:ring-indigo-500 focus:outline-none"
              required
            />
          </div>

          <div>
            <label className="block text-xs font-semibold text-gray-700 mb-1">Contribution Amount (USDC)</label>
            <input
              type="number"
              value={contributionAmountUsd}
              onChange={(e) => setContributionAmountUsd(Number(e.target.value))}
              className="w-full px-3 py-2 border border-gray-300 rounded-xl text-sm focus:ring-2 focus:ring-indigo-500 focus:outline-none"
              required
              min={10}
            />
            <div className="mt-1 text-xs">
              Local Value: <FormattedAmount amountUsd={contributionAmountUsd} showCanonicalFirst={false} />
            </div>
          </div>

          <button
            type="submit"
            className="w-full mt-4 py-3 bg-indigo-600 text-white font-semibold text-sm rounded-xl hover:bg-indigo-700 transition-colors shadow-sm"
          >
            Simulate & Preview Funding
          </button>
        </form>
      </div>
    </KillSwitchGate>
  );
};
