# Access Control Matrix

**Last Updated:** 2026-10-17  
**Purpose:** Comprehensive authorization matrix mapping every public entrypoint to required roles/permissions  
**Automated Checker:** `contracts/xtask/src/bin/check_access_matrix.rs`

## Overview

This document serves as the single source of truth for access control across all Kora smart contracts. Every public entrypoint is mapped to its required authorization, and an automated checker verifies that the implementation matches this specification.

The checker parses the tables below directly from this file on every run — there is no hardcoded copy of the matrix in the tool. Any drift between a table row and the implemented `require_auth` / `require_admin` / contract-caller checks fails CI.

## Role Definitions

### System Roles

| Role | Description | Granted By |
|------|-------------|------------|
| **Admin** | Protocol administrator with upgrade and configuration privileges | `access_control` multisig or single admin |
| **Verifier** | KYB/risk-assessment provider authorized to attest SME profiles | `risk_registry.add_verifier` (admin-only) |
| **Feeder** | Price oracle data provider authorized to submit exchange rates | `price_oracle.add_feeder` (admin-only) |
| **SME** | Small/medium enterprise seeking invoice financing | Self-registered via `risk_registry.register_sme` |
| **Investor** | Capital provider funding invoice listings | Any address (permissionless funding) |
| **Governance** | Multi-sig or DAO contract for dispute resolution | Configured in `dispute_resolution` |

### Special Authorization Patterns

| Pattern | Description | Check Method |
|---------|-------------|--------------|
| **Self** | Caller must be the resource owner (e.g., SME calling on their own invoice) | Direct address comparison |
| **Contract-to-Contract** | Authorized cross-contract caller (e.g., `marketplace` calling `financing_pool`) | Storage-configured contract address |
| **Conditional** | Authorization depends on state (e.g., only if not paused) | Multiple checks combined with `&&` |
| **Permissionless** | No authorization required (read-only views) | No `require_auth()` call |

---

## Access Matrix by Contract

### 1. access_control

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Sets initial admin |
| `pause` | **Admin** | Blocked when multisig configured; direct call rejected | Emergency pause |
| `unpause` | **Admin** | Blocked when multisig configured; direct call rejected | |
| `emergency_pause_by_guardian` | **Guardian (Self)** | Caller must hold Guardian role | No multisig gate; immediate pause |
| `get_last_emergency_pause_time` | Permissionless | Read-only | |
| `get_emergency_pause_initiator` | Permissionless | Read-only | |
| `grant_role` | **Admin** | Blocked when multisig configured | Cannot grant Admin directly |
| `revoke_role` | **Admin** | Blocked when multisig configured | Non-admin roles only |
| `transfer_admin` | **Admin** | Blocked when multisig configured | Single-step handover |
| `rotate_admin` | **Admin** | Blocked when multisig configured; no active proposals | Key-compromise rotation |
| `cancel_admin_proposal` | **Admin** | Removes pending entry if present | |
| `configure_multisig` | **Admin** | Multisig quorum required once configured | Threshold and signer set |
| `set_dispute_resolution` | **Admin** | Post-deployment wiring | Stores dispute contract address |
| `propose_action` | **Governance** | Multisig signer; records own approval | |
| `approve_action` | **Governance** | Multisig signer; rejects duplicate approvals | |
| `execute_action` | **Governance** | Multisig signer; executes at threshold | Pause/unpause/role/admin actions |
| `cancel_action` | **Governance** | Multisig signer; proposer or threshold | |
| `execute_batch` | **Governance** | Multisig signer; atomic multi-proposal | |
| `get_proposal` | Permissionless | Read-only | |
| `get_multisig_config` | Permissionless | Read-only | |
| `propose_parameter_change` | **Governance** | Multisig signer | Range-validated parameters |
| `vote_parameter_change` | **Governance** | Multisig signer; rejects repeat votes | |
| `execute_parameter_change` | **Governance** | Multisig signer; timelock elapsed | |
| `cancel_parameter_change` | **Governance** | Multisig signer; proposer or threshold | |
| `get_parameter` | Permissionless | Read-only | |
| `get_governance_timelock_delay` | Permissionless | Read-only | |
| `get_parameter_proposal` | Permissionless | Read-only | |
| `submit_community_proposal` | **Anyone** | Per-address cooldown; expires | Public submission |
| `get_community_proposal` | Permissionless | Read-only | |
| `sponsor_community_proposal` | **Governance** | Multisig signer converts community proposal | |
| `propose_signer_recovery` | **Governance** | Multisig signer; recovery timelock | |
| `object_signer_recovery` | **Governance** | Multisig signer objection blocks execution | |
| `execute_signer_recovery` | **Governance** | Multisig signer; zero objections; 30-day timelock | |
| `get_recovery_proposal` | Permissionless | Read-only | |
| `delegate_vote` | **Governance** | Multisig signer delegation; no self-delegation | |
| `revoke_delegation` | **Governance** | Multisig signer removes delegation | |
| `get_delegate` | Permissionless | Read-only | |
| `get_delegators` | Permissionless | Read-only | |
| `configure_council_terms` | **Admin** | Term length non-zero; grace period | Council rotation terms |
| `initiate_council_rotation` | **Governance** | Multisig signer opens election for expiring seat | |
| `vote_council_rotation` | **Governance** | Multisig signer; rejects repeat voters | |
| `conclude_council_rotation` | **Governance** | Multisig signer; threshold swaps seat | |
| `is_council_seat_term_expired` | Permissionless | Read-only | |
| `get_council_election` | Permissionless | Read-only | |
| `is_paused` | Permissionless | Read-only | |
| `get_role` | Permissionless | Read-only | Falls back to stored admin |
| `has_role` | Permissionless | Read-only | |
| `get_admin` | Permissionless | Read-only | |
| `get_role_members` | Permissionless | Read-only | Paged, 1-50 per page |
| `propose_upgrade` | **Admin** | 24h timelock proposal | WASM hash staging |
| `execute_upgrade` | **Admin** | Timelock elapsed | Applies WASM upgrade |
| `get_audit_log` | Permissionless | Read-only | Paged, newest first |

