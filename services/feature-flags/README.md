# Feature Flag Service

Server-evaluated feature flags for gradual rollout of Kora Protocol's new features (secondary market, fractionalization, insurance pool, etc.).

## Design

Flags are evaluated **server-side** — the client only receives resolved `true/false` values, never targeting rules. This prevents trivial bypass by inspecting local state.

```
Client                    API Gateway              Feature Flag Service
  │                           │                           │
  │── GET /flags ────────────►│                           │
  │                           │── resolveForUser(id) ────►│
  │                           │◄── FlagSnapshot ──────────│
  │◄── { flags: {...} } ──────│                           │
  │                           │                           │
  │── GET /flags/stream ──────►│ (SSE keep-alive)          │
  │                           │── onKillSwitch ───────────►│
  │◄── SSE: flag-changed ─────│◄──────────────────────────│ (admin disables)
```

### Kill-Switch

`PUT /admin/flags/:name/disable` disables a flag **instantly** for all users. Connected SSE clients receive a `flag-changed` event within milliseconds. No redeploy required.

The frontend must listen on `/flags/stream` and gracefully handle a `value: false` event for any feature the user is currently using (show a "feature temporarily unavailable" message rather than a broken UI).

### Percentage Rollout

The rollout bucket is computed deterministically:

```
bucket = SHA256(userId + ":" + flagName + ":" + salt) % 10000
enabled = bucket < rolloutBps
```

The same user always lands in the same bucket for a given flag. Different flags use different salts so rollout membership doesn't correlate across flags.

### Audit Logging

Every flag enable/disable/update is sent to the `audit-log` service. Audit failures are non-fatal — they are logged but do not block flag operations.

## API

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/flags` | Session-start snapshot — returns all flag values for the authenticated user |
| `GET` | `/flags/stream` | SSE stream — pushed `flag-changed` events for kill-switch propagation |
| `PUT` | `/admin/flags/:name/enable` | Re-enable a flag |
| `PUT` | `/admin/flags/:name/disable` | Kill-switch: instantly disable for all users |
| `POST` | `/admin/flags` | Create or update a flag definition |

## Protocol Flow Kill Switches (#1005)

The following flags gate protocol flows in `apps/web`. Disabling a flag hides the primary CTA and shows a graceful INCIDENT_RESPONSE-linked message without requiring a web redeploy.

| Flag name | Gated flow |
|---|---|
| `secondary-market` | `FundingFlow`, `MarketplaceBrowsingFlow` |
| `partial-repayment` | `RepaymentFlow` |
| `fractionalization` | `InvoiceSubmissionFlow`, `InvoiceSubmissionWizard` |
| `multi-asset-pool` | `MultiStepFundingFlow` |

### Web client cache and SSE refresh interval (#1005)

The `FeatureFlagProvider` in `apps/web` fetches the snapshot once at session start and refreshes it every **30 seconds** in the background. Kill-switch events delivered via SSE apply **immediately** (no polling delay). The maximum stale window is therefore bounded by the SSE reconnect delay (5 seconds on error) rather than the refresh interval.

### Maximum stale window SLA (#1006)

| Scenario | Max stale window |
|---|---|
| SSE connected | < 1 second (event pushed synchronously on `store.setEnabled`) |
| SSE reconnecting (after error) | ≤ 5 seconds (SSE_RECONNECT_DELAY_MS) |
| Snapshot polling only (SSE disabled) | ≤ 30 seconds (SNAPSHOT_REFRESH_INTERVAL_MS) |
| Feature-flags service `cacheTtlMs` | 5 seconds (default) — irrelevant for in-process writes |

## Usage

```typescript
import { FlagStore, FeatureFlagService } from "@kora/feature-flags";

const store = new FlagStore("flags.db");
const svc = new FeatureFlagService(store, auditClient);

// Session start — send to client
const snapshot = svc.resolveForUser(userId);
// → { "secondary-market": true, "fractionalization": false, ... }

// Mid-request check
if (svc.isEnabled("secondary-market", userId)) { ... }

// Kill-switch
await svc.disable("secondary-market", "admin@kora.finance");
// All SSE listeners receive: { type: "flag-changed", name: "secondary-market", value: false }
```

## Testing

```bash
npm test
# Coverage threshold: 90% lines/branches/functions/statements
```

## Graceful Mid-Flow Disable

When a flag is disabled while a user is mid-flow:

1. SSE stream pushes `{ type: "flag-changed", name: "...", value: false }` to all connected clients
2. Frontend listens for this event and shows a user-friendly message:
   > "This feature has been temporarily paused. Your progress has been saved."
3. The UI prevents further interaction with the disabled feature without a page reload
4. No partial state is committed — the backend rejects requests for disabled features server-side

The `isEnabled()` check on every API route ensures server-side enforcement even if the client misses the SSE event.
