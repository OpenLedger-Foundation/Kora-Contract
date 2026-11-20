# Incident response templates

These templates are starting points for a coordinated response. Complete the
placeholders using the glossary in [`../INCIDENT_RESPONSE.md`](../INCIDENT_RESPONSE.md).
Use the internal template only in the private incident channel; use the
external-safe template after the Incident Commander and Communications Lead
approve it.

**Never add exploit steps, credentials, private keys, transaction addresses,
reporter contact details, or other sensitive indicators to these templates.**
Keep those in the approved restricted incident record.

## Required header

Every completed message must include:

- **Severity:** `Critical`, `High`, `Medium`, or `Low` (the operational Sev-1/2/3 label may be added)
- **Status:** current response state
- **Owner:** named Incident Commander or delegate
- **Next update:** UTC time or cadence

## Templates

| Scenario | Internal | External-safe |
|---|---|---|
| Sev-1 pause | [sev-1-pause-internal.md](sev-1-pause-internal.md) | [sev-1-pause-external.md](sev-1-pause-external.md) |
| Key compromise | [key-compromise-internal.md](key-compromise-internal.md) | [key-compromise-external.md](key-compromise-external.md) |
| Indexer lag | [indexer-lag-internal.md](indexer-lag-internal.md) | [indexer-lag-external.md](indexer-lag-external.md) |
| Bounty disclosure | [bounty-disclosure-internal.md](bounty-disclosure-internal.md) | [bounty-disclosure-external.md](bounty-disclosure-external.md) |
