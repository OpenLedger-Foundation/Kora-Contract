/**
 * Reads Kora network, contract and gateway settings from the environment.
 *
 * Variables can be given as `KORA_*`, `VITE_KORA_*` or `REACT_APP_KORA_*`
 * (first match wins) so the same names work with any bundler. See the
 * "Environment variables" section of apps/web/README.md and `.env.example`.
 */
import { MAINNET, TESTNET } from '@kora-protocol/sdk';
import type { DeploymentManifest, KoraAddresses } from '@kora-protocol/sdk';
import { KoraUserError } from './errors';

export type KoraEnv = Record<string, string | undefined>;
export type KoraNetworkName = 'testnet' | 'mainnet';

export interface KoraWebConfig {
  network: KoraNetworkName;
  rpcUrl: string;
  networkPassphrase: string;
  /** Read-only API gateway (services/api/gateway); optional. */
  gatewayUrl?: string;
  addresses: KoraAddresses;
  /** Decimals of the settlement token (USDC on Stellar uses 7). */
  tokenDecimals: number;
  /** Investor-side UI policy, not on-chain values. */
  minContributionUsd: number;
  maxConcentrationCapUsd: number;
  /** Set when the manifest is older than KORA_MANIFEST_MAX_AGE_DAYS. */
  manifestWarning?: string;
}

export type KoraConfigResult =
  | { status: 'configured'; config: KoraWebConfig }
  | { status: 'unconfigured' }
  | { status: 'invalid'; error: KoraUserError };

const PREFIXES = ['KORA_', 'VITE_KORA_', 'REACT_APP_KORA_'];

/**
 * `KoraAddresses` field → env var suffix. Lower-casing the suffix without
 * `_CONTRACT_ID` gives the manifest key (`invoice_nft`, `financing_pool`, …).
 */
export const CONTRACT_ENV_KEYS: Record<keyof KoraAddresses, string> = {
  invoiceNft: 'INVOICE_NFT_CONTRACT_ID',
  marketplace: 'MARKETPLACE_CONTRACT_ID',
  financingPool: 'FINANCING_POOL_CONTRACT_ID',
  treasury: 'TREASURY_CONTRACT_ID',
  riskRegistry: 'RISK_REGISTRY_CONTRACT_ID',
  accessControl: 'ACCESS_CONTROL_CONTRACT_ID',
  priceOracle: 'PRICE_ORACLE_CONTRACT_ID',
  disputeEscrow: 'DISPUTE_ESCROW_CONTRACT_ID',
  fractionalizer: 'FRACTIONALIZER_CONTRACT_ID',
  secondaryMarket: 'SECONDARY_MARKET_CONTRACT_ID',
  governanceVoting: 'GOVERNANCE_VOTING_CONTRACT_ID',
  debtorAttestation: 'DEBTOR_ATTESTATION_CONTRACT_ID',
};

const REQUIRED_FIELDS: Array<keyof KoraAddresses> = [
  'invoiceNft',
  'marketplace',
  'financingPool',
  'treasury',
  'riskRegistry',
  'accessControl',
  'priceOracle',
];

export function defaultEnv(): KoraEnv {
  return typeof process !== 'undefined' && process.env ? (process.env as KoraEnv) : {};
}

export function readVar(env: KoraEnv, name: string): string | undefined {
  for (const prefix of PREFIXES) {
    const value = env[`${prefix}${name}`]?.trim();
    if (value) return value;
  }
  return undefined;
}

function readNumber(env: KoraEnv, name: string, fallback: number): number {
  const raw = readVar(env, name);
  if (raw === undefined) return fallback;
  const n = Number(raw);
  if (!Number.isFinite(n) || n < 0) {
    throw new KoraUserError('INVALID_CONFIG', `KORA_${name} must be a non-negative number.`);
  }
  return n;
}

/**
 * True when mock/fixture data must never be shown: production builds, or when
 * KORA_REQUIRE_SDK=true is set explicitly.
 */
export function requiresLiveData(env: KoraEnv = defaultEnv()): boolean {
  return env.NODE_ENV === 'production' || readVar(env, 'REQUIRE_SDK') === 'true';
}