**Authorization Checker:**
```rust
fn require_admin(&env, caller: &Address) -> Result<(), AccessControlError> {
    let admin = env.storage().instance().get(&DataKey::Admin).unwrap();
    if caller != &admin { return Err(AccessControlError::NotAdmin); }
    // If multisig configured, verify quorum via approve_pending_action
    Ok(())
}
```

---

### 2. risk_registry

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Stores admin and staking params |
| `transfer_admin` | **Admin** | Proposes PendingAdmin handover | Two-step transfer |
| `accept_admin` | **PendingAdmin** | Caller must be the proposed new admin | Completes handover |
| `set_invoice_nft` | **Admin** | Rejects self address | Wires invoice_nft contract |
| `cancel_admin_proposal` | **Admin** | Removes pending proposal if present | |
| `add_verifier` | **Admin** | Converts verifier stake | Active/suspended/pending checks |
| `top_up_stake` | **Admin** | Active verifier only | Adds to verifier stake |
| `remove_verifier` | **Admin** | Returns stake; clears records | Marks Removed |
| `suspend_verifier` | **Admin** | Active to Suspended | Records transition timestamp |
| `reinstate_verifier` | **Admin** | Suspended to Active | |
| `request_verifier_removal` | **Admin** | Active or Suspended to PendingRemoval | Cooldown starts |
| `cancel_verifier_removal` | **Admin** | PendingRemoval to Active | Clears request timestamp |
| `finalize_verifier_removal` | **Admin** | Cooldown elapsed | Returns stake; marks Removed |
| `get_verifier_status` | Permissionless | Read-only | |
| `get_verifier_status_changed_at` | Permissionless | Read-only | |
| `get_verifier_removal_deadline` | Permissionless | Read-only | |
| `add_sub_account` | **Verifier** | Primary verifier delegates; no self-registration | Sub-account allowlist |
| `remove_sub_account` | **Verifier** | Primary verifier removes delegate | Ownership check |
| `register_sme` | **Verifier** | Active verifier or sub-account | Registers SME with risk score |
| `update_sme_score` | **Verifier** | Verifier-of-record only | Reentrancy guarded |
| `set_credit_limit` | **Verifier** | Verifier-of-record only | Non-negative aggregate limit |
| `reassign_sme_verifier` | **Admin** | New verifier must be active | Audited reassignment |
| `increment_invoice_count` | **InvoiceNft Contract** | Caller must be registered invoice_nft address | Cross-contract |
| `record_default` | **Admin** | Reentrancy guarded | Slashes stake; may trip breaker |
| `record_successful_repayment` | **Admin** | Caps reputation at 100 | |
| `set_debtor_score` | **Verifier** | Active verifier; 1h cooldown; max 50 attestors | 32-byte debtor hash |
| `get_sme_profile` | Permissionless | Read-only | |
| `is_verified_sme` | Permissionless | Read-only | |
| `is_compliance_attested` | Permissionless | Read-only | |
| `get_verifier_stake` | Permissionless | Read-only | |
| `get_verifier_reputation` | Permissionless | Read-only | |
| `is_verifier` | Permissionless | Read-only | Active status check |
| `get_primary_verifier` | Permissionless | Read-only | Sub-account lookup |
| `is_sub_account` | Permissionless | Read-only | |
| `get_debtor_score` | Permissionless | Read-only | Aggregates active attestations |
| `get_debtor_risk_tier` | Permissionless | Read-only | Derives tier from score |
| `get_debtor_score_attestation` | Permissionless | Read-only | Per-verifier attestation |
| `set_min_debtor_score` | **Admin** | Governed minimum score gate; 0 disables | |
| `get_min_debtor_score` | Permissionless | Read-only | |
| `is_debtor_verified` | Permissionless | Read-only | Gate check vs average attestation |
| `get_admin` | Permissionless | Read-only | |
| `propose_upgrade` | **Admin** | 24h timelock proposal | WASM hash staging |
| `execute_upgrade` | **Admin** | Timelock elapsed | Applies WASM upgrade |
| `get_audit_log` | Permissionless | Read-only | Paged, newest first |
| `configure_slashing` | **Admin** | Dispute window and treasury address | |
| `raise_dispute` | **Verifier** | Challenger opens vs active verifier | Extends bond lock |
| `resolve_dispute` | **Admin** | Resolves dispute; optional slash to treasury | |
| `get_bond` | Permissionless | Read-only | Verifier bond and lock |
| `get_dispute_count` | Permissionless | Read-only | |
| `configure_circuit_breaker` | **Admin** | Breaker threshold and sample size | |
| `clear_circuit_breaker_debtor` | **Admin** | Clears per-debtor pause flag | |
| `clear_circuit_breaker_tier` | **Admin** | Clears tier pause flag | |
| `is_debtor_paused` | Permissionless | Read-only | |
| `is_tier_paused` | Permissionless | Read-only | |
| `get_current_risk_tier_definition` | Permissionless | Read-only | Active or default definition |
| `get_risk_tier_definition_version` | Permissionless | Read-only | Historical version lookup |
| `update_risk_tier_definition` | **Admin** | Ordered tier boundaries | Versioned definition |

**Special Authorizations:**
- `SubAccount(address)` → allows delegate addresses to act on behalf of primary verifier
- Verifier-of-record: Each SME has an assigned verifier; only that verifier can update the SME's profile

---

