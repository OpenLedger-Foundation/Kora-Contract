# Invoice Metadata Pinning Service

**Issue #754 — Build Invoice Metadata Pinning Service for IPFS**

Accepts SME-submitted invoice documents/metadata, pins them redundantly to
IPFS, and returns the CID recorded on-chain in `invoice_nft`. Verifies pin
persistence periodically and re-pins/alerts on failure.

Out of scope: on-chain CID recording itself (the SME's own transaction handles
that).

## Architecture

```
upload(PinRequest)
  ├─ validate format/size (allowlist + maxFileSizeBytes)
  ├─ require piiHandled (PII hashed/encrypted before pinning)
  ├─ hash-verify SHA-256 BEFORE pinning (reject INTEGRITY_MISMATCH)
  ├─ pin via PinProvider[] (≥2 backends: e.g. Pinata + web3.storage)
  │    ├─ retry transient errors with bounded exponential backoff
  │    └─ persist non-retryable provider errors in SQLite DLQ
  └─ persist PinStatusRecord (SQLite) ──► PinVerifier.verifyOnce() (periodic)
                                            ├─ isPinned per provider
                                            ├─ repin from cache on failure
                                            └─ onAlert after N failures
```

## Usage

```ts
import { PinningService, PinStore, InMemoryPinProvider } from "@kora/ipfs-pinning";

const store = new PinStore("./pins.db");
const svc = new PinningService(
  [new HttpPinProvider("pinata", process.env.PINATA_URL!, process.env.PINATA_TOKEN!),
   new HttpPinProvider("web3storage", process.env.W3S_URL!, process.env.W3S_TOKEN!)],
  store,
);
const res = await svc.upload({
  filename: "invoice-42.json",
  content: bytes,
  contentType: "application/json",
  claimedSha256Hex: "<client-computed sha256>",
  piiHandled: true,
});
// res.cid → record on-chain via invoice_nft.mint_invoice
```

## Configuration

| Option | Default | Description |
|---|---|---|
| `maxFileSizeBytes` | `10 MiB` | Files above are rejected pre-pin |
| `allowedContentTypes` | pdf/json/png/jpeg | MIME allowlist |
| `allowedExtensions` | `.pdf .json .png .jpg .jpeg` | Extension allowlist |
| `minSuccesses` | `1` | Minimum provider pins; total failure throws |
| `pinRetryAttempts` | `3` | Total attempts per provider, including the first |
| `pinRetryBackoffMs` | `250 ms` | Initial retry delay; doubles per retry |
| `pinRetryMaxBackoffMs` | `5,000 ms` | Maximum retry delay |

HTTP 408, 425, 429, and 5xx responses and network errors are retryable.
Other HTTP 4xx responses are permanent and create a pending `pin_dead_letters`
entry. The DLQ stores the CID, provider, failure, and replay state, but never
stores file bytes.

## DLQ operations and metrics

`PinningService.getMetrics()` reports retry, permanent-failure, and replay
counters plus pending DLQ depth. `metricsText()` exports the same values as
Prometheus text, including `kora_ipfs_pin_dead_letter_pending`.

To replay a pending entry, inspect `PinStore.listDeadLetters()` (or fetch one by
ID), retrieve the corresponding document through the approved protected source,
and call `await pinningService.replayDeadLetter(id, content)`. Replay verifies
that SHA-256(content) matches the recorded CID before retrying only the failed
provider. A successful replay marks the entry resolved; a failed replay
increments its attempt count and leaves it pending. Never add document bytes
to the DLQ as an operational shortcut.

## Testing

```bash
npm install
npm test   # 90% line/branch/function/statement coverage enforced
```

Covers dual-provider success, partial-failure degradation, transient retries,
permanent-failure DLQ/replay, integrity-mismatch rejection, and periodic
re-pin-on-failure + alerting.

## REST API surface (consumed by `apps/web` — Issue #1001)

The web invoice wizard talks to this service through `apps/web/src/services/ipfsPinningClient.ts`:

| Method | Path | Description |
|---|---|---|
| `POST` | `/pins` | Upload + redundant pin. Raw bytes as body; `X-Filename`, `X-SHA256` (client-computed digest), `X-Content-Type`, `X-PII-Handled` headers. Responds with the `PinResult` JSON (content-derived CID, `sha256Hex`, per-provider outcomes, `degraded`). |
| `GET` | `/pins/:cid` | Pin status polling (`pinned` / `degraded` / `failed` / `repinned`). |
| `GET` | `/pins/:cid/content` | Pinned bytes — used by the wizard to re-hash content before mint (issue #1002). |
| `GET` | `/pins/:cid/verify` | Service-side verification (`{ cid, verified, sha256Hex }`). |

Errors are returned as `{"error": "<CODE>", "message": "...", "providers": [...]}`
with codes mirroring the pipeline (`FILE_TOO_LARGE`, `EMPTY_FILE`,
`UNSUPPORTED_TYPE`, `UNSUPPORTED_EXTENSION`, `EXTENSION_TYPE_MISMATCH`,
`CONTENT_TYPE_MISMATCH`, `INTEGRITY_MISMATCH`, `PII_NOT_HANDLED`, `PIN_FAILED`).

### Content-type allowlist (issue #1186)

Pinned invoice metadata is restricted to `application/pdf`,
`application/json`, `image/png` and `image/jpeg`:

- The declared type is normalized (case, whitespace, `charset=utf-8` only);
  any other MIME parameter is rejected as `UNSUPPORTED_TYPE`.
- The filename extension must belong to the declared type
  (`EXTENSION_TYPE_MISMATCH`, e.g. `invoice.pdf` declared as `image/png`).
- Content is sniffed after the integrity check: PDF/PNG/JPEG magic bytes, and
  strict UTF-8 + `JSON.parse` for JSON (`CONTENT_TYPE_MISMATCH`). This blocks
  HTML/SVG/script blobs disguised as allowlisted types from being served as
  active content by gateways.
- `maxSizeBytesByContentType` adds per-type caps under `maxFileSizeBytes`
  (default: JSON metadata ≤ 256 KiB) to protect pin quota.
- Misconfiguration fails closed at construction with `CONFIG_ERROR`: empty
  allowlists, a non-positive size cap, or an allowlisted type with no sniffing
  rule. Duplicate pins of the same content hash are
idempotent: the CID is SHA-256-derived and the pin record is upserted.

Client configuration: set `VITE_IPFS_PINNING_URL` in `apps/web` (see
`apps/web/README.md`).
