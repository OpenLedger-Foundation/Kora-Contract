/**
 * Issue #938 — funding and risk services wired to @kora-protocol/sdk.
 * All chain and gateway access is faked through the KoraReadClient / fetch seams.
 */
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { readKoraConfig, readVar, requiresLiveData } from '../src/sdk/config';
import { GatewayRateLimitError, KoraUserError, mapSdkError, withRetry } from '../src/sdk/errors';
import { GatewayClient, parseRetryAfter } from '../src/sdk/gatewayClient';
import { KoraReadClient, KoraRuntime, createKoraRuntime, getKoraRuntime, setKoraRuntime } from '../src/sdk/koraClient';
import { enumName, field, fromBaseUnits, parseInvoiceId, toBigInt, toHex, toNumber } from '../src/sdk/normalize';
import { FundingService } from '../src/services/fundingService';
import { RiskService } from '../src/services/riskService';
import { FundingFlowBody, FundingFlowLoader } from '../src/components/flows/MultiStepFundingFlow';
import { RiskScoreCard, RiskScoreVisualization } from '../src/components/risk/RiskScoreVisualization';
import { ErrorPanel } from '../src/components/common/KoraDataState';
import { WalletProvider } from '../src/context/WalletContext';
import { SimulationProvider } from '../src/context/SimulationContext';
import { WalletManager } from '../src/wallet/manager';
import type { WalletAdapter } from '../src/wallet/types';

const C1 = 'CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526';
const C2 = 'CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ';
const SME = 'GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF';
const NOW_MS = Date.UTC(2026, 9, 1);
const NOW_S = NOW_MS / 1000;
const USDC = 10_000_000n; // 7 decimals

const ALL_IDS = {
  KORA_INVOICE_NFT_CONTRACT_ID: C1,
  KORA_MARKETPLACE_CONTRACT_ID: C1,
  KORA_FINANCING_POOL_CONTRACT_ID: C1,
  KORA_TREASURY_CONTRACT_ID: C1,
  KORA_RISK_REGISTRY_CONTRACT_ID: C1,
  KORA_ACCESS_CONTROL_CONTRACT_ID: C1,
  KORA_PRICE_ORACLE_CONTRACT_ID: C1,
};

function manifest(network = 'testnet', deployedAt = '2026-09-20T00:00:00Z') {
  const names = ['invoice_nft', 'marketplace', 'financing_pool', 'treasury', 'risk_registry', 'access_control', 'price_oracle'];
  return JSON.stringify({
    network,
    deployed_at: deployedAt,
    admin: SME,
    contracts: Object.fromEntries(names.map((n) => [n, { address: C2, wasm_hash: 'ab' }])),
  });
}

/** Chain data decoded the way scValToNative returns it (snake_case, enum vectors). */
function fakeClient(overrides: Partial<Record<string, jest.Mock>> = {}): KoraReadClient {
  return {
    marketplace: {
      getListing: overrides.getListing ?? jest.fn(async () => ({
        invoice_id: 101n,
        seller: SME,
        asking_price: 48_000n * USDC,
        face_value: 50_000n * USDC,
        token: C1,
        funded_amount: 12_000n * USDC,
        funding_deadline: BigInt(NOW_S + 7 * 86_400),
        is_active: true,
      }) as any),
      getFeeBps: overrides.getFeeBps ?? jest.fn(async () => 75),
    },
    invoiceNft: {
      getInvoice: overrides.getInvoice ?? jest.fn(async () => ({
        id: 101n,
        sme: SME,
        debtor_hash: new Uint8Array([0xde, 0xad, 0xbe, 0xef, 1, 2, 3, 4, 5]),
        amount: 50_000n * USDC,
        due_date: BigInt(NOW_S + 60 * 86_400),
        risk_score: 42,
        risk_tier: ['A'],
        status: ['Listed'],
      }) as any),
    },
    financingPool: {
      getPool: overrides.getPool ?? jest.fn(async () => ({ total_funded: 20_000n * USDC }) as any),
    },
    riskRegistry: {
      getSmeProfile: overrides.getSmeProfile ?? jest.fn(async () => ({
        address: SME,
        verified: true,
        risk_score: 35,
        total_invoices: 14,
        defaults: 1,
        registered_at: BigInt(NOW_S - 86_400),
      }) as any),
    },
  };
}