### 3. invoice_nft

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Stores admin and NextId; schema v2 |
| `migrate` | Permissionless | Delegates to migrate_versions (Admin) | Version stepper |
| `migrate_versions` | **Admin** | Adjacent-step migration only | Audited when changed |
| `set_risk_registry` | **Admin** | Wires risk registry address | Audited |
| `set_debtor_attestation` | **Admin** | Wires debtor attestation address; rejects self address; idempotent on re-set | Audited; emits `DEP_EXEC` (tag 7) |
| `get_debtor_attestation` | Permissionless | Read-only | Returns configured address or None |
| `set_authorized_callers` | **Admin** | Wires marketplace and financing_pool | Rejects self/distinctness violations |
| `set_protocol_config` | **Admin** | Validates max risk score | |
| `get_protocol_config` | Permissionless | Read-only | Zeroed default when unset |
| `set_amount_bounds` | **Admin** | Non-negative ordered amounts | Per-tier bounds |
| `get_amount_bounds` | Permissionless | Read-only | |
| `set_mint_rate_limit` | **Admin** | Non-zero max mints and window | Anti-spam protection |
| `get_mint_rate_limit` | Permissionless | Read-only | |
| `get_sme_mint_window` | Permissionless | Read-only | Window start and mints used |
| `mint_invoice` | **SME (Self)** | Rate-limited by mint_rate_limit | Caller must be verified SME |
| `mint_invoices_batch` | **SME (Self)** | Batch size bounded; rate limit per invoice | Atomic batch mint |
| `amend_invoice` | **SME (Self)** | Owner only; Created status only | Rewrites amount/due/risk |
| `update_metadata_cid` | **SME (Self)** | Owner only; Created status only | Bounded CID history push |
| `get_metadata_cid_history` | Permissionless | Read-only | |
| `withdraw_invoice` | **SME (Self)** | Owner only; Created status only | Deletes keys; decrements exposure |
| `set_created` | **Marketplace Contract** | Caller must be registered marketplace address | Listed to Created relist |
| `set_listed` | **Marketplace Contract** | Caller must be registered marketplace address | Created to Listed |
| `set_funded` | **FinancingPool Contract** | Caller must be registered financing_pool address | Listed to Funded |
| `set_repaid` | **FinancingPool Contract** | Caller must be registered financing_pool address | Funded to Repaid; releases exposure |
| `set_defaulted` | **Admin** | Funded status; past due date | Manual default marking |
| `archive_invoice` | **Admin** | Repaid or Defaulted only | Deletes hot and cold keys |
| `get_invoice` | Permissionless | Read-only | Merges hot and cold storage |
| `next_id` | Permissionless | Read-only | Next invoice ID counter |
| `invoice_count` | Permissionless | Read-only | Minted invoice count |
| `get_outstanding_exposure` | Permissionless | Read-only | SME exposure or 0 |
| `reconcile_outstanding_exposure` | Permissionless | Read-only | Rescans hot keys |
| `get_sme_invoice_ids` | Permissionless | Read-only | Paged SME invoice IDs |
| `commit_metadata_hash` | **SME (Self)** | Owner only; Created status; one-time commit | On-chain SHA-256 commitment (see debtor-attestation.md) |
| `flag_metadata_mismatch` | **Challenger (Self)** | Evidence hash; freezes invoice | Opens metadata dispute |
| `resolve_metadata_dispute` | **Admin** | Dispute open and unresolved | Unfreezes when not upheld |
| `admin_correct_metadata_hash` | **Admin** | Created status only | Overwrites hash with audit event |
| `freeze_invoice` | **Admin** | Blocks further state transitions | Per-invoice freeze flag |
| `unfreeze_invoice` | **Admin** | Removes freeze flag | Audited |
| `is_invoice_frozen` | Permissionless | Read-only | |
| `freeze_sme_invoices` | **Admin** | Batches up to caller limit | Batch freeze for incident response |
| `unfreeze_sme_invoices` | **Admin** | Batches up to caller limit | |
| `refresh_risk_score` | **Admin** | Funded status; refreshes from risk registry | |
| `propose_upgrade` | **Admin** | 24h timelock proposal | WASM hash staging |
| `execute_upgrade` | **Admin** | Timelock elapsed | Applies WASM upgrade |
| `add_allowed_currency` | **Admin** | Adds currency symbol to allowlist | |
| `remove_allowed_currency` | **Admin** | Removes currency from allowlist | |
| `is_currency_allowed` | Permissionless | Read-only | |

**State Transition Authorization:**
- Only marketplace can transition `Created → Funded`
- Only financing_pool can transition `Funded → Repaid`
- Only admin can transition any state → `Defaulted`

---

