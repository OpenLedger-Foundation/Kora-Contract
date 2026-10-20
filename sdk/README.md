# @kora-protocol/sdk

TypeScript client bindings for the [Kora Protocol](https://github.com/your-org/kora-contract) — on-chain invoice financing on Stellar Soroban.

## Install

```bash
npm install @kora-protocol/sdk
```

## Quick start — mint → list → fund → repay on testnet

```ts
import { KoraClient } from "@kora-protocol/sdk";
import { Keypair } from "@stellar/stellar-sdk";

// Load your deployment addresses (from deployments/testnet.json)
const addresses = {
  invoiceNft:    "C...",
  marketplace:   "C...",
  financingPool: "C...",
  treasury:      "C...",
  riskRegistry:  "C...",
  accessControl: "C...",
  priceOracle:   "C...",
};

const kora = new KoraClient(addresses, KoraClient.TESTNET);

const sme      = Keypair.fromSecret("S...");
const investor = Keypair.fromSecret("S...");
const USDC     = "C..."; // testnet USDC contract address

// 1. Mint an invoice NFT
const invoiceId = await kora.invoiceNft.mintInvoice(
  sme,
  Buffer.alloc(32, 0xab),          // SHA-256 of debtor info
  10_000_000_000n,                 // 10,000 USDC (7 decimals)
  "USDC",
  BigInt(Math.floor(Date.now() / 1000) + 86_400 * 60), // due in 60 days
  "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
  30,                              // risk score → RiskTier::AA
);
console.log("Minted invoice", invoiceId);

// 2. List the invoice on the marketplace (5% discount)
await kora.marketplace.listInvoice(
  sme,
  invoiceId,
  9_500_000_000n,  // asking price
  10_000_000_000n, // face value
  USDC,
  BigInt(Math.floor(Date.now() / 1000) + 86_400 * 30), // 30-day funding window
);

// 3. Investor funds the invoice
await kora.marketplace.fundInvoice(investor, invoiceId, 9_500_000_000n);

// 4. SME repays the face value
await kora.financingPool.repay(sme, invoiceId, USDC, 10_000_000_000n);

// 5. Read final state
const invoice = await kora.invoiceNft.getInvoice(invoiceId);
console.log("Invoice status:", invoice.status); // "Repaid"

const collected = await kora.treasury.getCollected(USDC);
console.log("Treasury collected fees:", collected);
```

## Recovering an interrupted transaction

If the RPC connection drops after submission, a write can reject with
`TransactionOutcomeUnknownError`. Its `hash` identifies the transaction; the
error does not imply that the transaction failed. Reconcile after connectivity
returns instead of resubmitting:

```ts
import { TransactionOutcomeUnknownError } from "@kora-protocol/sdk";

try {
  await kora.marketplace.fundInvoice(investor, invoiceId, amount);
} catch (error) {
  if (!(error instanceof TransactionOutcomeUnknownError)) throw error;

  const status = await kora.reconcileTransaction(error.hash);
  if (status.status === "success") {
    // The ledger confirms the transaction; refresh affected contract state.
  } else if (status.status === "failed") {
    // The ledger confirms failure; the user may choose whether to retry.
  } else {
    // NOT_FOUND is inconclusive. Keep the transaction unresolved and check again.
  }
}
```

`KoraClient.reconcileTransaction` queries Soroban RPC transaction status. A
successful or failed result is authoritative; `not_found` and RPC errors are
not evidence of failure. Applications should retain the hash and unresolved
transaction state across wallet/network reconnection, retry reconciliation
when RPC access returns, and only offer a new submission after a confirmed
failure or an explicit user decision. Wallet connection state is owned by the
integrating application; this SDK API provides the on-chain status needed to
recover its transaction tracker.

## Contract clients

| Client | Contract |
|---|---|
| `kora.invoiceNft` | `invoice_nft` — mint, get, status transitions |
| `kora.marketplace` | `marketplace` — list, fund, cancel, tier fees |
| `kora.financingPool` | `financing_pool` — repay, pools, positions |
| `kora.treasury` | `treasury` — balances, collected fees, withdraw |
| `kora.riskRegistry` | `risk_registry` — SME/verifier management |
| `kora.accessControl` | `access_control` — pause/unpause, roles |
| `kora.priceOracle` | `price_oracle` — asset prices |
| `kora.parameterRegistry` | `parameter_registry` — governable parameter storage |

## Parameter registry keys

The `parameter_registry` contract stores every governable protocol parameter under
a typed key. Treasury-specific keys:

| Key | Default | Min | Max | Description |
|-----|---------|-----|-----|-------------|
| `TreasuryFeeBps` | 50 | 0 | 10 000 | Protocol fee in basis points |
| `ReserveAllocationBps` | 0 | 0 | 10 000 | Portion of fees routed to loss reserve |

Consuming contracts read from the registry at runtime. Governance updates go
through the registry's multisig proposal flow (`propose_param_update` →
`approve_param_update` → `execute_param_update`).