function runtime(env: Record<string, string>, client: KoraReadClient | null, gateway: GatewayClient | null = null): () => KoraRuntime {
  const config = readKoraConfig(env, NOW_MS);
  return () => ({ config, client, gateway, liveDataRequired: requiresLiveData(env) || config.status !== 'unconfigured' });
}

const noSleep = { retries: 2, sleep: async () => undefined };

function jsonResponse(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return {
    status,
    ok: status >= 200 && status < 300,
    headers: { get: (k: string) => headers[k] ?? null },
    json: async () => body,
  } as unknown as Response;
}

describe('readKoraConfig', () => {
  it('is unconfigured when no Kora env is present', () => {
    expect(readKoraConfig({}).status).toBe('unconfigured');
  });

  it('builds addresses from explicit contract IDs with defaults', () => {
    const r = readKoraConfig({ ...ALL_IDS, KORA_GATEWAY_URL: 'https://gw.example/' });
    expect(r.status).toBe('configured');
    if (r.status !== 'configured') return;
    expect(r.config.addresses.financingPool).toBe(C1);
    expect(r.config.network).toBe('testnet');
    expect(r.config.rpcUrl).toBe('https://soroban-testnet.stellar.org');
    expect(r.config.gatewayUrl).toBe('https://gw.example');
    expect(r.config.tokenDecimals).toBe(7);
  });

  it('reads VITE_ and REACT_APP_ prefixed names', () => {
    expect(readVar({ VITE_KORA_NETWORK: 'mainnet' }, 'NETWORK')).toBe('mainnet');
    expect(readVar({ REACT_APP_KORA_NETWORK: 'testnet' }, 'NETWORK')).toBe('testnet');
    const r = readKoraConfig({ ...ALL_IDS, KORA_NETWORK: 'mainnet', KORA_RPC_URL: 'https://rpc.example' });
    expect(r.status === 'configured' && r.config.rpcUrl).toBe('https://rpc.example');
  });

  it('loads a deployment manifest and lets explicit IDs override it', () => {
    const r = readKoraConfig({ KORA_DEPLOYMENT_MANIFEST: manifest(), KORA_MARKETPLACE_CONTRACT_ID: C1 }, NOW_MS);
    expect(r.status).toBe('configured');
    if (r.status !== 'configured') return;
    expect(r.config.addresses.invoiceNft).toBe(C2);
    expect(r.config.addresses.marketplace).toBe(C1);
    expect(r.config.manifestWarning).toBeUndefined();
  });

  it('warns when the manifest is older than the max age', () => {
    const r = readKoraConfig({ KORA_DEPLOYMENT_MANIFEST: manifest('testnet', '2025-01-01T00:00:00Z') }, NOW_MS);
    expect(r.status === 'configured' && r.config.manifestWarning).toMatch(/older than 90 days/);
  });

  it.each([
    [{ KORA_DEPLOYMENT_MANIFEST: '{nope' }, /not valid JSON/],
    [{ KORA_DEPLOYMENT_MANIFEST: manifest('mainnet') }, /is for "mainnet"/],
    [{ ...ALL_IDS, KORA_NETWORK: 'futurenet' }, /must be "testnet" or "mainnet"/],
    [{ KORA_MARKETPLACE_CONTRACT_ID: C1 }, /Missing contract ID\(s\): KORA_INVOICE_NFT_CONTRACT_ID/],
    [{ ...ALL_IDS, KORA_TOKEN_DECIMALS: '-1' }, /TOKEN_DECIMALS/],
    [{ KORA_DEPLOYMENT_MANIFEST: manifest(), KORA_MANIFEST_MAX_AGE_DAYS: 'x' }, /MANIFEST_MAX_AGE_DAYS/],
  ])('rejects inconsistent settings %#', (env, message) => {
    const r = readKoraConfig(env as Record<string, string>, NOW_MS);
    expect(r.status).toBe('invalid');
    expect(r.status === 'invalid' && r.error.code).toBe('INVALID_CONFIG');
    expect(r.status === 'invalid' && r.error.userMessage).toMatch(message);
  });

  it('requires live data in production or when forced', () => {
    expect(requiresLiveData({ NODE_ENV: 'production' })).toBe(true);
    expect(requiresLiveData({ KORA_REQUIRE_SDK: 'true' })).toBe(true);
    expect(requiresLiveData({ NODE_ENV: 'development' })).toBe(false);
  });
});

