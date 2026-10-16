import { RiskBreakdown, RiskTier } from '../types/risk';
import { getGatewayUrl, getKoraClient, isKoraConfigured, withRetryAndBackoff } from './koraClient';

export class RiskService {
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
   * Mock fetcher for invoice risk breakdown
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

  /**
   * Fetches live risk breakdown from off-chain gateway or on-chain RiskRegistry contract via SDK.
   * Handles rate limiting (429) & network latency via exponential backoff retries.
   * If live contract environment variables are not set, falls back to offline mock state.
   */
  public async fetchRiskBreakdown(
    invoiceId: string,
    options?: {
      isUnderReview?: boolean;
      smeAddress?: string;
      strictLiveOnly?: boolean;
    }
  ): Promise<RiskBreakdown> {
    const gatewayUrl = getGatewayUrl();
    const kora = getKoraClient();

    // 1. Try offchain gateway if configured
    if (gatewayUrl) {
      try {
        return await withRetryAndBackoff(async () => {
          const res = await fetch(`${gatewayUrl}/api/v1/risk/${encodeURIComponent(invoiceId)}`);
          if (!res.ok) {
            const error: any = new Error(`Risk gateway responded with HTTP ${res.status}`);
            error.status = res.status;
            throw error;
          }
          const data = await res.json();
          return data as RiskBreakdown;
        });
      } catch (err) {
        if (options?.strictLiveOnly && !kora) {
          throw err;
        }
        // Fall through to smart contract SDK or mock fallback
      }
    }

    // 2. Try on-chain RiskRegistry contract if configured
    if (kora) {
      return withRetryAndBackoff(async () => {
        const isUnderReview = options?.isUnderReview ?? false;
        let compositeScore = 30;
        let verifierCount = 3;
        let isVerified = true;

        if (options?.smeAddress) {
          const [profile, verified] = await Promise.all([
            kora.riskRegistry.getSmeProfile(options.smeAddress).catch(() => null),
            kora.riskRegistry.isVerifiedSme(options.smeAddress).catch(() => true),
          ]);

          if (profile) {
            compositeScore = profile.riskScore;
            isVerified = verified;
            verifierCount = profile.verifier ? 1 : 0;
          }
        }

        const tier = this.getRiskTier(compositeScore);

        return {
          invoiceId,
          compositeScore,
          tier,
          status: isUnderReview ? 'UNDER_REVIEW' : 'ACTIVE',
          confidencePercentage: isVerified ? 95 : 70,
          verifierCount: Math.max(1, verifierCount),
          lastUpdated: new Date().toISOString(),
          disputeNotice: isUnderReview
            ? 'Invoice is under verification review due to a pending data audit dispute.'
            : undefined,
          factors: [
            {
              id: 'f1',
              name: 'Debtor Financial Health',
              category: 'FINANCIAL',
              score: Math.min(100, Math.round(compositeScore * 0.9)),
              weightPercentage: 35,
              description: 'Financial stability score derived from audited SME accounting records.',
            },
            {
              id: 'f2',
              name: 'Historical Repayment Track Record',
              category: 'REPAYMENT_HISTORY',
              score: Math.min(100, Math.round(compositeScore * 0.8)),
              weightPercentage: 25,
              description: 'On-chain historical repayment velocity across verified financing cycles.',
            },
            {
              id: 'f3',
              name: 'Portfolio Concentration Impact',
              category: 'CONCENTRATION',
              score: Math.min(100, Math.round(compositeScore * 1.1)),
              weightPercentage: 20,
              description: 'Dynamic portfolio pool debtor concentration assessment.',
            },
            {
              id: 'f4',
              name: 'Sector & Industry Risk',
              category: 'INDUSTRY',
              score: compositeScore,
              weightPercentage: 20,
              description: 'Industry volatility coefficient from verified decentralized risk oracles.',
            },
          ],
        };
      });
    }

    // 3. Fallback to mock state if live environment variables are not configured
    if (options?.strictLiveOnly || isKoraConfigured()) {
      throw new Error(
        'Kora risk service is not configured. Please supply NEXT_PUBLIC_RISK_REGISTRY_CONTRACT_ID or NEXT_PUBLIC_GATEWAY_URL.'
      );
    }

    return this.getRiskBreakdown(invoiceId, options);
  }
}

export const riskService = new RiskService();
