# Kora Protocol — Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added
- **feat(sdk): Invoice type parity with shared v2** — `Invoice` interface and `decodeInvoice` already carried all schema v2 fields (`metadataHash`, `notes`). Added fixture test suite (`sdk/tests/invoice-v2-schema.test.ts`) with 30 tests covering v2 field presence (INV-V2-1 through INV-V2-9), v1 pre-migration fallback, unknown-field silent-ignore policy, enum exhaustive round-trips, and `metadataHash` length semantics. Unknown-field policy and v2 field table documented in `sdk/README.md`. See `sdk/tests/invoice-v2-schema.test.ts`.
- **feat(packaging): indexer Docker smoke** — added `services/indexer/src/main.rs` binary entry-point (tokio-based HTTP server exposing `GET /api/v1/health` and `GET /metrics`; aborts with a clear message when `SOROBAN_RPC_URL` is unset). Added `.github/workflows/indexer-docker-smoke.yml` CI workflow that builds the Docker image, starts the Compose stack, retries the health endpoint, validates the JSON response body, and confirms the Prometheus metrics line, then tears down. See `packaging/Dockerfile`, `packaging/docker-compose.yml`, `.github/workflows/indexer-docker-smoke.yml`.
- **ci(security): automate reentrancy audit checklist** — added `scripts/check_reentrancy_audit.sh` which enforces six checks: audit document row count (≥ 27), regression test count (≥ 20), TR-6 and RR-6 open-item regression tests present, guard presence per contract (`RepaymentLock` / `ReentrancyGuard`), heuristic token-transfer scan with allowlist, and shared reentrancy module integrity. Wired as `reentrancy-audit-gate` job in `.github/workflows/ci.yml`. See `docs/REENTRANCY_AUDIT.md` for the allowlist policy.
- **fix(shared): close arithmetic matrix gaps** — added 44 tests in `contracts/tests/arithmetic_overflow_matrix.rs` covering four previously untracked type groups: `DecaySchedule` linear interpolation (DS-A through DS-D, 14 tests), `InstallmentSchedule` cumulative sum (IS-A through IS-C, 7 tests), `DiscretionaryWithdrawal` amount guards (DW-A through DW-C, 7 tests), and `RiskTierDefinition` score boundary comparisons (RT-A through RT-D, 11 tests). `docs/ARITHMETIC_MATRIX.md` updated: 9 sections, 44 operations, 0 gaps. Prior total was 30 operations across 5 sections.

### Added
- **#914 Dispute cross-contract harness** — add real pool/escrow callback wiring, evidence and outcome scenarios, custody/accounting assertions, failure rollback and token reentry regression cases, and a dedicated integration CI target. Runtime validation remains pending.
- **#912 / #911 Governance and escrow CI onboarding** — include governance, governance voting, parameter registry, and dispute escrow in workspace validation, WASM budgets, optimization, metrics, and release hashes. Escrow deployment is opt-in pending production financing-pool callback integration.
- **#919 Dispute state-machine fuzzing** — add exact-error transition modeling, authorization and terminal-state checks, escrow custody invariants with a mock pool, regression seeds, and bounded CI smoke coverage. Runtime validation remains pending.
- **Security Hardening Wave** — comprehensive security improvements across the protocol:
  - **Economic attack simulation harness** — 15 test scenarios modeling fund-refund gaming, referral self-dealing, insurance pool timing manipulation, and concentration cap bypass attempts. All attacks verified unprofitable (<1% ROI). See `contracts/tests/economic-sim/` and `docs/SECURITY_HARDENING_SUMMARY.md`.
  - **Security-relevant Clippy lints** — 18 lints enforced covering arithmetic safety (prevent overflow), panic safety (prevent DoS), unsafe safety (require documentation), and logic correctness (prevent silent failures). See `docs/SECURITY_LINTS.md` and workspace `Cargo.toml`.
  - **Key rotation framework** — secure on-chain mechanics for rotating compromised multi-sig co-signers, verifier keys, and admin keys with quorum/timelock protection, minimum quorum enforcement, and atomic state transfer. See `contracts/shared/src/key_rotation.rs` and `docs/KEY_ROTATION_PROCEDURE.md`.
  - **TTL exhaustion protection** — four-layer defense (auto-extension on access, permissionless bump functions, keeper bot monitoring, grace period alerts) prevents fund-locking via expired storage entries. See `contracts/shared/src/ttl_protection.rs` and `docs/TTL_EXHAUSTION_PROTECTION.md`.
