# Incident Response & Disaster Recovery Runbook

This runbook covers what the team does **after** a critical exploit or anomaly is detected
live on mainnet. It complements [docs/SECURITY.md](SECURITY.md) (which covers prevention and
the pause-enforcement model) and the top-level [SECURITY.md](../SECURITY.md) (vulnerability
reporting). Prevention is documented elsewhere; this document is purely about **response**.

> **Primary objective:** stop the bleeding (pause), preserve funds and forensic state, then
> remediate via the timelocked upgrade path — in that order.

---

## 1. Roles & Authority

| Role | Holder | Responsibility |
|------|--------|----------------|
| **Incident Commander (IC)** | On-call protocol lead | Owns the incident, makes the pause/unpause call, coordinates everyone below. |
| **Pause Authority** | Admin key holder (`access_control` admin) | Executes `pause()` / `unpause()`. Must be reachable 24/7. |
| **Upgrade Authority** | Admin key holder (same admin, gated by B1 timelock) | Proposes and, after the timelock, executes contract upgrades. |
| **Comms Lead** | Designated team member | Owns external/internal communication and the disclosure timeline. |
| **Scribe** | Any responder | Maintains the incident timeline (timestamps, tx hashes, decisions). |

Key custody: the admin key is the single most sensitive asset in an incident. It controls both
`pause()` and the upgrade path. It must be held in a hardware wallet / multisig and **never**
pasted into a shared channel. If the admin key itself is suspected compromised, treat it as a
**Sev-1** and rotate admin (`transfer_admin`) before anything else.

---

## 2. Severity Classification

| Severity | Definition | Initial action |
|----------|------------|----------------|
| **Sev-1** | Funds at risk / active exploit draining value, or admin key compromise. | **Pause immediately**, then investigate. |
| **Sev-2** | Exploitable bug confirmed, not yet actively exploited. | Pause if exploitation is cheap/likely; otherwise prepare upgrade under guard. |
| **Sev-3** | Degraded behaviour, no direct fund loss (e.g. stuck listing, bad event). | No pause; schedule a normal timelocked upgrade. |

When in doubt, **escalate up** (treat as more severe), not down.

---

## 3. Detection

An incident may be surfaced by any of:

- **On-chain monitoring** — alerts on abnormal volume, repeated reverts, unexpected
  `set_defaulted` / `mark_default` calls, or large outflows.
- **Event stream anomalies** — gaps or unexpected ordering in emitted events.
- **External report** — via `security@kora.finance` (see root `SECURITY.md`).
- **Failed invariant checks** — e.g. pool accounting that no longer balances.

On detection, the first responder **immediately opens an incident**, assigns an IC, starts the
timeline, and classifies severity per §2.

---

## 4. Containment — Pausing

For Sev-1 (and most Sev-2), pause first; analysis comes after the bleeding stops.

```bash
# Pause all new activity. Repayments remain enabled by design (see docs/SECURITY.md).
stellar contract invoke --id $ACCESS_CONTROL \
  --source $ADMIN -- pause --admin $ADMIN_ADDRESS

# Confirm the pause took effect
stellar contract invoke --id $ACCESS_CONTROL -- is_paused
```