### 4. marketplace

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `migrate_versions` | **Admin** | Adjacent-step migration only | Shared schema migration helper |
| `schema_version` | Permissionless | Read-only | Stored schema version |
| `initialize` | None (one-time) | Must not already be initialized | Stores admin and dependency addresses |
| `set_referrer_split_bps` | **Admin** | Multisig quorum if configured | Referrer fee split |
| `set_fee_bps` | **Admin** | Multisig quorum if configured | Base protocol fee |
| `update_fee_bps` | Permissionless | Delegates to set_fee_bps (Admin) | Alias; auth enforced in delegate |
| `get_fee_bps` | Permissionless | Read-only | Base fee from config |
| `set_min_contribution` | **Admin** | Inline admin check | DoS protection floor |
| `get_min_contribution` | Permissionless | Read-only | |
| `set_max_investor_share_bps` | **Admin** | Inline admin check | Concentration cap |
| `get_max_investor_share_bps` | Permissionless | Read-only | |
| `set_investor_accredited` | **Admin** | Inline admin check | KYC/accreditation flag |
| `is_investor_accredited` | Permissionless | Read-only | |
| `set_tier_fee_bps` | **Admin** | Multisig quorum if configured | Per-tier fee override |
| `get_tier_fee_bps` | Permissionless | Read-only | Falls back to base fee |
| `get_config` | Permissionless | Read-only | Full config struct |
| `get_admin` | Permissionless | Read-only | Stored admin address |
| `propose_token_whitelist` | **Admin** | Starts timelock | Timelocked whitelist proposal |
| `execute_token_whitelist` | **Admin** | Timelock elapsed | Whitelists token |
| `remove_token_whitelist` | **Admin** | Immediate (no timelock) | |
| `is_token_whitelisted` | Permissionless | Read-only | |
| `set_token_currency` | **Admin** | Inline admin check | Maps token to oracle symbol |
| `get_token_currency` | Permissionless | Read-only | |
| `list_invoice` | **Seller (Self)** | Invoice in Created state; compliance and debtor checks | Seller must hold pool position |
| `set_token_exposure_cap` | **Admin** | Inline admin check | Per-token aggregate cap |
| `get_token_exposure_cap` | Permissionless | Read-only | |
| `get_token_exposure_current` | Permissionless | Read-only | |
| `fund_invoice` | **Investor (Self)** | Not paused; priority-window and concentration checks | Converts via price_oracle |
| `fund_invoices_batch` | **Investor (Self)** | Same as fund_invoice per listing | Bounded atomic or best-effort batch |
| `cancel_listing` | **Seller (Self)** | Seller or admin (inline address check) | No funding yet |
| `set_priority_window` | **Admin** | Inline admin check | Whitelist-only funding period |
| `withdraw_listing` | **Seller (Self)** | Seller or admin (inline); unfunded only | Reverts NFT to Created |
| `request_cancellation` | **Seller (Self)** | Seller or admin; two-phase when funded | Stores request for admin confirm |
| `admin_confirm_cancellation` | **Admin** | Pending request must exist | Enables investor refunds |
| `claim_refund` | **Investor (Self)** | Cancellation confirmed or deadline passed unfunded | Refunds net contribution |
| `amend_listing` | **Seller (Self)** | Seller or admin (inline); unfunded only | Price/deadline/token edits |
| `get_listing_investors` | Permissionless | Read-only | Paged investor slice |
| `get_outstanding_contribution` | Permissionless | Read-only | Sums net contributions |
| `get_listing` | Permissionless | Read-only | |
| `propose_upgrade` | **Admin** | Multisig quorum if configured | Timelocked WASM proposal |
| `execute_upgrade` | **Admin** | Multisig quorum if configured; timelock elapsed | Applies WASM upgrade |
| `propose_admin_action` | **Governance** | Multisig signer; stores proposal | |
| `approve_admin_action` | **Governance** | Multisig signer; rejects duplicates | |
| `execute_admin_action` | **Governance** | Multisig signer; executes at threshold | Dispatches on threshold met |
| `get_admin_proposal` | Permissionless | Read-only | |
| `is_multisig_required` | Permissionless | Read-only | True when threshold above 1 |
| `propose_dependency_update` | **Admin** | Inline admin check; timelocked proposal | Dependency address rotation |
| `execute_dependency_update` | **Admin** | Inline admin check; timelock elapsed | Replaces one dependency address |

**Cross-Currency Funding:**
- `fund_invoice` converts investor token → listing token via `price_oracle`
- Authorization unchanged; oracle manipulation defenses apply (see ORACLE_ADVERSARIAL_TESTING.md)

---

### 5. financing_pool

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `migrate_versions` | **Admin** | Adjacent-step migration only | Shared schema migration helper |
| `schema_version` | Permissionless | Read-only | |
| `initialize` | None (one-time) | Must not already be initialized | Wires all dependency addresses |
| `release_funds` | **Marketplace Contract** | Caller must be marketplace | Opens pool; marks NFT funded |
| `set_max_position_bps` | **Admin** | Non-zero concentration cap | Per-investor cap |
| `set_late_penalty_split` | **Admin** | Treasury vs investor split | |
| `get_late_penalty_split` | Permissionless | Read-only | |
| `get_max_position_bps` | Permissionless | Read-only | |
| `get_grace_period` | Permissionless | Read-only | Default grace period |
| `set_marketplace` | **Admin** | Rejects self address | Authorized marketplace wiring |
| `set_auto_compound` | **Investor (Self)** | Opt-in flag per investor | Reinvestment preference |
| `get_auto_compound` | Permissionless | Read-only | |
| `get_price_oracle` | Permissionless | Read-only | Stored oracle address |
| `record_position` | **Admin** | Blocks if positions at cap (100) | DoS protection; marketplace-routed |
| `split_position` | **Investor (Self)** | Caller splits own position | New transferable share |
| `transfer_share` | **Investor (Self)** | Caller owns the share | OTC share transfer |
| `list_share_for_sale` | **Seller (Self)** | Caller owns the share | Single share offer |
| `buy_share` | **Buyer (Self)** | Share listed for sale | Buyer pays seller |
| `repay_partial` | **SME (Self)** | Invoice not frozen | Installment repayment |
| `repay` | **SME (Self)** | Approval required above threshold | Full or installment repayment |
| `net_settle` | **SME (Self)** | All invoices belong to payer; max 10 netted | Cross-invoice netting |
| `mark_default` | **Admin** | Grace period elapsed | Manual default declaration |
| `propose_repayment` | **Payer (Self)** | Amount at or above threshold | Single pending approval |
| `approve_repayment` | **Approver** | Caller in approver list; no duplicates | |
| `execute_repayment` | **Payer (Self)** | Approval count at 2 or more | Marks executed; runs repayment |
| `propose_early_settlement` | **SME (Self)** | Pool open; buyout bounds checked | Escrows buyout amount |
| `accept_early_settlement` | **Investor (Self)** | Pending offer; unanimous acceptance | Settles pool at 100 percent bps |
| `cancel_early_settlement` | **SME (Self)** | Offer not fully accepted | Refunds escrow |
| `get_early_settlement` | Permissionless | Read-only | Pending offer |
| `get_pool` | Permissionless | Read-only | Pool state for invoice |
| `get_positions` | Permissionless | Read-only | All positions as flat list |
| `get_protocol_stats` | Permissionless | Read-only | Aggregate protocol stats |
| `set_installment_schedule` | **Admin** | No repayment started yet | Validated sum and due dates |
| `get_installment_schedule` | Permissionless | Read-only | |
| `list_position_for_sale` | **Investor (Self)** | Caller owns the position | Whole-position offer (legacy) |
| `get_positions_page` | Permissionless | Read-only | Bounded page of positions |
| `buy_position` | **Buyer (Self)** | Position listed for sale | Moves position; buyer pays seller |
| `get_positions_count` | Permissionless | Read-only | |
| `propose_upgrade` | **Admin** | Timelocked WASM proposal | |
| `execute_upgrade` | **Admin** | Timelock elapsed | Applies WASM upgrade |
| `get_position` | Permissionless | Read-only | Zeroed default when absent |
| `transfer_position` | **Investor (Self)** | Caller is current holder; no self-transfer | Moves position between investors |

