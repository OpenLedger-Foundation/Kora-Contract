/**
 * FX Ingestion Service — Surge Pricing Monitor
 *
 * Polls Soroban RPC `getFeeStats` on an interval, detects Stellar surge
 * pricing episodes and derives an *advisory* fee multiplier that wallets can
 * apply to the inclusion fee of protocol transactions
 * (see docs/MEV_RESISTANCE_NOTES.md §4.2 "Fee estimation").
 *
 * Fee hints are advisory only: they raise the probability of inclusion during
 * congestion but make no consensus or ordering guarantee.
 *
 * The monitor only needs the public RPC URL. It never receives the relay
 * secret, keeper keys or any signing material.
 *
 * Issue #1034
 */

import axios from "axios";
import pino from "pino";

const logger = pino({ name: "fx-ingestion:fee-monitor" });

/** Stellar network minimum base (inclusion) fee per operation, in stroops. */
export const STELLAR_MIN_BASE_FEE = 100;

/** 1.0x expressed in basis points. */
export const BPS_ONE = 10_000;

/** Percentile summary returned by Soroban RPC `getFeeStats` (all strings, stroops). */
export interface FeeDistribution {
  max: string;
  min: string;
  mode: string;
  p10?: string;
  p20?: string;
  p30?: string;
  p40?: string;
  p50: string;
  p60?: string;
  p70?: string;
  p80?: string;
  p90: string;
  p95?: string;
  p99?: string;
  transactionCount: string;
  ledgerCount: number;
}

/** `result` of a Soroban RPC `getFeeStats` call. */
export interface FeeStatsResult {
  sorobanInclusionFee: FeeDistribution;
  inclusionFee: FeeDistribution;
  latestLedger: number;
}

/** Fetches raw fee stats. Injected so tests can replay recorded RPC fixtures. */
export type FeeStatsFetcher = () => Promise<FeeStatsResult>;

export interface FeeMonitorConfig {
  /** Soroban RPC endpoint (public URL only — no credentials). */
  rpcUrl: string;
  /** Poll interval (ms). Default 15_000. */
  pollIntervalMs: number;
  /** Surge is active when observed p90 fee >= base fee x this (bps). Default 15_000 (1.5x). */
  surgeThresholdBps: number;
  /** Upper clamp for the suggested multiplier (bps). Default 100_000 (10x). */
  maxMultiplierBps: number;
  /** A hint older than this (ms) is reported stale. Default 3 x pollIntervalMs. */
  staleAfterMs: number;
  /** Network label reported in the hint (e.g. "testnet", "pubnet"). */
  network: string;
}

/** Advisory fee hint served at GET /network/fee-hint. */
export interface FeeHint {
  network: string;
  /** Network minimum inclusion fee per operation (stroops). */
  base_fee: number;
  /** Suggested multiplier in bps (10_000 = 1.0x), clamped to the configured max. */
  surge_multiplier_bps: number;
  /** Same multiplier as a decimal (e.g. 2.5). */
  surge_multiplier: number;
  /** `ceil(base_fee * surge_multiplier_bps / 10_000)` — suggested inclusion fee per op. */
  suggested_fee: number;
  surge_active: boolean;
  /** ISO timestamp of the observation this hint was derived from (null if never observed). */
  observed_at: string | null;
  /** Ledger the observation was taken at (null if never observed). */
  latest_ledger: number | null;
  /** True when served from last-known-good data (RPC error, flat/stuck stats, or age > staleAfterMs). */
  stale: boolean;
  /** True when the most recent poll failed; hint falls back to last-known-good or 1.0x. */
  degraded: boolean;
  /** Always true — hints carry no ordering or inclusion guarantee. */
  advisory: true;
}

export interface FeeMonitorMetrics {
  /** 1 while a surge episode is in progress, else 0. */
  surgeActive: 0 | 1;
  /** Count of surge episodes started since process start. */
  surgeEpisodesTotal: number;
  /** Count of failed polls. */
  pollErrorsTotal: number;
  /** Current suggested multiplier (bps). */
  multiplierBps: number;
}

export interface SurgeEpisode {
  startedAt: string;
  endedAt: string | null;
  peakMultiplierBps: number;
  startLedger: number;
}

