/**
 * Creates the `KoraClient` used by the funding and risk services.
 *
 * Services depend on the narrow `KoraReadClient` interface (the read methods
 * they call), which `KoraClient` from `@kora-protocol/sdk` satisfies. Tests
 * inject fakes through the same interface.
 */
import { KoraClient } from '@kora-protocol/sdk';
import type { Invoice, Listing, Pool, SmeProfile } from '@kora-protocol/sdk';
import { KoraConfigResult, KoraEnv, KoraWebConfig, defaultEnv, readKoraConfig, requiresLiveData } from './config';
import { GatewayClient } from './gatewayClient';

export interface KoraReadClient {
  invoiceNft: { getInvoice(invoiceId: bigint): Promise<Invoice> };
  marketplace: { getListing(invoiceId: bigint): Promise<Listing>; getFeeBps(): Promise<number> };
  financingPool: { getPool(invoiceId: bigint): Promise<Pool> };
  riskRegistry: { getSmeProfile(sme: string): Promise<SmeProfile> };
}

export function createKoraClient(config: KoraWebConfig): KoraReadClient {
  return new KoraClient(config.addresses, { rpcUrl: config.rpcUrl, networkPassphrase: config.networkPassphrase });
}

/** Everything a service needs to decide between live SDK reads and dev fixtures. */
export interface KoraRuntime {
  config: KoraConfigResult;
  client: KoraReadClient | null;
  gateway: GatewayClient | null;
  /** When true, fixture data must never be returned. */
  liveDataRequired: boolean;
}

export function createKoraRuntime(env: KoraEnv = defaultEnv(), now: number = Date.now()): KoraRuntime {
  const config = readKoraConfig(env, now);
  const configured = config.status === 'configured' ? config.config : null;
  return {
    config,
    client: configured ? createKoraClient(configured) : null,
    gateway: configured?.gatewayUrl ? new GatewayClient({ baseUrl: configured.gatewayUrl }) : null,
    // Once any Kora env is set we never fall back to fixtures.
    liveDataRequired: requiresLiveData(env) || config.status !== 'unconfigured',
  };
}

let cached: KoraRuntime | null = null;

/** Lazily-created runtime from `process.env`, shared by the default service singletons. */
export function getKoraRuntime(): KoraRuntime {
  if (!cached) cached = createKoraRuntime();
  return cached;
}

/** Test hook: replace (or clear with `null`) the shared runtime. */
export function setKoraRuntime(runtime: KoraRuntime | null): void {
  cached = runtime;
}
