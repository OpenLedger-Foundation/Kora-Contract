# Incident Response Communication Templates

Standardized, pre-approved stakeholder communication templates for the Kora Protocol on-call team, incident commanders, and communication leads during security incidents, protocol emergencies, and operational disruptions.

---

## Purpose

During active, high-stress security incidents, drafting public announcements from scratch introduces communication latency, inconsistent messaging, and risks of accidental disclosure. These templates provide:

1. **Pre-vetted language** that prevents speculation while maintaining transparency.
2. **Standardized fill-in tokens** (e.g., `[TIMESTAMP_UTC]`, `[IMPACT_SUMMARY]`) to ensure critical operational details are never omitted.
3. **Structured cadence** matching the update schedules defined in [`docs/INCIDENT_RESPONSE.md`](../INCIDENT_RESPONSE.md#communication-protocol).

---

## Template Directory

| Template | Scenario | Primary Channels | Target Response |
|----------|----------|------------------|-----------------|
| [`sev-1-pause.md`](./sev-1-pause.md) | Protocol emergency pause (active exploit or Sev-1 critical bug) | Discord `#announcements`, X/Twitter, Status Page | < 15–30 minutes of pause execution |
| [`key-compromise.md`](./key-compromise.md) | Co-signer, guardian, or risk verifier key rotation/revocation | Discord `#announcements`, Status Page, Multi-sig channel | < 1 hour of rotation proposal |
| [`indexer-lag.md`](./indexer-lag.md) | Event indexer desync, RPC latency, or frontend state delay | Discord `#support`, X/Twitter, Status Page | < 30 minutes of lag detection |
| [`bounty-disclosure.md`](./bounty-disclosure.md) | Coordinated disclosure of patched bug bounty report | Discord `#dev-announcements`, Blog, Security advisory | Post-remediation & timelock completion |

---

## On-Call Dispatch Workflow

When an incident is declared by the Incident Commander:

```
[Incident Declared]
       │
       ▼
[Select Corresponding Template]
       │
       ▼
[Fill Standardized Tokens (Bracketed)]
       │
       ▼
[Peer Review by Incident Commander & Tech Lead]
       │
       ▼
[Broadcast Across Designated Channels]
       │
       ▼
[Schedule Next Status Update Timer]
```

### Review Rules
- **No speculative causes:** Do not guess the root cause publicly before the Technical Lead confirms the proof of concept or patch.
- **Always specify user action:** Explicitly state whether users need to withdraw, revoke allowances, or do nothing.
- **Maintain update cadence:** If `[NEXT_UPDATE_ETA]` is specified, an update MUST be posted before or at that timestamp, even if only to state that investigation is ongoing.

---

## Common Fill-In Token Reference

| Token | Description | Example |
|-------|-------------|---------|
| `[INCIDENT_ID]` | Internal incident tracking identifier | `INC-2026-10-A` |
| `[TIMESTAMP_UTC]` | Current date and time in UTC | `2026-10-20 14:00 UTC` |
| `[AFFECTED_COMPONENTS]` | Contracts, pools, or infrastructure involved | `financing-pool`, `indexer-v2` |
| `[IMPACT_SUMMARY]` | Concise, factual statement of user impact | `Protocol paused; no user funds drained.` |
| `[USER_ACTION_REQUIRED]` | Instructions for end users and liquidity providers | `No user action required at this time.` |
| `[NEXT_UPDATE_ETA]` | Promised timestamp for next official update | `2026-10-20 16:00 UTC (in 2 hours)` |
| `[STATUS_PAGE_URL]` | Official status dashboard link | `https://status.kora.finance` |