function invalid(message: string): KoraConfigResult {
  return { status: 'invalid', error: new KoraUserError('INVALID_CONFIG', message) };
}

/**
 * Builds the web config. Returns `unconfigured` when no contract settings are
 * present at all, and `invalid` (never a silent fallback) when settings are
 * present but incomplete or inconsistent.
 */
export function readKoraConfig(env: KoraEnv = defaultEnv(), now: number = Date.now()): KoraConfigResult {
  const manifestRaw = readVar(env, 'DEPLOYMENT_MANIFEST');
  const explicit: Partial<KoraAddresses> = {};
  for (const [field, suffix] of Object.entries(CONTRACT_ENV_KEYS) as Array<[keyof KoraAddresses, string]>) {
    const value = readVar(env, suffix);
    if (value) explicit[field] = value;
  }
  if (!manifestRaw && Object.keys(explicit).length === 0) return { status: 'unconfigured' };

  const networkRaw = (readVar(env, 'NETWORK') ?? 'testnet').toLowerCase();
  if (networkRaw !== 'testnet' && networkRaw !== 'mainnet') {
    return invalid(`KORA_NETWORK must be "testnet" or "mainnet" (got "${networkRaw}").`);
  }
  const network = networkRaw as KoraNetworkName;
  const base = network === 'mainnet' ? MAINNET : TESTNET;

  let addresses: Partial<KoraAddresses> = {};
  let manifestWarning: string | undefined;
  if (manifestRaw) {
    let manifest: DeploymentManifest;
    try {
      manifest = JSON.parse(manifestRaw) as DeploymentManifest;
    } catch {
      return invalid('KORA_DEPLOYMENT_MANIFEST is not valid JSON.');
    }
    if (manifest.network && manifest.network.toLowerCase() !== network) {
      return invalid(
        `Deployment manifest is for "${manifest.network}" but KORA_NETWORK is "${network}". ` +
          'Use the manifest for the selected network.',
      );
    }
    for (const [field, suffix] of Object.entries(CONTRACT_ENV_KEYS) as Array<[keyof KoraAddresses, string]>) {
      // INVOICE_NFT_CONTRACT_ID → invoice_nft (the key scripts/deploy.sh writes)
      const entry = manifest.contracts?.[suffix.replace(/_CONTRACT_ID$/, '').toLowerCase()];
      if (entry?.address) addresses[field] = entry.address;
    }
    let maxAgeDays: number;
    try {
      maxAgeDays = readNumber(env, 'MANIFEST_MAX_AGE_DAYS', 90);
    } catch (error) {
      return { status: 'invalid', error: error as KoraUserError };
    }
    const deployedAt = Date.parse(manifest.deployed_at);
    if (Number.isFinite(deployedAt) && maxAgeDays > 0 && now - deployedAt > maxAgeDays * 86_400_000) {
      manifestWarning =
        `Contract addresses come from a deployment manifest dated ${manifest.deployed_at}, ` +
        `older than ${maxAgeDays} days. Data may be stale if contracts were redeployed.`;
    }
  }
  addresses = { ...addresses, ...explicit };

  const missing = REQUIRED_FIELDS.filter((f) => !addresses[f]);
  if (missing.length > 0) {
    return invalid(`Missing contract ID(s): ${missing.map((f) => `KORA_${CONTRACT_ENV_KEYS[f]}`).join(', ')}.`);
  }

  try {
    return {
      status: 'configured',
      config: {
        network,
        rpcUrl: readVar(env, 'RPC_URL') ?? base.rpcUrl,
        networkPassphrase: base.networkPassphrase,
        gatewayUrl: readVar(env, 'GATEWAY_URL')?.replace(/\/+$/, ''),
        addresses: addresses as KoraAddresses,
        tokenDecimals: readNumber(env, 'TOKEN_DECIMALS', 7),
        minContributionUsd: readNumber(env, 'MIN_CONTRIBUTION_USD', 100),
        maxConcentrationCapUsd: readNumber(env, 'MAX_CONCENTRATION_CAP_USD', 25_000),
        manifestWarning,
      },
    };
  } catch (error) {
    return { status: 'invalid', error: error as KoraUserError };
  }
}
