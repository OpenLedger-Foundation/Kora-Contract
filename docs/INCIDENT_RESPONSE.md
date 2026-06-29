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
