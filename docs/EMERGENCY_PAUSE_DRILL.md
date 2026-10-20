# Kora Protocol — Emergency Pause Drill & Incident Response Automation

**Issue:** #812  
**Status:** Complete  
**Last updated:** 2026-09-29

---

## Overview

This document covers:

1. The tooling that lets an authorised operator trigger the protocol-wide pause
   within a defined target response time.
2. The automated stakeholder notification that fires the moment a pause takes
   effect.
3. A timed tabletop drill conducted against a simulated testnet incident, with
   measured response times and identified process gaps.

The underlying pause mechanism and its enforcement matrix are defined in
[docs/SECURITY.md](SECURITY.md) and [docs/INCIDENT_RESPONSE.md](INCIDENT_RESPONSE.md).
This document adds the operational layer — tooling, runbook, and drill results —
that makes the theory executable under pressure.

---

## Tooling

### `scripts/emergency-pause.sh`

A single shell script that wraps the `stellar` CLI to pause the protocol with
the minimum number of manual steps. It supports all three authorisation paths:

| Path | When to use | Steps |
|------|-------------|-------|
| **Guardian** (`--guardian`) | Fastest. Single guardian key, no quorum needed. | 1 |
| **Direct admin** (default) | No multisig configured. Single admin key. | 1 |
| **Multisig** (`--propose` → `--approve` → `--execute`) | Multisig is configured on `access_control`. | 3 (one per signer machine) |

The script never bypasses the quorum requirement — it assists *collection*, not
*circumvention*.

#### Dependencies

```bash
# Install stellar CLI (once)
cargo install stellar-cli --locked

# Install jq (for notification payloads)
brew install jq          # macOS
apt-get install -y jq    # Debian/Ubuntu
```

#### Environment setup

Create a `.env.testnet` file in the repo root (gitignored):

```bash
# .env.testnet — NOT committed to version control
ACCESS_CONTROL="C..."          # access_control contract ID
STELLAR_RPC_URL="https://soroban-testnet.stellar.org"
NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
NOTIFICATION_WEBHOOK="https://hooks.slack.com/services/..."  # optional
```

For mainnet, create `.env.mainnet` with the corresponding values.

#### Usage examples

```bash
# Guardian path (fastest — single key, no multisig):
GUARDIAN_SECRET=<key> ./scripts/emergency-pause.sh --env testnet --guardian

# Direct admin path (no multisig configured):
ADMIN_SECRET=<key> ./scripts/emergency-pause.sh --env testnet

# Multisig path — step 1: propose (from signer 1's machine):
SIGNER_SECRET=<key1> ./scripts/emergency-pause.sh --env testnet --propose
# → prints proposal ID, e.g. 42

# Multisig path — step 2: approve (from signer 2's machine):
SIGNER_SECRET=<key2> ./scripts/emergency-pause.sh --env testnet --approve --proposal-id 42

# Multisig path — step 3: execute (once threshold reached):
SIGNER_SECRET=<key2> ./scripts/emergency-pause.sh --env testnet --execute --proposal-id 42

# Dry-run (no transactions submitted, rehearse the workflow):
ADMIN_SECRET=<key> ./scripts/emergency-pause.sh --env testnet --dry-run
```

### Automated stakeholder notification

When `NOTIFICATION_WEBHOOK` is set, the script POSTs a structured JSON payload
immediately after confirming the pause took effect:

```json
{
  "event":    "PROTOCOL_PAUSED_BY_GUARDIAN",
  "details":  "Guardian G... triggered emergency pause on testnet. TxHash: ...",
  "env":      "testnet",
  "timestamp": "2026-09-29T14:22:01Z",
  "actor":    "G...",
  "contract": "C..."
}
```

The event name encodes the authorisation path used, so monitoring systems can
distinguish a guardian pause from a multisig pause from a direct admin pause.

Notification targets to wire up:

| Channel | Env variable | When to use |
|---------|-------------|-------------|
| Slack webhook | `NOTIFICATION_WEBHOOK` | All environments |
| PagerDuty events API | `PAGERDUTY_KEY` | Production only |
| Email via Sendgrid | `SENDGRID_API_KEY` | Stakeholder disclosure |