describe('createKoraRuntime', () => {
  afterEach(() => setKoraRuntime(null));

  it('creates a real KoraClient and gateway when configured', () => {
    const rt = createKoraRuntime({ ...ALL_IDS, KORA_GATEWAY_URL: 'https://gw.example' }, NOW_MS);
    expect(typeof rt.client?.marketplace.getListing).toBe('function');
    expect(rt.gateway).toBeInstanceOf(GatewayClient);
    expect(rt.liveDataRequired).toBe(true);
  });

  it('never allows fixtures once any Kora env is set, even when invalid', () => {
    const rt = createKoraRuntime({ KORA_MARKETPLACE_CONTRACT_ID: C1 }, NOW_MS);
    expect(rt.client).toBeNull();
    expect(rt.liveDataRequired).toBe(true);
    expect(createKoraRuntime({}, NOW_MS).liveDataRequired).toBe(false);
  });

  it('caches the shared runtime and allows replacing it', () => {
    const fake = { config: { status: 'unconfigured' as const }, client: null, gateway: null, liveDataRequired: false };
    setKoraRuntime(fake);
    expect(getKoraRuntime()).toBe(fake);
    setKoraRuntime(null);
    expect(getKoraRuntime()).toBe(getKoraRuntime());
  });
});

describe('mapSdkError', () => {
  it('maps not-found contract codes to an empty state', () => {
    const e = mapSdkError(new Error('HostError: Error(Contract, #20)'), 'marketplace');
    expect(e.code).toBe('NOT_FOUND');
    expect(e.userMessage).toBe('Listing not found.');
    expect(e.contractCode).toBe(20);
  });

  it('maps other contract codes with the SDK error table', () => {
    expect(mapSdkError(new Error('Error(Contract, #4)')).userMessage).toBe('The contract rejected this request: Protocol paused.');
    expect(mapSdkError(new Error('Error(Contract, #8)'), 'governance_voting').userMessage).toMatch(/Already voted/);
    expect(mapSdkError(new Error('Error(Contract, #10)'), 'governance_voting').code).toBe('CONTRACT');
    expect(mapSdkError(new Error('Error(Contract, #9999)')).userMessage).toMatch(/code 9999/);
  });

  it('maps rate limits, stale manifests, network and unknown errors', () => {
    expect(mapSdkError(new GatewayRateLimitError(3_000)).userMessage).toMatch(/3 seconds/);
    expect(mapSdkError(new GatewayRateLimitError(500)).userMessage).toMatch(/1 second\./);
    expect(mapSdkError({ response: { status: 429 }, message: 'Request failed' }).code).toBe('RATE_LIMITED');
    expect(mapSdkError(new Error('Error(Storage, MissingValue)')).code).toBe('STALE_MANIFEST');
    expect(mapSdkError(new Error('fetch failed')).code).toBe('NETWORK');
    expect(mapSdkError({ status: 503, message: '' }).code).toBe('NETWORK');
    expect(mapSdkError('weird').code).toBe('UNKNOWN');
    const same = new KoraUserError('NOT_CONFIGURED', 'x');
    expect(mapSdkError(same)).toBe(same);
  });
});

