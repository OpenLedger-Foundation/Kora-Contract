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
