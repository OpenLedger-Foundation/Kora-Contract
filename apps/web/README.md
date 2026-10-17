# Kora Protocol Web Application (`apps/web`)

Responsive mobile-first user interface and front-end integration layer for Kora Protocol.

## Implemented Features & Issues Addressed

### 1. Mobile-First Responsive Layout (`#781`)
- **Core Flows**: Mobile-first components built for invoice submission, marketplace browsing, fractional funding, and SME repayment.
- **Viewport Matrix**: Fully responsive from narrow mobile devices (320px width) up through desktop viewports.
- **Components**: `Header`, `Navigation`, `MobileDrawer`, `Layout`, `InvoiceSubmissionFlow`, `MarketplaceBrowsingFlow`, `FundingFlow`, `RepaymentFlow`.

### 2. In-App Notification Center (`#782`)
- **Features**: Bell icon with unread badge counter, slide-over notification panel, category filtering (Funding Milestones, Due Date Warnings, System Alerts, Governance), mark as read / mark all as read, and user preferences management.
- **Components**: `NotificationCenter`, `NotificationPanel`, `NotificationContext`, `notificationService`.

### 3. Currency & Locale Switcher (`#784`)
- **African Market Support**: Local currency converter supporting USD (USDC), NGN (Naira), KES (Shilling), ZAR (Rand), GHS (Cedi).
- **Canonical Display**: Always presents canonical on-chain settlement amounts (USDC) alongside converted local amounts.
- **Stale Data Indicator**: Automatically flags FX rates older than 1 hour as stale.
- **i18n Support**: Locale context with English (`en`), Swahili (`sw`), French (`fr`), Yoruba (`yo`), and Hausa (`ha`).
- **Components**: `CurrencySwitcher`, `LocaleSwitcher`, `FormattedAmount`, `LocaleContext`, `CurrencyContext`, `fxService`.

### 4. Transaction Simulation Preview (`#785`)
- **Pre-Flight Inspection**: Simulates Soroban smart contract calls prior to opening wallet signature prompts.
- **Decoded State Changes**: Provides human-readable previews of wallet balances, escrow changes, and protocol fee deductions.
- **Failure Blocking**: Blocks signature prompts when a transaction simulation fails or exceeds limits, preventing wasted gas and failed executions.
- **Components**: `SimulationPreviewModal`, `SimulationResultView`, `SimulationContext`, `simulationService`.

### 5. Unified Stellar Wallet Connection (`#774`)
- **Adapters**: A shared `WalletAdapter` contract supports Freighter and Rabet without coupling consuming components to extension-specific APIs.
- **Explicit states**: disconnected, connecting, connected, install-required, network-mismatch, and error are exposed through `WalletProvider` / `useWallet`.
- **Network safety**: Known mainnet/testnet mismatches are surfaced before signing; an unknown wallet network fails closed instead of being treated as compatible.
- **Session recovery**: Only the last wallet choice is persisted. Freighter can restore an already-authorized session without prompting; Rabet reconnect remains an explicit user action.
- **Account changes**: Freighter wallet-change monitoring and Rabet `accountChanged` / `networkChanged` events update the active address/network mid-session.
- **No secrets persisted**: Kora never stores wallet private keys, seed phrases, signed XDR, or account credentials in local storage.
- **SDK handoff**: Configure the Kora SDK with its `TESTNET` or `MAINNET` network config matching the `WalletProvider expectedNetwork` value before enabling a signature flow.

```tsx
<WalletProvider expectedNetwork="testnet">
  <WalletConnection />
</WalletProvider>
```

If an extension is missing, `WalletConnection` renders the wallet's install link. If the wallet is on a different known network, it renders an alert instructing the user to switch networks before signing. Freighter uses the official `@stellar/freighter-api` package; Rabet uses its documented injected `window.rabet` provider.

#### Connection lifecycle and validation

See [wallet validation evidence](./docs/wallet-validation.md) for commands, results, and baseline limitations.

The canonical network passphrase takes precedence over the wallet's display
name. Unknown, custom, or whitespace-modified passphrases are not treated as
Stellar mainnet/testnet. For providers returning names only, only exact documented
aliases are recognized; a substring such as `private-testnet` is insufficient.

Each manager operation owns a generation. Late installation checks cannot open
an obsolete connect prompt, and queued callbacks from a previous connection
(including the same wallet) cannot replace the current account. Local disconnect
is immediate even when the extension is slow or rejects cleanup. A later session
and its stored wallet choice are not cleared by an older disconnect response.