What `pause()` does and does **not** block is defined by the
[Pause Enforcement Matrix](SECURITY.md#enforcement-matrix). Critically:

- **Blocked:** `mint_invoice`, `set_listed`, `set_funded`, `list_invoice`, `fund_invoice`,
  `record_position`, `mark_default` — i.e. all *new* activity and capital inflow.
- **Never blocked:** `repay`, `set_repaid`, `set_defaulted`, `cancel_listing` — existing
  obligations and exits remain open so the pause does not punish SMEs/investors.

Because repayment and cancellation stay live, a pause is safe to trigger early and reverse later.

---

## 5. Investigation

With activity frozen, the IC + responders:

1. Capture the **exact failing tx(s)**, ledger sequence, and contract addresses into the timeline.
2. Reproduce against a forked/testnet environment (never experiment on mainnet).
3. Identify root cause and the **minimal** code change that closes it.
4. Confirm the fix does not regress the pause-enforcement matrix or repayment exemption.

Preserve forensic state: do **not** unpause or upgrade until the root cause is understood and a
reviewed fix exists.

---

## 6. Remediation — Timelocked Upgrade (B1)

Upgrades are governed by the B1 timelock primitive
(`propose_upgrade` → wait → `execute_upgrade`), with a delay of
`UPGRADE_TIMELOCK_DELAY = 86_400` seconds (24h) defined in
`contracts/shared/src/validation.rs`.

```bash
# 1. Build & hash the patched WASM, then propose the upgrade
stellar contract invoke --id $INVOICE_NFT --source $ADMIN -- \
  propose_upgrade --admin $ADMIN_ADDRESS --new_wasm_hash $WASM_HASH

# 2. Wait out the timelock (24h). The protocol stays PAUSED during this window.
#    Use the time for independent review of the patched WASM.

# 3. After the delay elapses, execute
stellar contract invoke --id $INVOICE_NFT --source $ADMIN -- \
  execute_upgrade --admin $ADMIN_ADDRESS
```

**Timelock tension under pressure:** the 24h delay is deliberate and protects users from a
rushed or malicious upgrade, but it means a fix is *not* instant. The mitigation is that the
protocol remains **paused** for the full window, so no new exploitable activity can occur while
the patch matures. If a faster path is ever required, that is a governance decision — it must
**not** be worked around by bypassing the timelock.

After `execute_upgrade` succeeds and the fix is verified on-chain, **unpause**:

```bash
stellar contract invoke --id $ACCESS_CONTROL --source $ADMIN -- \
  unpause --admin $ADMIN_ADDRESS
```

---

## 7. Post-Incident Disclosure

1. **Acknowledge** the original reporter within 48h (per root `SECURITY.md`).
2. **Patch** critical issues, targeting the 7-day window in `SECURITY.md`.
3. **Disclose** publicly only after the fix is live and users are protected. The disclosure
   includes: timeline, root cause, impact (funds affected, if any), the fix, and follow-up
   actions. Credit the reporter if they consent.
4. **Retrospective** within one week: what detected it, what slowed response, and concrete
   action items (monitoring gaps, runbook fixes, missing tests). File the action items as issues.

---

## 7b. Admin Key Compromise — Rotation Runbook (Issue #607)

If the admin key is suspected or confirmed compromised, treat it as a **Sev-1** and execute
this runbook *immediately* — before any other remediation step, because a live attacker with
the admin key can pause, drain treasury fees, or transfer admin to themselves.

### When to use `rotate_admin` vs. `transfer_admin`

| Function | When to use |
|----------|-------------|
| `transfer_admin` | Routine ownership handoff (e.g. treasury, team transition). Single-admin path only. |
| `rotate_admin` | Emergency key-compromise recovery. Same storage effect; emits a distinct `admin_rotated` event (`ADM_ROT`) so off-chain monitors can alert on the recovery specifically. |

Both functions require the multisig flow (`propose_action(AdminAction::RotateAdmin(...))` →
`approve_action` → `execute_action`) once a multisig is configured.

### Pre-rotation checklist

Before issuing the rotation, verify:

1. **New key is ready and secured** — hardware wallet, never touched the compromised machine.
2. **New key is distinct** from all current signers and the old admin.
3. **No conflicting governance proposals are in flight** — the `rotate_admin` path checks for
   active proposals and rejects the rotation if any exist, to prevent governance races.
   Cancel or execute pending proposals first.

### Rotation procedure (multisig environment)

```bash
# 1. Identify the new admin address
NEW_ADMIN="G..."

# 2. Propose the rotation (any current multisig signer)
stellar contract invoke --id $ACCESS_CONTROL --source $SIGNER_1 -- \
  propose_action \
  --proposer $SIGNER_1_ADDRESS \
  --action '{"RotateAdmin": "$NEW_ADMIN"}'

# Record the returned proposal ID:
PROPOSAL_ID=<returned-id>

# 3. Collect threshold approvals from other signers
stellar contract invoke --id $ACCESS_CONTROL --source $SIGNER_2 -- \
  approve_action \
  --approver $SIGNER_2_ADDRESS \
  --proposal_id $PROPOSAL_ID

# (Add more approve_action calls until threshold is met)

# 4. Execute the rotation once threshold is reached
stellar contract invoke --id $ACCESS_CONTROL --source $SIGNER_2 -- \
  execute_action \
  --executor $SIGNER_2_ADDRESS \
  --proposal_id $PROPOSAL_ID

# 5. Confirm the new admin is live
stellar contract invoke --id $ACCESS_CONTROL -- get_admin
```

### Rotation procedure (single-admin environment — no multisig)

If the multisig has **not** been configured yet (rare: early deployment or stripped config):

```bash
stellar contract invoke --id $ACCESS_CONTROL --source $CURRENT_ADMIN -- \
  rotate_admin \
  --current_admin $CURRENT_ADMIN_ADDRESS \
  --new_admin $NEW_ADMIN_ADDRESS

# Confirm
stellar contract invoke --id $ACCESS_CONTROL -- get_admin
```

> **Note:** `rotate_admin` will reject the call with `DirectCallProhibited` if a multisig
> is configured — use the multisig proposal flow in that case.

### Post-rotation steps

1. **Revoke the compromised key** — rotate the Stellar account's signers if using a hardware
   wallet; invalidate the seed phrase or key file.
2. **Audit the event log** — query `ADM_ROT` events on-chain; if you see an unexpected
   `admin_rotated` event before yours, the attacker may have already rotated — escalate.
3. **Re-configure multisig with the new admin** — call `configure_multisig` with the new admin
   and a fresh signer set.
4. **Verify no further damage** — check treasury balance, fee settings, and recent listings for
   anomalies using the investigation steps in §5.
5. **Disclose per §7** — rotation of the admin key is a material security event.

### Edge case: rotation attempted while a governance proposal is in flight

The `rotate_admin` function (and `execute_action(AdminAction::RotateAdmin(...))`) checks for
active proposals before applying the key change. If a pending proposal exists:

- The rotation will fail with `RotationBlockedByPendingProposal`.
- Cancel the pending proposal (`cancel_action`), then retry.
- If the pending proposal was itself created by the attacker, cancel it using a different signer
  who has not been compromised (any signer can cancel a proposal they proposed; a quorum can
  cancel any proposal).

---

## 8. Tabletop Drill

A tabletop drill was conducted against a **deliberately-injected testnet bug** to exercise this
runbook end-to-end. The mainnet deployment was never touched.

### Scenario

A faulty patch was deployed to **testnet** in which `record_position` under-counted a pool's
`total_funded`, allowing a position to be recorded without the corresponding capital being
accounted for (a value-leak class bug). This was injected solely to give responders something
concrete to detect, contain, and remediate.

### Exercise log (testnet)

| Phase | Action | Result |
|-------|--------|--------|
| Detection | Invariant check (`sum(positions) == total_funded`) flagged a mismatch. | ✅ Caught within the monitoring interval. |
| Triage | IC assigned, classified **Sev-1** (funds-at-risk class). | ✅ Roles filled from §1. |
| Containment | `pause()` called; `is_paused()` confirmed `true`. | ✅ New `record_position` calls reverted with `ProtocolPaused`. |
| Verify exemption | Issued a `repay` against an existing pool while paused. | ✅ Repayment succeeded — exemption holds, SMEs unaffected. |
| Investigation | Reproduced on a fork, isolated the accounting line, wrote the minimal fix. | ✅ Root cause understood before any upgrade. |
| Remediation | `propose_upgrade` → waited out timelock → `execute_upgrade`. | ✅ Patched WASM live; invariant restored. |
| Recovery | `unpause()` called; normal activity resumed. | ✅ |
| Disclosure | Dry-run of the disclosure template + retro. | ✅ |

### Findings & action items

- **F1 — Timelock vs. urgency.** A 24h pause is operationally fine because repayment stays
  live, but responders wanted the WASM-hash/review checklist *pre-written* so the 24h window is
  spent reviewing, not scrambling. → Action: add a pre-upgrade review checklist (tracked
  separately).
- **F2 — Key reachability.** The drill assumed the admin key holder was instantly available;
  define a documented backup/escalation path for off-hours. → Action: document on-call rotation.
- **F3 — Monitoring coverage.** The leak was caught by an invariant check; not all invariants
  are currently monitored. → Action: enumerate critical invariants and alert on each.
- **F4 — Runbook usability.** This runbook performed well as the single source of truth during
  the exercise; the role table and the copy-paste pause/upgrade commands were the most-used parts.

The drill validated that **pause → investigate → timelocked upgrade → unpause** is executable
under pressure and that the repayment exemption protects users throughout. Re-run this drill
after any change to the pause matrix or the upgrade path.
