# Security: Protocol Pause Enforcement Matrix

When an authorized administrator or guardian calls `access_control.pause()` or `access_control.emergency_pause_by_guardian()`, state-mutating entrypoints across the protocol are gated by a cross-contract check to `AccessControlContract.is_paused()`.
Any blocked call immediately halts and returns `KoraError::ProtocolPaused`.

This document serves as the canonical source of truth for protocol-wide pause semantics across all workspace contract crates.

---

## Pause-Semantics Enforcement Matrix

Every state-mutating entrypoint across all 19 workspace smart contract packages is audited below and categorized by contract family. For role definitions, caller authentication, and admin privilege requirements, see [ACCESS_MATRIX.md](ACCESS_MATRIX.md).

### 1. Core Lending & Invoicing Contracts

| Contract | Entrypoint | Blocked | Rationale & Code Citation |
| :--- | :--- | :---: | :--- |
| **invoice_nft** | `mint_invoice` | ✓ YES | `require_not_paused`: Prevents new debt/invoice issuance during an emergency (`invoice_nft/src/lib.rs:627`). |
| **invoice_nft** | `mint_invoices_batch` | ✓ YES | `require_not_paused`: Prevents batch invoice creation while protocol is paused (`invoice_nft/src/lib.rs:806`). |
| **invoice_nft** | `amend_invoice` | ✓ YES | `require_not_paused`: Invoice terms modification suspended during emergency review (`invoice_nft/src/lib.rs:908`). |
| **invoice_nft** | `update_metadata_cid` | ✓ YES | `require_not_paused`: Metadata updates locked while paused (`invoice_nft/src/lib.rs:955`). |
| **invoice_nft** | `withdraw_invoice` | ✓ YES | `require_not_paused`: Status transition blocked during pause (`invoice_nft/src/lib.rs:1006`). |
| **invoice_nft** | `set_created` | ✓ YES | `require_not_paused`: State lifecycle mutations blocked during pause (`invoice_nft/src/lib.rs:1039`). |
| **invoice_nft** | `set_listed` | ✓ YES | `require_not_paused`: Listing state transition blocked during pause (`invoice_nft/src/lib.rs:1056`). |
| **invoice_nft** | `set_funded` | ✓ YES | `require_not_paused`: Capital deployment state transitions blocked (`invoice_nft/src/lib.rs:1073`). |
| **invoice_nft** | `set_repaid` | ✓ YES | `require_not_paused`: Explicit pause check halts status update if caller initiates settlement while paused (`invoice_nft/src/lib.rs:1094`). |
| **invoice_nft** | `set_defaulted` | ✗ NO | **EXEMPT**: Admin/pool default marking must be executable even during a pause to document unrecoverable exposures (`invoice_nft/src/lib.rs:1145`). |
| **invoice_nft** | `migrate` / `migrate_versions` | ✗ NO | **EXEMPT**: Admin upgrade/schema migration pathway reserved for protocol upgrades. |
| **marketplace** | `list_invoice` | ✓ YES | `require_not_paused`: Blocks new primary market listings (`marketplace/src/lib.rs:540`). |
| **marketplace** | `amend_listing` | ✓ YES | `require_not_paused`: Listing term alterations blocked (`marketplace/src/lib.rs:1489`). |
| **marketplace** | `fund_invoice` | ✓ YES | `require_not_paused`: Freezes inbound capital into primary listings (`marketplace/src/lib.rs:729`). |
| **marketplace** | `fund_invoices_batch` | ✓ YES | `require_not_paused`: Freezes batch capital funding (`marketplace/src/lib.rs:754`). |
| **marketplace** | `cancel_listing` | ✗ NO | **EXEMPT**: Sellers must always retain the unilateral right to withdraw their listed assets from the market (`marketplace/src/lib.rs:804`). |
| **marketplace** | `claim_refund` | ✗ NO | **EXEMPT**: Investors must be able to recover committed funds from expired/unfulfilled listings (`marketplace/src/lib.rs:865`). |
| **marketplace** | `set_fee_bps` / `whitelist_token` | ✗ NO | **EXEMPT**: Admin configuration entrypoints synchronized with governance action (see [ACCESS_MATRIX.md § 4](ACCESS_MATRIX.md#4-marketplace)). |
| **financing_pool** | `release_funds` | ✓ YES | `require_not_paused`: No new capital disbursement or pool position opening (`financing_pool/src/lib.rs:667`). |
| **financing_pool** | `record_position` | ✓ YES | `require_not_paused`: Blocks new investor position recording (`financing_pool/src/lib.rs:1131`). |
| **financing_pool** | `split_position` | ✓ YES | `require_not_paused`: Re-structuring pool positions suspended during pause (`financing_pool/src/lib.rs:1255`). |
| **financing_pool** | `transfer_share` / `transfer_position` | ✓ YES | `require_not_paused`: Internal share ledger reassignments blocked (`financing_pool/src/lib.rs:1342`, `3390`). |
| **financing_pool** | `list_share_for_sale` / `buy_share` | ✓ YES | `require_not_paused`: Internal share orderbook trades blocked (`financing_pool/src/lib.rs:1393`, `1467`). |
| **financing_pool** | `list_position_for_sale` / `buy_position` | ✓ YES | `require_not_paused`: Internal position sales blocked (`financing_pool/src/lib.rs:2964`, `3069`). |
| **financing_pool** | `net_settle` | ✓ YES | `require_not_paused`: Automated net balance settlement deferred (`financing_pool/src/lib.rs:1921`). |
| **financing_pool** | `mark_default` | ✓ YES | `require_not_paused`: Pool-level default cascade gated by pause (`financing_pool/src/lib.rs:2265`). |
| **financing_pool** | `propose_early_settlement` / `accept` | ✓ YES | `require_not_paused`: Early settlement negotiation blocked (`financing_pool/src/lib.rs:2565`, `2639`). |
| **financing_pool** | `set_installment_schedule` | ✓ YES | `require_not_paused`: Schedule alterations blocked during pause (`financing_pool/src/lib.rs:2875`). |
| **financing_pool** | `repay` | ✗ NO | **EXEMPT**: Entrypoint itself has no direct pause guard. Borrowers/SMEs must always be permitted to submit debt repayment to extinguish liabilities (`financing_pool/src/lib.rs:1754`). |

---

### 2. Secondary Market & Fractionalization Contracts

| Contract | Entrypoint | Blocked | Rationale & Code Citation |
| :--- | :--- | :---: | :--- |
| **secondary_market** | `list_position` | ✓ YES | `require_not_paused`: Blocks creation of new secondary market listings during an emergency (`secondary_market/src/lib.rs:167`). |
| **secondary_market** | `buy_position` | ✓ YES | `require_not_paused`: Blocks purchases on secondary market listings to halt secondary capital flows (`secondary_market/src/lib.rs:241`). |
| **secondary_market** | `cancel_listing` | ✗ NO | **EXEMPT**: Sellers must always retain the unilateral right to withdraw their positions from the secondary orderbook (`secondary_market/src/lib.rs:293`). |
| **secondary_market** | `pause` / `unpause` | ✗ NO | **EXEMPT**: Admin pause controls operate independently (`secondary_market/src/lib.rs:120`, `128`). |
| **secondary_market** | `update_protocol_fee_bps` | ✗ NO | **EXEMPT**: Admin fee configuration (see [ACCESS_MATRIX.md § 9](ACCESS_MATRIX.md#9-secondary_market-added-by-issue-917)). |
| **fractionalizer** | `wrap` | ✓ YES | `require_not_paused`: Blocks depositing pool positions to mint fractional ERC-20-style shares (`fractionalizer/src/lib.rs:99`). |
| **fractionalizer** | `unwrap` | ✓ YES | `require_not_paused`: Reconstituting underlying positions locked until pause lifted (`fractionalizer/src/lib.rs:345`). |
| **fractionalizer** | `transfer_shares` | ✗ NO | **EXEMPT**: Tokenized share transferability remains unencumbered for peer-to-peer custody (`fractionalizer/src/lib.rs:198`). |
| **fractionalizer** | `settle` | ✗ NO | **EXEMPT**: Settling matured/repaid positions must continue to process proceeds (`fractionalizer/src/lib.rs:245`). |
| **fractionalizer** | `redeem` | ✗ NO | **EXEMPT**: Token holders must be able to redeem pro-rata capital upon settlement (`fractionalizer/src/lib.rs:290`). |
| **tranche** | `create_tranche` / `fund_tranche` | ✗ NO | **EXEMPT**: Structured finance tranche factory; admin and governance authorized via multisig. |

---

### 3. Dispute Resolution & Escrow Contracts

| Contract | Entrypoint | Blocked | Rationale & Code Citation |
| :--- | :--- | :---: | :--- |
| **dispute_resolution** | `open_dispute` | ✓ YES | `require_not_paused`: Prevents initiating new dispute proceedings while protocol is paused (`dispute_resolution/src/lib.rs:89`). |
| **dispute_resolution** | `submit_evidence` | ✗ NO | **EXEMPT**: Disputing parties must be allowed to meet evidence deadlines without risk of statutory default (`dispute_resolution/src/lib.rs:125`). |
| **dispute_resolution** | `resolve_dispute` | ✗ NO | **EXEMPT**: Authorized arbitrator/council must be capable of resolving active disputes during an emergency (`dispute_resolution/src/lib.rs:160`). |
| **dispute_resolution** | `propose_upgrade` / `execute` | ✗ NO | **EXEMPT**: Governance upgrades bypass runtime pause. |
| **dispute_escrow** | `open_dispute` | ✗ NO | **EXEMPT**: Automated collateral locking called by financing pool or dispute contract; exempt to prevent state divergence. |
| **dispute_escrow** | `reserve_escrow` | ✗ NO | **EXEMPT**: Pool-driven escrow reservations are non-paused settlement hooks (`dispute_escrow/src/lib.rs`). |
| **dispute_escrow** | `settle_escrow` / `resolve_timeout` | ✗ NO | **EXEMPT**: Outbound distribution of escrowed capital to verified claimants must proceed to protect user property. |

---

### 4. Governance & DAO Voting Contracts (Non-Pauseable)

> [!IMPORTANT]
> **Governance Immunity Principle:** Governance and voting contracts **MUST NEVER BE PAUSE-GUARDED**. Protocol governance is the sovereign authority utilized by the community and multisig signers to remediate incidents, replace compromised keys, cancel malicious actions, and unpause the protocol. Gating governance Behind a pause guard would introduce an unrecoverable operational deadlock (Catch-22).

| Contract | Entrypoint | Blocked | Rationale & Code Citation |
| :--- | :--- | :---: | :--- |
| **governance** | `create_proposal` / `vote` / `execute_proposal` | ✗ NO | **NON-PAUSEABLE**: Community governance execution must remain accessible under all protocol conditions. |
| **governance** | `configure_fast_track` / `create_fast_track_proposal` | ✗ NO | **NON-PAUSEABLE**: Emergency fast-track proposals are explicitly designed for rapid incident mitigation. |
| **governance** | `register_critical_bug_report` / `vote_fast_track` | ✗ NO | **NON-PAUSEABLE**: White-hat security disclosure and emergency patching pipelines. |
| **governance_voting** | `set_voting_weight` / `delegate` / `revoke_delegation` | ✗ NO | **NON-PAUSEABLE**: Voting stake weight and delegation mechanisms operate independently. |
| **governance_voting** | `create_proposal` / `vote` / `execute_proposal` | ✗ NO | **NON-PAUSEABLE**: Core ballot submission and tally verification are never pause-gated. |

---

### 5. Administrative, Oracle & Registry Contracts

| Contract | Entrypoint | Blocked | Rationale & Code Citation |
| :--- | :--- | :---: | :--- |
| **access_control** | `pause` / `unpause` | ✗ NO | **NON-PAUSEABLE**: The pause circuit-breaker itself cannot be paused (`access_control/src/lib.rs:150`). |
| **access_control** | `emergency_pause_by_guardian` | ✗ NO | **NON-PAUSEABLE**: Guardian circuit breaker for immediate 1-signature incident pause. |
| **access_control** | `grant_role` / `revoke_role` / `transfer_admin` | ✗ NO | **NON-PAUSEABLE**: Role management must operate during incidents to revoke compromised accounts. |
| **access_control** | `configure_multisig` / `propose_action` / `execute_action` | ✗ NO | **NON-PAUSEABLE**: Multisig governance operations must never be paralyzed. |
| **price_oracle** | `set_price` | ✓ YES | `require_not_paused`: Prevents feeding potentially manipulated or stale off-chain price ticks while paused (`price_oracle/src/lib.rs:139`). |
| **price_oracle** | `convert_with_decimals` | ✓ YES | Gated if conversion relies on live feed updates during pause (`price_oracle/src/lib.rs:719`). |
| **price_oracle** | `get_price` / `convert` | ✗ NO | **EXEMPT**: Read-only price inspection queries remain functional for downstream contracts. |
| **price_oracle** | `set_access_control` / `add_feeder` / `remove_feeder` | ✗ NO | **EXEMPT**: Admin configuration functions synchronized via multisig. |
| **treasury** | `disburse_from_reserve` | ✓ YES | `require_not_paused`: Reserve capital disbursements locked during emergency pause (`treasury/src/lib.rs:588`). |
| **treasury** | `withdraw` / `emergency_withdraw` | ✗ NO | **EXEMPT**: Admin withdrawal and incident fund rescue pathways operate independently (`treasury/src/lib.rs:563`). |
| **treasury** | `collect_fee` / `whitelist_token` / `set_fee_bps` | ✗ NO | **EXEMPT**: Passive fee collection and administrative token management. |
| **risk_registry** | `register_sme` / `update_sme_score` / `record_default` | ✗ NO | **NON-PAUSEABLE**: SME verification, scoring updates, and credit risk registries remain active out-of-band. |
| **parameter_registry** | `admin_set_parameter` / `propose_param_update` / `execute` | ✗ NO | **NON-PAUSEABLE**: Dynamic parameter reconfiguration (e.g. widening grace periods or reducing caps during an incident) must remain available. |
| **debtor_attestation** | `attest` / `attest_via_relay` / `reject` | ✗ NO | **NON-PAUSEABLE**: Debtor off-chain attestation tracking operates independently of protocol liquidity. |
| **verifier_nomination** | `nominate` / `stake` / `withdraw` | ✗ NO | **NON-PAUSEABLE**: Staking and nomination registry for verifiers. |
| **vesting** | `create_grant` / `release` / `revoke_grant` | ✗ NO | **NON-PAUSEABLE**: Contributor and employee token vesting schedules execute predictably without liquidity risk. |
| **referral_rewards** | `deposit` / `claim_referral_reward` | ✗ NO | **NON-PAUSEABLE**: Outbound referral incentive claims do not compromise core protocol solvency. |

---

## Design Principles & Safety Invariants

1. **Circuit-Breaker for Inbound Economic Risk, Not Outbound Settlement:**
   - The pause mechanism strictly halts **inbound capital exposure**: new invoices (`mint_invoice`), new listings (`list_invoice`, `list_position`), and new liquidity commitments (`fund_invoice`, `buy_position`, `wrap`).
   - The pause intentionally does **not trap existing user capital**: listing cancellations (`cancel_listing`), unfulfilled refunds (`claim_refund`), and secondary share redemption (`redeem`) remain operable.
2. **Governance Deadlock Avoidance:**
   - Contracts in the governance hierarchy (`governance`, `governance_voting`, `access_control`) possess zero pause gates to guarantee that incident response teams can always submit, vote on, and execute recovery actions.
3. **Cross-Contract Synchronization:**
   - Downstream contracts (`marketplace`, `financing_pool`, `dispute_resolution`, `fractionalizer`, `price_oracle`, `treasury`) query the central authority `access_control.is_paused()`. Secondary market also maintains a local circuit breaker for isolated halts.

---

## Verification & Audit Commands

To verify and re-audit pause guards against the live codebase:

```bash
# 1. Search for all pause checks across contract sources
git grep -n -E "require_not_paused|is_paused|ProtocolPaused" contracts/

# 2. Run threat-model test suite verifying pause enforcement
cargo test -p kora-tests --test threat_model_tests test_tm_pause -- --nocapture

# 3. Verify access control and role requirements across all entrypoints
cargo run -p kora-xtask --bin check-access-matrix
```
