import { PipelineSuggestion, RegistryRiskFields, RiskBreakdown, RiskFactor, RiskTier } from '../types/risk';
import { KoraUserError, mapSdkError, withRetry } from '../sdk/errors';
import { KoraRuntime, getKoraRuntime } from '../sdk/koraClient';
import { enumName, field, parseInvoiceId, toNumber } from '../sdk/normalize';

const PIPELINE_CATEGORY: Record<string, RiskFactor['category']> = {
  payment_history: 'REPAYMENT_HISTORY',
  invoice_frequency: 'FINANCIAL',
  debtor_concentration: 'CONCENTRATION',
  macro_trade: 'INDUSTRY',
};

const PIPELINE_LABEL: Record<string, string> = {
  payment_history: 'Payment history',
  invoice_frequency: 'Invoice frequency',
  debtor_concentration: 'Debtor concentration',
  macro_trade: 'Macro trade conditions',
};

export interface RiskServiceDeps {
  runtime?: () => KoraRuntime;
  now?: () => number;
  retry?: { retries?: number; sleep?: (ms: number) => Promise<void> };
}

export class RiskService {
  private readonly runtime: () => KoraRuntime;
  private readonly now: () => number;
  private readonly retryOptions: RiskServiceDeps['retry'];

  constructor(deps: RiskServiceDeps = {}) {
    this.runtime = deps.runtime ?? getKoraRuntime;
    this.now = deps.now ?? Date.now;
    this.retryOptions = deps.retry;
  }

  /**
   * Helper to compute the composite risk tier based on numeric score (0 - 100)
   */
  public getRiskTier(score: number): RiskTier {
    if (score <= 30) return 'LOW';
    if (score <= 60) return 'MEDIUM';
    if (score <= 85) return 'HIGH';
    return 'CRITICAL';
  }

  /**
   * Gets accessible color and icon metadata for a given risk tier
   */
  public getRiskTierMetadata(tier: RiskTier) {
    switch (tier) {
      case 'LOW':
        return {
          label: 'Low Risk',
          badgeClass: 'bg-emerald-100 text-emerald-800 border-emerald-300',
          icon: '✓',
          ariaLabel: 'Low risk tier score',
        };
      case 'MEDIUM':
        return {
          label: 'Medium Risk',
          badgeClass: 'bg-amber-100 text-amber-800 border-amber-300',
          icon: '▲',
          ariaLabel: 'Medium risk tier score',
        };
      case 'HIGH':
        return {
          label: 'High Risk',
          badgeClass: 'bg-orange-100 text-orange-800 border-orange-300',
          icon: '⚠️',
          ariaLabel: 'High risk tier score',
        };
      case 'CRITICAL':
        return {
          label: 'Critical Risk',
          badgeClass: 'bg-rose-100 text-rose-800 border-rose-300',
          icon: '⛔',
          ariaLabel: 'Critical risk tier score',
        };
    }
  }

