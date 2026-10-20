import React, { useState } from 'react';
import { useI18n } from '../../context/LocaleContext';
import { useTransactionSimulation } from '../../context/SimulationContext';
import { FormattedAmount } from '../localization/FormattedAmount';
import { KillSwitchGate } from '../common/KillSwitchGate';

export const InvoiceSubmissionFlow: React.FC = () => {
  const { t } = useI18n();
  const { runSimulation } = useTransactionSimulation();

  const [debtorName, setDebtorName] = useState('Acme Global Ltd');
  const [amountUsd, setAmountUsd] = useState(5000);
  const [dueDateDays, setDueDateDays] = useState(60);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    await runSimulation({
      flowType: 'INVOICE_MINTING',
      contractAddress: 'C_INVOICE_NFT_001',
      methodName: 'mint_invoice',
      args: {
        debtorName,
        amount: amountUsd,
        currency: 'USDC',
        dueDateDays,
        invoiceId: 105,
      },
    });
  };

  return (
    <KillSwitchGate flagName="fractionalization">
      <div className="flow-card max-w-xl mx-auto bg-white border border-gray-200 rounded-2xl p-4 sm:p-6 shadow-sm">
        <h2 className="text-lg font-bold text-gray-900 mb-1">{t('flow.submit_invoice')}</h2>
        <p className="text-xs text-gray-500 mb-6">
          Submit SME invoice details to mint an Invoice NFT on Soroban and request liquidity.
        </p>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <label className="block text-xs font-semibold text-gray-700 mb-1">Debtor / Customer</label>
            <input
              type="text"
              value={debtorName}
              onChange={(e) => setDebtorName(e.target.value)}
              className="w-full px-3 py-2 border border-gray-300 rounded-xl text-sm focus:ring-2 focus:ring-indigo-500 focus:outline-none"
              required
            />
          </div>

          <div>
            <label className="block text-xs font-semibold text-gray-700 mb-1">Invoice Amount (USDC)</label>
            <input
              type="number"
              value={amountUsd}
              onChange={(e) => setAmountUsd(Number(e.target.value))}
              className="w-full px-3 py-2 border border-gray-300 rounded-xl text-sm focus:ring-2 focus:ring-indigo-500 focus:outline-none"
              required
              min={100}
            />
            <div className="mt-1 text-xs">
              Converted Preview: <FormattedAmount amountUsd={amountUsd} showCanonicalFirst={false} />
            </div>
          </div>

          <div>
            <label className="block text-xs font-semibold text-gray-700 mb-1">Payment Term (Days)</label>
            <select
              value={dueDateDays}
              onChange={(e) => setDueDateDays(Number(e.target.value))}
              className="w-full px-3 py-2 border border-gray-300 rounded-xl text-sm focus:ring-2 focus:ring-indigo-500 focus:outline-none"
            >
              <option value={30}>30 Days</option>
              <option value={60}>60 Days</option>
              <option value={90}>90 Days</option>
            </select>
          </div>

          <button
            type="submit"
            className="w-full mt-4 py-3 bg-indigo-600 text-white font-semibold text-sm rounded-xl hover:bg-indigo-700 transition-colors shadow-sm"
          >
            Simulate & Mint Invoice NFT
          </button>
        </form>
      </div>
    </KillSwitchGate>
  );
};
