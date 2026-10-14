# Bug Bounty & Responsible Disclosure Operations Guide

**Last Updated:** 2026-09-28  
**Owner:** Security Team  
**Public Disclosure Policy:** Coordinated disclosure with 90-day embargo

## Executive Summary

This document defines the operational workflow, tooling, and SLA-based response process for Kora's bug bounty and responsible disclosure program. It ensures external security researchers receive fast, professional responses to real findings, critical for attracting top-tier researchers as the protocol's surface area grows.

## Program Scope

### In-Scope Targets

**Smart Contracts (On-Chain):**
- ✅ `contracts/access_control` — Authorization and multisig logic
- ✅ `contracts/risk_registry` — KYB/verifier staking
- ✅ `contracts/invoice_nft` — Invoice lifecycle and state machine
- ✅ `contracts/marketplace` — Listing, funding, token whitelist
- ✅ `contracts/financing_pool` — Repayment, yield distribution, netting
- ✅ `contracts/price_oracle` — Price feed aggregation and defenses
- ✅ `contracts/dispute_resolution` — Challenge and resolution flow
- ✅ `contracts/treasury` — Revenue allocation and distribution

**Supporting Infrastructure (On-Chain):**
- ✅ Upgrade mechanisms (timelock, multisig approval)
- ✅ Cross-contract authorization patterns
- ✅ Access control matrix enforcement

### Out-of-Scope

**Explicitly Excluded:**
- ❌ Off-chain services (indexer, frontend, KYB API) — separate program
- ❌ Social engineering attacks (phishing, pretexting)
- ❌ Physical security of team/infrastructure
- ❌ DDoS attacks against RPC nodes (external to protocol)
- ❌ Third-party dependencies (Soroban runtime, Stellar network)
- ❌ Issues already documented in known limitations (see docs/LOOP_BOUNDS_AUDIT.md, docs/ORACLE_ADVERSARIAL_TESTING.md)

### Vulnerability Categories

**Accepted Impact Types:**
1. **Unauthorized fund access/theft** (Critical)
2. **Protocol insolvency/bankruptcy scenarios** (Critical)
3. **Unauthorized state transitions** (High)
4. **Denial-of-service via unbounded loops** (High)
5. **Oracle manipulation bypassing defenses** (High)
6. **Authorization bypass (privilege escalation)** (High)
7. **Reentrancy attacks** (Medium-High)
8. **Integer overflow/underflow** (Medium)
9. **Staleness/expiration exploits** (Medium)
10. **Griefing attacks (storage/gas exhaustion)** (Low-Medium)

---

## Severity Classification Rubric

### Critical (P0) — $10,000 - $50,000*

**Definition:** Direct theft of user funds or protocol insolvency.

**Examples:**
- Drain investor funds from `financing_pool` without authorization
- Mint infinite invoices bypassing rate limit
- Steal staked verifier tokens from `risk_registry`
- Bypass multisig to gain admin control
- Protocol-wide accounting corruption leading to insolvency

**SLA:** Acknowledge within **4 hours**, patch within **48 hours**, public disclosure after 90-day embargo.

---

### High (P1) — $2,000 - $10,000*

**Definition:** Unauthorized state transitions or privilege escalation without direct fund theft.

**Examples:**
- Mark any invoice as `Repaid` without payment
- Bypass admin authorization checks (non-multisig paths)
- Manipulate oracle price bypassing all defenses (staleness, reciprocal, peg, rate-change)
- Freeze arbitrary invoices as non-admin
- DoS via unbounded loop not covered by existing bounds

**SLA:** Acknowledge within **24 hours**, patch within **7 days**, public disclosure after 90-day embargo.

---

### Medium (P2) — $500 - $2,000*

**Definition:** Exploits requiring specific conditions or causing limited impact.

**Examples:**
- Reentrancy in non-critical paths (no fund loss)
- Integer overflow in edge-case arithmetic (caught by checked math)
- Griefing attacks with bounded cost to attacker
- Information disclosure of non-sensitive data
- Bypass of non-security access controls (e.g., read-only views)

**SLA:** Acknowledge within **72 hours**, patch within **30 days**, public disclosure after 90-day embargo.