interface Observation {
  multiplierBps: number;
  surgeActive: boolean;
  observedAtMs: number;
  latestLedger: number;
}

const MAX_EPISODE_HISTORY = 50;

export function loadFeeMonitorConfig(env: NodeJS.ProcessEnv = process.env): FeeMonitorConfig {
  const pollIntervalMs = Number(env.FEE_HINT_POLL_INTERVAL_MS ?? "15000");
  return {
    rpcUrl: env.STELLAR_RPC_URL ?? "https://soroban-testnet.stellar.org",
    pollIntervalMs,
    surgeThresholdBps: Number(env.FEE_HINT_SURGE_THRESHOLD_BPS ?? "15000"),
    maxMultiplierBps: Number(env.FEE_HINT_MAX_MULTIPLIER_BPS ?? "100000"),
    staleAfterMs: Number(env.FEE_HINT_STALE_AFTER_MS ?? String(pollIntervalMs * 3)),
    network: env.STELLAR_NETWORK ?? "testnet",
  };
}

/** Default fetcher: JSON-RPC `getFeeStats` against the configured Soroban RPC. */
export function rpcFeeStatsFetcher(rpcUrl: string, timeoutMs = 5_000): FeeStatsFetcher {
  return async () => {
    const res = await axios.post(
      rpcUrl,
      { jsonrpc: "2.0", id: 1, method: "getFeeStats" },
      { timeout: timeoutMs, headers: { "content-type": "application/json" } }
    );
    const body = res.data as { result?: FeeStatsResult; error?: { message?: string } };
    if (body.error || !body.result) {
      throw new Error(`getFeeStats failed: ${body.error?.message ?? "empty result"}`);
    }
    return body.result;
  };
}

function parseStroops(v: string | undefined): number {
  const n = Number(v);
  return Number.isFinite(n) && n >= 0 ? n : 0;
}

/**
 * Derive the multiplier (bps) from fee stats. Uses the larger p90 of the
 * classic and Soroban inclusion-fee distributions so a surge on either lane
 * is reflected. Never below 1.0x; clamped to `maxMultiplierBps`.
 */
export function computeMultiplierBps(
  stats: FeeStatsResult,
  maxMultiplierBps: number,
  baseFee = STELLAR_MIN_BASE_FEE
): number {
  const p90 = Math.max(
    parseStroops(stats.inclusionFee?.p90),
    parseStroops(stats.sorobanInclusionFee?.p90)
  );
  const raw = Math.ceil((p90 * BPS_ONE) / baseFee);
  const cap = Math.max(BPS_ONE, maxMultiplierBps);
  return Math.min(Math.max(raw, BPS_ONE), cap);
}

export class SurgePricingMonitor {
  private timer: ReturnType<typeof setInterval> | null = null;
  private lastGood: Observation | null = null;
  private lastPollFailed = false;
  private flat = false;
  private episodes: SurgeEpisode[] = [];
  private readonly metrics: FeeMonitorMetrics = {
    surgeActive: 0,
    surgeEpisodesTotal: 0,
    pollErrorsTotal: 0,
    multiplierBps: BPS_ONE,
  };

  constructor(
    private readonly config: FeeMonitorConfig,
    private readonly fetchStats: FeeStatsFetcher = rpcFeeStatsFetcher(config.rpcUrl),
    private readonly now: () => number = Date.now
  ) {}

  start(): void {
    if (this.timer) return;
    logger.info({ intervalMs: this.config.pollIntervalMs }, "Surge pricing monitor started");
    void this.poll();
    this.timer = setInterval(() => void this.poll(), this.config.pollIntervalMs);
  }

  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  /** Run one poll. Never throws: RPC failures degrade the hint instead. */
  async poll(): Promise<void> {
    let stats: FeeStatsResult;
    try {
      stats = await this.fetchStats();
    } catch (err) {
      this.lastPollFailed = true;
      this.metrics.pollErrorsTotal += 1;
      logger.warn({ err: (err as Error).message }, "Fee stats unavailable — serving degraded hint");
      return;
    }
    this.lastPollFailed = false;

    // Flat / stuck stats: the RPC returned the same ledger as last time, so
    // the distribution cannot reflect current congestion. Keep last-known-good
    // and let the staleness rules flag it.
    if (this.lastGood && stats.latestLedger <= this.lastGood.latestLedger) {
      this.flat = true;
      logger.warn({ latestLedger: stats.latestLedger }, "Fee stats did not advance — keeping last-known-good");
      return;
    }
    this.flat = false;

    const multiplierBps = computeMultiplierBps(stats, this.config.maxMultiplierBps);
    const surgeActive = multiplierBps >= this.config.surgeThresholdBps;
    const observedAtMs = this.now();
    this.recordEpisode(surgeActive, multiplierBps, observedAtMs, stats.latestLedger);

    this.lastGood = { multiplierBps, surgeActive, observedAtMs, latestLedger: stats.latestLedger };
    this.metrics.surgeActive = surgeActive ? 1 : 0;
    this.metrics.multiplierBps = multiplierBps;
  }

