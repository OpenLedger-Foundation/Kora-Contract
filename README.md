<div align="center">

# Kora Protocol

**On-chain Invoice Financing for African Trade Finance**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Built on Stellar](https://img.shields.io/badge/Built%20on-Stellar%20Soroban-7B2FBE)](https://stellar.org)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange)](https://www.rust-lang.org)

*Tokenize unpaid invoices. Unlock instant liquidity. Power African trade.*

</div>

---

## Overview

Kora is a decentralized invoice financing protocol built on [Stellar Soroban](https://soroban.stellar.org). It enables small and medium enterprises (SMEs) — particularly across African markets — to tokenize their unpaid invoices as NFTs and sell them at a discount to liquidity providers, receiving immediate working capital without waiting 30–90 days for invoice settlement.

The protocol is designed for:

- **African trade finance infrastructure** — addressing the $81B+ SME financing gap on the continent
- **Institutional DeFi** — production-grade contract architecture with formal access control and risk scoring
- **Real-world asset (RWA) tokenization** — invoices as on-chain NFTs with IPFS-backed metadata
- **Scalable fintech rails** — modular contracts that can be extended for factoring, supply chain finance, and trade credit

---

## How It Works

```
SME                    Marketplace              Investors
 │                         │                       │
 │── mint_invoice() ──────►│                       │
 │                         │── list_invoice() ────►│
 │                         │                       │── fund_invoice()
 │                         │◄──────────────────────│
 │◄── funds released ──────│                       │
 │                         │                       │
 │── repay() ─────────────►│                       │
 │                         │── distribute yield ──►│
```

1. **SME mints an invoice NFT** — invoice metadata (amount, debtor, due date, IPFS CID) is stored on-chain. Sensitive debtor PII is hashed; full details live on IPFS.
2. **Invoice is listed on the marketplace** — SME sets an asking price (discounted from face value) and a funding deadline.
3. **Investors fund the invoice** — partial funding is supported. The marketplace collects a protocol fee on each contribution.
4. **Funds are released to the SME** — once fully funded, the financing pool releases net proceeds to the SME.
5. **SME repays the face value** — on or before the due date.
6. **Investors receive principal + yield** — the spread between the discounted price paid and the face value repaid is the investor's return.
7. **Defaults are handled on-chain** — if the SME fails to repay, the admin can mark the invoice as defaulted and distribute any partial recovery to investors.

---

## Contract Architecture

| Contract | Responsibility |
|---|---|
| `invoice_nft` | Mints and manages invoice NFTs. Owns the canonical invoice state machine. |
| `marketplace` | Lists invoices, accepts investor funding, collects fees, triggers pool release. |
| `financing_pool` | Holds investor funds, tracks positions, distributes repayments and yield. |
| `treasury` | Accumulates protocol fees. Admin-controlled withdrawal. |
| `risk_registry` | Verifier-managed SME and debtor risk scoring. |
| `access_control` | Protocol-wide pause/unpause, role management, admin transfer. |
| `shared` | Common types, errors, events, and validation utilities. |

See [ARCHITECTURE.md](docs/ARCHITECTURE.md) for a detailed breakdown.

---

## Project Structure

```
Kora-Contract/
├── Cargo.toml                    # Workspace root
├── Makefile                      # Build, test, deploy targets
├── contracts/
│   ├── shared/                   # Shared types, errors, events, validation
│   ├── invoice_nft/              # Invoice NFT contract
│   ├── marketplace/              # Marketplace contract
│   ├── financing_pool/           # Financing pool contract
│   ├── treasury/                 # Treasury/fee contract
│   ├── risk_registry/            # Risk registry contract
│   ├── access_control/           # Access control contract
│   └── tests/                    # Integration test suite
├── scripts/
│   ├── deploy.sh                 # Full deployment script
│   └── interact.sh               # Shell helpers for contract interaction
├── deployments/                  # Generated deployment manifests (gitignored)
└── docs/
    ├── ARCHITECTURE.md
    ├── CONTRACTS.md
    └── SECURITY.md
```

---

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) 1.75+
- [stellar CLI](https://developers.stellar.org/docs/tools/stellar-cli) (latest)
- `wasm32-unknown-unknown` target

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add WASM target
rustup target add wasm32-unknown-unknown

# Install stellar CLI
cargo install stellar-cli --locked
```

### Build

```bash
# Clone the repository
git clone https://github.com/your-org/kora-contract.git
cd kora-contract

# Build all contracts
make build

# Build and optimize WASM binaries
make build-optimized
```

### Test

```bash
# Run all unit and integration tests
make test

# Run with output
make test-verbose
```

### Lint

```bash
make fmt    # Format code
make lint   # Run clippy (warnings as errors)
make check  # Type-check without building
```

---

## Deployment

### Testnet

```bash
export DEPLOYER_SECRET="your-stellar-secret-key"
make deploy-testnet
```

This will:
1. Build and optimize all WASM binaries
2. Deploy each contract to Stellar testnet
3. Initialize all contracts with correct cross-contract references
4. Write a deployment manifest to `deployments/testnet.json`

### Mainnet

```bash
export DEPLOYER_SECRET="your-stellar-secret-key"
make deploy-mainnet
```

Mainnet deployment requires manual confirmation at the prompt.

### Deployment Environment Variables

All constructor parameters accept environment variable overrides with sensible defaults, allowing the same script to be used for testnet, mainnet, or custom forks without editing it.

| Variable | Default | Description |
|---|---|---|
| `TREASURY_FEE_BPS` | `50` | Treasury protocol fee (50 bps = 0.5%) |
| `MARKETPLACE_FEE_BPS` | `50` | Marketplace protocol fee (50 bps = 0.5%) |
| `MARKETPLACE_REFERRER_BPS` | `0` | Fraction of marketplace fee paid to referrer (0 = off) |
| `LATE_PENALTY_BPS` | `200` | Late-repayment penalty (200 bps = 2%) |
| `MAX_POSITION_BPS` | `5000` | Max per-investor pool share (5000 bps = 50%) |
| `ORACLE_BASE_CURRENCY` | `USDC` | Price oracle base quote currency symbol |

**Example — reduced fees for testnet fork:**
```bash
export DEPLOYER_SECRET="your-stellar-secret-key"
export TREASURY_FEE_BPS=10
export MARKETPLACE_FEE_BPS=10
export LATE_PENALTY_BPS=100
export MAX_POSITION_BPS=3000
make deploy-testnet
```

**Example — production mainnet with referral incentives enabled:**
```bash
export DEPLOYER_SECRET="your-stellar-secret-key"
export MARKETPLACE_REFERRER_BPS=2000   # 20% of fee to referrer
make deploy-mainnet
```

---

## Example Interactions

After deployment, load the interaction helpers:

```bash
source scripts/interact.sh testnet
```

**Register a verifier and SME:**
```bash
kora_add_verifier "$ADMIN_ADDRESS" "$VERIFIER_ADDRESS"
kora_register_sme "$VERIFIER_ADDRESS" "$SME_ADDRESS" 35
```

**Mint an invoice:**
```bash
kora_mint_invoice \
  "$SME_ADDRESS" \
  "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890" \
  10000000000 \
  "USDC" \
  1780000000 \
  "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi" \
  30
```

**List the invoice:**
```bash
kora_list_invoice "$SME_ADDRESS" 1 9500000000 10000000000 "$USDC_TOKEN" 1750000000
```

**Fund the invoice (investor):**
```bash
kora_fund_invoice "$INVESTOR_ADDRESS" 1 4750000000
```

**Repay:**
```bash
kora_repay "$SME_ADDRESS" 1 "$USDC_TOKEN" 10000000000
```

**Query state:**
```bash
kora_get_invoice 1
kora_get_pool 1
kora_get_sme_profile "$SME_ADDRESS"
```

---

## Invoice NFT Data Model

```rust
pub struct Invoice {
    pub id: u64,
    pub sme: Address,
    pub debtor_hash: Bytes,    // SHA-256 of debtor info — PII stays off-chain
    pub amount: i128,          // Face value in stroops (7 decimal places)
    pub currency: Symbol,      // e.g. USDC, EURC
    pub due_date: u64,         // Unix timestamp
    pub ipfs_cid: String,      // IPFS CID of full invoice metadata
    pub risk_score: u32,       // 0–100 (assigned by verifier)
    pub risk_tier: RiskTier,   // AAA / AA / A / B / C
    pub status: InvoiceStatus, // Created → Listed → Funded → Repaid | Defaulted
    pub created_at: u64,
    pub funded_at: Option<u64>,
    pub repaid_at: Option<u64>,
}
```

---

## Risk Tiers

| Tier | Score Range | Description |
|------|-------------|-------------|
| AAA  | 0–20        | Lowest risk. Blue-chip debtors, strong SME history. |
| AA   | 21–40       | Low risk. Established SME, reliable debtor. |
| A    | 41–60       | Moderate risk. Standard trade finance profile. |
| B    | 61–80       | Elevated risk. Newer SME or less-known debtor. |
| C    | 81–100      | High risk. Requires higher yield to attract investors. |

---

## Protocol Fees

| Fee | Default | Description |
|-----|---------|-------------|
| Marketplace fee | 0.5% (50 bps) | Charged on each investor contribution |
| Late penalty | 2% (200 bps) | Applied to late repayments |

All fees are configurable by the admin within safe bounds (0–100%).

---

## Security

- All state-mutating functions require `require_auth()` on the relevant signer
- Input validation on all public entry points (amounts, timestamps, scores, strings)
- Safe arithmetic via `checked_mul` / `checked_div` — no silent overflows
- Protocol-wide pause mechanism via `access_control`
- Role-based access: Admin, Operator, Verifier
- Debtor PII never stored on-chain — only a SHA-256 hash
- See [docs/SECURITY.md](docs/SECURITY.md) for the full security model

---

## Contributing

We welcome contributions from the community. Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

---

## License

MIT — see [LICENSE](LICENSE).

---

## Acknowledgements

Built with [Stellar Soroban](https://soroban.stellar.org). Inspired by the real-world invoice financing gap facing African SMEs and the potential of blockchain infrastructure to close it.
