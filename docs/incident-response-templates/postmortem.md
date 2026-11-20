# Post-mortem — [PLACEHOLDER: short incident title]

> **Instructions:** Complete this document within 7 days of incident resolution
> (see `../INCIDENT_RESPONSE.md` — Post-Incident Review). Publish as
> `docs/INCIDENT_YYYY_MM_DD.md` and announce on Discord, Twitter, and GitHub
> Security Advisories.
>
> Remove all PLACEHOLDER markers before publishing externally. The internal
> version (with full technical detail) lives in the restricted incident record;
> this document is the public-facing or cross-team summary.
>
> **Do not include:** unexploited vulnerability details that remain unpatched,
> private reporter contact information, credentials, or internal addresses
> not already public.

---

**Incident ID:** [KORA-YYYY-NNN — PLACEHOLDER]  
**Date of incident:** [PLACEHOLDER: YYYY-MM-DD]  
**Date of resolution:** [PLACEHOLDER: YYYY-MM-DD]  
**Severity:** [Critical|High|Medium|Low / Sev-1|Sev-2|Sev-3 — PLACEHOLDER]  
**Status:** Resolved  
**Author(s):** [PLACEHOLDER: name(s) or role(s)]  
**Reviewed by:** [PLACEHOLDER: Technical Lead, Security Engineer, Communications Lead]

---

## 1. Summary

[PLACEHOLDER: 2–4 sentences. What happened, what the impact was, and how it
was resolved. Write for an audience of technically literate community members
who were not involved in the response. No exploit path details if the
vulnerability has not been fully patched across all dependent components.]

---

## 2. Timeline

> All times in UTC. Record every significant event: detection, escalation,
> decisions, actions, and resolution. Gaps longer than 15 minutes during an
> active Sev-1 response should be explained.

| UTC time | Event | Owner |
|---|---|---|
| [PLACEHOLDER: YYYY-MM-DD HH:MM] | Incident detected: [PLACEHOLDER: source — monitor, report, etc.] | [PLACEHOLDER] |
| [PLACEHOLDER] | Incident Commander assigned | [PLACEHOLDER] |
| [PLACEHOLDER] | Severity classified as [Sev-N — PLACEHOLDER] | [PLACEHOLDER] |
| [PLACEHOLDER] | [PLACEHOLDER: first containment action, e.g., protocol paused, feature disabled] | [PLACEHOLDER] |
| [PLACEHOLDER] | Root cause identified | [PLACEHOLDER] |
| [PLACEHOLDER] | Fix developed and reviewed | [PLACEHOLDER] |
| [PLACEHOLDER] | Fix deployed to testnet | [PLACEHOLDER] |
| [PLACEHOLDER] | Fix deployed to mainnet | [PLACEHOLDER] |
| [PLACEHOLDER] | Protocol resumed / service restored | [PLACEHOLDER] |
| [PLACEHOLDER] | External communication published | [PLACEHOLDER] |
| [PLACEHOLDER] | Incident resolved | [PLACEHOLDER] |

---

## 3. Root cause analysis

### What went wrong

[PLACEHOLDER: technical description of the root cause. For published
post-mortems, omit unexploited vulnerability details.]

### Why it went wrong

[PLACEHOLDER: underlying conditions — missing guard, misconfiguration,
untested edge case, design assumption violation, etc.]

### Why it was not caught earlier

[PLACEHOLDER: gap in tests, monitoring, audit scope, or review process that
allowed the condition to reach production.]

---

## 4. Impact assessment

| Dimension | Detail |
|---|---|
| Users affected | [PLACEHOLDER: estimated count or "none confirmed"] |
| Funds at risk | [PLACEHOLDER: amount or "none"] |
| Actual losses | [PLACEHOLDER: amount or "none"] |
| Protocol downtime | [PLACEHOLDER: duration or "none"] |
| Data affected | [PLACEHOLDER: description or "none"] |
| External exposure | [PLACEHOLDER: publicly disclosed before fix? yes/no] |

---

## 5. Response effectiveness

### What went well

- [PLACEHOLDER: e.g., "Alert fired within 30 seconds of exploit attempt"]
- [PLACEHOLDER]

### What could be improved

- [PLACEHOLDER: e.g., "Runbook for this scenario was missing"]
- [PLACEHOLDER]

### Response time metrics

| Milestone | Target | Actual |
|---|---|---|
| Time to detection | [PLACEHOLDER] | [PLACEHOLDER] |
| Time to Incident Commander assignment | [PLACEHOLDER] | [PLACEHOLDER] |
| Time to containment | [PLACEHOLDER] | [PLACEHOLDER] |
| Time to fix deployed | [PLACEHOLDER] | [PLACEHOLDER] |
| Time to resolution | [PLACEHOLDER] | [PLACEHOLDER] |

---

## 6. Preventive measures

### Code changes

- [PLACEHOLDER: PR or commit reference, description of change]

### Process improvements

- [PLACEHOLDER: e.g., "Add this scenario to quarterly drill rotation"]

### Monitoring enhancements

- [PLACEHOLDER: e.g., "Add alert for X metric threshold"]

### Documentation updates

- [PLACEHOLDER: e.g., "Update runbook Y to cover this case"]

---

## 7. Compensation plan

> Complete this section only if users suffered losses attributable to the
> incident. If no compensation is applicable, replace this section with
> "No compensation is required for this incident."

- Affected users: [PLACEHOLDER: description of eligibility criteria]
- Compensation amount: [PLACEHOLDER]
- Distribution method: [PLACEHOLDER]
- Distribution timeline: [PLACEHOLDER]
- Governance approval required: [yes/no — PLACEHOLDER]

---

## 8. References

- Incident record: [PLACEHOLDER: link to restricted internal document or "restricted"]
- Related PR(s): [PLACEHOLDER]
- Related issue(s): [PLACEHOLDER]
- Security advisory: [PLACEHOLDER: GitHub advisory URL or "pending"]
- Bug bounty report: [KORA-YYYY-NNN — PLACEHOLDER or "not applicable"]
- Disclosure template used: `docs/incident-response-templates/bounty-disclosure-external.md`