---

### Low (P3) — $100 - $500*

**Definition:** Informational findings or minor deviations from best practices.

**Examples:**
- Code quality issues (unused variables, suboptimal patterns)
- Gas optimization opportunities
- Documentation inconsistencies
- Non-exploitable edge cases
- Theoretical attacks requiring unrealistic assumptions

**SLA:** Acknowledge within **1 week**, fix on best-effort basis, public disclosure optional.

---

**\*Payout Amounts:** Indicative ranges; final payout determined by severity, quality of report, and novelty. Paid in USDC on Stellar.

---

## Secure Intake Channel

### Submission Methods

**Primary (Encrypted Email):**
```
Email: security@kora.finance
PGP Key: https://kora.finance/.well-known/pgp-key.txt
Fingerprint: [TO BE GENERATED]
```

**Alternative (Private GitHub Security Advisory):**
```
Navigate to: https://github.com/kora-finance/contracts/security/advisories/new
Select "Report a vulnerability"
Provide details using template below
```

**Not Accepted:**
- ❌ Public GitHub issues
- ❌ Social media DMs (Twitter, Discord, Telegram)
- ❌ Unencrypted email containing sensitive details

### Submission Template

```markdown
## Vulnerability Report

**Reporter:** [Your Name / Handle]
**Contact:** [Email for encrypted communication]
**Date:** [YYYY-MM-DD]

### Summary
[One-sentence description of the vulnerability]

### Severity Self-Assessment
[Critical / High / Medium / Low]

### Affected Components
- Contract: [e.g., financing_pool]
- Function: [e.g., net_settle]
- Lines: [e.g., lib.rs:1200-1350]

### Vulnerability Details
[Detailed technical description]

### Proof of Concept
[Reproducible test case or exploit script]

```rust
// Example: Exploit demonstrating unauthorized fund drain
#[test]
fn test_exploit_unauthorized_drain() {
    // Setup
    let (env, attacker, client) = setup();
    
    // Exploit
    // ... steps to reproduce ...
    
    // Result: attacker drained X tokens
    assert_eq!(attacker_balance, STOLEN_AMOUNT);
}
```

### Impact Assessment
- **Funds at Risk:** [Amount in USD or "All protocol funds"]
- **Users Affected:** [Number or "All"]
- **Attack Prerequisites:** [e.g., "Requires admin compromised", "None (permissionless)"]
- **Attack Cost:** [Gas fees, collateral, setup cost]

### Suggested Remediation
[Optional: Proposed fix with code diff]

### Supporting Materials
[Screenshots, logs, network traces if applicable]

### Public Disclosure Preferences
- [ ] Anonymous credit
- [ ] Public credit as [Name/Handle]
- [ ] No credit preferred

### Payment Address (USDC on Stellar)
[G... address or "To be provided after triage"]
```

---

## Triage & Response Workflow

### Stage 1: Initial Receipt (T+0 to T+SLA)

**Responsible:** Security Triage Lead (rotating on-call)

