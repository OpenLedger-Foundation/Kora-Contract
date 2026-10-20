# Reporting Service

**Issue #770 — Data Export Service for Regulatory Reporting**

Generates structured, auditable data exports (CSV and PDF) covering funding, repayment, yield, and fee history for individual users or the protocol as a whole. Designed for regulatory and tax reporting needs in target jurisdictions.

---

## Architecture

```
HTTP request (scope, format, date range)
        │
        ▼
  ExportJobQueue.enqueue()  →  returns jobId immediately
        │  (async background processing)
        ▼
  ReportingDataSource (indexer + analytics)
        │
        ▼
  StatementBuilder  →  UserStatement | ProtocolStatement
        │
        ├─ CsvRenderer → .csv file
        └─ PdfRenderer → .pdf file
        │
        ▼
  output written to disk
        │
  Job status: ready
        │
  caller polls GET /export/:jobId/status
  caller downloads GET /export/:jobId/download
```

### Key design decisions

| Concern | Decision |
|---|---|
| Multi-asset separation | Per-asset totals are always computed independently — USDC and EURC are never added together |
| Reproducibility | `StatementBuilder` and renderers are deterministic functions — same inputs always produce same output |
| Large exports | Generated asynchronously; callers receive a jobId and poll until `ready`, then download |
| Async processing | SQLite-backed job queue; jobs survive service restarts |
| PDF generation | PDFKit streaming renderer |

---

## Export scopes

| Scope | Description |
|---|---|
| `per_user` | Funding, repayment, yield, fee history for one user address over a date range |
| `protocol_wide` | Aggregate totals and invoice counts across the entire protocol |

---

## Formats

| Format | Description |
|---|---|
| `csv` | Multi-section CSV with comment headers; machine-readable |
| `pdf` | Formatted A4 PDF for human review and filing |

---

## Configuration

| Env var | Default | Description |
|---|---|---|
| `REPORTING_DB_PATH` | `./reporting-jobs.db` | Export job queue DB |
| `REPORTING_OUTPUT_DIR` | `./exports` | Directory for generated files |
| `INDEXER_URL` | `http://localhost:8787` | Indexer service base URL (#1003) |
| `REPORTING_USE_FIXTURES` | `false` | Use static fixture data source in dev/CI (#1003) |
| `REPORTING_PORT` | `3040` | HTTP server port (#1004) |
| `REPORTING_HOST` | `127.0.0.1` | HTTP server bind address (#1004) |
| `REPORTING_API_KEYS` | _(none)_ | Comma-separated valid API keys (#1004) |
| `REPORTING_ADMIN_KEYS` | _(none)_ | Comma-separated admin API keys (#1004) |
| `REPORTING_RETENTION_TTL_S` | `604800` | Job retention in seconds (default 7 days) (#1004) |
| `REPORTING_RETENTION_CHECK_MS` | `60000` | Retention sweep interval in ms (#1004) |

---

## Running

```bash
npm install
npm run build
npm start
```

## Testing

```bash
npm test
```

Minimum 90% coverage enforced. Multi-asset separation, reproducibility, and async large-export completion are all explicitly tested.
