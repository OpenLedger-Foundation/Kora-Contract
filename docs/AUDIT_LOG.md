# Kora Protocol — Audit Log

This document tracks audit findings, fixes, and their resolution across all protocol releases.

---

## Issue #1158: Audit-log tamper-evident hash chain verification tool

**Status:** ✅ Tool and tests added.

**Severity:** High (auditability)

**Description:**

Audit-log entries are chained by a tamper-evident hash (`prev_hash` → `entry_hash`).
Without a verifier, gaps or tampering in the chain cannot be detected during an
incident, making the log unauditable.

**Location:**

- `services/audit-log/` — chain writer and entry types
- `scripts/verify-audit-log` — chain walker / verifier

**Fix Applied:**

Added a verification tool that walks the audit-log hash chain in order and fails
closed on any of the following:

- **Gap** — a missing sequence number or broken `prev_hash` link.
- **Tamper** — recomputed `entry_hash` does not match the stored hash.
- **Empty/missing config** — no log path or empty chain returns a typed error
  instead of silently succeeding.
- **Paused/degraded dependency** — verification reports the degraded state and
  exits non-zero rather than treating it as a pass.

Re-entry is idempotent: re-running the verifier over an unchanged chain yields
the same result and exit code.

**Verification:**

- Happy path: a well-formed chain verifies with exit code `0`.
- Negative: a mutated entry, a removed entry (gap), and an empty/missing log all
  fail with typed errors and non-zero exit codes.
- Edge: paused/degraded dependency and repeated runs behave as described above.

Run targeted tests for the touched package and the verifier script.

**Cross-References:**

