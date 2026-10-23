import { ConcentrationCapCheck, FeeBreakdown, ListingFundingState, YieldPreview } from '../types/funding';
import { KoraUserError, withRetry } from '../sdk/errors';
import { KoraRuntime, getKoraRuntime } from '../sdk/koraClient';
import { enumName, field, fromBaseUnits, parseInvoiceId, toBigInt, toHex, toNumber } from '../sdk/normalize';

const SECONDS_PER_DAY = 86_400;

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}

export interface FundingServiceDeps {
  /** Defaults to the shared runtime built from env vars. */
  runtime?: () => KoraRuntime;
  /** Unix ms clock (tests). */
  now?: () => number;
  retry?: { retries?: number; sleep?: (ms: number) => Promise<void> };
}

export class FundingService {
  /** Used only when the on-chain fee cannot be read (dev fixtures). */
  private readonly DEFAULT_PLATFORM_FEE_PCT = 0.5;
  private readonly runtime: () => KoraRuntime;
  private readonly now: () => number;
  private readonly retryOptions: FundingServiceDeps['retry'];

  constructor(deps: FundingServiceDeps = {}) {
    this.runtime = deps.runtime ?? getKoraRuntime;
    this.now = deps.now ?? Date.now;
    this.retryOptions = deps.retry;
  }

  /**
   * Calculates fee breakdown for a contribution
   */
  public calculateFeeBreakdown(grossAmountUsd: number, platformFeePercentage = this.DEFAULT_PLATFORM_FEE_PCT): FeeBreakdown {
    const platformFeeUsd = Math.round((grossAmountUsd * (platformFeePercentage / 100)) * 100) / 100;
    const netContributionUsd = Math.max(0, grossAmountUsd - platformFeeUsd);
    return {
      grossAmountUsd,
      platformFeePercentage,
      platformFeeUsd,
      netContributionUsd,
    };
  }

  /**
   * Computes client-side expected yield preview matching on-chain smart contract formulas
   */
  public calculateYieldPreview(
    contributionAmountUsd: number,
    annualYieldPercentage: number,
    tenorDays: number,
    platformFeePercentage = this.DEFAULT_PLATFORM_FEE_PCT
  ): YieldPreview {
    const feeInfo = this.calculateFeeBreakdown(contributionAmountUsd, platformFeePercentage);

    // Interest = Principal * (Annual Rate / 100) * (Tenor / 365)
    const expectedGrossYieldUsd = Math.round((contributionAmountUsd * (annualYieldPercentage / 100) * (tenorDays / 365)) * 100) / 100;
    const expectedNetYieldUsd = Math.max(0, Math.round((expectedGrossYieldUsd - feeInfo.platformFeeUsd) * 100) / 100);
    const estimatedReturnOnInvestmentPct = contributionAmountUsd > 0
      ? Math.round(((expectedNetYieldUsd / contributionAmountUsd) * 100) * 100) / 100
      : 0;

    return {
      contributionAmountUsd,
      annualYieldPercentage,
      tenorDays,
      expectedGrossYieldUsd,
      platformFeeUsd: feeInfo.platformFeeUsd,
      expectedNetYieldUsd,
      estimatedReturnOnInvestmentPct,
    };
  }

  /**
   * Validates if a proposed contribution breaches concentration caps before on-chain signature
   */
  public checkConcentrationCap(
    debtorName: string,
    currentExposureUsd: number,
    proposedContributionUsd: number,
    maxCapUsd: number,
    totalPortfolioUsd: number
  ): ConcentrationCapCheck {
    const projectedExposure = currentExposureUsd + proposedContributionUsd;
    const newTotalPortfolio = totalPortfolioUsd + proposedContributionUsd;

    const currentPercentage = totalPortfolioUsd > 0 ? (currentExposureUsd / totalPortfolioUsd) * 100 : 0;
    const projectedPercentage = newTotalPortfolio > 0 ? (projectedExposure / newTotalPortfolio) * 100 : 0;

    const willBreach = projectedExposure > maxCapUsd;
    const breachReason = willBreach
      ? `Contribution of $${proposedContributionUsd.toLocaleString()} would bring total debtor exposure for ${debtorName} to $${projectedExposure.toLocaleString()}, exceeding the maximum limit of $${maxCapUsd.toLocaleString()}`
      : undefined;

    return {
      debtorName,
      currentExposureUsd,
      proposedContributionUsd,
      maxCapUsd,
      currentPercentage: Math.round(currentPercentage * 10) / 10,
      projectedPercentage: Math.round(projectedPercentage * 10) / 10,
      willBreach,
      breachReason,
    };
  }

