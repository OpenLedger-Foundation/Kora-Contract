# Risk Registry

The `risk_registry` contract is the trust anchor for the Kora protocol. It manages verifier
on-boarding, SME creditworthiness scoring, debtor scoring, and the credit-limit enforcement
that prevents over-exposure. Every invoice minted by `invoice_nft` and every funding decision
in `marketplace` ultimately depends on the data held here.

---

## Overview

```
Admin
 │── add_verifier(verifier, stake) ──► Verifier registered (stake escrowed)
 │
Verifier
 │── register_sme(sme, score, compliance_attested) ──► SmeProfile created
 │── update_sme_score(sme, new_score)               ──► score + risk_tier updated
 │── set_credit_limit(sme, limit)                   ──► credit ceiling set
 │── set_debtor_score(debtor_hash, score)            ──► debtor scored
 │
invoice_nft
 │── increment_invoice_count(sme)  ──► total_invoices++
 │
Admin
 └── record_default(sme)           ──► defaults++, verifier stake slashed
```

---

## Verifier Trust Model

### Registration

A verifier is a trusted, off-chain KYC/AML provider or credit bureau. Before a verifier
can score any SME they must be registered by the admin with a staking deposit:

```
add_verifier(admin, verifier, stake_amount)
```

- `stake_amount` must be ≥ `minimum_stake` (set at initialization).
- The stake is transferred from `verifier` to the registry contract immediately.
- Initial `reputation` is set to **100**.

### Reputation & Slashing

Verifier reputation is a signal of historical accuracy. When an SME that a verifier
registered defaults on an invoice, the admin calls `record_default(admin, sme)`:

1. `SmeProfile.defaults` is incremented.
2. A fraction of the verifier's remaining stake is slashed: `slash = stake * slash_percentage_bps / 10_000`.
3. The slashed amount is burned (transferred to the zero address / left in contract).
4. The verifier's `reputation` score is decremented by 1 per default (floor: 0).

Verifiers with a depleted stake or low reputation are a signal to the admin to
call `remove_verifier`, which returns any remaining stake and purges the verifier records.

### Removal

```
remove_verifier(admin, verifier)
```

Removes all three verifier storage entries (`Verifier`, `VerifierStake`,
`VerifierReputation`) and returns the remaining (unslashed) stake to the verifier address.

### Rotation lifecycle (Issue #739)

```
Active ──suspend_verifier──► Suspended ──reinstate_verifier──► Active
  │                              │
  └─request_verifier_removal──► PendingRemoval ──finalize_verifier_removal──► Removed
         (Suspended may also request)      ▲ 7-day cooldown           │ 7-day re-add cooldown
                                           │                          ▼
                              cancel_verifier_removal               Active (re-added)
```

| Function | Transition | Effect |
|---|---|---|
| `suspend_verifier(admin, verifier)` | `Active` → `Suspended` | Blocks all new submissions (`register_sme`, `update_sme_score`, `set_credit_limit`, `set_debtor_score`, `top_up_stake`, sub-account delegation). Historical SME profiles and debtor attestations remain readable. Suspended weight **still counts** toward the `get_debtor_score` composite aggregate until formal removal. |
| `reinstate_verifier(admin, verifier)` | `Suspended` → `Active` | Restores full submission rights. |
| `request_verifier_removal(admin, verifier)` | `Active`/`Suspended` → `PendingRemoval` | Starts the 7-day removal cooldown (`VERIFIER_REMOVAL_COOLDOWN_SECS`). Submissions blocked; weight still counts pending finalization. |
| `cancel_verifier_removal(admin, verifier)` | `PendingRemoval` → `Active` | Aborts a pending removal without touching stake or reputation. |
| `finalize_verifier_removal(admin, verifier)` | `PendingRemoval` → `Removed` (after cooldown) | Returns remaining stake, clears the active flag, records `VerifierLastRemovedAt`. Per-verifier debtor attestation rows are **preserved** (never deleted) but excluded from the aggregate going forward — exclusion is prospective, not retroactive. |
| `remove_verifier(admin, verifier)` | any → `Removed` (immediate) | Backward-compatible immediate-removal escape hatch. Records the same `Removed` marker and re-add cooldown timestamp as the two-step flow. Prefer the request/finalize flow for planned rotations. |

