# Sev-2 — exploitable vulnerability, not yet active — internal

> **Classification:** High severity / Sev-2.
> Use this template in the private `#incident-response` channel.
> Do **not** include exploit steps, proof-of-concept code, transaction hashes,
> addresses, credentials, reporter contact details, or any information that
> could accelerate exploitation.

---

Severity: [High / Sev-2]  
Status: [Investigating|Confirmed|Patching|Deployed|Resolved]  
Owner: [Incident Commander — PLACEHOLDER: name or role]  
Next update: [UTC timestamp or cadence, e.g. every 60 minutes]

---

## Context (internal only)

- Detection source: [on-chain monitor | external report | internal audit | PLACEHOLDER]
- Detection time (UTC): [PLACEHOLDER: YYYY-MM-DD HH:MM]
- Affected component: [contract name or service — high-level, no exploit path]
- Exploitation cost / likelihood: [cheap | expensive | theoretical — PLACEHOLDER]
- Funds at risk (estimated): [PLACEHOLDER: amount or "not applicable"]
- Bug report ID: [KORA-YYYY-NNN or "not yet assigned" — PLACEHOLDER]

## Response team

| Role | Assigned to | Contact |
|---|---|---|
| Incident Commander | [PLACEHOLDER] | [PLACEHOLDER] |
| Technical Lead | [PLACEHOLDER] | [PLACEHOLDER] |
| Security Engineer | [PLACEHOLDER] | [PLACEHOLDER] |
| Communications Lead | [PLACEHOLDER] | [PLACEHOLDER] |

## Current status

[PLACEHOLDER: one or two sentences on what is known and what is being done.
Do not include exploit details.]

## Decision log

| UTC time | Decision or action | Owner |
|---|---|---|
| [PLACEHOLDER] | [PLACEHOLDER] | [PLACEHOLDER] |

## Next steps

- [ ] Confirm exploitability with Security Engineer
- [ ] Assess whether Sev-1 escalation (pause) is warranted
- [ ] Develop and review minimal patch (see fast-track procedure in `../INCIDENT_RESPONSE.md`)
- [ ] Confirm patch on testnet before mainnet deployment
- [ ] Prepare external-safe communication (`sev-2-external.md`)
- [ ] Schedule coordinated disclosure date with any external reporter

## Blockers

[PLACEHOLDER: none, or describe what is blocking progress]

---

> **Escalation:** If exploitation risk increases or funds become at risk,
> escalate immediately to Sev-1 and open `sev-1-pause-internal.md`.
