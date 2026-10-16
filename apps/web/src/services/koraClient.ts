import {
  KoraClient,
  KoraAddresses,
  NetworkConfig,
  TESTNET,
} from '@kora-protocol/sdk';

/**
 * Options for resilient network calls (RPC latency & Gateway 429 rate limiting)
 */
export interface RetryOptions {
  maxRetries?: number;
  baseDelayMs?: number;
  maxDelayMs?: number;
  backoffFactor?: number;
}

/**
 * Resolves Kora smart contract addresses from environment variables.
 */
export function getKoraAddresses(): KoraAddresses | null {
  const invoiceNft =
    process.env.NEXT_PUBLIC_INVOICE_NFT_CONTRACT_ID ||
    process.env.INVOICE_NFT_CONTRACT_ID;
  const marketplace =
    process.env.NEXT_PUBLIC_MARKETPLACE_CONTRACT_ID ||
    process.env.MARKETPLACE_CONTRACT_ID;
  const financingPool =
    process.env.NEXT_PUBLIC_FINANCING_POOL_CONTRACT_ID ||
    process.env.FINANCING_POOL_CONTRACT_ID;
  const treasury =
    process.env.NEXT_PUBLIC_TREASURY_CONTRACT_ID ||
    process.env.TREASURY_CONTRACT_ID;
  const riskRegistry =
    process.env.NEXT_PUBLIC_RISK_REGISTRY_CONTRACT_ID ||
    process.env.RISK_REGISTRY_CONTRACT_ID;
  const accessControl =
    process.env.NEXT_PUBLIC_ACCESS_CONTROL_CONTRACT_ID ||
    process.env.ACCESS_CONTROL_CONTRACT_ID;
  const priceOracle =
    process.env.NEXT_PUBLIC_PRICE_ORACLE_CONTRACT_ID ||
    process.env.PRICE_ORACLE_CONTRACT_ID;

  if (
    !invoiceNft ||
    !marketplace ||
    !financingPool ||
    !treasury ||
    !riskRegistry ||
    !accessControl ||
    !priceOracle
  ) {
    return null;
  }

  return {
    invoiceNft,
    marketplace,
    financingPool,
    treasury,
    riskRegistry,
    accessControl,
    priceOracle,
  };
}

/**
 * Checks whether live Soroban contract IDs are configured in the environment.
 */
export function isKoraConfigured(): boolean {
  return getKoraAddresses() !== null;
}

/**
 * Resolves Soroban network configuration from environment variables.
 */
export function getKoraNetworkConfig(): NetworkConfig {
  const rpcUrl =
    process.env.NEXT_PUBLIC_SOROBAN_RPC_URL ||
    process.env.SOROBAN_RPC_URL ||
    TESTNET.rpcUrl;
  const networkPassphrase =
    process.env.NEXT_PUBLIC_STELLAR_NETWORK_PASSPHRASE ||
    process.env.STELLAR_NETWORK_PASSPHRASE ||
    TESTNET.networkPassphrase;

  return {
    rpcUrl,
    networkPassphrase,
  };
}

/**
 * Returns the off-chain gateway URL if defined.
 */
export function getGatewayUrl(): string | null {
  return (
    process.env.NEXT_PUBLIC_GATEWAY_URL ||
    process.env.GATEWAY_URL ||
    null
  );
}

let cachedClient: KoraClient | null = null;

/**
 * Returns a configured KoraClient instance, or null if contract environment variables are not set.
 */
export function getKoraClient(): KoraClient | null {
  if (cachedClient) {
    return cachedClient;
  }

  const addresses = getKoraAddresses();
  if (!addresses) {
    return null;
  }

  const network = getKoraNetworkConfig();
  cachedClient = new KoraClient(addresses, network);
  return cachedClient;
}

/**
 * Resets cached KoraClient instance (useful during tests or network config transitions).
 */
export function resetKoraClient(): void {
  cachedClient = null;
}

/**
 * Helper to execute asynchronous network calls with exponential backoff and jitter,
 * specially crafted to absorb HTTP 429 (rate limits) and RPC transient latency spikes.
 */
export async function withRetryAndBackoff<T>(
  operation: () => Promise<T>,
  options: RetryOptions = {}
): Promise<T> {
  const maxRetries = options.maxRetries ?? 3;
  const baseDelayMs = options.baseDelayMs ?? 300;
  const maxDelayMs = options.maxDelayMs ?? 3000;
  const backoffFactor = options.backoffFactor ?? 2;

  let attempt = 0;

  while (true) {
    try {
      return await operation();
    } catch (err: any) {
      attempt++;
      if (attempt > maxRetries) {
        throw err;
      }

      // Check if response indicates HTTP 429 / rate limiting
      let waitMs = baseDelayMs * Math.pow(backoffFactor, attempt - 1);
      const isRateLimit =
        err?.status === 429 ||
        err?.statusCode === 429 ||
        err?.message?.includes('429') ||
        err?.message?.toLowerCase()?.includes('rate limit');

      if (isRateLimit && err?.headers?.get) {
        const retryAfterHeader = err.headers.get('retry-after');
        if (retryAfterHeader) {
          const seconds = parseInt(retryAfterHeader, 10);
          if (!isNaN(seconds)) {
            waitMs = Math.max(waitMs, seconds * 1000);
          }
        }
      }

      // Add jitter (up to 20%)
      const jitter = Math.random() * 0.2 * waitMs;
      const finalWaitMs = Math.min(waitMs + jitter, maxDelayMs);

      await new Promise((resolve) => setTimeout(resolve, finalWaitMs));
    }
  }
}
