# Sev-3 — degraded service, no fund loss — internal

> **Classification:** Medium severity / Sev-3.
> Use this template in the internal incident channel.
> No pause is typically triggered for Sev-3; a normal upgrade or
> configuration change is scheduled instead.
> Do **not** include credentials, internal addresses, or sensitive indicators.

---

Severity: [Medium / Sev-3]  
Status: [Investigating|Mitigating|Resolved]  
Owner: [Technical Lead — PLACEHOLDER: name or role]  
Next update: [UTC timestamp or cadence — PLACEHOLDER]

---

## Context

- Detection source: [monitor | user report | internal review | PLACEHOLDER]
- Detection time (UTC): [PLACEHOLDER: YYYY-MM-DD HH:MM]
- Affected component: [service or contract — high-level — PLACEHOLDER]
- User-visible symptom: [PLACEHOLDER: e.g., "read latency elevated", "query
  returns stale data", "pagination cursor invalid for large result sets"]
- Fund loss risk: None confirmed
- Bug report ID: [KORA-YYYY-NNN or "not applicable" — PLACEHOLDER]

## Response team

| Role | Assigned to |
|---|---|
| Technical Lead | [PLACEHOLDER] |
| On-call engineer | [PLACEHOLDER] |
| Communications Lead | [PLACEHOLDER] |

## Current status

[PLACEHOLDER: brief description of what is known and in progress. No internal
addresses, credentials, or exploit paths.]

## Mitigation plan

- [ ] Root cause confirmed
- [ ] Fix scoped to minimum change
- [ ] PR reviewed and merged
- [ ] Deployment scheduled: [PLACEHOLDER: UTC time or sprint]
- [ ] User impact resolved
- [ ] External communication sent if user-facing

## Decision log

| UTC time | Decision or action | Owner |
|---|---|---|
| [PLACEHOLDER] | [PLACEHOLDER] | [PLACEHOLDER] |

---

> **Escalation:** If a security dimension is identified (potential exploit or
> fund risk), immediately re-classify as Sev-2 or Sev-1 and open the
> corresponding template.