Provider subscriptions invalidate their callbacks before removing listeners.
Freighter fallback polling is serialized. Rabet account refreshes keep the latest
account and network evidence rather than applying out-of-order responses.

Run the focused tests from `apps/web`:

```sh
npm test -- --runInBand --runTestsByPath tests/wallet.test.ts tests/wallet-lifecycle.test.ts tests/wallet-ui.test.tsx --coverage --collectCoverageFrom='src/wallet/**/*.ts'
```

The lifecycle suite includes an independent state-machine oracle that drives the
**actual WalletManager** through all 4,096 four-action sequences of connect,
disconnect, network changes, expected-network changes, and session loss. Directed
deferred-Promise tests separately cover cancellation and out-of-order responses.
The UI integration tests render the real provider/component for connection,
installation, mismatch, unknown-network, and cleanup-failure states. Providers
are mocked: these tests do not contact a network, open an extension, sign a
transaction, or move funds. They are not a live-wallet integration certification.

![Wallet connection states](./docs/wallet-connection-states.png)

### 6. Wave Contributor Dashboard
- **Public Transparency**: Tracks Wave-style contributor initiatives with full visibility from GitHub to on-chain payouts.
- **Issue Tracking**: Real-time sync with GitHub issues, complexity-based points (Low: 50, Medium: 100, High: 200), status tracking.
- **Payout Transparency**: Cross-references GitHub issues with on-chain treasury disbursements, shows transaction hashes.
- **Contributor Leaderboard**: Rankings by total points earned, completed issues, disbursed vs pending payouts.
- **Program Statistics**: Completion rates, disbursement rates, active contributor counts, visual breakdowns.
- **Components**: `WaveDashboard`, `WaveIssueList`, `ContributorLeaderboard`, `WaveStatsOverview`, `waveService`.
- **Documentation**: See `README_WAVE.md` for detailed guide.

## Verification & Testing
Run unit tests and verification via:
```bash
cd apps/web
node tests/runTests.js
```

## Issue #1001 — Real IPFS Pinning Client in the Invoice Wizard

`InvoiceSubmissionWizard` uploads invoice documents through the
`services/ipfs-pinning` REST API via `src/services/ipfsPinningClient.ts`
(POST `/pins` upload + pin, GET `/pins/:cid` status polling, GET
`/pins/:cid/content` for byte-exact verification). When no pinning service is
configured the wizard falls back to the deterministic local dev mock, which is
never used in production paths.

### Environment variables

| Variable | Required | Description |
|---|---|---|
| `VITE_IPFS_PINNING_URL` | No | Base URL of the ipfs-pinning service REST API (e.g. `https://pins.kora.finance`). Unset → local dev mock pinning. When set, every document upload goes through the real service and produces a content-derived CID (no mock CID prefix). |

## Issue #1002 — metadata_hash Validation Before Invoice Mint

Before the mint step the wizard:

1. Computes SHA-256 over the exact document bytes with Web Crypto
   (`InvoiceWizardService.computeMetadataHash`) — browsers without
   `crypto.subtle` get a graceful "unsupported" message and minting stays
   disabled.
2. Rejects empty files *before* the hash step.
3. Displays the SHA-256 (`metadata_hash` byte commitment) on the review step
   and re-verifies the pinned bytes retrieved from the pinning service
   (`verifyPinnedDocumentHash`) — this mirrors
   `services/ipfs-pinning/src/verifier.ts` client-side, and any gateway
   content swap **fails closed**: the mint button stays disabled while the
   computed hash differs from the displayed/confirmed hash.
4. Passes the validated 32-byte hash to the mint channel (SDK hook via
   `InvoiceWizardService.setMintSubmitter`, compatible with the
   `invoice_nft` mint API and `commit_metadata_hash`), and fires an optional
   analytics hook (`InvoiceWizardService.setHashMismatchListener`) whenever a
   mismatch is detected.

See `docs/debtor-attestation.md` for the full metadata pipeline.


## Environment variables
- GITHUB_REPO (optional): GitHub repo in owner/name form. Defaults to OpenLedger-Foundation/Kora-Contract.
- GITHUB_TOKEN (optional): GitHub personal access token for higher rate limits.
- INDEXER_API_BASE (optional): Indexer API base URL.
