# Secondary Market

`contracts/secondary_market` lets investors list financing-pool positions for
sale. Buyers pay the listed price; a protocol fee (0–1000 bps, default 250) is
routed to the treasury and the rest to the seller, then the position is moved
with `financing_pool.transfer_position`.

## Fill accounting

All settlement amounts come from one pure function,
`kora_secondary_market::quote_fill(price, size, filled, fill_amount, fee_bps)`,
which returns a `FillQuote { gross, fee, seller_proceeds, filled_after, remaining }`.

- A listing has a total `price` for `size` units, of which `filled` are already sold.
- Fills are priced **cumulatively**: `gross = floor(price·filled_after/size) − floor(price·filled/size)`
  and `fee = bps_of(cum_after) − bps_of(cum_before)`. Rounding dust therefore
  lands on a later fill instead of being lost or charged twice.
- `buy_position` is the exact fill of a single-unit listing
  (`quote_fill(price, 1, 0, 1, fee_bps)`), so a whole-position sale charges
  exactly `price` and exactly `bps_of(price, fee_bps)`.
- `quote_purchase(invoice_id, seller)` is a read-only view that discloses the
  quote a buyer would settle at the current fee rate.

## Invariants

| # | Invariant | Covered by (`contracts/tests/secondary_market_fill_invariants.rs`) |
|---|-----------|------|
| 1 | **Fee bps identity:** for every fill `fee + seller_proceeds == gross`, `0 <= fee <= gross`. | `multi_fill_invariants` |
| 2 | **Remaining accounting:** `filled_after + remaining == size`; `remaining` strictly decreases with each fill. | `multi_fill_invariants` |
| 3 | **No overcharge on multi-fill:** cumulative `gross <= price` and cumulative `fee <= bps_of(price, fee_bps)` at every step. | `multi_fill_invariants` |
| 4 | **No rounding drift:** any sequence of fills that closes the listing charges exactly `price` and exactly `bps_of(price, fee_bps)` — the same as a single fill. | `complete_fill_matches_single_shot`, `rounding_dust_is_not_lost_across_fills` |
| 5 | **Overfill rejected:** `fill_amount > size − filled` returns `Overfill` (this includes any fill on a closed listing). | `overfill_rejected`, `exact_fill_closes_listing_and_further_fills_overfill` |
| 6 | **Exact fill closes listing:** `remaining == 0`; on-chain the listing is removed and a second `buy_position` returns `ListingNotFound`. | `exact_fill_closes_listing_and_further_fills_overfill`, `on_chain_exact_fill_settles_disclosed_quote_and_closes_listing` |
| 7 | **Cancel with residual:** cancelling moves no funds; the unfilled residual (`size − filled`) stays with the seller and can be relisted. | `cancel_with_residual`, `cancel_leaves_residual_with_seller_and_moves_no_funds` |
| 8 | **Disclosure == settlement:** token deltas of buyer, seller and treasury equal the `quote_purchase` disclosure, including after a fee update. | `on_chain_exact_fill_settles_disclosed_quote_and_closes_listing`, `disclosure_tracks_fee_updates` |
| 9 | **Unpriceable fills rejected:** a fill whose cumulative price delta is 0 returns `InvalidAmount` rather than transferring units for free. | `rejects_invalid_inputs` |

Fee routing for whole-position sales is also covered by
`contracts/tests/secondary_market_fee_routing.rs`.

Run:

```bash
cargo test -p kora-tests --test secondary_market_fill_invariants
```
