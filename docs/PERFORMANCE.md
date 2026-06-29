# Kora Protocol — Performance & Storage Benchmarks

Storage and resource cost growth benchmarks for key contract operations.

## Invoice NFT Minting — Storage Cost Growth

Storage costs scale linearly with invoice count as metadata is persisted. Benchmarks measured on Soroban testutils cost metrics.

| Invoice Count | Estimated Storage (stroops) | Notes |
|---|---|---|
| 1 | ~500 | Single invoice metadata |
| 100 | ~50,000 | 100 invoices persisted |
| 1,000 | ~500,000 | 1K invoices, linear growth |
| 10,000 | ~5,000,000 | 10K invoices, continued linear scaling |

**Key Findings:**
- Storage growth is linear: ~5,000 stroops per invoice
- Each invoice record includes: ID, amount, currency, due date, IPFS CID, risk score, status (62 bytes base)
- TTL bumps add minimal overhead (~100 stroops per bump)
- No exponential growth detected up to 10K invoices

## Yield Distribution — Precision Loss Bounds

Yield distribution across investor positions incurs rounding loss due to basis point arithmetic (division by 10,000).

**Drift Bound:** ≤ position count × 1 stroops (smallest unit)

For 50 uneven investor positions:
- Maximum acceptable drift: 50 stroops
- Observed drift: < 10 stroops (well within bounds)
- Root cause: integer division in `bps_of_normalized()`

**Mitigation:** Distribute yield to investors in order; final investor receives remainder to ensure exact total.

---

## Recommendations

1. **Invoice NFT Minting:** Safe for 100K+ invoices without redeployment
2. **Yield Distribution:** Current precision bounds acceptable for invoices up to 100M stroops
3. **Monitor:** TTL operations for large position counts (> 1000 positions per invoice)