describe('withRetry', () => {
  it('retries rate-limited calls then succeeds', async () => {
    const sleep = jest.fn(async () => undefined);
    const fn = jest.fn().mockRejectedValueOnce(new Error('429 Too Many Requests')).mockResolvedValue('ok');
    await expect(withRetry(fn, { sleep })).resolves.toBe('ok');
    expect(sleep).toHaveBeenCalledTimes(1);
  });

  it('does not retry non-retryable errors and gives up after the budget', async () => {
    const contractFail = jest.fn().mockRejectedValue(new Error('Error(Contract, #4)'));
    await expect(withRetry(contractFail, { sleep: async () => undefined })).rejects.toMatchObject({ code: 'CONTRACT' });
    expect(contractFail).toHaveBeenCalledTimes(1);
    const netFail = jest.fn().mockRejectedValue(new Error('ECONNREFUSED'));
    await expect(withRetry(netFail, { retries: 1, sleep: async () => undefined })).rejects.toMatchObject({ code: 'NETWORK' });
    expect(netFail).toHaveBeenCalledTimes(2);
  });

  it('uses real timers by default', async () => {
    const fn = jest.fn().mockRejectedValueOnce(new Error('fetch failed')).mockResolvedValue(1);
    await expect(withRetry(fn, { baseDelayMs: 1 })).resolves.toBe(1);
  });
});

describe('GatewayClient', () => {
  it('waits for Retry-After on 429 then returns JSON', async () => {
    const sleep = jest.fn(async () => undefined);
    const fetchFn = jest.fn()
      .mockResolvedValueOnce(jsonResponse(429, {}, { 'Retry-After': '2' }))
      .mockResolvedValueOnce(jsonResponse(200, { ok: 1 }));
    const gw = new GatewayClient({ baseUrl: 'https://gw/', fetchFn, sleep });
    await expect(gw.getJson('/x')).resolves.toEqual({ ok: 1 });
    expect(sleep).toHaveBeenCalledWith(2_000);
    expect(fetchFn).toHaveBeenCalledWith('https://gw/x', expect.anything());
  });

  it('throws GatewayRateLimitError when 429 persists or the wait is too long', async () => {
    const always429 = jest.fn(async () => jsonResponse(429, {}, { 'Retry-After': '1' }));
    const gw = new GatewayClient({ baseUrl: 'https://gw', fetchFn: always429, sleep: async () => undefined, maxRetries: 1 });
    await expect(gw.getJson('/x')).rejects.toBeInstanceOf(GatewayRateLimitError);
    expect(always429).toHaveBeenCalledTimes(2);
    const longWait = new GatewayClient({
      baseUrl: 'https://gw',
      fetchFn: async () => jsonResponse(429, {}, { 'Retry-After': '60' }),
    });
    await expect(longWait.getJson('/x')).rejects.toMatchObject({ retryAfterMs: 60_000 });
  });

  it('returns null on 404 and throws on other HTTP errors', async () => {
    const gw404 = new GatewayClient({ baseUrl: 'https://gw', fetchFn: async () => jsonResponse(404, {}) });
    await expect(gw404.getJson('/x')).resolves.toBeNull();
    const gw500 = new GatewayClient({ baseUrl: 'https://gw', fetchFn: async () => jsonResponse(500, {}) });
    await expect(gw500.getJson('/x')).rejects.toMatchObject({ status: 500 });
  });

  it('parses Retry-After seconds, HTTP dates and garbage', () => {
    expect(parseRetryAfter(null, 0)).toBe(1_000);
    expect(parseRetryAfter('1.5', 0)).toBe(1_500);
    expect(parseRetryAfter(new Date(10_000).toUTCString(), 4_000)).toBe(6_000);
    expect(parseRetryAfter('soon', 0)).toBe(1_000);
  });
});

