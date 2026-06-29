# Kora Protocol — Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Fixed
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
