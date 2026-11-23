# Fractionalizer Contract

**Contract:** `contracts/fractionalizer/`  
**Issue:** [#744](https://github.com/OpenLedger-Foundation/Kora-Contract/issues/744) — Invoice NFT Fractionalization for Micro-Investment

---

## Overview

The Fractionalizer contract wraps a funded `financing_pool` investor position into a fixed supply of fungible share tokens. This enables retail micro-investors to participate in invoice financing below the effective minimum contribution by purchasing fractional shares from a position holder.

```
Investor (position holder)
    │
    │  wrap(invoice_id, total_shares=1000)
    ▼
FractionalizerContract
    ├── calls financing_pool.transfer_position(from=investor, to=self)
    ├── mints 1000 shares to investor
    └── stores WrapRecord

Investor distributes shares to micro-investors via transfer_shares()

On pool repayment (pool.is_closed == true):
    Anyone calls settle(invoice_id, original_investor)
    Each share holder calls redeem() → receives pro-rata payout
```

---

## Architecture

### Position Wrapping

`wrap(investor, invoice_id, total_shares)` atomically:
1. Reads the investor's `Position` from `financing_pool` (contributed amount, share_bps)
2. Reads the `Pool` to get the token address
3. Stores a `WrapRecord` with `total_shares`, `outstanding_shares`, `contributed`, `token`
4. Mints all `total_shares` to the investor's share balance
5. Calls `financing_pool.transfer_position` to move ownership to this contract

The position is now owned by the fractionalizer. The investor holds all shares initially.

### Share Distribution

`transfer_shares(from, original_investor, invoice_id, to, amount)` moves share balances between addresses. Shares are tracked as `u64` balances in persistent storage keyed by `(invoice_id, original_investor, holder)`.

Share transfers are gated by the same incident controls as the rest of the protocol:

- **Pause:** if `access_control.is_paused()` returns `true`, `transfer_shares` reverts with `ProtocolPaused` (code 4). This prevents share movement while the protocol is in a degraded/incident state.
- **Blacklist:** if either `from` or `to` is blacklisted in the access-control module, `transfer_shares` reverts with `Blacklisted` (code 15). Both the sender and the recipient are checked so a blacklisted address can neither send nor receive shares.

These checks run before any balance mutation, so a rejected transfer leaves balances unchanged (fail-closed, idempotent re-entry).

### Settlement

`settle(invoice_id, original_investor)` must be called after the pool closes:
1. Reads `pool.is_closed` — reverts with `PoolNotClosed` if not yet closed
2. Reads the position (now owned by this contract) to get `share_bps`
3. Computes `total_payout = pool.repaid_amount * share_bps / 10_000`
4. Stores `total_payout` in the `WrapRecord` and marks `settled = true`

### Pro-Rata Redemption

`redeem(holder, invoice_id, original_investor)`:
- Requires `settled == true`
- Prevents double-claim via `DataKey::Claimed(invoice_id, original_investor, holder)`
- Computes `payout = total_payout * holder_shares / total_shares`
- **Dust handling:** the last redeemer (when `outstanding_shares == holder_shares`) receives the exact remaining `total_payout` balance, absorbing any integer division remainder
- Burns the holder's shares (sets balance to 0)
- Transfers tokens from this contract to the holder

### Unwrap Guard

`unwrap(investor, invoice_id)` returns the position to the original investor, but **only if** the investor still holds all `total_shares`. If any shares have been transferred to third parties, `unwrap` reverts with `SharesOutstanding`. This prevents an investor from reclaiming a position while micro-investors hold outstanding shares.

---

## Storage Layout

| Key | Type | Description |
|-----|------|-------------|
| `Admin` | `Address` | Contract admin |
| `FinancingPool` | `Address` | Financing pool contract address |
| `AccessControl` | `Address` | Access control contract address |
| `Wrap(invoice_id, original_investor)` | `WrapRecord` | Wrap state for a position |
| `ShareBalance(invoice_id, original_investor, holder)` | `u64` | Share balance per holder |
| `Claimed(invoice_id, original_investor, holder)` | `bool` | Double-claim prevention flag |

---

## WrapRecord

```rust
pub struct WrapRecord {
    pub invoice_id: u64,
    pub original_investor: Address,
    pub total_shares: u64,        // fixed at wrap time
    pub outstanding_shares: u64,  // decremented on each redeem
    pub contributed: i128,        // underlying position contributed amount
    pub token: Address,           // pool token
    pub total_payout: i128,       // set by settle(); decremented on each redeem
    pub settled: bool,            // true after settle() is called
}
```

---

## Events

| Topic | Data | Description |
|-------|------|-------------|
| `FRAC_WRAP` | `(investor, total_shares, contributed)` | Position wrapped |
| `FRAC_SETL` | `(original_investor, payout)` | Wrap settled after pool close |
| `FRAC_REDM` | `(holder, holder_shares, payout)` | Shares redeemed |
| `FRAC_UNWP` | `(investor,)` | Position unwrapped |

---

## Error Codes

| Error | Code | Description |
|-------|------|-------------|
| `AlreadyInitialized` | 1 | Contract already initialized |
| `NotInitialized` | 2 | Contract not initialized |
| `NotAdmin` | 3 | Caller is not admin |
| `ProtocolPaused` | 4 | Protocol is paused |
| `InvalidAmount` | 5 | Zero or negative amount |
| `InvalidAddress` | 6 | Self-referential address |
| `ArithmeticOverflow` | 7 | Arithmetic overflow |
| `WrapNotFound` | 8 | No wrap record for this position |
| `SharesOutstanding` | 9 | Cannot unwrap while shares are distributed |
| `PoolNotClosed` | 10 | Pool not yet closed / not yet settled |
| `AlreadyClaimed` | 11 | Holder already redeemed |
| `NotShareHolder` | 12 | Caller holds no shares |
| `InvalidShareCount` | 13 | Share count is 0 or exceeds MAX_SHARES (1,000,000) |
| `PositionNotFound` | 14 | No position exists for investor |
| `Blacklisted` | 15 | Sender or recipient is blacklisted |

---

## Security Properties

- **Auth:** `wrap`, `unwrap`, `transfer_shares`, `redeem` all require `require_auth()` on the acting address
- **Pause:** `wrap`, `unwrap`, and `transfer_shares` check `access_control.is_paused()` and revert with `ProtocolPaused`
- **Blacklist:** `transfer_shares` checks both `from` and `to` against the access-control blacklist and reverts with `Blacklisted` if either is listed
- **CEI pattern:** `WrapRecord` is written to storage before `transfer_position` is called in `wrap`; `Claimed` flag is set before token transfer in `redeem`
- **Double-claim prevention:** `DataKey::Claimed` is set atomically before the token transfer; a second `redeem` call reverts with `AlreadyClaimed`
- **Dust determinism:** integer division remainder always goes to the last redeemer (when `outstanding_shares == holder_shares`), making dust allocation deterministic and auditable
- **Unwrap lock:** `unwrap` checks `investor_shares == wrap.total_shares` — any distributed share blocks unwrapping

---

## Lifecycle Diagram

```
wrap()
  └─► WrapRecord { settled: false, outstanding_shares: N }
        │
        ├─► transfer_shares() × M  (distribute to micro-investors)
        │     └─► gated by pause + blacklist checks
        │
        │   [pool repays, pool.is_closed = true]
        │
        ├─► settle()  →  WrapRecord { settled: true, total_payout: P }
        │
        └─► redeem() × M  (each holder claims pro-rata)
              └─► Claimed flag set, shares burned, tokens transferred
```

---

## Integration

The fractionalizer is a standalone contract that integrates with `financing_pool` via:
- `financing_pool.get_position(invoice_id, investor)` — read position before wrap
- `financing_pool.get_pool(invoice_id)` — read pool token and closed status
- `financing_pool.transfer_position(invoice_id, from, to)` — transfer ownership

No changes to `financing_pool` are required. The fractionalizer uses the existing `transfer_position` entry point.