  private recordEpisode(active: boolean, multiplierBps: number, atMs: number, ledger: number): void {
    const current = this.episodes[this.episodes.length - 1];
    const open = current && current.endedAt === null ? current : null;
    if (active && !open) {
      this.metrics.surgeEpisodesTotal += 1;
      this.episodes.push({
        startedAt: new Date(atMs).toISOString(),
        endedAt: null,
        peakMultiplierBps: multiplierBps,
        startLedger: ledger,
      });
      if (this.episodes.length > MAX_EPISODE_HISTORY) this.episodes.shift();
      logger.warn({ multiplierBps, ledger }, "Surge pricing episode started");
    } else if (active && open) {
      open.peakMultiplierBps = Math.max(open.peakMultiplierBps, multiplierBps);
    } else if (!active && open) {
      open.endedAt = new Date(atMs).toISOString();
      logger.info({ peakMultiplierBps: open.peakMultiplierBps }, "Surge pricing episode ended");
    }
  }

  /** Current advisory hint. Always returns a usable hint (1.0x when nothing is known). */
  getHint(): FeeHint {
    const obs = this.lastGood;
    const multiplierBps = obs?.multiplierBps ?? BPS_ONE;
    const tooOld = obs ? this.now() - obs.observedAtMs > this.config.staleAfterMs : true;
    return {
      network: this.config.network,
      base_fee: STELLAR_MIN_BASE_FEE,
      surge_multiplier_bps: multiplierBps,
      surge_multiplier: multiplierBps / BPS_ONE,
      suggested_fee: Math.ceil((STELLAR_MIN_BASE_FEE * multiplierBps) / BPS_ONE),
      surge_active: obs?.surgeActive ?? false,
      observed_at: obs ? new Date(obs.observedAtMs).toISOString() : null,
      latest_ledger: obs?.latestLedger ?? null,
      stale: tooOld || this.flat || this.lastPollFailed,
      degraded: this.lastPollFailed || obs === null,
      advisory: true,
    };
  }

  getMetrics(): FeeMonitorMetrics {
    return { ...this.metrics };
  }

  getEpisodes(): SurgeEpisode[] {
    return this.episodes.map((e) => ({ ...e }));
  }
}

/** Render monitor metrics in Prometheus text exposition format. */
export function renderPrometheusMetrics(m: FeeMonitorMetrics): string {
  return [
    "# HELP kora_surge_pricing_active 1 while Stellar surge pricing is detected, else 0.",
    "# TYPE kora_surge_pricing_active gauge",
    `kora_surge_pricing_active ${m.surgeActive}`,
    "# HELP kora_surge_episodes_total Surge pricing episodes observed since process start.",
    "# TYPE kora_surge_episodes_total counter",
    `kora_surge_episodes_total ${m.surgeEpisodesTotal}`,
    "# HELP kora_fee_hint_multiplier_bps Suggested inclusion-fee multiplier in basis points.",
    "# TYPE kora_fee_hint_multiplier_bps gauge",
    `kora_fee_hint_multiplier_bps ${m.multiplierBps}`,
    "# HELP kora_fee_stats_poll_errors_total Failed getFeeStats polls since process start.",
    "# TYPE kora_fee_stats_poll_errors_total counter",
    `kora_fee_stats_poll_errors_total ${m.pollErrorsTotal}`,
    "",
  ].join("\n");
}
