import React, { useState } from 'react';
import { useI18n } from '../../context/LocaleContext';
import { useTransactionSimulation } from '../../context/SimulationContext';
import { FormattedAmount } from '../localization/FormattedAmount';
import { KillSwitchGate } from '../common/KillSwitchGate';

export const RepaymentFlow: React.FC = () => {
  const { t } = useI18n();
  const { runSimulation } = useTransactionSimulation();

  const [invoiceId, setInvoiceId] = useState<number>(101);
  const [repaymentAmountUsd, setRepaymentAmountUsd] = useState<number>(10000);

  const handleRepaySubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    await runSimulation({
      flowType: 'REPAYMENT',
      contractAddress: 'C_FINANCING_POOL_001',
      methodName: 'repay_invoice',
      args: {
        invoiceId,
        amount: repaymentAmountUsd,
        currency: 'USDC',
      },
    });
  };

  return (
    <KillSwitchGate flagName="partial-repayment">
      <div className="flow-card max-w-xl mx-auto bg-white border border-gray-200 rounded-2xl p-4 sm:p-6 shadow-sm">
        <h2 className="text-lg font-bold text-gray-900 mb-1">{t('flow.repay_invoice')}</h2>
        <p className="text-xs text-gray-500 mb-6">
          SME repayment for matured invoice #{invoiceId} to distribute principal + yield back to investors.
        </p>

        <form onSubmit={handleRepaySubmit} className="space-y-4">
          <div>
            <label className="block text-xs font-semibold text-gray-700 mb-1">Invoice NFT ID</label>
            <input
              type="number"
              value={invoiceId}
              onChange={(e) => setInvoiceId(Number(e.target.value))}
              className="w-full px-3 py-2 border border-gray-300 rounded-xl text-sm focus:ring-2 focus:ring-indigo-500 focus:outline-none"
              required
            />
          </div>

          <div>
            <label className="block text-xs font-semibold text-gray-700 mb-1">Full Repayment Amount (USDC)</label>
            <input
              type="number"
              value={repaymentAmountUsd}
              onChange={(e) => setRepaymentAmountUsd(Number(e.target.value))}
              className="w-full px-3 py-2 border border-gray-300 rounded-xl text-sm focus:ring-2 focus:ring-indigo-500 focus:outline-none"
              required
              min={1}
            />
            <div className="mt-1 text-xs">
              Repayment in Local Currency: <FormattedAmount amountUsd={repaymentAmountUsd} showCanonicalFirst={false} />
            </div>
          </div>

          <button
            type="submit"
            className="w-full mt-4 py-3 bg-emerald-600 text-white font-semibold text-sm rounded-xl hover:bg-emerald-700 transition-colors shadow-sm"
          >
            Simulate & Repay Invoice
          </button>
        </form>
      </div>
    </KillSwitchGate>
  );
};
