/**
 * Historical protocol metrics for the yield calculator (issue #786).
 *
 * Reads `GET {KORA_ANALYTICS_URL}/metrics/historical-yields`, which must
 * return the `HistoricalMetrics` shape from `yieldProjection/model.ts`.
 * Malformed tier rows are dropped rather than guessed at.
 */
import { KoraEnv, defaultEnv, readVar, requiresLiveData } from '../sdk/config';
import { KoraUserError, mapSdkError } from '../sdk/errors';
import { GatewayClient } from '../sdk/gatewayClient';
import { HistoricalMetrics, RISK_TIERS, ProjectionTier, TierHistory } from '../yieldProjection/model';

export const HISTORICAL_YIELDS_PATH = '/metrics/historical-yields';

function isNonNegative(n: unknown): n is number {
  return typeof n === 'number' && Number.isFinite(n) && n >= 0;
}

/** Validates an analytics response. Throws when the envelope is unusable. */
export function parseHistoricalMetrics(raw: unknown): HistoricalMetrics {
  if (!raw || typeof raw !== 'object' || !Array.isArray((raw as { tiers?: unknown }).tiers)) {
    throw new KoraUserError('UNKNOWN', 'The analytics service returned data in an unexpected format.');
  }
  const body = raw as Record<string, unknown> & { tiers: unknown[] };
  const tiers: TierHistory[] = [];
  for (const row of body.tiers) {
    if (!row || typeof row !== 'object') continue;
    const r = row as Record<string, unknown>;
    if (!RISK_TIERS.includes(r.tier as ProjectionTier)) continue;
    if (!isNonNegative(r.realizedYieldBps) || !isNonNegative(r.sampleSize) || !isNonNegative(r.defaults)) continue;
    if ((r.defaults as number) > (r.sampleSize as number)) continue;
    tiers.push({
      tier: r.tier as ProjectionTier,
      tenorDays: isNonNegative(r.tenorDays) ? r.tenorDays : undefined,
      realizedYieldBps: r.realizedYieldBps,
      yieldP10Bps: isNonNegative(r.yieldP10Bps) ? r.yieldP10Bps : undefined,
      yieldP90Bps: isNonNegative(r.yieldP90Bps) ? r.yieldP90Bps : undefined,
      sampleSize: Math.floor(r.sampleSize as number),
      defaults: Math.floor(r.defaults as number),
      lossGivenDefaultBps:
        isNonNegative(r.lossGivenDefaultBps) && r.lossGivenDefaultBps <= 10_000 ? r.lossGivenDefaultBps : undefined,
    });
  }
  return {
    asOf: typeof body.asOf === 'string' ? body.asOf : 'unknown date',
    source: typeof body.source === 'string' ? body.source : 'services/analytics',
    periodStart: typeof body.periodStart === 'string' ? body.periodStart : undefined,
    periodEnd: typeof body.periodEnd === 'string' ? body.periodEnd : undefined,
    tiers,
  };
}

/**
 * Sample history for local development only — shown with a "sample data"
 * label and never returned when KORA_ANALYTICS_URL is set or in production.
 * The C tier deliberately has too little history, to show low-confidence flags.
 */
export const DEV_SAMPLE_METRICS: HistoricalMetrics = {
  asOf: '2026-09-30',
  source: 'Development sample (not real protocol data)',
  periodStart: '2025-10-01',
  periodEnd: '2026-09-30',
  tiers: [
    { tier: 'AAA', realizedYieldBps: 820, yieldP10Bps: 760, yieldP90Bps: 880, sampleSize: 140, defaults: 0, lossGivenDefaultBps: 4_000 },
    { tier: 'AA', realizedYieldBps: 1_010, yieldP10Bps: 930, yieldP90Bps: 1_090, sampleSize: 118, defaults: 1, lossGivenDefaultBps: 4_500 },
    { tier: 'A', realizedYieldBps: 1_240, yieldP10Bps: 1_120, yieldP90Bps: 1_380, sampleSize: 96, defaults: 2, lossGivenDefaultBps: 5_000 },
    { tier: 'A', tenorDays: 60, realizedYieldBps: 1_260, yieldP10Bps: 1_150, yieldP90Bps: 1_390, sampleSize: 41, defaults: 1, lossGivenDefaultBps: 5_000 },
    { tier: 'B', realizedYieldBps: 1_580, yieldP10Bps: 1_390, yieldP90Bps: 1_790, sampleSize: 33, defaults: 2, lossGivenDefaultBps: 6_000 },
    { tier: 'C', realizedYieldBps: 2_150, sampleSize: 3, defaults: 1 },
  ],
};

export interface AnalyticsServiceDeps {
  env?: KoraEnv;
  fetchFn?: typeof fetch;
  sleep?: (ms: number) => Promise<void>;
}

export class AnalyticsService {
  private readonly env: KoraEnv;
  private readonly fetchFn?: typeof fetch;
  private readonly sleep?: (ms: number) => Promise<void>;

  constructor(deps: AnalyticsServiceDeps = {}) {
    this.env = deps.env ?? defaultEnv();
    this.fetchFn = deps.fetchFn;
    this.sleep = deps.sleep;
  }

  async loadHistoricalMetrics(): Promise<{ metrics: HistoricalMetrics; sample: boolean }> {
    const baseUrl = readVar(this.env, 'ANALYTICS_URL');
    if (!baseUrl) {
      if (requiresLiveData(this.env)) {
        throw new KoraUserError('NOT_CONFIGURED', 'Historical data is unavailable: KORA_ANALYTICS_URL is not configured.');
      }
      return { metrics: DEV_SAMPLE_METRICS, sample: true };
    }
    const client = new GatewayClient({ baseUrl, fetchFn: this.fetchFn, sleep: this.sleep });
    let raw: unknown;
    try {
      raw = await client.getJson(HISTORICAL_YIELDS_PATH);
    } catch (error) {
      throw mapSdkError(error);
    }
    if (raw === null) throw new KoraUserError('NOT_FOUND', 'No historical yield data has been published yet.');
    return { metrics: parseHistoricalMetrics(raw), sample: false };
  }
}

export const analyticsService = new AnalyticsService();