  /**
   * Loads the risk panel for an invoice.
   *
   * Registry-backed fields come from `invoice_nft.get_invoice` (score, tier,
   * status) and `risk_registry.get_sme_profile`. When KORA_GATEWAY_URL is set,
   * the risk-pipeline factor breakdown is fetched from
   * `GET /risk-pipeline/suggestions/:sme`; if that fails the panel still
   * renders registry data with `pipelineNotice` explaining what is missing.
   */
  public async loadRiskBreakdown(invoiceId: string): Promise<RiskBreakdown> {
    const runtime = this.runtime();
    if (runtime.config.status === 'invalid') throw runtime.config.error;
    if (!runtime.client || runtime.config.status !== 'configured') {
      if (runtime.liveDataRequired) {
        throw new KoraUserError('NOT_CONFIGURED', 'Risk data is unavailable: Kora contract settings are not configured.');
      }
      return { ...this.getRiskBreakdown(invoiceId), source: 'dev-fixture' };
    }
    const id = parseInvoiceId(invoiceId);
    if (id === null) throw new KoraUserError('NOT_FOUND', 'This invoice id is not valid.');
    const { client, gateway } = runtime;

    const invoice = await withRetry(() => client.invoiceNft.getInvoice(id), {
      ...this.retryOptions,
      contract: 'invoice_nft',
    });
    const sme = String(field(invoice, 'sme') ?? '');
    const compositeScore = Math.min(100, Math.max(0, toNumber(field(invoice, 'riskScore'))));
    const invoiceStatus = enumName(field(invoice, 'status'));

    let registry: RegistryRiskFields | null = null;
    if (sme) {
      try {
        const profile = await withRetry(() => client.riskRegistry.getSmeProfile(sme), {
          ...this.retryOptions,
          contract: 'risk_registry',
        });
        const registeredAt = toNumber(field(profile, 'registeredAt'));
        registry = {
          smeAddress: sme,
          verified: field<boolean>(profile, 'verified') === true,
          smeRiskScore: toNumber(field(profile, 'riskScore')),
          totalInvoices: toNumber(field(profile, 'totalInvoices')),
          defaults: toNumber(field(profile, 'defaults')),
          registeredAt: registeredAt > 0 ? new Date(registeredAt * 1000).toISOString() : undefined,
        };
      } catch (error) {
        if ((error as KoraUserError).code !== 'NOT_FOUND') throw error;
      }
    }

    let suggestion: PipelineSuggestion | null = null;
    let pipelineNotice: string | undefined;
    if (!gateway) {
      pipelineNotice = 'Factor breakdown is not configured (set KORA_GATEWAY_URL).';
    } else if (sme) {
      try {
        suggestion = await gateway.getJson<PipelineSuggestion>(`/risk-pipeline/suggestions/${encodeURIComponent(sme)}`);
        if (!suggestion) pipelineNotice = 'No risk-pipeline breakdown has been computed for this SME yet.';
      } catch (error) {
        pipelineNotice = `Factor breakdown unavailable: ${mapSdkError(error).userMessage}`;
      }
    }

    const factors: RiskFactor[] = (suggestion?.breakdown ?? [])
      .filter((f) => !f.missing)
      .map((f) => ({
        id: f.factorId,
        name: PIPELINE_LABEL[f.factorId] ?? f.factorId,
        category: PIPELINE_CATEGORY[f.factorId] ?? 'FINANCIAL',
        score: Math.round(f.score),
        weightPercentage: Math.round(f.appliedWeight * 100),
        description: f.rationale,
      }));

    const disputed = invoiceStatus === 'Defaulted';
    return {
      invoiceId: `INV-${id}`,
      compositeScore,
      tier: this.getRiskTier(compositeScore),
      status: disputed ? 'DISPUTED' : 'ACTIVE',
      confidencePercentage: suggestion ? Math.round(suggestion.confidence * 100) : null,
      verifierCount: registry?.verified ? 1 : 0,
      factors,
      lastUpdated: new Date(suggestion ? suggestion.computedAt * 1000 : this.now()).toISOString(),
      disputeNotice: disputed ? 'This invoice is marked as defaulted on-chain.' : undefined,
      source: 'sdk',
      onChainTier: enumName(field(invoice, 'riskTier')),
      registry,
      pipelineNotice,
      notice: runtime.config.config.manifestWarning,
    };
  }

  /**
   * Development fixture. Only returned by `loadRiskBreakdown` when no Kora
   * env is configured and the build is not production.
   */
  public getRiskBreakdown(invoiceId: string, options?: { isUnderReview?: boolean }): RiskBreakdown {
    const isUnderReview = options?.isUnderReview ?? false;

    return {
      invoiceId,
      compositeScore: 28,
      tier: 'LOW',
      status: isUnderReview ? 'UNDER_REVIEW' : 'ACTIVE',
      confidencePercentage: 94,
      verifierCount: 5,
      lastUpdated: new Date().toISOString(),
      disputeNotice: isUnderReview ? 'Invoice is under verification review due to a pending data audit dispute.' : undefined,
      factors: [
        {
          id: 'f1',
          name: 'Debtor Financial Health',
          category: 'FINANCIAL',
          score: 20,
          weightPercentage: 35,
          description: 'Strong cashflow and audited financial balance sheet.',
        },
        {
          id: 'f2',
          name: 'Historical Repayment Track Record',
          category: 'REPAYMENT_HISTORY',
          score: 15,
          weightPercentage: 25,
          description: '100% on-time repayment history across 14 past invoices.',
        },
        {
          id: 'f3',
          name: 'Portfolio Concentration Impact',
          category: 'CONCENTRATION',
          score: 35,
          weightPercentage: 20,
          description: 'Debtor exposure represents 12% of pool capacity.',
        },
        {
          id: 'f4',
          name: 'Industry Sector Volatility',
          category: 'INDUSTRY',
          score: 45,
          weightPercentage: 20,
          description: 'Logistics and supply chain sector seasonal risk factor.',
        },
      ],
    };
  }
}

export const riskService = new RiskService();