**Distribute Yield (Internal):**
- Private function called after repayment/settlement
- Iterates over positions (bounded by MAX_POSITIONS_PER_POOL)
- Auto-compound investors: attempts marketplace listing; falls back to wallet transfer

---

### 6. price_oracle

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Stores admin and access_control |
| `set_access_control` | **Admin** | Post-deployment wiring | Updates linked access_control |
| `set_price` | **Feeder** | Not paused; reciprocal/rate-change/peg checks | Fresh prices aggregated |
| `add_feeder` | **Admin** | Sets feeder flag | Feeders bounded by policy |
| `remove_feeder` | **Admin** | Removes feeder flag | |
| `set_base_currency` | **Admin** | For multi-hop triangulation | |
| `set_max_deviation` | **Admin** | Deprecated; use peg_config or max_rate_change | |
| `set_peg_config` | **Admin** | Validates ratio and tolerance | Stablecoin deviation tolerance |
| `remove_peg_config` | **Admin** | Removes peg config and flag | |
| `get_peg_config` | Permissionless | Read-only | |
| `clear_peg_flag` | **Admin** | Resume after deviation investigation | |
| `is_peg_flagged` | Permissionless | Read-only | |
| `set_max_staleness` | **Admin** | Non-zero per-pair staleness | |
| `get_max_staleness` | Permissionless | Read-only | Effective staleness or default |
| `set_fallback_feed` | **Admin** | Stores fallback feed address | |
| `remove_fallback_feed` | **Admin** | Removes fallback feed address | |
| `get_fallback_feed` | Permissionless | Read-only | |
| `get_price` | Permissionless | Read-only | Median of fresh feeders; fallback on all-stale |
| `get_price_at` | Permissionless | Read-only | Nearest business-day snapshot |
| `convert` | Permissionless | Read-only | Direct pair conversion; no-op when equal |
| `transfer_admin` | **Admin** | Writes new admin address | |
| `propose_upgrade` | **Admin** | 24h timelock proposal | WASM hash staging |
| `execute_upgrade` | **Admin** | Timelock elapsed | Applies WASM upgrade |
| `convert_with_decimals` | Permissionless | Read-only | Decimal-rescaled conversion |

**Defense Layers (non-authorization):**
- Staleness guard: prices older than max staleness excluded
- Reciprocal check: forward/reverse pair consistency (1% tolerance)
- Peg deviation: stablecoin auto-flag on excessive drift
- Rate-change bound: max percentage jump per update (configurable)

---

### 7. dispute_resolution

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Stores admin and access_control |
| `open_dispute` | **Challenger (Anyone)** | Invoice defaulted; no existing dispute; within window | Permissionless challenge |
| `submit_evidence` | **Challenger (Self)** | Caller must be original opener | IPFS CID evidence |
| `resolve_dispute` | **Governance** | Dispute open and unresolved | Resolver must be governance address |
| `get_dispute` | Permissionless | Read-only | |
| `has_open_dispute` | Permissionless | Read-only | |
| `propose_upgrade` | **Admin** | 24h timelock proposal | WASM hash staging |
| `execute_upgrade` | **Admin** | Timelock elapsed | Applies WASM upgrade |

**Governance Resolution:**
- `resolver` must be address configured as governance in contract initialization
- Upheld dispute may trigger verifier slashing (cross-contract call to risk_registry)

---

