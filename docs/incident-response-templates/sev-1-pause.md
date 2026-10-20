# Incident Template: Sev-1 Protocol Emergency Pause

Use this template when an emergency pause is executed across Kora Protocol contracts via `scripts/emergency-pause.sh` (Guardian, Admin, or Multi-sig authorization) in response to a critical (Sev-1) security vulnerability or active exploit.

---

## Stage 1: Initial Pause Alert (< 15–30 Minutes)

> **Channels:** Discord `#announcements`, X/Twitter broadcast, Status Page (`https://status.kora.finance`)  
> **Objective:** Acknowledge the pause immediately, reassure users regarding fund safety, and establish the communication timeline.

### Message Template

```markdown
🚨 **KORA PROTOCOL SECURITY ALERT — PROTOCOL PAUSED**

**Incident ID:** [INCIDENT_ID]  
**Timestamp:** [TIMESTAMP_UTC]  
**Status:** Protocol Paused (Precautionary / Active Response)

As a safety precaution, the Kora Protocol has been emergency-paused by authorized signers.

- **Scope:** [AFFECTED_COMPONENTS] (e.g., all financing pools and marketplace interactions)
- **Pause Transaction:** `[PAUSE_TX_HASH]`
- **Fund Status:** [FUNDS_STATUS] (e.g., "All existing deposits and collateral balances are frozen safely in smart contract storage. No unauthorized withdrawals have been observed.")
- **User Action Required:** [USER_ACTION_REQUIRED] (e.g., "None. Do not attempt to interact with contract functions until further notice.")

Our core engineering and security response teams are actively investigating the situation. We will provide updates at least every 2 hours until resolution.

- **Next Official Update:** [NEXT_UPDATE_ETA]
- **Status Dashboard:** [STATUS_PAGE_URL]
- **Official Inquiries:** security@kora.finance
```

---

## Stage 2: Ongoing Progress Update (Every 2 Hours)

> **Channels:** Discord `#announcements`, X/Twitter thread, Status Page  
> **Objective:** Maintain transparency without disclosing unremediated exploit mechanics.

### Message Template

```markdown
📢 **INCIDENT UPDATE: [INCIDENT_ID] — [TIMESTAMP_UTC]**

**Current Status:** [INVESTIGATION / PATCHING / TESTING]

- **Summary of Findings:** [FACTUAL_NON_SENSITIVE_SUMMARY] (e.g., "The underlying issue has been identified in the financing pool math calculation. No user principal has been lost.")
- **Remediation Progress:** [REMEDIATION_STEP] (e.g., "A patch has been developed, and unit and fuzz tests are currently running in the test environment.")
- **Governance & Deployment:** [GOVERNANCE_STATUS] (e.g., "Fast-track governance proposal prepared; multi-sig signers are currently assembling for quorum.")
- **User Action Required:** [USER_ACTION_REQUIRED]

We appreciate your patience as we ensure the protocol is completely secure before resuming operations.

- **Next Official Update:** [NEXT_UPDATE_ETA]
- **Status Dashboard:** [STATUS_PAGE_URL]
```

---

## Stage 3: Remediation Deployed & Unpause Schedule

> **Channels:** Discord `#announcements`, X/Twitter thread, Status Page  
> **Objective:** Announce successful deployment of the fix and communicate the exact unpause timeline.

### Message Template

```markdown
✅ **REMEDIATION UPDATE: [INCIDENT_ID] — FIX DEPLOYED & UNPAUSE SCHEDULE**

**Timestamp:** [TIMESTAMP_UTC]  
**Status:** Fix Verified; Unpause Scheduled

The security team has resolved the vulnerability and successfully deployed the fix via on-chain governance:

- **Fix Proposal / Transaction:** `[FIX_TX_HASH]`
- **Contract Patch Commit:** `[PATCH_COMMIT_HASH]`
- **Verification:** All regression and fuzz suites passed with zero regressions.
- **Unpause Timestamp:** Protocol operations will safely resume at **[UNPAUSE_TIMESTAMP_UTC]**.

**Post-Unpause Operational Notes:**
- [DEPOSIT_WITHDRAWAL_INSTRUCTIONS] (e.g., "Pending loan settlements will process normally following unpause.")
- [USER_ACTION_REQUIRED]

We thank our community, security partners, and multi-sig signers for their rapid coordination.

- **Detailed Retrospective ETA:** Within 7 days
- **Status Dashboard:** [STATUS_PAGE_URL]
```

---

## Stage 4: Resolution & Post-Mortem Announcement

> **Channels:** Discord `#announcements`, Blog, Developer Forum  
> **Objective:** Publish transparent post-mortem following the 7-day review requirement.

### Message Template

```markdown
📋 **INCIDENT RESOLUTION & POST-MORTEM: [INCIDENT_ID]**

**Incident Date:** [INCIDENT_DATE]  
**Total Downtime:** [DOWNTIME_DURATION]  
**Direct Impact:** [FINAL_IMPACT_METRICS]

The comprehensive post-incident review for [INCIDENT_ID] has been published in accordance with Section 6 of our Incident Response Guide.

- **Full Post-Mortem Report:** `[POST_MORTEM_URL]`
- **Root Cause Summary:** [ROOT_CAUSE_BRIEF]
- **Preventative Measures Implemented:** [PREVENTATIVE_ACTIONS]

We remain committed to radical transparency and institutional-grade protocol security.
```