**Actions:**
1. **Acknowledge receipt** via encrypted email within SLA (4h / 24h / 72h / 1w based on self-assessed severity)
2. **Assign tracking ID** (e.g., `KORA-2026-001`)
3. **Initial severity classification** (may differ from reporter's assessment)
4. **Log in secure triage system** (see Tooling section below)

**Response Template:**
```
Subject: [KORA-2026-001] Vulnerability Report Acknowledged

Dear [Reporter],

Thank you for reporting this vulnerability to Kora. We have assigned
tracking ID KORA-2026-001 and classified it as [Severity].

Our initial assessment:
- Severity: [Critical/High/Medium/Low]
- Estimated patch timeline: [X days]
- Next update: [Date]

We will keep you informed as we investigate and remediate. Please do
not publicly disclose until we coordinate a disclosure date (typically
90 days post-patch).

Security Team
security@kora.finance
```

---

### Stage 2: Validation & Reproduction (T+SLA to T+2×SLA)

**Responsible:** Core Dev Team + External Auditor (for Critical/High)

**Actions:**
1. **Reproduce exploit** in local testnet environment
2. **Confirm impact** matches or exceeds reporter's assessment
3. **Assess exploitability** in production (deployed contract versions)
4. **Document root cause** with code references

**Outcomes:**
- ✅ **Confirmed:** Proceed to Stage 3 (Remediation)
- ⚠️ **Partially Confirmed:** Lower severity; proceed with adjusted timeline
- ❌ **Cannot Reproduce:** Request clarification from reporter; mark "Needs Info"
- ❌ **Invalid/Duplicate:** Thank reporter; no payout; close ticket

**Validation Report Template:**
```markdown
## Validation Report: KORA-2026-001

**Validator:** [Dev Name]
**Date:** [YYYY-MM-DD]

### Reproduction Status
- [x] Exploit successfully reproduced
- [ ] Could not reproduce (reason: ___)

### Confirmed Impact
- Severity: [Adjusted if different from initial]
- Funds at Risk: $[Amount]
- Root Cause: [Technical summary]

### Production Exposure
- Affected Contracts: [List deployed addresses]
- Deployed Versions: [v0.1.2, v0.1.3]
- Exploited in Wild: [No / Unknown / Yes - see incident log]

### Recommended Actions
1. [Emergency pause if Critical]
2. [Patch development priority]
3. [User notification if funds at risk]
```

---

### Stage 3: Remediation & Testing (T+2×SLA to T+PatchDeadline)

**Responsible:** Core Dev Team

**Actions:**
1. **Develop fix** with unit test reproducing exploit (now passing)
2. **Adversarial testing** against variations of exploit
3. **Code review** by 2+ devs + external auditor (Critical/High only)
4. **Regression testing** full test suite
5. **Deployment plan** (coordinate with multisig signers for upgrade)

**Patch Quality Gates:**
- ✅ Exploit test case included in test suite (prevents regression)
- ✅ No new vulnerabilities introduced (auditor sign-off)
- ✅ Backward-compatible OR migration plan documented
- ✅ Gas cost impact assessed (no >20% increase in common paths)

---

### Stage 4: Deployment & Notification (T+PatchDeadline to T+Deployment+24h)

**Responsible:** DevOps + Security Team

**Actions:**
1. **Deploy patch** via upgrade mechanism (respecting timelock if non-emergency)
2. **Notify affected users** if funds were at risk (see User Notification section)
3. **Monitor for exploitation** of newly-disclosed details post-patch
4. **Confirm with reporter** patch is effective

**Emergency Deployment (Critical P0):**
- Multisig fast-track: Collect signatures offline; execute upgrade ASAP
- Protocol pause: Trigger via `access_control.set_protocol_paused` if needed
- Post-mortem: Document in `docs/INCIDENT_RESPONSE.md`

---

### Stage 5: Payout & Public Disclosure (T+Deployment+90d)

**Responsible:** Security Team + Finance

**Actions:**
1. **Calculate final payout** based on severity matrix and quality
2. **Transfer USDC** to reporter's Stellar address
3. **Coordinate disclosure date** with reporter (default: 90 days post-patch)
4. **Publish security advisory** crediting reporter (unless anonymous)
5. **Update CHANGELOG.md** and `AUDIT_LOG.md`

**Public Advisory Template:**
```markdown
# Security Advisory: KORA-2026-001

**Severity:** High  
**Affected Versions:** v0.1.2, v0.1.3  
**Patched Versions:** v0.1.4  
**Disclosure Date:** 2026-12-25  
**Credit:** [Researcher Name/Handle] via Kora Bug Bounty  

## Summary
A vulnerability in the `financing_pool.net_settle` function allowed
an attacker to [high-level impact description] under specific conditions.

## Impact
- **Funds at Risk:** $[Amount] (theoretical maximum)
- **Users Affected:** [Number] active pools
- **Exploitation:** No evidence of exploitation in production

## Technical Details
[Detailed explanation of vulnerability, now safe to disclose]

## Remediation
Upgrade to v0.1.4 immediately. No user action required for existing positions.

## Timeline
- 2026-09-28: Vulnerability reported
- 2026-09-29: Confirmed and patch development started
- 2026-10-05: Patch deployed to testnet
- 2026-10-12: Patch deployed to mainnet
- 2026-12-25: Public disclosure (90 days post-patch)
```

---

## Bug Bounty Tooling

### 1. Secure Triage System

**Tool:** `scripts/triage_vulnerability.py`

**Purpose:** Log and track vulnerability reports in encrypted local database (not committed to git).

**Usage:**
```bash
# Initialize secure vault (one-time setup)
python scripts/triage_vulnerability.py init --gpg-key security@kora.finance

# Register new report
python scripts/triage_vulnerability.py new \
  --id KORA-2026-001 \
  --severity Critical \
  --reporter "researcher@example.com" \
  --summary "Unauthorized fund drain in net_settle"

# Update status
python scripts/triage_vulnerability.py update KORA-2026-001 \
  --status "Patch Deployed" \
  --payout 15000

# List all open vulnerabilities
python scripts/triage_vulnerability.py list --status Open

# Export for incident response drill
python scripts/triage_vulnerability.py export KORA-2026-001 --format json
```

**Storage:** `~/.kora-security/vulnerabilities.db.gpg` (GPG-encrypted SQLite)

**Schema:**
```sql
CREATE TABLE vulnerabilities (
    id TEXT PRIMARY KEY,              -- KORA-YYYY-NNN
    reported_date TEXT,
    reporter_email TEXT,
    reporter_credit TEXT,
    severity TEXT CHECK(severity IN ('Critical','High','Medium','Low')),
    status TEXT CHECK(status IN ('New','Confirmed','Patching','Deployed','Disclosed','Closed')),
    affected_contracts TEXT,          -- JSON array
    root_cause TEXT,
    patch_version TEXT,
    payout_amount INTEGER,            -- in USDC cents
    payout_address TEXT,
    disclosure_date TEXT,
    notes TEXT
);
```

---

### 2. Automated Severity Classifier

**Tool:** `scripts/classify_severity.py`

**Purpose:** ML-based severity suggestion from report text (advisory only; human confirms).

**Training Data:** Historical CVEs, audit findings, past Kora reports.

**Usage:**
```bash
python scripts/classify_severity.py --report report.md
# Output: Suggested Severity: High (confidence: 0.87)
#         Rationale: Mentions "admin bypass", "state transition", high-impact keywords
```

**Integration:** Runs automatically in triage workflow; results logged for calibration.

---

### 3. Tabletop Exercise Simulator

**Tool:** `scripts/security_drill.py`

**Purpose:** Simulates incoming critical vulnerability report to test team response SLA.

**Usage:**
```bash
# Run drill at random time within next 7 days
python scripts/security_drill.py schedule --severity Critical

# When drill triggers (via cron), sends encrypted email to security@kora.finance with:
# - Synthetic vulnerability report (realistic but fake exploit)
# - Timer starts
# - Team must follow full triage workflow
# - Drill report generated at end with SLA compliance metrics
```

**Drill Scenarios:**
1. **Weekend Critical:** Friday 6pm report requiring emergency patch
2. **Coordinated Multi-Vector:** Two related vulnerabilities reported simultaneously
3. **False Positive:** Realistic-looking report that's actually invalid (tests validation rigor)
4. **Slow Burn:** Medium severity report escalates to High during investigation

---

## User Notification Guidelines

### When to Notify Users

**Always Notify (Within 24h of Patch):**
- ✅ Any Critical vulnerability where funds were at risk
- ✅ High vulnerability if user action required (e.g., withdraw funds)
- ✅ Known exploitation in production (even if patched)

**Optional Notification:**
- 🟡 High vulnerability, no known exploitation, no user action needed
- 🟡 Medium/Low vulnerabilities (bundle in monthly security update)

**Never Notify Before Patch:**
- ❌ Disclosing details pre-patch increases exploitation risk

### Notification Channels

1. **In-App Banner** (if frontend exists)
2. **Email to affected addresses** (if contact info collected)
3. **Blog post** at `https://kora.finance/security/KORA-2026-001`
4. **Social media** (Twitter/X, Discord) with link to full advisory
5. **GitHub Security Advisory** (automated from workflow)

### Notification Template

```
Subject: [Action Required] Security Update - Kora Protocol v0.1.4

Dear Kora User,

We have identified and patched a security vulnerability in Kora Protocol
versions v0.1.2 and v0.1.3. Your funds are now secure.

WHAT HAPPENED
A vulnerability in the financing pool contract could have allowed [high-level
impact] under specific conditions. We have no evidence this was exploited.

WHAT WE'VE DONE
- Patched the vulnerability in v0.1.4 (deployed 2026-10-12)
- Audited the fix with external security firm
- Enhanced monitoring to detect similar issues

WHAT YOU SHOULD DO
[If action required: "Withdraw your funds from old pools and re-deposit"]
[If no action: "No action required. Your funds are secure."]

LEARN MORE
Full technical advisory: https://kora.finance/security/KORA-2026-001

We take security seriously and appreciate the researcher who responsibly
disclosed this issue. Questions? Email security@kora.finance.

Kora Security Team
```

---

## Coordination with External Auditors

### Pre-Deployment Audits

**Process:**
1. Engage auditor 4-6 weeks before mainnet launch
2. Provide full codebase + deployment plan + threat model
3. Auditor findings → triage via same workflow as bug bounty
4. Critical/High findings must be resolved before deployment
5. Medium/Low findings resolved or accepted-as-risk with justification

**Post-Patch Audits (Critical Vulnerabilities):**
1. Before deploying Critical patch, share with auditor for expedited review (24h SLA)
2. Auditor confirms fix + no new vulnerabilities introduced
3. Sign-off required for production deployment

---

## Integration with Incident Response

### Escalation to Incident Response

**Trigger:** Confirmed Critical vulnerability **OR** evidence of active exploitation.

**Actions:**
1. Activate incident response team (see `docs/INCIDENT_RESPONSE.md`)
2. Execute emergency pause if needed (`access_control.set_protocol_paused`)
3. Forensics: Analyze on-chain transactions for exploitation
4. User notification: Immediate if funds at risk
5. Post-incident review: Update threat model, add regression tests

**Incident Commander:** Rotates weekly; always available via PagerDuty.

---

## Metrics & Continuous Improvement

### Key Performance Indicators

**SLA Compliance:**
- % of Critical reports acknowledged within 4h (Target: 100%)
- % of High reports patched within 7d (Target: 95%)
- Average time-to-patch by severity

**Program Health:**
- Number of reports per month (growth indicates program visibility)
- Valid vs. invalid report ratio (Target: >40% valid)
- Repeat reporters (indicates positive experience)
- Payout distribution (fair incentives attract better researchers)

**Dashboard:** `https://internal.kora.finance/security-metrics` (team-only)

### Quarterly Review

**Process:**
1. Security team reviews all reports from past quarter
2. Identify patterns (e.g., repeated misses in access control)
3. Update threat model, add regression tests, improve docs
4. Adjust payout ranges if attracting low-quality submissions
5. Calibrate severity classifier ML model with new data

---

## Legal & Compliance

### Safe Harbor

**Kora provides safe harbor for good-faith security research:**

✅ **We will not pursue legal action** against researchers who:
- Comply with this responsible disclosure policy
- Do not access/exfiltrate user data beyond what's necessary to demonstrate vulnerability
- Do not intentionally harm users or disrupt the protocol
- Coordinate public disclosure with our team

❌ **Safe harbor does NOT cover:**
- Social engineering, phishing, physical attacks
- DDoS or intentional service disruption
- Accessing other users' data beyond proof-of-concept
- Violating laws beyond scope of security testing (e.g., money laundering via exploit)

### Tax & Payment

**Payout Process:**
- Paid in USDC on Stellar to researcher-provided address
- Researcher responsible for tax reporting in their jurisdiction
- Kora will issue 1099 (US researchers >$600) or equivalent if required

---

## Researcher FAQ

**Q: How long until I hear back?**  
A: See SLA matrix above. Critical: 4 hours. High: 24 hours. Medium: 72 hours. Low: 1 week.

**Q: Can I disclose publicly after 90 days even if you haven't patched?**  
A: Yes, but we strongly prefer coordination. If we're unresponsive past SLA, you may disclose responsibly.

**Q: What if I find something but I'm not sure it's exploitable?**  
A: Report it! We'll investigate and provide feedback. Even informational findings are appreciated.

**Q: Do you accept reports on known issues / design limitations?**  
A: Check docs first (LOOP_BOUNDS_AUDIT.md, ORACLE_ADVERSARIAL_TESTING.md). Known limitations aren't eligible for bounty but novel exploitation paths are.

**Q: How do I report anonymously?**  
A: Use Tor + burner email for submission. Specify "anonymous credit" in report. Payout to Stellar address is pseudonymous.

---

## Appendix: Sample Drill Report

### Security Drill: DRILL-2026-Q3-01

**Scenario:** Critical vulnerability in `marketplace.fund_invoice` allowing double-spend

**Timeline:**
- T+0:00 — Synthetic report sent to security@kora.finance (Friday 5:47pm)
- T+0:23 — Acknowledged by on-call engineer (✅ Within 4h SLA)
- T+1:45 — Validated and reproduced by core dev
- T+8:30 — Patch developed and reviewed
- T+12:00 — Emergency multisig collected (5/7 signatures)
- T+13:15 — Patch deployed to mainnet
- T+13:45 — User notification sent
- T+14:00 — Drill concluded

**SLA Compliance:** ✅ PASS (Critical patched within 48h)

**Lessons Learned:**
1. ✅ Friday evening report handled well despite off-hours
2. ⚠️ Multisig collection took longer than optimal (8h to reach 5/7) → Improvement: Pre-stage signatures for emergency scenarios
3. ✅ User notification template worked smoothly

**Action Items:**
- [ ] Implement emergency multisig pre-staging process
- [ ] Add drill scenario library with 10+ realistic exploits
- [ ] Schedule next drill (random date in Q4)

---

## Contact & Escalation

**Primary Contact:**  
security@kora.finance  
PGP: https://kora.finance/.well-known/pgp-key.txt

**External Auditors:**  
[Firm Name] — audits@firm.com  
Emergency Contact: +1-XXX-XXX-XXXX

---

## Panic Safety & Production Exemption Policy

### Overview
In accordance with Kora's security hardening standards, production smart contract code (`contracts/*/src/`) strictly denies `.unwrap()`, `.expect()`, and direct `panic!()` invocations to eliminate denial-of-service vulnerabilities and unexpected transaction reverts.

All production contracts are statically audited via an automated AST scanner:
```bash
cargo run -p kora-xtask --bin panic-safety-audit
```

### Exemption Criteria
An exemption in `contracts/panic_safety_allowlist.json` is granted **only** when all of the following conditions are satisfied:
1. **Provable Invariant**: The operation is mathematically or structurally guaranteed to never panic (e.g., loop index strictly bounded by `0..vec.len()`, or preceding explicit length verification).
2. **Explicit Justification**: The entry in `contracts/panic_safety_allowlist.json` contains a clear technical explanation documenting the invariant.
3. **No Alternative**: Replacing the unwrap with `?` error propagation or safe fallback would introduce unnecessary bytecode overhead or is constrained by an external Soroban SDK API interface.

### Exemption Request Process
1. Run the local scanner:
   ```bash
   cargo run -p kora-xtask --bin panic-safety-audit
   ```
2. For each identified unwrap, attempt to refactor using `?` error propagation or safe helper functions from `kora_shared::validation`.
3. If an exemption is strictly required, append an entry to `contracts/panic_safety_allowlist.json`:
   ```json
   {
     "file": "contracts/<crate>/src/lib.rs",
     "line": <line_number>,
     "kind": "unwrap",
     "snippet": "<code_snippet>",
     "justification": "<detailed_security_invariant_explanation>"
   }
   ```
4. Verify the scanner passes with 0 violations and 0 stale allowlist entries.
5. All exemptions must receive sign-off from the Security Team during PR review.

---

## Version History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-09-28 | Initial operational guide |

**Next Review:** 2027-03-28 (6 months)

---

**This guide is consistent with SECURITY.md and supersedes any conflicting informal processes.**