describe('normalize helpers', () => {
  it('reads camelCase or snake_case fields and converts values', () => {
    expect(field({ total_funded: 1 }, 'totalFunded')).toBe(1);
    expect(field({ totalFunded: 2 }, 'totalFunded')).toBe(2);
    expect(field(null, 'x')).toBeUndefined();
    expect(toBigInt(5)).toBe(5n);
    expect(toBigInt('7')).toBe(7n);
    expect(toBigInt('x', 9n)).toBe(9n);
    expect(toNumber(3n)).toBe(3);
    expect(toNumber('4')).toBe(4);
    expect(toNumber('', 1)).toBe(1);
    expect(enumName(['Funded'])).toBe('Funded');
    expect(enumName([])).toBeUndefined();
    expect(enumName('Listed')).toBe('Listed');
    expect(enumName(null)).toBeUndefined();
    expect(toHex(new Uint8Array([1, 255]))).toBe('01ff');
    expect(toHex('abc')).toBe('abc');
    expect(toHex(1)).toBe('');
    expect(fromBaseUnits(12_345_000n, 7)).toBeCloseTo(1.2345);
  });

  it('parses numeric and display invoice ids', () => {
    expect(parseInvoiceId('INV-2026-101')).toBe(101n);
    expect(parseInvoiceId('INV-7')).toBe(7n);
    expect(parseInvoiceId(' 42 ')).toBe(42n);
    expect(parseInvoiceId(3)).toBe(3n);
    expect(parseInvoiceId(5n)).toBe(5n);
    expect(parseInvoiceId(-1n)).toBeNull();
    expect(parseInvoiceId(1.5)).toBeNull();
    expect(parseInvoiceId('abc')).toBeNull();
  });
});

describe('FundingService.loadListingState', () => {
  it('maps listing, invoice, pool and fee from the SDK', async () => {
    const client = fakeClient();
    const svc = new FundingService({ runtime: runtime(ALL_IDS, client), now: () => NOW_MS, retry: noSleep });
    const s = await svc.loadListingState(101);
    expect(client.marketplace.getListing).toHaveBeenCalledWith(101n);
    expect(s).toMatchObject({
      source: 'sdk',
      invoiceId: 'INV-101',
      debtorName: 'Debtor deadbeef…',
      targetFundingUsd: 48_000,
      currentFundingUsd: 20_000,
      remainingFundingUsd: 28_000,
      tenorDays: 60,
      platformFeePercentage: 0.75,
      isFullyFunded: false,
      isOpenForFunding: true,
      minContributionUsd: 100,
    });
    // (50k - 48k) / 48k * 365/60 * 100
    expect(s.annualYieldPercentage).toBeCloseTo(25.35, 2);
    expect(s.fundingDeadline).toBe(new Date((NOW_S + 7 * 86_400) * 1000).toISOString());
  });

  it('tolerates a missing pool and an unreadable fee', async () => {
    const client = fakeClient({
      getPool: jest.fn(async () => { throw new Error('Error(Contract, #30)'); }),
      getFeeBps: jest.fn(async () => { throw new Error('Error(Contract, #4)'); }),
      getInvoice: jest.fn(async () => ({ sme: SME, due_date: BigInt(NOW_S - 10), status: ['Funded'] })),
    });
    const svc = new FundingService({ runtime: runtime(ALL_IDS, client), now: () => NOW_MS, retry: noSleep });
    const s = await svc.loadListingState(101);
    expect(s.currentFundingUsd).toBe(12_000);
    expect(s.platformFeePercentage).toBeUndefined();
    expect(s.tenorDays).toBe(0);
    expect(s.annualYieldPercentage).toBe(0);
    expect(s.isFullyFunded).toBe(true);
    expect(s.debtorName).toBe('Undisclosed debtor');
  });

  it('surfaces pool failures other than not-found', async () => {
    const client = fakeClient({ getPool: jest.fn(async () => { throw new Error('Error(Contract, #4)'); }) });
    const svc = new FundingService({ runtime: runtime(ALL_IDS, client), now: () => NOW_MS, retry: noSleep });
    await expect(svc.loadListingState(101)).rejects.toMatchObject({ code: 'CONTRACT' });
  });

  it('turns a missing listing into NOT_FOUND and retries 429s', async () => {
    const getListing = jest.fn()
      .mockRejectedValueOnce(new Error('status code 429'))
      .mockRejectedValue(new Error('Error(Contract, #20)'));
    const svc = new FundingService({ runtime: runtime(ALL_IDS, fakeClient({ getListing })), retry: noSleep });
    await expect(svc.loadListingState(5)).rejects.toMatchObject({ code: 'NOT_FOUND' });
    expect(getListing).toHaveBeenCalledTimes(2);
  });

  it('never returns fixture data in production without config', async () => {
    const svc = new FundingService({ runtime: runtime({ NODE_ENV: 'production' }, null) });
    await expect(svc.loadListingState(1)).rejects.toMatchObject({ code: 'NOT_CONFIGURED' });
  });

  it('throws configuration errors and invalid ids', async () => {
    await expect(new FundingService({ runtime: runtime({ KORA_MARKETPLACE_CONTRACT_ID: C1 }, null) }).loadListingState(1))
      .rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    await expect(new FundingService({ runtime: runtime(ALL_IDS, fakeClient()) }).loadListingState(-3))
      .rejects.toMatchObject({ code: 'NOT_FOUND' });
  });

  it('returns the labelled dev fixture when nothing is configured outside production', async () => {
    const s = await new FundingService({ runtime: runtime({}, null) }).loadListingState(7);
    expect(s.source).toBe('dev-fixture');
    expect(s.listingId).toBe(7);
  });

  it('passes manifest warnings through', async () => {
    const env = { KORA_DEPLOYMENT_MANIFEST: manifest('testnet', '2024-01-01T00:00:00Z') };
    const s = await new FundingService({ runtime: runtime(env, fakeClient()), now: () => NOW_MS }).loadListingState(1);
    expect(s.notice).toMatch(/older than/);
  });
});