### 8. treasury

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Stores admin and fee bps |
| `set_fee_bps` | **Admin** | Blocked when linked multisig threshold above 1 | Fee update |
| `set_price_oracle` | **Admin** | Stores price oracle address | |
| `whitelist_token` | **Admin** | Idempotent token mark | |
| `set_marketplace` | **Admin** | Stores authorized marketplace contract | |
| `get_marketplace` | Permissionless | Read-only | |
| `collect_fee` | Permissionless | Permissionless accounting; amount only credited | Adds to Collected ledger |
| `refund_fee` | **Marketplace Contract** | Caller must be stored marketplace | Claws back collected fees |
| `withdraw` | **Admin** | Blocked when linked multisig threshold above 1 | Pause/cap/reentrancy gated |
| `withdraw_batch` | **Admin** | Atomic all-or-nothing batch | Multiple withdrawals |
| `emergency_withdraw` | **Admin** | Emergency declared; multisig quorum check | Full drain path |
| `declare_emergency` | **Admin** | Sets EmergencyDeclared flag | |
| `revoke_emergency` | **Admin** | Clears EmergencyDeclared flag | |
| `is_emergency_declared` | Permissionless | Read-only | |
| `is_paused` | Permissionless | Read-only | Cross-contract pause check |
| `propose_withdrawal_cap` | **Admin** | Stages per-token cap with timestamp | Timelocked cap proposal |
| `execute_withdrawal_cap` | **Admin** | Timelock elapsed | Applies staged cap |
| `get_withdrawal_cap` | Permissionless | Read-only | Zero means uncapped |
| `get_fee_bps` | Permissionless | Read-only | |
| `get_balance` | Permissionless | Read-only | Live token balance |
| `get_audit_log` | Permissionless | Read-only | Newest first |
| `get_audit_checksum` | Permissionless | Read-only | Rolling sha256 checksum |
| `get_audit_total` | Permissionless | Read-only | Monotonic entry count |
| `generate_transparency_report` | **Admin** | Builds per-token inflow/outflow report | Persists by epoch |
| `get_transparency_report` | Permissionless | Read-only | |
| `get_last_reported_epoch` | Permissionless | Read-only | |
| `get_collected` | Permissionless | Read-only | Per-token Collected ledger |
| `get_admin` | Permissionless | Read-only | |
| `get_total_collected_value` | Permissionless | Read-only | Aggregates via price oracle |
| `propose_admin` | **Admin** | Stages PendingAdmin | Two-step transfer |
| `accept_admin` | **PendingAdmin** | Caller must be proposed new admin | Completes handover |
| `cancel_admin_proposal` | **Admin** | Removes pending entry | |
| `propose_upgrade` | **Admin** | 24h timelock proposal | WASM hash staging |
| `execute_upgrade` | **Admin** | Timelock elapsed | Applies WASM upgrade |
| `propose_recipient` | **Admin** | Stages recipient with timestamp | Allowlist proposal |
| `execute_recipient` | **Admin** | Timelock elapsed | Matures staged recipient |
| `is_recipient_allowed` | Permissionless | Read-only | |
| `set_reserve_allocation_bps` | **Admin** | Validated 0 to 10000 | Loss reserve allocation |
| `set_reserve_caller` | **Admin** | Grants or revokes reserve authorization | |
| `disburse_from_reserve` | **Authorized Caller (Self)** | Caller must be authorized reserve caller | Draws down loss reserve |
| `get_reserve_balance` | Permissionless | Read-only | |
| `get_reserve_allocation_bps` | Permissionless | Read-only | |
| `is_reserve_caller` | Permissionless | Read-only | |
| `set_access_control` | **Admin** | Links access_control contract | Rejects self address |
| `get_access_control` | Permissionless | Read-only | |
| `propose_treasury_action` | **Governance** | Multisig signer proposes with own approval | Linked access_control signers |
| `approve_treasury_action` | **Governance** | Multisig signer; rejects duplicates | |
| `execute_treasury_action` | **Governance** | Multisig signer; threshold and timelock met | Dispatches withdrawal/fee/upgrade |
| `get_treasury_proposal` | Permissionless | Read-only | |
| `submit_grant_proposal` | **Anyone** | Whitelisted token; positive milestones | Public grant submission |
| `get_grant_proposal` | Permissionless | Read-only | |
| `propose_distribution` | **Admin** | Stages revenue distribution proposal | |
| `execute_distribution` | **Admin** | Locks in pending config for epoch | Clears proposal |
| `claim_share` | **Stakeholder (Self)** | Eligible stakeholder; pro-rata share | Draws from distribution pool |
| `get_distribution_epoch` | Permissionless | Read-only | |
| `get_distribution_config` | Permissionless | Read-only | |
| `get_pending_distribution` | Permissionless | Read-only | |
| `configure_sweep_splits` | **Admin** | Splits sum to exactly 10000 | Sweep destinations |
| `get_sweep_splits` | Permissionless | Read-only | |
| `get_sweep_primary_destination` | Permissionless | Read-only | Dust destination |
| `sweep` | Permissionless | Permissionless pump to splits and dust | Surplus distribution |
| `set_asset_allocation_policy` | **Admin** | Ordered min/max allocation bps | Per-asset policy |
| `get_asset_allocation_policy` | Permissionless | Read-only | |
| `check_allocation_drift` | Permissionless | Read-only | Drift entries outside policy |

---

### 9. secondary_market

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Stores admin, pool, treasury, fee |
| `pause` | **Admin** | Direct invoker must equal stored admin | No require_auth; invoker-equality check |
| `unpause` | **Admin** | Direct invoker must equal stored admin | No require_auth; invoker-equality check |
| `update_protocol_fee_bps` | **Admin** | Direct invoker must equal stored admin; 0 to 1000 bps | No require_auth; invoker-equality check |
| `list_position` | **Seller (Self)** | Pool open; seller holds financing_pool position; one active listing | Verifies position via pool cross-call |
| `buy_position` | **Buyer (Self)** | Listing exists; buyer is not seller; pool open | Downstream pool transfer_position requires seller auth (see pool hook note) |
| `cancel_listing` | **Seller (Self)** | Listing exists | Only lister may cancel; not pause-gated |

**Pool-Authorized Hook (cross-reference):**
- `buy_position` settles payment (fee to treasury, proceeds to seller) then calls `financing_pool.transfer_position`, which authenticates the seller (`from.require_auth()`). The seller's authorization is therefore enforced at the pool boundary even though `buy_position` itself only authenticates the buyer. Reviewers can trace `buy_position` here to `financing_pool.transfer_position` in section 5.
- Admin entrypoints here use `env.invoker()` equality against the stored admin instead of `admin.require_auth()` — weaker than the standard pattern; any change to add `require_auth` must update these rows (the checker will fail until then).

---

### 10. vesting

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Stores admin and token; see vesting.md |
| `create_grant` | **Admin** | Beneficiary must be new; schedule must satisfy vesting over cliff | Admin-authenticated grant; see vesting.md |
| `release` | Permissionless | Anyone may trigger; only vested amounts move | Funds always go to beneficiary; see vesting.md |
| `revoke_grant` | **Admin** | Grant must exist and be unrevoked | Vested portion stays claimable; see vesting.md |
| `get_vested_balance` | Permissionless | Read-only | Governance weight uses vested only |
| `get_unvested_balance` | Permissionless | Read-only | Total granted minus vested |
| `get_grant` | Permissionless | Read-only | Full grant struct or None |
| `get_admin` | Permissionless | Read-only | |
| `get_token` | Permissionless | Read-only | Managed token address |
| `propose_upgrade` | **Admin** | 24h timelock proposal | WASM hash staging |
| `execute_upgrade` | **Admin** | Timelock elapsed | Applies WASM upgrade |

**Vesting Authorization Notes (see `docs/vesting.md` and `docs/verifier-nomination.md`):**
- Grant creation and revocation are admin-only (`admin.require_auth()` plus stored-admin equality); release is intentionally permissionless because token transfers can only credit the beneficiary.
- Governance weight must use `get_vested_balance`, never the total grant — unvested balances are excluded by construction.

---

## Conditional Authorization Patterns

### 1. Protocol Pause