- **#283 Verifier-delegated sub-accounts** — `risk_registry` now supports a primary verifier delegating action rights to N sub-accounts via `add_sub_account` / `remove_sub_account`. All SME registrations and score updates performed by a sub-account are attributed to the primary verifier for reputation and staking purposes. The primary verifier is stored in `SmeProfile.verifier` regardless of which signer performed the action. Sub-accounts cannot themselves be primary verifiers, and an address can only be a sub-account of one primary at a time.
- **#280 Storage rent/TTL cost model guide** — new `docs/storage-rent-cost-model.md` explains Soroban's three storage tiers (persistent / instance / temporary), the TTL constants used in each Kora contract, which keys are most expensive at scale, worked cost projections at 1 000 and 10 000 active invoices, and keeper-bot / archival responsibilities.
- **#276 Treasury contract documentation** — new `docs/treasury.md` covers the full `fee_bps` lifecycle, `withdraw` and `emergency_withdraw` flows with step-by-step ordering, the reentrancy guard design, the rolling 24-hour withdrawal rate-limit, the two-step cap/upgrade timelock, a complete public API table, the storage layout, and a threat-model / security analysis.

### Fixed
- **#914 Escrow custody reentry protection** — guard open, adjudication, and timeout mutations across external pool/token interactions; preserve existing error codes and add a dedicated reentry error. See `docs/AUDIT_LOG.md`.
- **#343 Duplicate `KoraError` discriminant 95** *(breaking ABI change)* — `EmptyBytes` and `Reentrancy` previously both used discriminant 95, causing any reentrancy violation to be silently decoded as `EmptyBytes` by off-chain clients and preventing the crate from compiling (E0081). Discriminants are now unique: `NoContribution = 95`, `NotInitialized = 96`, `EmptyBytes = 97`, `Reentrancy = 98`. **Clients that pattern-match on `KoraError::Reentrancy` by its raw u32 value must update their discriminant from 95 to 98.**
- **#420 Invoice-lifecycle event schema inconsistencies** *(breaking event-schema change)* — `invoice_defaulted` was missing an `amount` field carried by every sibling invoice event, and none of the seven invoice-lifecycle events (`invoice_created`, `invoice_listed`, `invoice_funded`, `invoice_repaid`, `invoice_amended`, `invoice_withdrawn`, `invoice_defaulted`) included the invoice's `currency`, forcing indexers to make a separate `get_invoice` call to interpret the monetary value. `invoice_defaulted` now carries `amount`, and all seven events now carry `currency` (see `docs/EVENTS.md`). **Off-chain indexers decoding these event tuples by fixed position/arity must update to the new schemas.**

### Changed
- **Removed duplicate sme_invoice_counted event** — use sme_invoice_count_incremented instead across all SME profile tracking (see `contracts/shared/src/events.rs` and AUDIT_LOG.md)

### Planned
- Multisig admin with timelock
- Contract upgrade mechanism
- Secondary market for pool positions
- Keeper network for TTL management
- On-chain FX oracle integration

---

## [0.1.0] — 2026-05-18

### Added
- `invoice_nft` contract — mint, status transitions, invoice NFT data model
- `marketplace` contract — list, fund, cancel, fee collection, whitelist
- `financing_pool` contract — fund custody, position tracking, repayment, yield distribution, default handling
- `treasury` contract — fee accumulation, admin withdrawal, emergency drain
- `risk_registry` contract — verifier management, SME profiles, debtor scoring
- `access_control` contract — pause/unpause, role management, admin transfer
- `shared` library — types, errors, events, validation utilities
- Integration test suite covering full invoice lifecycle and edge cases
- Deployment scripts for testnet and mainnet
- Makefile with build, test, lint, and deploy targets
- README, CONTRIBUTING, ARCHITECTURE, CONTRACTS, SECURITY documentation