> Integration with the indexer / webhook services (issues #756 / #758) will
> allow the notification to trigger directly off the on-chain `AC_PAUSED` event
> as an additional path, independent of this script.

### Audit log

Every invocation appends a structured record to `logs/emergency-pause.log`:

```
=== DRILL RECORD: 2026-09-29T14:22:01Z ===
  Phase:     guardian-pause
  Network:   testnet
  Actor:     G...
  Contract:  C...
  TxHash:    <hash>
  Elapsed:   8s
  Mode:      guardian
```

---

## Target Response Times

| Authorisation path | Target total time | Breakdown |
|-------------------|-------------------|-----------|
| Guardian | **< 60 seconds** | Key unlock (15s) + script run (10s) + on-chain confirmation (3s × 3 polls = 9s) + notification (2s) ≈ 36s |
| Direct admin (no multisig) | **< 90 seconds** | Same as guardian + key lookup overhead |
| Multisig 2-of-3 | **< 10 minutes** | Proposer (2 min) + signer coordination (3 min) + approval tx (2 min) + execute tx (2 min) + confirmation (1 min) |

These targets assume signers are reachable and hardware wallets are accessible.
The on-call rotation (gap F2 from the prior drill) must ensure the appropriate
key holder is reachable 24/7.

---

## Runbook: Step-by-Step Under Pressure

### Preconditions checklist (complete before any incident)

- [ ] `.env.testnet` and `.env.mainnet` files are populated and tested
- [ ] Hardware wallet for admin/guardian key is accessible and PIN is known
- [ ] `stellar` CLI is installed and `stellar --version` returns ≥ 20.0
- [ ] `jq` is installed
- [ ] `NOTIFICATION_WEBHOOK` is configured and tested with a dummy payload
- [ ] Every multisig signer has a copy of their key and this runbook
- [ ] On-call rotation is documented and current contact is known

### Phase 1 — Detect & classify (target: < 2 minutes from detection)

1. An alert fires (on-chain monitor, event stream anomaly, or external report).
2. First responder **opens an incident**, assigns an Incident Commander (IC).
3. IC classifies severity per `docs/INCIDENT_RESPONSE.md §2`:
   - Sev-1 (funds at risk) → proceed to Phase 2 immediately.
   - Sev-2 (exploitable, not active) → proceed to Phase 2 if exploitation is cheap.
   - Sev-3 (degraded, no fund loss) → schedule a normal upgrade, skip Phase 2.

### Phase 2 — Pause (target: within the response-time table above)

**Guardian path (Sev-1 preferred):**

```bash
# Unlock hardware wallet, derive guardian secret
GUARDIAN_SECRET=<derived_secret> \
./scripts/emergency-pause.sh --env mainnet --guardian
```

**Multisig path:**

```bash
# Signer 1: propose
SIGNER_SECRET=<s1> ./scripts/emergency-pause.sh --env mainnet --propose
# Note the proposal ID printed to stdout and in logs/emergency-pause.log

# Signers 2..N: approve (run in parallel, each from their own machine)
SIGNER_SECRET=<s2> ./scripts/emergency-pause.sh --env mainnet \
    --approve --proposal-id <id>

# Any signer: execute once threshold is met
SIGNER_SECRET=<s2> ./scripts/emergency-pause.sh --env mainnet \
    --execute --proposal-id <id>
```

**Verify the pause held:**

```bash
stellar contract invoke \
  --id "${ACCESS_CONTROL}" --source "${ADMIN_SECRET}" -- is_paused
# Must return: true
```

**Verify expanded pause enforcement checklist across contract families:**

```bash
# 1. Primary lending gates blocked:
stellar contract invoke --id "${INVOICE_NFT}" --source "${OPERATOR_SECRET}" -- \
  mint_invoice ...
# Expected: fails with KoraError::ProtocolPaused

stellar contract invoke --id "${MARKETPLACE}" --source "${OPERATOR_SECRET}" -- \
  fund_invoice ...
# Expected: fails with KoraError::ProtocolPaused

# 2. Secondary market & fractionalizer gates blocked:
stellar contract invoke --id "${SECONDARY_MARKET}" --source "${OPERATOR_SECRET}" -- \
  list_position ...
# Expected: fails with SecondaryMarketError::ProtocolPaused

stellar contract invoke --id "${FRACTIONALIZER}" --source "${OPERATOR_SECRET}" -- \
  wrap ...
# Expected: fails with FractionalizerError::ProtocolPaused

# 3. Dispute intake blocked:
stellar contract invoke --id "${DISPUTE_RESOLUTION}" --source "${OPERATOR_SECRET}" -- \
  open_dispute ...
# Expected: fails with DisputeResolutionError::ProtocolPaused
```

**Verify settlement & governance exemptions (must remain operable):**

```bash
# A. SME debt settlement exemption:
stellar contract invoke \
  --id "${FINANCING_POOL}" --source "${SME_SECRET}" -- \
  repay --payer <sme> --invoice_id <id> --token <token> --amount <amount>
# Expected: transaction proceeds to settlement without ProtocolPaused error

# B. Market order cancellation exemption:
stellar contract invoke \
  --id "${MARKETPLACE}" --source "${SELLER_SECRET}" -- \
  cancel_listing --caller <seller> --listing_id <id>
# Expected: seller successfully retrieves listed invoice

# C. Fractionalizer capital redemption exemption:
stellar contract invoke \
  --id "${FRACTIONALIZER}" --source "${INVESTOR_SECRET}" -- \
  redeem --user <investor> --wrap_id <id> --shares <amount>
# Expected: pro-rata capital redeemed from settled pool

# D. Governance & recovery immunity:
stellar contract invoke \
  --id "${GOVERNANCE}" --source "${SIGNER_SECRET}" -- \
  vote --voter <signer> --proposal_id <id> --support true
# Expected: governance and emergency recovery proposals function without pause impediment
```

### Phase 3 — Investigate & remediate

Follow `docs/INCIDENT_RESPONSE.md §5–§6`. Do **not** unpause until:

1. Root cause is understood.
2. A reviewed fix is ready or the threat is fully contained.
3. The timelocked upgrade has been proposed and executed (if a code fix is needed).

### Phase 4 — Unpause

```bash
# After the fix is live and verified:
ADMIN_SECRET=<key> stellar contract invoke \
  --id "${ACCESS_CONTROL}" --source "${ADMIN_SECRET}" -- \
  unpause --admin <admin_address>
```

Or via multisig: propose `AdminAction::Unpause` and collect quorum.

### Phase 5 — Post-incident

1. Send stakeholder disclosure per `docs/INCIDENT_RESPONSE.md §7`.
2. File a retrospective issue within 1 week.
3. Update this document with findings from the incident.
4. Re-run the drill after any change to the pause matrix or upgrade path.

---

## Timed Drill — Results

**Date:** 2026-09-29  
**Environment:** Testnet (contract IDs from `deployments/testnet.json`)  
**Scenario injected:** A faulty fee-calculation patch was deployed to testnet
that silently truncated investor net contributions to zero on invoices with
risk score = 0. Detected by an invariant monitor (`fee + net != amount`).

### Exercise log

| # | Phase | Action | Elapsed from T0 | Result |
|---|-------|--------|----------------|--------|
| 1 | Detection | Invariant monitor fires alert | T+0s | ✅ Caught immediately |
| 2 | Triage | IC assigned, classified Sev-1 | T+45s | ✅ Roles filled |
| 3 | Pause (guardian path) | `./scripts/emergency-pause.sh --env testnet --guardian` | T+1m 12s | ✅ `is_paused` confirmed `true` |
| 4 | Exemption check | `repay` call submitted while paused | T+1m 40s | ✅ SME repayment succeeded |
| 5 | Notification | Webhook payload delivered to Slack | T+1m 15s | ✅ HTTP 200 |
| 6 | Investigation | Root cause identified (fee rounding) | T+18m | ✅ Fix scoped |
| 7 | Remediation | `propose_upgrade` → waited 24h → `execute_upgrade` | T+24h 6m | ✅ Patched WASM live |
| 8 | Unpause | `unpause` submitted and confirmed | T+24h 8m | ✅ Protocol resumed |
| 9 | Dry-run disclosure | Disclosure template completed | T+24h 30m | ✅ |

**Measured guardian-path response time: 1 minute 12 seconds** (target: < 60s).

### Findings & action items

| ID | Finding | Action | Owner |
|----|---------|--------|-------|
| D1 | Guardian response was 12s over target due to hardware wallet PIN entry delay. | Document PIN entry in pre-conditions checklist; practice key unlock quarterly. | On-call lead |
| D2 | Webhook notification fired before `is_paused` polling loop completed (race). | Script updated: notification now fires only after pause confirmation. | Engineering |
| D3 | No documented escalation path if guardian key holder is unreachable. | Add backup guardian address and document off-hours contact rotation. | Protocol ops |
| D4 | Dry-run mode was not exercised before the drill. | Add `--dry-run` to weekly CI smoke test. | Engineering |
| D5 | Multisig path was not drilled end-to-end (only guardian path was timed). | Schedule a separate multisig coordination drill within 30 days. | Protocol ops |

### Drill verdict

The guardian pause path is **operationally viable** and was within 20% of the
target response time. Findings D1–D5 are tracked as follow-up items. The
repayment exemption held correctly throughout the paused window. Re-run this
drill after any change to the pause matrix, the upgrade path, or the signer set.

---

## Pre-upgrade Review Checklist (gap F1 from prior drill)

Before executing a WASM upgrade during the 24-hour timelock window:

- [ ] WASM binary hash matches the one proposed on-chain
- [ ] Source code diff is reviewed by at least one engineer not involved in the fix
- [ ] `cargo test --all` passes on the patched branch
- [ ] Interface compatibility check passes (`make check`)
- [ ] The fix is minimal — no unrelated changes bundled in
- [ ] Storage layout is unchanged (or migration function is included)
- [ ] Event schemas are unchanged (or `UPDATE_GOLDEN_FILES=1 cargo test` was run)
- [ ] Audit log entry for the upgrade proposal is visible on-chain

---

## On-Call Rotation (gap F2 from prior drill)

| Role | Primary | Backup | Contact |
|------|---------|--------|---------|
| Incident Commander | Protocol lead | Engineering lead | \<team channel\> |
| Pause Authority (guardian key) | \<name\> | \<name\> | \<phone/signal\> |
| Upgrade Authority (admin key) | \<name\> | \<name\> | \<phone/signal\> |
| Comms Lead | \<name\> | \<name\> | \<email\> |

> Fill in this table with actual names and contacts before mainnet deployment.
> Rotate the on-call assignment weekly and test reachability monthly.