**Affects:**
- `marketplace.fund_invoice` → BLOCKED
- `marketplace.list_invoice` → BLOCKED
- `financing_pool.repay` → BLOCKED
- `financing_pool.record_position` → BLOCKED
- `price_oracle.set_price` → BLOCKED
- `secondary_market.list_position` → BLOCKED
- `secondary_market.buy_position` → BLOCKED

**Implementation:**
```rust
fn require_not_paused(env: &Env) -> Result<(), Error> {
    let ac: Address = env.storage().instance().get(&DataKey::AccessControl)?;
    let ac_client = AccessControlClient::new(env, &ac);
    if ac_client.is_protocol_paused() {
        return Err(Error::ProtocolPaused);
    }
    Ok(())
}
```

### 2. Invoice State Guards

**Transitions:**
- `Created` → `Funded`: only `marketplace.fund_invoice` (when fully funded)
- `Funded` → `Repaid`: only `financing_pool.repay*` or `net_settle`
- Any → `Defaulted`: only `financing_pool.mark_default` (admin) or `admin.set_defaulted`

**Enforcement:** `invoice_nft` state machine validates caller address matches authorized contract for each transition.

### 3. Multisig Quorum

**Applies to:**
- All `access_control` admin operations when `multisig_threshold > 1`
- `marketplace.set_fee_bps` and other config changes
- `financing_pool` admin operations

**Implementation:** Admin must call `access_control.approve_pending_action` to collect signatures; final caller submits with quorum attached.

### 4. Rate Limiting

**Applies to:**
- `invoice_nft.mint_invoice*`: configurable max invocations per time window per SME
- Enforced via `MintRateLimit` storage tracking `(sme, window_start, count)`

### 5. Secondary-Market Pool Hook

**Applies to:**
- `secondary_market.buy_position` → `financing_pool.transfer_position`

**Enforcement:** `buy_position` authenticates only the buyer; position ownership moves through the pool's `transfer_position`, which requires the seller's authorization (`from.require_auth()`). The listing is cleared before external calls (checks-effects-interactions), and payment splits (protocol fee to treasury, proceeds to seller) settle before the ownership transfer.

---

## Automated Verification Checker

### Tool: `contracts/xtask/src/bin/check_access_matrix.rs`

**Purpose:** Static analysis tool that parses the per-contract tables in this file and verifies every `pub fn` has authorization matching its row. The markdown file is the single source of truth — the tool carries no hardcoded copy of the matrix.

**Markdown parsing:**
- Section headings (`### 9. secondary_market`) set the contract scope.
- Each `| Entrypoint | Required Role | Conditional Authorization | Notes |` row becomes one expected entry.
- `Required Role` cells map to authorization variants: `Admin` (promoted to `AdminWithMultisig` when the conditional mentions multisig or quorum), `Verifier`, `Feeder`, `Governance`, contract names (`Marketplace Contract`, `FinancingPool or Marketplace`, …), `Self`/`SME`/`Investor`/`Challenger`/caller roles, `Permissionless`, and `None (one-time)`.
- Unrecognized roles and malformed rows are reported with markdown line numbers.

**Detection Rules:**

1. **Admin-Only Functions:**
   - Must call `require_admin(&env, &caller)` OR `admin.require_auth()` + `Self::require_admin`
   - Multisig-gated when the body references multisig config or quorum checks
2. **Doc comments are stripped before detection** so prose mentioning authorization never affects classification — only implemented checks count.
3. **`#[cfg(test)]` modules are skipped** so mocks and unit tests are never treated as entrypoints.

4. **Role-Specific Functions:**
   - Verifier: must call `require_verifier` (including `_flag` / `_primary` variants), `resolve_verifier`, or check `is_verifier`
   - Feeder: must call `require_feeder`
   - SME/Investor: must call `caller.require_auth()` + ownership check

5. **Contract-to-Contract:**
   - Must validate `env.current_contract_address() == expected_caller`
   - Or check against storage-configured authorized contract address

6. **Permissionless (Read-Only):**
   - No `require_auth()` calls
   - No state mutations (returns `Result<T, E>` with no writes)

**Checker Algorithm:**
```rust
fn verify_entrypoint(func: &syn::ItemFn, contract: &str) -> Result<(), Drift> {
    let expected_auth = ACCESS_MATRIX.get(contract, func.sig.ident)?;
    let actual_auth = parse_authorization_calls(func)?;

    if actual_auth != expected_auth {
        return Err(Drift {
            contract,
            function: func.sig.ident,
            expected: expected_auth,
            actual: actual_auth,
        });
    }
    Ok(())
}
```

**CI Integration:**
```bash
# Run from the repository root on every PR
cargo run -p kora-xtask --bin check_access_matrix
# Exit code 1 if any drift detected
```

Run with `--contract <name>` to check a single contract and `--verbose` to dump every parsed entry and per-function result for debugging.

**Drift Handling:**
- Fail CI with error message: "Authorization drift detected in {contract}::{function}. Expected {role}, found {actual}. Update docs/ACCESS_MATRIX.md or fix code."
- Prevents accidental privilege escalation or forgotten authorization checks

---

## New Entrypoint Checklist

When adding a new public function to any contract:

1. ☑️ Add entry to this ACCESS_MATRIX.md with required role/authorization
2. ☑️ Implement authorization check matching the matrix entry
3. ☑️ Add test coverage for:
   - ✅ Authorized caller succeeds
   - ✅ Unauthorized caller rejected
   - ✅ Edge case: partial authorization (e.g., wrong SME calling on different invoice)
4. ☑️ Run `cargo run -p kora-xtask --bin check_access_matrix` locally before commit
5. ☑️ Document any conditional authorization (pause, state, multisig)

