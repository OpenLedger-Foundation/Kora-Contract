# FX Ingestion Service

**Issue #766 — Currency Conversion and FX Rate Ingestion Service**

Ingests FX rates for African local currencies against USDC/EURC from at least two independent data sources, cross-checks them for divergence, and publishes the consensus rate to the on-chain `price_oracle` contract via an authorized relay account.

---

## Architecture

```
ExchangeRateApiSource ─┐
                       ├─► CrossChecker ─► OracleRelay ─► price_oracle contract
CurrencyApiSource     ─┘
```

### Key design decisions

| Concern | Decision |
|---|---|
| Multi-source cross-check | Requires ≥2 sources. Consensus = arithmetic mean. Rejects if divergence > `maxDivergenceBps` |
| Source unavailability | If all sources fail → skip publication. Never publish a stale/guessed rate |
| Single-source fallback | If only 1 source responds → skip (cannot cross-check) |
| Staleness guard | Relay refuses to publish rates older than `maxStalenessSeconds` (mirrors oracle contract's own guard) |
| Rate limiting | Per-pair: at most 1 publication per `pollIntervalMs` cycle |
| Relay account | Separately permissioned `Feeder` role — not a protocol admin key |

---

## Supported currencies

NGN, KES, GHS, ZAR, TZS, UGX, XOF, EGP (configurable via `FX_CURRENCIES`)

---

## Configuration

| Env var | Default | Description |
|---|---|---|
| `FX_CURRENCIES` | `NGN,KES,GHS,ZAR,TZS,UGX,XOF,EGP` | Comma-separated currencies to track |
| `FX_QUOTE_CURRENCY` | `USDC` | Settlement stablecoin |
| `FX_MAX_DIVERGENCE_BPS` | `200` | Max source divergence before rejection |
| `FX_POLL_INTERVAL_MS` | `300000` | Ingestion cycle interval (5 min) |
| `ORACLE_CONTRACT_ADDRESS` | _(required)_ | price_oracle contract address |
| `FX_RELAY_SECRET` | _(required)_ | Relay account secret key |
| `EXCHANGE_RATE_API_KEY` | _(required)_ | exchangerate-api.com API key |
| `CURRENCY_API_KEY` | _(required)_ | currencyapi.com API key |
| `FX_MAX_STALENESS_SECONDS` | `3600` | Max rate age before staleness rejection |

In deployed environments, inject `FX_RELAY_SECRET`, provider API keys, and any
RPC credentials from the environment-specific `/kora/<environment>/fx-ingestion`
AWS Secrets Manager secret using a service-only read role. Keep public contract
addresses and non-secret settings in normal deployment configuration. Local
development should use disposable testnet credentials from a mode-`0600` file
under `$HOME/.config/kora/`; never use production credentials locally. See
[the secrets runbook](../../infra/secrets/README.md).

---

## Running

```bash
npm install
npm run build
npm start
```

Start the service only after its environment has been populated through the
local testnet secret file or the deployment secret manager. Do not put secret
values in command history.

## Testing

```bash
npm test
```

Minimum 90% coverage enforced. All HTTP calls are mockable via the `FxSource` interface.

---

## Surge pricing monitor & fee hints (#1034)

The same process runs a lightweight Stellar surge-pricing monitor
(`src/fee-monitor.ts`) and serves advisory fee hints (`src/fee-hint-server.ts`),
as recommended in [`docs/MEV_RESISTANCE_NOTES.md` §4.2](../../docs/MEV_RESISTANCE_NOTES.md#42-operational-best-practices).

```
Soroban RPC getFeeStats ──(poll)──► SurgePricingMonitor ──► GET /network/fee-hint
                                                      └──► GET /metrics (kora_surge_pricing_active, …)
```

- **Detection:** every poll takes the larger `p90` of the classic
  (`inclusionFee`) and Soroban (`sorobanInclusionFee`) distributions and
  divides it by the 100-stroop network minimum. A surge episode is active while
  that multiplier is `>= FEE_HINT_SURGE_THRESHOLD_BPS` (default 1.5x). Episodes
  start/end on poll boundaries, so `kora_surge_pricing_active` and
  `kora_surge_episodes_total` change within one poll interval.
- **Clamp:** the multiplier is clamped to `[1.0x, FEE_HINT_MAX_MULTIPLIER_BPS]`.
- **Degraded mode:** if `getFeeStats` errors, the service keeps running and
  serves the last-known-good hint (or 1.0x if none) with `degraded: true`,
  `stale: true` and header `X-Fee-Hint-Degraded: true`.
- **Flat stats:** if the RPC reports the same `latestLedger` as the previous
  poll, the reading is ignored and last-known-good is served with
  `X-Fee-Hint-Stale: true`. Hints older than `FEE_HINT_STALE_AFTER_MS` are also
  marked stale; `Age` reports seconds since observation.
- **Keys:** the monitor receives only `STELLAR_RPC_URL`. It never sees
  `FX_RELAY_SECRET` or any keeper/relayer key, and the endpoint never exposes
  configuration.
- **Rate limiting:** per-client fixed window (`FEE_HINT_RATE_LIMIT` per
  `FEE_HINT_RATE_LIMIT_WINDOW_MS`) returns `429` + `Retry-After`. The handler
  only reads cached state, so request volume never reaches the RPC. Keep the
  public gateway's own limits in front of it as well.

### `GET /network/fee-hint`

```json
{
  "network": "testnet",
  "base_fee": 100,
  "surge_multiplier_bps": 25000,
  "surge_multiplier": 2.5,
  "suggested_fee": 250,
  "surge_active": true,
  "observed_at": "2026-11-22T10:15:00.000Z",
  "latest_ledger": 1185010,
  "stale": false,
  "degraded": false,
  "advisory": true
}
```

Hints are **advisory**: a higher inclusion fee improves the chance of being
included during congestion but guarantees neither inclusion nor ordering.

### Fee denomination: testnet vs pubnet

Both networks denominate inclusion fees in **stroops** (1 XLM = 10^7 stroops)
with a 100-stroop minimum per operation, so `base_fee`/`suggested_fee` mean the
same on both. Differences are in *level*, not unit: testnet is reset
periodically and rarely congested, so surge episodes are mostly seen on
pubnet. Soroban resource fees (CPU, I/O, rent) are computed by
`simulateTransaction`/`prepareTransaction` and are **not** scaled by the hint.
Set `STELLAR_NETWORK` so the hint labels which network it describes.

### Configuration

| Env var | Default | Description |
|---|---|---|
| `FEE_HINT_ENABLED` | `true` | Set `false` to disable the monitor and endpoint |
| `STELLAR_RPC_URL` | `https://soroban-testnet.stellar.org` | Soroban RPC polled for `getFeeStats` (shared with the relay) |
| `STELLAR_NETWORK` | `testnet` | Network label reported in the hint |
| `FEE_HINT_PORT` | `8089` | HTTP port for `/network/fee-hint` and `/metrics` |
| `FEE_HINT_POLL_INTERVAL_MS` | `15000` | Fee-stats poll interval; also `Cache-Control: max-age` |
| `FEE_HINT_SURGE_THRESHOLD_BPS` | `15000` | Multiplier at/above which surge is active (1.5x) |
| `FEE_HINT_MAX_MULTIPLIER_BPS` | `100000` | Upper clamp for the suggested multiplier (10x) |
| `FEE_HINT_STALE_AFTER_MS` | `3 x poll interval` | Age after which a hint is reported stale |
| `FEE_HINT_RATE_LIMIT` | `60` | Requests per client per window |
| `FEE_HINT_RATE_LIMIT_WINDOW_MS` | `60000` | Rate-limit window |

Prometheus scrapes `fx-ingestion:8089/metrics` (`infra/monitoring/prometheus.yml`);
the "Stellar surge pricing" and "Fee hint multiplier" panels live in
`infra/monitoring/grafana-dashboard.json`. Tests replay recorded RPC fixtures
from `tests/fixtures/fee-stats-*.json` (`tests/fee-monitor.test.ts`).