- [CHANGELOG.md](CHANGELOG.md) → [Unreleased] → Added
- [GitHub Issue](https://github.com/OpenLedger-Foundation/Kora-Contract/issues/1158)

---

## Issue #914: Dispute custody callbacks and reentry protection

**Status:** Code changes and regression cases added; runtime validation pending.

Escrow called pool reservation/settlement hooks that were absent from the pool,
and custody mutations had no shared lock around external token/pool calls.
The pool now authenticates its one-time configured escrow, reserves exact
amounts, and validates settlement before updating repayment accounting. The
escrow uses the shared RAII guard for open, adjudication, and timeout paths.
Existing error discriminants and stored pool/dispute record shapes are retained.

`contracts/tests/dispute_cross_contract.rs` adds cross-contract regression
scenarios, including unauthorized callbacks, duplicate settlement, failed
transfers, and adversarial-token reentry. The implementation received static
review only: no application code, tests, builds, or formatters were executed,
as directed by `task2.txt`. See [scenario matrix](dispute-escrow.md#cross-contract-scenarios-914).

---

## Finding B27: Event Deduplication in SME Profile Tracking

**Status:** ✅ Fixed in [Unreleased]

**Severity:** Low

**Description:**

The shared events module contained a duplicate event `sme_invoice_counted` that was redundant with `sme_invoice_count_incremented`. This duplication created confusion in off-chain analytics and event indexing.

**Location:**

- `contracts/shared/src/events.rs` — event definitions

**Fix Applied:**

Removed `sme_invoice_counted` event. All SME invoice count tracking now uses `sme_invoice_count_incremented`.

**Verification:**

1. Grep the codebase for `sme_invoice_counted` — should return no results
2. Verify all tests pass: `cargo test --all`
3. Review event emission sites in risk_registry and financing_pool contracts to confirm only the incremented event is used

**Cross-References:**

- [CHANGELOG.md](CHANGELOG.md) → [Unreleased] → Fixed
- [CONTRIBUTING.md](CONTRIBUTING.md) → Changelog Process section

---

## Finding B16: Marketplace Token Whitelist Enforcement

**Status:** ✅ Mitigated by design

**Severity:** Medium

**Description:**

Only whitelisted tokens can be used for invoice funding. Protects against accidental or malicious use of untrusted tokens.

**Location:**

- `contracts/marketplace/src/lib.rs` — `whitelist_token()`, `fund_invoice()` validation

**Mitigation:**

The marketplace contract enforces a token whitelist:

```rust
if !env.storage().persistent().has(&DataKey::WhitelistedToken(token.clone())) {
    return Err(KoraError::TokenNotWhitelisted);
}
```

Whitelist is admin-controlled. Tokens can only be added/removed by the admin address.

**Verification:**

- Unit test: `test_fund_invoice_with_unlisted_token` confirms rejection
- Integration test: `test_marketplace_lifecycle` confirms whitelisted tokens work

---

## Finding B2: Multisig Admin Requirements

**Status:** ⏳ Planned for v2

**Severity:** High

**Description:**

Single admin key control creates operational and key-management risk. Multisig (M-of-N threshold) is recommended for production deployments.

**Location:**

- `contracts/access_control/src/lib.rs` — `transfer_admin()` single-key design

**Mitigation (Current):**

- Admin key is protected and operated by the Kora Foundation
- Pause mechanism allows immediate emergency freeze of protocol
- All admin actions are logged and auditable via event emission

**Planned Fix (v2):**

- Replace single `admin: Address` with `admins: Vec<Address>` and `threshold: u32`
- Implement `propose_admin_action()` and `approve_admin_action()` with timelock
- Require M-of-N signatures for sensitive operations (pause, fee changes, token whitelist)

**Target Release:** v2.0.0 (Q3 2026)

---

## Audit Finding Template

Use this template when documenting new findings:

```markdown
## Finding BXX: [Short Title]

**Status:** [✅ Fixed | ⏳ Planned | 🚨 Open]

**Severity:** [Critical | High | Medium | Low]

**Description:**

[What is the issue and why does it matter?]

**Location:**

- `path/to/file.rs` — relevant functions

**Fix Applied / Mitigation:**

[What was done or what mitigates the risk?]

**Verification:**

- [How to verify the fix works]

**Cross-References:**

- [CHANGELOG.md](CHANGELOG.md) → [version] → Fixed
- [GitHub Issue](https://github.com/OpenLedger-Foundation/Kora-Contract/issues/XXX)
```

---

## Release Checklist

Before releasing a new version:

1. ✓ All open findings are resolved or have a documented timeline
2. ✓ CHANGELOG.md reflects all fixes and new features
3. ✓ AUDIT_LOG.md is updated with resolution status
4. ✓ Code review and security sign-off obtained
5. ✓ All tests pass: `make fmt && make lint && cargo test --all`
6. ✓ WASM binaries built and hashes recorded in release notes

---

*Last updated: 2026-06-27*
This file transcribes the embedded audit findings that were documented inline in
`contracts/financing_pool/src/lib.rs` and `contracts/risk_registry/src/lib.rs`
during the internal code-review pass. Findings have since been resolved; this
document records what was found, the severity, and how each issue was fixed so
the history is traceable without reading git blame.

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to report new security issues.

---

## Financing Pool (`contracts/financing_pool`)

Source: doc-block removed after remediation (commit `b834849` and `75a2367`).

| # | Finding | Severity | Status |
|---|---------|----------|--------|
| FP-01 | Unsafe `unwrap()` calls on storage reads (lines ~63, ~188, ~266) | Medium | Fixed |
| FP-02 | CEI violation: token transfer occurred before state update in `repay()` | High | Fixed |
| FP-03 | Missing reentrancy guard in `repay()` — no lock to prevent concurrent transfers | High | Fixed |
| FP-04 | Reentrancy guard used wrong error type (`ProtocolPaused` instead of a dedicated error) | Low | Fixed |
| FP-05 | Missing pause checks in `release_funds`, `record_position`, `repay`, `mark_default` | Medium | Fixed |
| FP-06 | Duplicate amount validation — same check performed twice | Low | Fixed |
| FP-07 | Silent arithmetic failure: `unwrap_or(0)` in yield calculation hid underflow | Medium | Fixed |
| FP-08 | Missing initialization check in `release_funds` — pool could be double-created | Medium | Fixed |
| FP-09 | Incomplete event emissions — `release_funds` and `record_position` emitted no events | Low | Fixed |
| FP-10 | No upper-bound validation on amounts (overflow risk on `i128` arithmetic) | Medium | Fixed |
| FP-11 | No cross-contract validation that the `marketplace` caller is the registered marketplace | Low | Deferred to v2 |
| FP-12 | Pool `token` field initialized as a placeholder instead of the value passed by marketplace | Medium | Fixed |

### Detail

**FP-01 — Unsafe unwrap().**
Storage reads in `release_funds`, `record_position`, and `repay` called `.unwrap()` directly.
On a cold contract address (not yet initialized) this would trap the transaction with no
meaningful error. Fixed by replacing every storage read with `.ok_or(KoraError::NotInitialized)?`.

**FP-02 — CEI violation in repay().**
`pool.repaid_amount` was updated *after* the `token::Client::transfer` call, violating the
Checks-Effects-Interactions pattern. A reentering call during the transfer could exploit the
stale state. Fixed by moving all state writes before the cross-contract transfer.

**FP-03 — Missing reentrancy guard in repay().**
Even with the CEI fix, `repay()` lacked a storage-backed lock. Added `RepaymentLock(u64)`
persistent storage entry that is set before and cleared after the function body, causing any
re-entrant `repay()` call on the same invoice to return `Unauthorized`.

**FP-04 — Wrong error type for reentrancy guard.**
The guard initially returned `KoraError::ProtocolPaused` when triggered. Fixed by using the
dedicated `KoraError::Reentrancy` variant (added in `kora_shared::errors`).

**FP-05 — Miss

/* … truncated 4359 chars — edit only what you need near the top … */
