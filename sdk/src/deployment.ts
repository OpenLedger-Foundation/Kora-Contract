import { readFileSync } from "fs";
import type { KoraAddresses } from "./KoraClient";

/**
 * Shape written by `scripts/deploy.sh` to `deployments/<network>.json`:
 * a nested, snake_case map of contract name -> { address, wasm_hash }.
 */
export interface DeploymentManifest {
  network: string;
  deployed_at: string;
  admin: string;
  parameters?: Record<string, unknown>;
  contracts: Record<string, { address: string; wasm_hash: string }>;
}

/**
 * Maps a `KoraAddresses` field name to the snake_case contract key used in
 * the deployment manifest's `contracts` map.
 */
const MANIFEST_CONTRACT_KEYS: Record<keyof KoraAddresses, string> = {
  invoiceNft: "invoice_nft",
  marketplace: "marketplace",
  financingPool: "financing_pool",
  treasury: "treasury",
  riskRegistry: "risk_registry",
  accessControl: "access_control",
  priceOracle: "price_oracle",
  disputeEscrow: "dispute_escrow",
  fractionalizer: "fractionalizer",
  secondaryMarket: "secondary_market",
  governanceVoting: "governance_voting",
};

const REQUIRED_CONTRACTS: (keyof KoraAddresses)[] = [
  "invoiceNft",
  "marketplace",
  "financingPool",
  "treasury",
  "riskRegistry",
  "accessControl",
  "priceOracle",
];

/**
 * Transforms a parsed deployment manifest (the shape `scripts/deploy.sh`
 * writes to `deployments/<network>.json`) into the flat, camelCase
 * `KoraAddresses` shape `KoraClient` expects.
 *
 * Throws if any contract required by `KoraAddresses` is missing from the
 * manifest, naming exactly which one(s) — instead of leaving callers to
 * discover a missing/undefined address at contract-call time.
 */
export function manifestToAddresses(manifest: DeploymentManifest): KoraAddresses {
  const missing: string[] = [];
  const addresses = {} as KoraAddresses;

  for (const field of Object.keys(MANIFEST_CONTRACT_KEYS) as (keyof KoraAddresses)[]) {
    const manifestKey = MANIFEST_CONTRACT_KEYS[field];
    const entry = manifest.contracts?.[manifestKey];
    if (!entry || !entry.address) {
      if (REQUIRED_CONTRACTS.includes(field)) {
        missing.push(manifestKey);
      }
      continue;
    }
    addresses[field] = entry.address;
  }

  if (missing.length > 0) {
    throw new Error(
      `Deployment manifest is missing required contract(s): ${missing.join(", ")}. ` +
        `Re-run scripts/deploy.sh or update the manifest before constructing KoraAddresses.`
    );
  }

  return addresses;
}

/**
 * Reads and parses a deployment manifest JSON file (as produced by
 * `scripts/deploy.sh`, e.g. `deployments/testnet.json`) and returns a
 * validated `KoraAddresses` object ready to pass to `new KoraClient(...)`.
 */
export function loadKoraAddresses(manifestPath: string): KoraAddresses {
  const raw = readFileSync(manifestPath, "utf-8");
  const manifest = JSON.parse(raw) as DeploymentManifest;
  return manifestToAddresses(manifest);
}