Views: `get_verifier_status(verifier) → VerifierStatus`, `get_verifier_status_changed_at(verifier) → Option<u64>`, `get_verifier_removal_deadline(verifier) → Option<u64>` (earliest finalization timestamp).

Re-addition anti-bypass: `add_verifier` rejects a `Removed` address until `VERIFIER_READD_COOLDOWN_SECS` (7 days) has elapsed (`ReadditionCooldownNotElapsed`), preventing immediate re-registration to launder slashing/reputation history. `Suspended`/`PendingRemoval` addresses cannot be re-added at all (`AlreadyInitialized`) — use reinstate/cancel/finalize instead.

Errors: `VerifierSuspended` (submission while not `Active`), `RemovalCooldownNotElapsed`, `NoRemovalRequested`, `InvalidStatusTransition`, `ReadditionCooldownNotElapsed`.

---

## SME Registration & Scoring Lifecycle

### Registration

Only a registered verifier can introduce an SME to the protocol:

```
register_sme(verifier, sme, risk_score, compliance_attested)
```

This creates an `SmeProfile` entry:

| Field | Initial value |
|---|---|
| `address` | `sme` |
| `verified` | `true` |
| `verifier` | caller |
| `risk_score` | 0–100 |
| `total_invoices` | 0 |
| `defaults` | 0 |
| `registered_at` | current ledger timestamp |
| `compliance_attested` | caller's attestation flag |

Re-registration of an existing SME is rejected (`KoraError::AlreadyInitialized`) to
prevent silent reset of default and invoice counters.

### Score Updates

A verifier may update the risk score at any time:

```
update_sme_score(verifier, sme, new_score)
```

The `risk_score` and derived `risk_tier` (see below) in the `SmeProfile` are updated
atomically. This affects future invoice minting only — already-minted invoices retain
the score they were issued with.

### Credit Limit

A verifier sets a maximum outstanding face-value exposure for an SME:

```
set_credit_limit(verifier, sme, credit_limit)
```

- `credit_limit = 0` means **uncapped**.
- When `invoice_nft::mint_invoice` is called, it queries
  `invoice_nft::get_outstanding_exposure(sme)` and rejects the mint if
  `outstanding + new_amount > credit_limit`.

### Verifier-of-Record Model & Reassignment

To prevent unauthorized changes, both `update_sme_score` and `set_credit_limit` enforce that the calling verifier (or their resolved primary verifier address) is the designated **verifier-of-record** stored in the SME's profile (`SmeProfile.verifier`). Any attempt by an unrelated verifier to modify these parameters will fail with `RiskRegistryError::NotSmeVerifier`.

If a verifier is removed or custody needs to be transferred, the admin can reassign the verifier-of-record for an SME:

```
reassign_sme_verifier(admin, sme, new_verifier)
```
- `new_verifier` must be an active, registered verifier.
- Reassignment updates `SmeProfile.verifier` to the new address and emits the `sme_verifier_reassigned` event.

---

## Risk Tier Thresholds

The `RiskTier` is derived from the numeric `risk_score` (0–100) via
`RiskTier::from_score`:

| Tier | Score Range | Risk Profile |
|------|-------------|--------------|
| AAA  | 0–20        | Lowest risk. Blue-chip debtors, strong SME repayment history. |
| AA   | 21–40       | Low risk. Established SME, reliable debtor. |
| A    | 41–60       | Moderate risk. Standard trade finance profile. |
| B    | 61–80       | Elevated risk. Newer SME or less-known debtor. |
| C    | 81–100      | High risk. Requires higher yield to attract investors. |

