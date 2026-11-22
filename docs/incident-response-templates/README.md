# Incident response templates

> Closes #1096.

These templates are starting points for a coordinated response. Complete every
`[PLACEHOLDER: ...]` field before sending or publishing. Use the internal
template only in the private incident channel; use the external-safe template
only after the Incident Commander and Communications Lead have approved it.

Full classification criteria, response procedures, and post-incident review
requirements are in [`../INCIDENT_RESPONSE.md`](../INCIDENT_RESPONSE.md).

**Never add exploit steps, proof-of-concept code, transaction hashes, private
keys, credentials, reporter contact details, or other sensitive indicators to
any template.** Keep those in the approved restricted incident record.

---

## Required header fields

Every completed message must include:

| Field | Values |
|---|---|
| **Severity** | `Critical`, `High`, `Medium`, or `Low`; add operational label `Sev-1`, `Sev-2`, or `Sev-3` |
| **Status** | `Investigating`, `Confirmed`, `Contained`, `Patching`, `Recovering`, or `Resolved` |
| **Owner** | Named Incident Commander (internal) or Communications Lead (external) |
| **Next update** | UTC timestamp or cadence (e.g. "every 60 minutes") |

---

## Template index

### Communication templates

| Scenario | Severity | Internal | External-safe |
|---|---|---|---|
| Active exploit — protocol paused | Sev-1 / Critical | [sev-1-pause-internal.md](sev-1-pause-internal.md) | [sev-1-pause-external.md](sev-1-pause-external.md) |
| Exploitable vulnerability, not yet active | Sev-2 / High | [sev-2-internal.md](sev-2-internal.md) | [sev-2-external.md](sev-2-external.md) |
| Degraded service, no fund loss | Sev-3 / Medium | [sev-3-internal.md](sev-3-internal.md) | [sev-3-external.md](sev-3-external.md) |
| Key compromise | Sev-1–2 | [key-compromise-internal.md](key-compromise-internal.md) | [key-compromise-external.md](key-compromise-external.md) |
| Indexer lag | Sev-2–3 | [indexer-lag-internal.md](indexer-lag-internal.md) | [indexer-lag-external.md](indexer-lag-external.md) |
| Bounty disclosure | Any | [bounty-disclosure-internal.md](bounty-disclosure-internal.md) | [bounty-disclosure-external.md](bounty-disclosure-external.md) |

### Post-incident

| Template | Purpose |
|---|---|
| [postmortem.md](postmortem.md) | Mandatory within 7 days of resolution; publish as `docs/INCIDENT_YYYY_MM_DD.md` |

---

## Severity quick-reference

| Label | Meaning | Typical first action |
|---|---|---|
| **Sev-1** | Active exploit or funds at risk | Open `sev-1-pause-internal.md`; execute pause immediately |
| **Sev-2** | Exploitable vulnerability confirmed, not yet active | Open `sev-2-internal.md`; assess whether pause is warranted |
| **Sev-3** | Degraded service, no security or fund risk | Open `sev-3-internal.md`; schedule normal upgrade |

For the full classification table and response-time targets see
[`../INCIDENT_RESPONSE.md` — Incident Classification](../INCIDENT_RESPONSE.md#incident-classification).

---

## Shared wording reference

[templates.md](templates.md) contains compact shared header and body wording
for copy-paste when authoring new scenario variants.
