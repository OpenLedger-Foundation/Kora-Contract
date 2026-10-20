# Incident Response Guide

This document outlines procedures for responding to security incidents, critical vulnerabilities, and protocol emergencies in the Kora Protocol.

---

## Table of Contents

1. [Incident Classification](#incident-classification)
2. [Response Team](#response-team)
3. [Fast-Track Governance for Critical Fixes](#fast-track-governance-for-critical-fixes)
4. [Emergency Pause Procedure](#emergency-pause-procedure)
5. [Communication Protocol](#communication-protocol)
6. [Post-Incident Review](#post-incident-review)

---

## Incident Classification

### Severity Levels

| Level | Description | Response Time | Examples |
|-------|-------------|---------------|----------|
| **Critical** | Active exploit, funds at risk | Immediate (< 1 hour) | Active drain, price oracle manipulation, reentrancy exploit |
| **High** | Vulnerability found, not yet exploited | 4-8 hours | Publicly disclosed critical bug, potential for fund loss |
| **Medium** | Issue affecting functionality | 24-48 hours | Contract state inconsistency, degraded performance |
| **Low** | Minor issue, no security impact | Best effort | UI bug, documentation error |

---

## Response Team

### Roles

- **Incident Commander:** Overall coordination and decision-making
- **Technical Lead:** Vulnerability analysis and patch development
- **Security Engineer:** Exploit assessment and mitigation
- **Communications Lead:** Community updates and transparency
- **Multi-sig Signers:** Fast-track governance execution

### Contact Information

- **Emergency Discord Channel:** `#incident-response` (private)
- **PagerDuty:** Critical alerts
- **Email:** security@kora.finance
- **Multi-sig Signers:** Maintain updated contact list

---

## Fast-Track Governance for Critical Fixes

### When to Use

Fast-track governance is **strictly reserved** for confirmed critical security vulnerabilities that:
- Pose immediate risk to user funds
- Are actively being exploited or publicly disclosed
- Require on-chain contract changes to fix

**Do NOT use for:**
- Non-security parameter changes
- Business-driven updates
- Feature additions (even if urgent)

### Prerequisites

1. **Bug Bounty Report:** Critical vulnerability must be triaged and confirmed via bug bounty system
2. **Minimal Fix:** Patch must be scoped to address vulnerability only (no bundled changes)
3. **Size Constraint:** Proposal size ≤ 1000 bytes
4. **Multi-sig Availability:** Minimum threshold of signers (e.g., 3/5) available within 2 hours

### Fast-Track Procedure

#### Step 1: Triage and Confirmation (< 30 minutes)

```
1. Security team receives report via bug bounty
2. Assess severity using CVSS scoring
3. Confirm exploitability and impact
4. If Critical: proceed to Step 2
5. If High/Medium: standard governance path
```

#### Step 2: Register Critical Bug Report (< 15 minutes)

```rust
// Incident Commander executes
governance.register_critical_bug_report(admin, bug_report_id);
```

**Effect:** Unlocks fast-track proposal creation linked to this report.

#### Step 3: Develop Minimal Fix (< 2 hours)

```
1. Technical Lead develops minimal patch
2. Peer review by Security Engineer
3. Verify patch size ≤ 1000 bytes
4. Test on testnet
5. Generate action_hash
```

**Critical:** Fix must be **minimal and single-purpose** — no feature additions, refactoring, or unrelated changes.

#### Step 4: Create Fast-Track Proposal (< 15 minutes)

```rust
let proposal_id = governance.create_fast_track_proposal(
    proposer,
    target_contract,
    fix_action_hash,
    bug_report_id,
    proposal_size_bytes,
);
```

**Validation:**
- Bug report registered ✓
- Proposal size ≤ max ✓
- Proposer has stake ✓

#### Step 5: Multi-sig Voting (< 2 hours)

```
1. Incident Commander alerts multi-sig signers
2. Signers review fix on testnet
3. Signers vote (75% threshold required)
```

```rust
governance.vote_fast_track(signer1, proposal_id, true);
governance.vote_fast_track(signer2, proposal_id, true);
governance.vote_fast_track(signer3, proposal_id, true);
```

**Threshold:** Higher than standard (e.g., 75% vs 50%)

#### Step 6: Execute Fix (after 2h timelock)

```rust
governance.execute_fast_track_proposal(proposal_id);
```

**Timelock:** Shorter than standard (2 hours vs 24 hours) but **non-zero** for minimum review.

#### Step 7: Mandatory Post-Hoc Disclosure (< 24 hours after execution)

```rust
governance.publish_fast_track_disclosure(admin, proposal_id);
```

**Content Required:**
- Vulnerability description (after fix deployed)
- Impact assessment
- Fix explanation
- Why fast-track was necessary
- Confirmation no bundled changes

**Channels:**
- Discord announcement
- Twitter thread
- GitHub security advisory
- Blog post

---

## Emergency Pause Procedure

For incidents requiring immediate protocol suspension (e.g., active exploit):

### Step 1: Activate Pause (Immediate)

```rust
access_control.pause_protocol(admin);
```

**Effect:**
- All core functions blocked: `fund_invoice`, `repay`, `withdraw`
- Emergency functions remain available: `emergency_withdraw`, `mark_default`

### Step 2: Assess Situation (< 30 minutes)

```
1. Identify affected contracts
2. Estimate funds at risk
3. Check for active exploits
4. Determine if fast-track fix needed
```

### Step 3: Fix or Mitigate (< 4 hours)

**Option A: Fast-Track Fix**
- Follow fast-track procedure above
- Protocol remains paused until fix deployed

**Option B: Manual Mitigation**
- Use emergency admin functions
- Transfer funds to safe addresses
- Disable affected features

### Step 4: Resume Protocol

```rust
access_control.unpause_protocol(admin);
```

**Only after:**
- Fix deployed and verified
- Funds secured
- No ongoing exploit activity
- Post-mortem prepared

---

## Communication Protocol

### Internal Communication

**Critical Incident:**
1. Incident Commander declares incident on `#incident-response`
2. Page all response team members
3. Start Zoom bridge for real-time coordination
4. Log all actions in incident document (Google Docs)

**Status Updates:**
- Every 30 minutes during active response
- Include: current status, next steps, blockers

### External Communication

**Principles:**
- Transparency without compromising security
- Timely updates (no dark periods > 4 hours)
- Honest about unknowns
- Clear on user actions required

**Initial Alert (< 1 hour of detection):**
```
⚠️ SECURITY ALERT

We are investigating a potential security issue. 
As a precaution, the protocol has been paused.

Your funds are safe. No user action required.

Updates will follow every 2 hours.

Status: https://status.kora.finance
```

**Ongoing Updates (every 2-4 hours):**
- Current status
- Actions taken
- Estimated resolution time
- User impact

**Resolution Announcement:**
- Issue summary
- Fix deployed
- User actions (if any)
- Compensation plan (if applicable)
- Link to detailed post-mortem

---

## Post-Incident Review

### Timing

Within 7 days of incident resolution

### Required Sections

1. **Timeline**
   - First detection
   - Key decision points
   - Resolution

2. **Root Cause Analysis**
   - What went wrong
   - Why it went wrong
   - Why it wasn't caught earlier

3. **Impact Assessment**
   - Users affected
   - Funds at risk
   - Actual losses
   - Protocol downtime

4. **Response Effectiveness**
   - What went well
   - What went poorly
   - Response time metrics

5. **Preventive Measures**
   - Code changes
   - Process improvements
   - Monitoring enhancements

6. **Compensation Plan** (if applicable)
   - Affected users
   - Compensation amount
   - Distribution method

### Publication

- Publish on GitHub as `INCIDENT_YYYY_MM_DD.md`
- Announce on Discord and Twitter
- Add to security advisories

---

## Fast-Track Usage Audit Trail

All fast-track governance usage is permanently logged on-chain via events.

**Monitoring:**
- Alert on any `FastTrackProposalCreated` event
- Community review mandatory within 24h of execution
- Quarterly report on all fast-track usage

**Acceptable Use:**
- Critical security fix only
- Bug bounty confirmation
- Post-hoc disclosure published

**Abuse Indicators:**
- Non-security changes
- Repeated fast-track usage
- Bundled unrelated changes
- Missing post-hoc disclosure

**Response to Abuse:**
1. Community governance vote to disable fast-track
2. Review and potential removal of abusing signers
3. Strengthen eligibility requirements

---

## Escalation Paths

### If Fast-Track Threshold Not Reachable

**Scenario:** Critical fix needed but < 75% of multi-sig signers available

**Options:**
1. **Wait for threshold** — Safe if no active exploit
2. **Emergency admin action** — Use emergency_withdraw to secure funds temporarily
3. **Protocol pause** — Buy time to reach threshold

### If Fix Exceeds Size Limit

**Scenario:** Security fix requires > 1000 bytes

**Options:**
1. **Split into multiple proposals** — If logically separable
2. **Standard governance** — If not time-critical
3. **One-time size limit increase** — Requires standard governance vote first

### If Bug Report Not in Bounty System

**Scenario:** Critical vulnerability found outside formal bug bounty

**Options:**
1. **Retroactive registration** — Admin registers as critical severity
2. **Standard governance** — If time permits
3. **Emergency pause + mitigation** — Secure funds first, fix later

---

## Testing and Drills

### Quarterly Incident Response Drill

**Scenario:** Simulated critical vulnerability

**Participants:** Full response team

**Duration:** 4 hours

**Objectives:**
- Test fast-track governance end-to-end
- Validate multi-sig signer availability
- Exercise communication protocols
- Measure response times

**Post-Drill:**
- Document lessons learned
- Update procedures
- Address identified gaps

---

### Kill-Switch Cache TTL Drill (#1006)

**Purpose:** Verify that disabling a protocol flow flag reaches all web clients within the SLA stale window.

**Pre-requisites:** Feature-flags service running, at least one web client session connected with SSE stream.

**Steps:**

1. Confirm all protocol flags are enabled:
   ```
   curl -H "X-Admin-Actor: drill@kora.finance" PUT /admin/flags/secondary-market/enable
   ```

2. Open a browser session and verify funding flow is visible (kill-switch off).

3. Start the drill timer.

4. Toggle the kill-switch off:
   ```
   curl -H "X-Admin-Actor: drill@kora.finance" PUT /admin/flags/secondary-market/disable
   ```

5. Observe the web client: the funding CTA must disappear within the **maximum stale window**:
   - SSE connected: **< 1 second**
   - SSE reconnecting: **≤ 5 seconds**
   - Snapshot polling only: **≤ 30 seconds**

6. Confirm server-side enforcement: any POST to the backing API for a disabled feature must return `503 Feature disabled`.

7. Re-enable the flag and verify the CTA returns:
   ```
   curl -H "X-Admin-Actor: drill@kora.finance" PUT /admin/flags/secondary-market/enable
   ```

8. Document results in `docs/drill-results/YYYY-MM-DD-kill-switch-drill.md`.

**Automated drill tests:** The Jest tests in `services/feature-flags/tests/evaluate.test.ts` (describe block "Kill-switch drill: TTL exhaustion #1006") run the equivalent timing assertions in CI without requiring a live environment. These pass without timing flake because they use injected short TTL values (50–80ms) rather than the 5s production default.

**CDN bypass:** If an intermediate CDN caches the `/flags` snapshot response, serve it with `Cache-Control: no-store` or use the SSE stream which explicitly sets `Cache-Control: no-cache`. CDN-cached snapshots are not subject to the SLA above; coordinate a cache purge with the CDN operator as part of any drill where CDN caching is active.

---

## Appendices

### A. Multi-sig Signer Availability Matrix

| Signer | Primary Contact | Backup Contact | Timezone | Availability |
|--------|----------------|----------------|----------|--------------|
| Signer 1 | Email/Phone | Discord | UTC-8 | 24/7 |
| Signer 2 | Email/Phone | Discord | UTC+0 | 24/7 |
| ... | ... | ... | ... | ... |

### B. Fast-Track Proposal Template

```rust
// Bug Report: #1234 - Reentrancy in financing_pool
// Severity: Critical (CVSS 9.8)
// Impact: Unlimited drain of pool funds
// Fix: Add reentrancy guard to withdraw function

governance.create_fast_track_proposal(
    proposer,
    financing_pool_address,
    fix_action_hash,  // sha256(patch_wasm)
    1234,             // bug report ID
    856,              // patch size in bytes
);
```

### C. Communication Templates

See `docs/incident-response-templates/` for pre-written messages for common scenarios.

---

## References

- Bug Bounty Program: `docs/BUG_BOUNTY_OPERATIONS.md`
- Governance Documentation: `docs/governance.md`
- Security Policy: `SECURITY.md`
- Access Control: `docs/ACCESS_MATRIX.md`