describe('RiskService.loadRiskBreakdown', () => {
  const suggestion = {
    score: 40,
    confidence: 0.82,
    formulaVersion: 'v2',
    breakdown: [
      { factorId: 'payment_history', score: 20, appliedWeight: 0.5, confidence: 1, missing: false, rationale: '13/14 on time' },
      { factorId: 'macro_trade', score: 60, appliedWeight: 0.5, confidence: 0.6, missing: false, rationale: 'stress 60' },
      { factorId: 'custom', score: 1, appliedWeight: 0, confidence: 0, missing: false, rationale: 'x' },
      { factorId: 'invoice_frequency', score: 50, appliedWeight: 0, confidence: 0, missing: true, rationale: 'none' },
    ],
    warnings: [],
    computedAt: NOW_S - 60,
  };

  it('combines invoice, registry and pipeline data', async () => {
    const fetchFn = jest.fn(async () => jsonResponse(200, suggestion));
    const gw = new GatewayClient({ baseUrl: 'https://gw', fetchFn });
    const svc = new RiskService({ runtime: runtime(ALL_IDS, fakeClient(), gw), now: () => NOW_MS, retry: noSleep });
    const b = await svc.loadRiskBreakdown('INV-2026-101');
    expect(fetchFn).toHaveBeenCalledWith(`https://gw/risk-pipeline/suggestions/${SME}`, expect.anything());
    expect(b).toMatchObject({
      source: 'sdk',
      invoiceId: 'INV-101',
      compositeScore: 42,
      tier: 'MEDIUM',
      onChainTier: 'A',
      status: 'ACTIVE',
      confidencePercentage: 82,
      verifierCount: 1,
      registry: { verified: true, smeRiskScore: 35, totalInvoices: 14, defaults: 1 },
    });
    expect(b.factors.map((f) => [f.category, f.weightPercentage])).toEqual([
      ['REPAYMENT_HISTORY', 50],
      ['INDUSTRY', 50],
      ['FINANCIAL', 0],
    ]);
    expect(b.pipelineNotice).toBeUndefined();
  });

  it('degrades gracefully when the pipeline is rate limited or empty', async () => {
    const limited = new GatewayClient({
      baseUrl: 'https://gw',
      fetchFn: async () => jsonResponse(429, {}, { 'Retry-After': '30' }),
    });
    const b = await new RiskService({ runtime: runtime(ALL_IDS, fakeClient(), limited), retry: noSleep }).loadRiskBreakdown('1');
    expect(b.pipelineNotice).toMatch(/Too many requests/);
    expect(b.confidencePercentage).toBeNull();
    expect(b.registry?.verified).toBe(true);

    const empty = new GatewayClient({ baseUrl: 'https://gw', fetchFn: async () => jsonResponse(404, {}) });
    const e = await new RiskService({ runtime: runtime(ALL_IDS, fakeClient(), empty) }).loadRiskBreakdown('1');
    expect(e.pipelineNotice).toMatch(/No risk-pipeline breakdown/);
  });

  it('handles unregistered SMEs, defaulted invoices and a missing gateway', async () => {
    const client = fakeClient({
      getSmeProfile: jest.fn(async () => { throw new Error('Error(Contract, #50)'); }),
      getInvoice: jest.fn(async () => ({ sme: SME, risk_score: 140, status: ['Defaulted'], risk_tier: ['C'] })),
    });
    const b = await new RiskService({ runtime: runtime(ALL_IDS, client), retry: noSleep }).loadRiskBreakdown('9');
    expect(b.registry).toBeNull();
    expect(b.compositeScore).toBe(100);
    expect(b.tier).toBe('CRITICAL');
    expect(b.status).toBe('DISPUTED');
    expect(b.verifierCount).toBe(0);
    expect(b.pipelineNotice).toMatch(/KORA_GATEWAY_URL/);
  });

  it('skips registry and pipeline when the invoice has no SME', async () => {
    const gw = new GatewayClient({ baseUrl: 'https://gw', fetchFn: jest.fn() });
    const client = fakeClient({ getInvoice: jest.fn(async () => ({ risk_score: 10 })) });
    const b = await new RiskService({ runtime: runtime(ALL_IDS, client, gw) }).loadRiskBreakdown('9');
    expect(b.registry).toBeNull();
    expect(client.riskRegistry.getSmeProfile).not.toHaveBeenCalled();
  });

  it('propagates registry failures that are not "not found"', async () => {
    const client = fakeClient({ getSmeProfile: jest.fn(async () => { throw new Error('Error(Contract, #4)'); }) });
    await expect(new RiskService({ runtime: runtime(ALL_IDS, client), retry: noSleep }).loadRiskBreakdown('9'))
      .rejects.toMatchObject({ code: 'CONTRACT' });
  });

  it('enforces live data, config validity and id format', async () => {
    await expect(new RiskService({ runtime: runtime({ KORA_REQUIRE_SDK: 'true' }, null) }).loadRiskBreakdown('1'))
      .rejects.toMatchObject({ code: 'NOT_CONFIGURED' });
    await expect(new RiskService({ runtime: runtime({ KORA_NETWORK: 'x', ...ALL_IDS }, null) }).loadRiskBreakdown('1'))
      .rejects.toMatchObject({ code: 'INVALID_CONFIG' });
    await expect(new RiskService({ runtime: runtime(ALL_IDS, fakeClient()) }).loadRiskBreakdown('bad id'))
      .rejects.toMatchObject({ code: 'NOT_FOUND' });
    const dev = await new RiskService({ runtime: runtime({}, null) }).loadRiskBreakdown('INV-1');
    expect(dev.source).toBe('dev-fixture');
  });
});