  /**
   * Loads listing funding state.
   *
   * With Kora env vars set, reads `marketplace.get_listing`,
   * `invoice_nft.get_invoice`, `financing_pool.get_pool` and
   * `marketplace.get_fee_bps` through the SDK (simulation, no wallet needed).
   * Without them, development builds get the sample fixture; production
   * builds throw `NOT_CONFIGURED` instead of showing sample data.
   */
  public async loadListingState(listingId: number): Promise<ListingFundingState> {
    const runtime = this.runtime();
    if (runtime.config.status === 'invalid') throw runtime.config.error;
    if (!runtime.client || runtime.config.status !== 'configured') {
      if (runtime.liveDataRequired) {
        throw new KoraUserError('NOT_CONFIGURED', 'Funding data is unavailable: Kora contract settings are not configured.');
      }
      return { ...this.getListingState(listingId), source: 'dev-fixture' };
    }
    const id = parseInvoiceId(listingId);
    if (id === null) throw new KoraUserError('NOT_FOUND', 'This listing id is not valid.');

    const { client } = runtime;
    const config = runtime.config.config;
    const read = <T,>(fn: () => Promise<T>, contract: string) => withRetry(fn, { ...this.retryOptions, contract });

    const [listing, invoice] = await Promise.all([
      read(() => client.marketplace.getListing(id), 'marketplace'),
      read(() => client.invoiceNft.getInvoice(id), 'invoice_nft'),
    ]);
    // A pool only exists once funding starts; the fee read is best-effort.
    const [pool, feeBps] = await Promise.all([
      read(() => client.financingPool.getPool(id), 'financing_pool').catch((e: KoraUserError) => {
        if (e.code === 'NOT_FOUND') return null;
        throw e;
      }),
      read(() => client.marketplace.getFeeBps(), 'marketplace').catch(() => null),
    ]);

    const dec = config.tokenDecimals;
    const target = fromBaseUnits(toBigInt(field(listing, 'askingPrice')), dec);
    const faceValue = fromBaseUnits(toBigInt(field(listing, 'faceValue')), dec);
    const listingFunded = fromBaseUnits(toBigInt(field(listing, 'fundedAmount')), dec);
    const poolFunded = pool ? fromBaseUnits(toBigInt(field(pool, 'totalFunded')), dec) : 0;
    const current = Math.max(listingFunded, poolFunded);
    const remaining = round2(Math.max(0, target - current));

    const nowSec = Math.floor(this.now() / 1000);
    const dueDate = toNumber(field(invoice, 'dueDate'));
    const tenorDays = Math.max(0, Math.ceil((dueDate - nowSec) / SECONDS_PER_DAY));
    const annualYieldPercentage =
      target > 0 && tenorDays > 0 ? round2(((faceValue - target) / target) * (365 / tenorDays) * 100) : 0;

    const status = enumName(field(invoice, 'status'));
    const isActive = field<boolean>(listing, 'isActive') !== false;
    const deadline = toNumber(field(listing, 'fundingDeadline'));
    const debtorHex = toHex(field(invoice, 'debtorHash'));

    return {
      listingId,
      invoiceId: `INV-${id}`,
      debtorName: debtorHex ? `Debtor ${debtorHex.slice(0, 8)}…` : 'Undisclosed debtor',
      targetFundingUsd: round2(target),
      currentFundingUsd: round2(current),
      remainingFundingUsd: remaining,
      minContributionUsd: config.minContributionUsd,
      maxConcentrationCapUsd: config.maxConcentrationCapUsd,
      tenorDays,
      annualYieldPercentage,
      isFullyFunded: remaining <= 0 || status === 'Funded',
      isOpenForFunding: isActive && remaining > 0 && (status === undefined || status === 'Listed'),
      source: 'sdk',
      platformFeePercentage: feeBps === null ? undefined : feeBps / 100,
      fundingDeadline: deadline > 0 ? new Date(deadline * 1000).toISOString() : undefined,
      notice: config.manifestWarning,
    };
  }

  /**
   * Development fixture. Only returned by `loadListingState` when no Kora
   * env is configured and the build is not production.
   */
  public getListingState(listingId: number): ListingFundingState {
    return {
      listingId,
      invoiceId: `INV-2026-${listingId}`,
      debtorName: 'Acme Global Logistics',
      targetFundingUsd: 50000,
      currentFundingUsd: 32000,
      remainingFundingUsd: 18000,
      minContributionUsd: 100,
      maxConcentrationCapUsd: 25000,
      tenorDays: 60,
      annualYieldPercentage: 14.5,
      isFullyFunded: false,
    };
  }
}

export const fundingService = new FundingService();
