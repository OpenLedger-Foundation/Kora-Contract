# Incident Template: Coordinated Bounty Disclosure

Use this template when publishing a public coordinated vulnerability disclosure following successful triage, patch deployment, and bounty payout under the Kora Protocol Bug Bounty Program, as specified in [`docs/BUG_BOUNTY_OPERATIONS.md`](../BUG_BOUNTY_OPERATIONS.md).

---

## Disclosure Guidelines

1. **Strict Post-Remediation Rule:** A disclosure announcement MUST NEVER be published before:
   - The contract patch is merged and deployed on mainnet.
   - The emergency governance timelock has elapsed.
   - The bounty payout has been executed to the researcher.
2. **Attribution & Consent:** Confirm with the researcher whether they prefer public attribution (handle/name) or anonymous recognition.
3. **Defense-in-Depth:** Always highlight the testing improvements and safeguards implemented to prevent similar vulnerability classes.

---

## Coordinated Disclosure Announcement

> **Channels:** Discord `#announcements` & `#dev-announcements`, X/Twitter broadcast, Official Security Blog, GitHub Security Advisory  
> **Objective:** Celebrate responsible white-hat researchers, explain the vulnerability and fix transparently, and reinforce protocol trust.

### Message Template

```markdown
🛡️ **COORDINATED VULNERABILITY DISCLOSURE: [BOUNTY_REPORT_ID] — [VULNERABILITY_TITLE]**

**Disclosure Date:** [DISCLOSURE_DATE_UTC]  
**Severity:** [SEVERITY_LEVEL] (CVSS [CVSS_SCORE])  
**Affected Component:** `[AFFECTED_CONTRACT]` (Function: `[AFFECTED_FUNCTION]`)  
**Status:** Resolved, Audited, Deployed on Mainnet

---

### Executive Summary

A security vulnerability in Kora Protocol's `[AFFECTED_CONTRACT]` was responsibly disclosed through our Bug Bounty Program. The issue allowed [HIGH_LEVEL_IMPACT_EXPLANATION] under specific condition [TRIGGER_CONDITION].

**Protocol Impact:**
- **User Funds Lost:** **$0** (No exploit occurred; identified proactively).
- **Remediation Status:** Verified patch deployed via on-chain governance proposal `[GOVERNANCE_PROPOSAL_ID]`.

---

### Vulnerability Details

- **Mechanism:** [TECHNICAL_EXPLANATION_BRIEF] (e.g., "An edge case in decimal scaling during cross-currency invoice settlement allowed rounding truncation in favor of the caller.")
- **Proof of Concept:** Validated in isolated testnet sandbox by core security engineers.
- **Root Cause:** [ROOT_CAUSE_DESCRIPTION]

---

### Remediation & Patch

The engineering team implemented a surgical fix:
- **Patch Commit:** `[PATCH_COMMIT_HASH]`
- **Pull Request:** [PATCH_PR_URL]
- **Verification:** Added [NUMBER_OF_TESTS] regression tests and fuzzing suites covering [EDGE_CASE_DESCRIPTION].
- **Audit:** Validated by internal security team and independent multi-sig reviewers.

---

### Responsible Researcher Recognition

We extend our sincere gratitude to security researcher **[RESEARCHER_HANDLE_OR_ANONYMOUS]** for their outstanding work and responsible disclosure adhering to our security guidelines.

- **Bounty Award:** **[REWARD_POINTS_OR_USDC]**
- **Researcher Profile:** [RESEARCHER_PROFILE_LINK]

The Kora Protocol Bug Bounty Program continues to safeguard user funds through community collaboration.

---

### Incident Timeline (UTC)

| Timestamp | Event |
|-----------|-------|
| `[REPORTED_AT]` | Report received via secure bug bounty portal |
| `[TRIAGED_AT]` | Severity confirmed as [SEVERITY_LEVEL] (CVSS [CVSS_SCORE]) |
| `[PATCH_DEVELOPED_AT]` | Minimal patch developed and regression tests written |
| `[PROPOSAL_CREATED_AT]` | Fast-track governance proposal created |
| `[PATCH_DEPLOYED_AT]` | Proposal executed on-chain; patch active |
| `[BOUNTY_PAID_AT]` | Bug bounty reward issued to researcher |
| `[DISCLOSURE_DATE_UTC]` | Coordinated public disclosure published |

---

### Resources
- Bug Bounty Program: `docs/BUG_BOUNTY_OPERATIONS.md`
- Security Guidelines: `SECURITY.md`
- Security Contact: security@kora.finance
```