The tier is stored directly on the `Invoice` struct at mint time. Marketplace
interfaces may surface tier information to investors to aid funding decisions.

---

## Debtor Scoring

Debtors are the counterparties who owe payment on the underlying invoices. To preserve
privacy, debtor PII is never stored on-chain — only a **SHA-256 hash** of the debtor
identity is used as the key.

Debtor risk scores are tracked per-verifier as independent attestations rather than a single overwritable global value:

```
set_debtor_score(verifier, debtor_hash, score)
```

- `debtor_hash` must be exactly 32 bytes (the raw SHA-256 digest).
- `score` is 0–100, following the same `RiskTier` mapping as SME scores.
- Only registered verifiers may set debtor scores.
- Updates are saved as independent attestations keyed by `(debtor_hash, verifier)`, with a per-verifier cooldown (`MIN_SCORE_UPDATE_INTERVAL = 3,600s`) enforced per attestation.

Queries:

```
get_debtor_score(debtor_hash) → Result<u32, RiskRegistryError::DebtorNotRegistered>
```
Computes and returns the aggregated average score across all active verifiers' attestations for `debtor_hash`. Unregistered/removed verifiers are excluded from the aggregate computation.

```
get_debtor_score_attestation(verifier, debtor_hash) → Result<u32, RiskRegistryError::DebtorNotRegistered>
```
Returns an individual verifier's specific score attestation for `debtor_hash`.

---

## Invoice Count Tracking

`invoice_nft` calls `increment_invoice_count(caller, sme)` automatically each time a
new invoice is minted. Only the `invoice_nft` address registered at initialization may
call this function.

This increments `SmeProfile.total_invoices`, providing an on-chain record of SME
activity volume for risk analysis.

---

## Authorized Callers (Issue #1078)

The system score mutators accept their built-in caller **or** an address on the
admin-managed authorized caller registry (`DataKey::AuthorizedCaller(Address)`):

| Mutator | Built-in caller | Also allowed |
|---|---|---|
| `increment_invoice_count` | `invoice_nft` (set at initialization) | authorized callers |
| `record_default` | admin | authorized callers |
| `record_successful_repayment` | admin | authorized callers |

Any other caller is rejected with `RiskRegistryError::Unauthorized`. Verifier score
mutators (`register_sme`, `update_sme_score`, `set_credit_limit`, `set_debtor_score`)
remain scoped by the verifier registry and reject unknown callers with `NotVerifier`.

Managing the registry (admin only):

- `add_authorized_caller(admin, caller)`: rejected while the protocol is paused and for
  the registry's own address. Emits `(RR_CALLR, caller)` → `true`.
- `remove_authorized_caller(admin, caller)`: allowed while paused, so a compromised
  integration can be cut off during an incident. The caller's next call fails with
  `Unauthorized`. Emits `(RR_CALLR, caller)` → `false`.
- `is_authorized_caller(caller)`: read-only.

Authorized callers are still subject to the protocol pause (`ProtocolPaused`).

### Migrating existing callers

Existing deployments keep working unchanged: `invoice_nft` and the admin are still
accepted, so no step is required for them. To let a contract or keeper write
without holding the admin key:

1. Deploy or upgrade risk_registry to a build that includes #1078.
2. For each integration that records outcomes on-chain (for example `financing_pool`,
   which calls `record_default` after a default, or an operations keeper), call
   `add_authorized_caller(admin, <integration address>)`.
3. Have the integration pass **its own address** as the `caller` / `admin` argument.
   `financing_pool` currently passes its stored admin address. Switching it to
   `env.current_contract_address()` is a separate change, and should be done
   together with step 2.
4. Verify with `is_authorized_caller(<address>)`. When rotating or decommissioning
   an integration, call `remove_authorized_caller`.

---

## Read-Only View Functions

