# Incident Template: Key Compromise & Revocation

Use this template when an administrative signing key, multi-sig co-signer key, guardian key, or risk-registry verifier key is suspected or confirmed of compromise, in accordance with [`docs/KEY_ROTATION_PROCEDURE.md`](../KEY_ROTATION_PROCEDURE.md).

---

## Stage 1: Key Revocation & Rotation Proposal Notice (< 1 Hour)

> **Channels:** Discord `#announcements`, Status Page (`https://status.kora.finance`), Multi-sig coordination channel  
> **Objective:** Notify stakeholders that a compromised key is being revoked/replaced, and confirm that multi-sig quorum rules prevented unilateral exploitation.

### Message Template

```markdown
🔐 **SECURITY NOTICE: ADMINISTRATIVE KEY ROTATION INITIATED**

**Incident ID:** [INCIDENT_ID]  
**Timestamp:** [TIMESTAMP_UTC]  
**Key Role:** [COMPROMISED_KEY_ROLE] (e.g., "Multi-sig Signer 2" / "Guardian Key" / "Risk Registry Verifier")

The Kora Protocol security team has initiated an emergency key rotation procedure:

- **Affected Key Identifier:** `[COMPROMISED_KEY_ADDRESS]`
- **Reason:** [COMPROMISE_REASON] (e.g., "Suspected local credential exposure / precautionary rotation")
- **Quorum Protection:** [QUORUM_STATUS] (e.g., "Kora Protocol multi-sig architecture requires M-of-N threshold signatures. At no point could the compromised key act unilaterally or move protocol funds.")
- **Revocation On-Chain Action:**
  - Proposal Transaction: `[ROTATION_PROPOSAL_TX_HASH]`
  - Replacement Key: `[NEW_KEY_ADDRESS]`
- **Fund Security:** [FUNDS_STATUS] (e.g., "All treasury, liquidity pool, and user escrow balances remain 100% secure.")
- **User Action Required:** None.

Multi-sig signers are executing the key replacement workflow defined in `docs/KEY_ROTATION_PROCEDURE.md`.

- **Next Official Update:** [NEXT_UPDATE_ETA]
- **Status Dashboard:** [STATUS_PAGE_URL]
```

---

## Stage 2: Timelock & Voting Progress Update

> **Channels:** Discord `#announcements`, Governance Forum  
> **Objective:** Provide progress on multi-sig signatures, timelock progression, and security verification.

### Message Template

```markdown
⏳ **KEY ROTATION UPDATE: [INCIDENT_ID] — [TIMESTAMP_UTC]**

**Rotation Status:** [SIGNATURE_COLLECTION / TIMELOCK_PENDING / READY_TO_EXECUTE]

- **Key Role:** [COMPROMISED_KEY_ROLE]
- **Signer Quorum:** [COLLECTED_SIGNATURES] / [REQUIRED_QUORUM] signatures collected.
- **Timelock Status:** [TIMELOCK_EXPIRATION_UTC] (e.g., "Fast-track emergency governance timelock expires at 18:00 UTC")
- **Signature Audit:** All historical on-chain calls signed by `[COMPROMISED_KEY_ADDRESS]` during the last [AUDIT_TIMEFRAME] have been fully audited; zero unauthorized transactions were executed.
- **User Action Required:** None.

- **Next Official Update:** [NEXT_UPDATE_ETA]
- **Status Dashboard:** [STATUS_PAGE_URL]
```

---

## Stage 3: Rotation Executed & Final Confirmation

> **Channels:** Discord `#announcements`, X/Twitter broadcast, Status Page  
> **Objective:** Confirm successful on-chain revocation of the compromised key and activation of the replacement key.

### Message Template

```markdown
🛡️ **KEY ROTATION COMPLETED — PROTOCOL FULLY SECURED**

**Incident ID:** [INCIDENT_ID]  
**Timestamp:** [TIMESTAMP_UTC]

The key rotation procedure has been executed on-chain and verified:

- **Revoked Key:** `[COMPROMISED_KEY_ADDRESS]` (Permissions completely removed)
- **Active Replacement Key:** `[NEW_KEY_ADDRESS]`
- **Execution Transaction:** `[EXECUTION_TX_HASH]`
- **Protocol State:** Normal operations continue without disruption.

We thank our multi-sig co-signers and security partners for their rapid execution and adherence to our key security standards.

- **Status Dashboard:** [STATUS_PAGE_URL]
- **Security Runbook Reference:** `docs/KEY_ROTATION_PROCEDURE.md`
```