**Adding a new contract to `scripts/deploy.sh`?** Also add all its `pub fn` rows below (issues #917).

**Undocumented New Entrypoint = CI Failure**

The automated checker flags any `pub fn` not present in this matrix as an error, forcing developers to consciously decide and document authorization requirements.

---

### 9. secondary_market (added by issue #917)

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | | Sets admin, financing_pool, treasury, fee |
| `pause` | **Admin** | | Halts all market operations |
| `unpause` | **Admin** | | Resumes market operations |
| `update_protocol_fee_bps` | **Admin** | fee ≤ 1000 bps | |
| `list_position` | **Investor (Self)** | Not paused; seller holds the position | Creates sale offer |
| `buy_position` | **Buyer (Self)** | Not paused; listing exists; buyer ≠ seller | CEI: clear listing before transfers |
| `cancel_listing` | **Seller (Self)** | Listing exists | Removes offer |

---

### 10. fractionalizer (added by issue #917)

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | | |
| `wrap` | **Position Owner (Self)** | Caller holds the position | Tokenises position into shares |
| `transfer_shares` | **Share Owner (Self)** | | OTC share transfer |
| `settle` | **Admin** | | Distributes proceeds to all shareholders |
| `redeem` | **Share Owner (Self)** | Settlement completed | Withdraws pro-rata proceeds |
| `unwrap` | **Original Owner (Self)** | All shares still held by wrapper | Reverses fractionalisation |
| `get_wrap` | Permissionless | Read-only | |
| `get_share_balance` | Permissionless | Read-only | |

---

### 11. dispute_escrow (added by issue #917)

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | | Sets admin and pool |
| `open_dispute` | **Challenger (Self)** | Invoice funded; no existing dispute | |
| `resolve_dispute` | **Admin** | Dispute open | Releases/returns escrow funds |
| `resolve_timeout` | **Anyone** | Dispute window elapsed | Permissionless timeout resolution |
| `get_dispute` | Permissionless | Read-only | |

---

### 12. governance_voting (added by issue #917)

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | | |
| `set_voting_weight` | **Admin** | | Sets stake weight for an address |
| `get_voting_weight` | Permissionless | Read-only | |
| `delegate` | **Voter (Self)** | Caller has voting weight | Standing delegation |
| `revoke_delegation` | **Voter (Self)** | Delegation exists | |
| `get_delegate` | Permissionless | Read-only | |
| `create_proposal` | **Voter (Self)** | Stake ≥ min_stake | |
| `vote` | **Voter (Self)** | Proposal active; not already voted | |
| `get_proposal` | Permissionless | Read-only | |
| `get_vote_record` | Permissionless | Read-only | |
| `execute_proposal` | **Admin** | Passed; timelock elapsed | |

---

### 13. referral_rewards (added by issue #917)

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | | |
| `deposit` | **Admin** | | Loads reward pool |
| `claim_referral_reward` | **SME (Self)** | Eligible; not yet claimed | |
| `has_claimed` | Permissionless | Read-only | |
| `paid_total` | Permissionless | Read-only | |

---

## Contributor Guide: Adding New Matrix Rows

When you add a new `pub fn` to any contract in `contracts/` **or** add a new contract to `scripts/deploy.sh`:

1. Add the row to this file under the appropriate contract section.
2. Run `cargo run -p kora-xtask --bin check-access-matrix` locally to verify no drift.
3. The CI `access-matrix-check` job will block the PR if the row is missing.

Naming convention for the `contracts` column: use the snake_case crate directory name (e.g. `secondary_market`, not `kora-secondary-market`).

---

## Security Audit Trail

**Last Full Audit:** 2026-10-17 (this wave)

**Changes Since Last Audit:**
- Added `MAX_POSITIONS_PER_POOL` bound in `financing_pool.record_position`
- Added `max_rate_change` guard in `price_oracle.set_price`
- Added `MAX_NETTING_INVOICES` bound in `financing_pool.net_settle`
- Added `MAX_VERIFIERS_PER_DEBTOR` bound in `risk_registry.set_debtor_score`
- Added sections 9 (secondary_market) and 10 (vesting): all public entrypoints documented with implemented `require_auth` checks (issue #1000)
- Replaced the hardcoded checker matrix with a markdown parser over this file; `#[cfg(test)]` mocks excluded (issue #999)
- Fixed corrupted duplicate `create_grant` in `contracts/vesting/src/lib.rs`

**Known Authorization Gaps:** None as of 2026-10-17.

---

## Appendix: Authorization by Severity

### Critical (Admin-Only, Irreversible)

- `access_control.transfer_admin` / `rotate_admin`
- `access_control.pause` (emergency protocol pause)
- `marketplace.execute_token_whitelist` (after timelock)
- `invoice_nft.set_defaulted`
- `financing_pool.mark_default`

### High (Admin Config, Reversible)

- `marketplace.set_fee_bps`
- `price_oracle.add_feeder` / `remove_feeder`
- `risk_registry.add_verifier` / `remove_verifier`
- `financing_pool.set_max_position_bps`
- `vesting.create_grant` / `vesting.revoke_grant`
- `secondary_market.update_protocol_fee_bps`
- `treasury.set_fee_bps` / `treasury.withdraw`

### Medium (User-Owned Resources)

- `invoice_nft.mint_invoice` (SME)
- `marketplace.list_invoice` (SME)
- `financing_pool.repay` (SME)
- `marketplace.fund_invoice` (Investor)
- `secondary_market.list_position` (Seller)
- `secondary_market.buy_position` (Buyer)

### Low (Permissionless Reads)

- All `get_*` / `is_*` view functions
- `marketplace.claim_refund` (conditional but permissionless)
- `vesting.release` (anyone may trigger; beneficiary-only payout)
- `dispute_resolution.open_dispute` (permissionless challenge)

---

## References

- Multisig implementation: `contracts/access_control/src/lib.rs`
- Audit trail: `contracts/shared/src/audit.rs`
- Role-based testing: `contracts/tests/access_control_edge_cases.rs`
- Upgrade timelock: `UPGRADE_TIMELOCK_DELAY` constant (86,400 seconds = 24 hours)
- Vesting authorization model: `docs/vesting.md`
- Verifier nomination flows: `docs/verifier-nomination.md`
- Secondary-market settlement flow: `contracts/tests/issue_471_secondary_market_settlement.rs`
- Pentest coverage: `contracts/tests/access_control_pentest.rs`