| Function | Returns |
|---|---|
| `get_sme_profile(sme)` | Full `SmeProfile` struct, or `KoraError::SMENotRegistered` |
| `is_verified_sme(sme)` | `true` if the SME has a profile and `verified == true` |
| `is_compliance_attested(sme)` | `true` if the verifier attested KYC/AML compliance |
| `get_verifier_stake(verifier)` | Remaining stake in token units (0 if not registered) |
| `get_verifier_reputation(verifier)` | Reputation score 0–100 (0 if not registered) |
| `is_verifier(verifier)` | `true` if the address is a registered verifier |
| `get_debtor_score(debtor_hash)` | Aggregated average debtor score across active verifiers, or `RiskRegistryError::DebtorNotRegistered` |
| `get_debtor_score_attestation(verifier, debtor_hash)` | Specific verifier's score attestation, or `RiskRegistryError::DebtorNotRegistered` |
| `get_verifier_status(verifier)` | `VerifierStatus` lifecycle state (`Active`/`Suspended`/`PendingRemoval`/`Removed`) |
| `get_verifier_status_changed_at(verifier)` | Timestamp of last status transition, if any |
| `get_verifier_removal_deadline(verifier)` | Earliest finalization timestamp for a pending removal, if any |
| `get_access_control()` | Configured `access_control` address, if any |
| `get_admin()` | Current admin address |

---

## Access Control Synchronization

When an `access_control` contract address is configured via `set_access_control(admin, access_control)`, `risk_registry` synchronizes verifier lifecycle events with `access_control` roles:
- `add_verifier`: grants `Role::Verifier` to the verifier address.
- `remove_verifier` / `finalize_verifier_removal`: revokes `Role::Verifier` following stake return per reentrancy audit ordering.

All read functions are authorization-free and safe to call from any context.

---

## Storage & TTL

All SME profiles and verifier entries use **persistent** storage with a ~30-day TTL
(518 400 ledgers). The operator or a keeper bot must periodically call `extend_ttl` on
active profiles to prevent expiry. Expired profiles are treated as unregistered by the
contract — `is_verified_sme` returns `false` for expired keys.

---

## Security Notes

- Verifier registration requires a token stake deposit that is slashed on SME defaults,
  creating skin-in-the-game incentives for honest scoring.
- Only the admin may add or remove verifiers; only verifiers may register or score SMEs.
- `increment_invoice_count` is callable only by the `invoice_nft` contract address
  set at initialization or an admin-approved authorized caller (#1078). It cannot be
  spoofed by an arbitrary caller.
- Debtor PII never appears on-chain; only SHA-256 hashes are stored.
- Reentrancy protection (`ReentrancyGuard`) is applied on `update_sme_score` which
  performs an inter-contract read.

---

## Protocol Configuration (`ProtocolConfig`)

`kora_shared::types::ProtocolConfig` is a shared struct (`fee_bps`, `late_penalty_bps`,
`max_risk_score`, `min_funding_period`) intended as the canonical protocol-wide config.
As of this writing, `risk_registry` does **not** store or read a `ProtocolConfig` —
`max_risk_score` enforcement lives in `invoice_nft` (see
[invoice-nft.md](invoice-nft.md#protocol-configuration-protocolconfig)), the first and
only adopter so far. `fee_bps`, `late_penalty_bps`, and `min_funding_period` remain
unenforced anywhere and are owned by `treasury`/`financing_pool` via their own local
parameters — wiring them into a single shared `ProtocolConfig` is follow-up work, not
part of this contract today.

---

## Related Documents

- [ARCHITECTURE.md](ARCHITECTURE.md) — full contract dependency graph
- [CONTRACTS.md](CONTRACTS.md) — per-contract reference table
- [SECURITY.md](SECURITY.md) — protocol-wide security model
- [docs/invoice-nft.md](invoice-nft.md) — credit-limit enforcement at mint time
