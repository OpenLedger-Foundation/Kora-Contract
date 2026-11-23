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
| `FX_METRICS_PORT` | `8081` | Port for the Prometheus `/metrics` scrape endpoint |

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

## Monitoring (staleness gauges)

The service exposes a Prometheus `/metrics` endpoint on `FX_METRICS_PORT`
(default `8081`) so monitoring alerts can detect stale FX rates before the
oracle's on-chain staleness guard rejects them:

- `kora_fx_rate_age_seconds{base,quote}` — seconds since the last successful
  publication. **No samples yet → a comment is emitted, not a bogus zero**, so a
  missing series drives alerts (see `infra/monitoring/README.md`).
- `kora_fx_last_ingestion_timestamp_seconds{base,quote}` — last publication time.
- `kora_fx_ingestion_cycles_total` — completed ingestion cycles.
- `kora_fx_publication_skipped_total{reason}` — skipped publications by reason.

Alert rules (`KoraFxRateStale`, `KoraFxRateNeverPublished`, `KoraFxIngestionDown`)
and the scrape target live in `infra/monitoring/`. Validate with
`promtool check rules infra/monitoring/alerts.yml` and
`promtool test rules infra/monitoring/tests/alerts.test.yml`.