describe('funding and risk UI states', () => {
  const listing = {
    listingId: 101, invoiceId: 'INV-101', debtorName: 'Debtor deadbeef…', targetFundingUsd: 48_000,
    currentFundingUsd: 20_000, remainingFundingUsd: 28_000, minContributionUsd: 100, maxConcentrationCapUsd: 25_000,
    tenorDays: 60, annualYieldPercentage: 25.35, isFullyFunded: false, source: 'sdk' as const,
    platformFeePercentage: 0.75, notice: 'Manifest is old',
  };

  it('shows a loading state before SDK data arrives', () => {
    const html = renderToStaticMarkup(<FundingFlowLoader listingId={1} service={new FundingService({ runtime: runtime({}, null) })} />);
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('Loading listing');
    const risk = renderToStaticMarkup(<RiskScoreVisualization invoiceId="1" service={new RiskService({ runtime: runtime({}, null) })} />);
    expect(risk).toContain('Loading risk data');
  });

  it('renders SDK listing data with notices', () => {
    const html = renderToStaticMarkup(<SimulationProvider><FundingFlowLoader initialListing={listing} /></SimulationProvider>);
    expect(html).toContain('$28,000 USD');
    expect(html).toContain('25.35% p.a.');
    expect(html).toContain('Manifest is old');
    expect(html).not.toContain('Development sample data');
    const dev = renderToStaticMarkup(
      <SimulationProvider><FundingFlowBody listing={{ ...listing, source: 'dev-fixture', notice: undefined }} /></SimulationProvider>,
    );
    expect(dev).toContain('Development sample data');
  });

  it('renders inside a wallet provider without a connected wallet', () => {
    const adapter: WalletAdapter = {
      id: 'freighter', label: 'Freighter', installUrl: 'https://www.freighter.app/',
      isInstalled: async () => true, connect: async () => ({ address: SME, network: 'testnet' }),
      restore: async () => null, disconnect: async () => undefined, subscribe: () => () => undefined,
    };
    const manager = new WalletManager([adapter], 'testnet');
    const html = renderToStaticMarkup(<SimulationProvider><WalletProvider manager={manager}><FundingFlowBody listing={listing} /></WalletProvider></SimulationProvider>);
    expect(html).toContain('Preview Yield');
    manager.dispose();
  });

  it('renders rate-limit, error and empty states', () => {
    const limited = renderToStaticMarkup(
      <ErrorPanel error={mapSdkError(new GatewayRateLimitError(4_000))} onRetry={() => undefined} />,
    );
    expect(limited).toContain('data-error-code="RATE_LIMITED"');
    expect(limited).toContain('try again in 4 seconds');
    expect(limited).toContain('Try again');
    const stale = renderToStaticMarkup(<ErrorPanel error={mapSdkError(new Error('MissingValue'))} />);
    expect(stale).toContain('deployment manifest');
    expect(stale).not.toContain('Try again');
    const empty = renderToStaticMarkup(
      <ErrorPanel error={mapSdkError(new Error('Error(Contract, #20)'))} emptyTitle="Listing not available" />,
    );
    expect(empty).toContain('Listing not available');
    expect(renderToStaticMarkup(<ErrorPanel error={mapSdkError(new Error('Error(Contract, #20)'))} />)).toContain('Nothing to show');
  });

  it('renders registry-backed risk fields, missing confidence and pipeline notices', () => {
    const base = {
      invoiceId: 'INV-101', compositeScore: 42, tier: 'MEDIUM' as const, status: 'ACTIVE' as const,
      confidencePercentage: null, verifierCount: 1, factors: [], lastUpdated: new Date(NOW_MS).toISOString(),
      source: 'sdk' as const, onChainTier: 'A', pipelineNotice: 'Factor breakdown unavailable: slow down',
    };
    const html = renderToStaticMarkup(
      <RiskScoreCard
        breakdown={{ ...base, registry: { smeAddress: SME, verified: true, smeRiskScore: 35, totalInvoices: 14, defaults: 1 } }}
      />,
    );
    expect(html).toContain('Risk registry fields');
    expect(html).toContain('35/100');
    expect(html).toContain('14 / 1');
    expect(html).toContain('n/a');
    expect(html).toContain('Factor breakdown unavailable');
    const unregistered = renderToStaticMarkup(
      <RiskScoreVisualization breakdown={{ ...base, registry: null, status: 'DISPUTED', disputeNotice: 'Defaulted', confidencePercentage: 80, onChainTier: undefined }} />,
    );
    expect(unregistered).toContain('SME not registered');
    expect(unregistered).toContain('Defaulted');
    expect(unregistered).toContain('80%');
  });
});