## Public API (third-party integrators)

The SDK also ships a typed client for the **rate-limited public API tier**
(`services/public-api/`), which sits in front of the core API gateway and is
intended for external integrators (factoring partners, analytics providers).
Unlike the internal service-to-service APIs, every request is authenticated
with an API key, metered per key, and rate limited according to the key's tier.

### Issuing a key

Keys are issued self-service or admin-approved and carry a configurable tier
that determines the rate limit and quota:

```ts
import { PublicApiClient } from "@kora-protocol/sdk";

// Self-service issuance (returns the plaintext key exactly once).
const { key, tier } = await PublicApiClient.issueKey({
  label: "acme-factoring",
  tier: "partner", // "free" | "partner" | "enterprise"
});

// Admin-approved issuance with a custom tier limit.
const approved = await PublicApiClient.issueKey({
  label: "acme-analytics",
  tier: "enterprise",
  requestsPerMinute: 6_000,
  approvedBy: "admin@kora.example",
});
```

### Calling the versioned API

All public endpoints are versioned under `/v1/...` so the surface can evolve
without breaking existing integrations:

```ts
const api = new PublicApiClient({
  baseUrl: "https://api.kora.example",
  apiKey: key,
});

const invoice = await api.v1.invoices.get(invoiceId);
const page    = await api.v1.invoices.list({ status: "Listed", limit: 50 });
```

### Rate limits and metering

Rate limiting uses a per-key token bucket (sliding-window refill) sized by the
key's tier. Every request is metered and written to a queryable usage store for
reporting:

```ts
const usage = await api.v1.usage.get({ from: "2024-01-01", to: "2024-02-01" });
console.log(usage.requests, usage.rateLimited, usage.remaining);
```

When a key exceeds its tier limit the API responds with `429 Too Many Requests`
and `Retry-After` / `X-RateLimit-*` headers.

### Revocation

Revoked keys are rejected immediately — there is no stale-token grace period.
Revocation takes effect on the next request:

```ts
await PublicApiClient.revokeKey(key); // effective immediately
```

### Versioning & deprecation policy

- The public surface is versioned by path prefix (`/v1/...`).
- New, non-breaking fields and endpoints are added within the current version.
- Breaking changes ship under a new version prefix (`/v2/...`); the previous
  version remains available for a documented deprecation window.
- Deprecated endpoints return a `Deprecation` header and are announced in the
  changelog before removal.

> Out of scope: billing/monetization of API access.

## Extended Contract Clients

In addition to the core 7 contracts, the SDK provides typed clients for extended protocol contracts:

- **`DisputeEscrowClient`** (`contracts/dispute_escrow`): Manage invoice repayment disputes, adjudications, and timelocks.
- **`FractionalizerClient`** (`contracts/fractionalizer`): Wrap financing-pool positions into fungible fixed-supply shares, transfer shares, settle pool payouts, and redeem shares.
- **`SecondaryMarketClient`** (`contracts/secondary_market`): List investor positions for secondary sale, purchase listed positions, and manage secondary trading fees.
- **`GovernanceVotingClient`** (`contracts/governance_voting`): Create and vote on governance proposals (Standard and Quadratic voting), manage vote delegation, configure vesting weight sources, and execute proposals.

These clients are available as standalone exports or optionally attached to `KoraClient` by specifying their addresses in `KoraAddresses`. Error mappings for all four contracts are exported via `DISPUTE_ESCROW_ERROR_MESSAGES`, `FRACTIONALIZER_ERROR_MESSAGES`, `SECONDARY_MARKET_ERROR_MESSAGES`, `GOVERNANCE_VOTING_ERROR_MESSAGES`, and `getErrorMessage`.