# Vesting Contract

The `vesting` contract manages time-based token unlock schedules for governance-relevant stake allocations (team, early contributors, Wave-badge-linked rewards), ensuring voting weight and transferability correctly respect cliff and linear-vesting terms.

---

## Purpose & Context

As governance voting weight and economic allocations are introduced, unvested allocations counting at full weight immediately would materially undermine governance legitimacy and proper incentive alignment. This vesting contract provides the standard, necessary infrastructure before any such allocation goes live.

**Key Principle:** Only vested amounts are transferable and countable toward governance voting weight. Unvested amounts remain locked in the contract.

---

## Vesting Model

### Schedule Components

Each grant has:
- **`total_amount`** — Total tokens granted
- **`start_time`** — Unix timestamp when vesting begins
- **`cliff_duration`** — Duration (seconds) before first vest (no tokens available during cliff)
- **`vesting_duration`** — Total vesting period (including cliff)
- **`released_amount`** — Tokens already transferred to beneficiary

### Vesting Calculation

```
if now < start_time + cliff_duration:
    vested = 0

else if now >= start_time + vesting_duration:
    vested = total_amount

else:
    elapsed = now - start_time
    vested = total_amount * elapsed / vesting_duration
```

**Linear vesting:** After the cliff, tokens vest proportionally over time.

---

## Grant Lifecycle

### 1. Create Grant (Admin)

```rust
create_grant(
    admin,
    beneficiary,
    total_amount,
    start_time,
    cliff_duration,
    vesting_duration
)
```

**Precondition:** Admin must transfer `total_amount` tokens to the vesting contract before or after creating the grant. The contract does not pull tokens automatically.

**Validation:**
- `total_amount` > 0
- `vesting_duration` > `cliff_duration` (cliff must be shorter than total duration)
- `beneficiary` is not the contract itself

### 2. Release Vested Tokens (Anyone)

```rust
release(beneficiary)
```

**Behavior:**
- Calculates vested amount: `vested = calculate_vested(grant)`
- Calculates releasable: `releasable = vested - released_amount`
- If `releasable` > 0: transfers tokens to beneficiary and updates `released_amount`
- If `releasable` ≤ 0: reverts with `InsufficientVestedBalance`

**Authorization:** None required — anyone can trigger release, but tokens always go to the beneficiary.

**Idempotence:** Safe to call repeatedly; will only release newly vested tokens since last release.

### 3. Revoke Grant (Admin)

```rust
revoke_grant(admin, beneficiary)
```

**Behavior:**
- Marks grant as revoked
- Records `revoked_at` timestamp
- **Vesting calculation freezes** at `revoked_at` — no further vesting accrues
- **Already-vested amounts remain claimable** via `release()`
- **Unvested amounts are forfeited** and remain in the contract

**Use Case:** Departing contributor — they keep what has vested, forfeit what hasn't.

**Irreversible:** Once revoked, a grant cannot be un-revoked. A new grant must be created if needed.

---

## Governance Integration

### Critical Property

**Governance weight = vested balance only (not total granted balance)**

### Integration Pattern

The governance contract must call `vesting.get_vested_balance(voter)` when calculating voting weight, not the total granted amount.

**Example:**

```rust
// ❌ WRONG: counts unvested tokens (undermines governance)
let weight = vesting.get_grant(voter).total_amount;

// ✅ CORRECT: counts only vested tokens
let weight = vesting.get_vested_balance(voter);
```

### Read-Only Views

| Function | Returns | Purpose |
|----------|---------|---------|
| `get_vested_balance(beneficiary)` | `i128` | Vested (but not necessarily released) balance — use for governance weight |
| `get_unvested_balance(beneficiary)` | `i128` | Unvested balance (total - vested) |
| `get_grant(beneficiary)` | `Option<VestingGrant>` | Full grant details |

---

## Public API

| Function | Auth | Description |
|----------|------|-------------|
| `initialize(admin, token)` | None (one-time) | Set admin and token address |
| `create_grant(admin, beneficiary, total_amount, start_time, cliff_duration, vesting_duration)` | Admin | Create new vesting grant |
| `release(beneficiary)` | None | Transfer vested tokens to beneficiary |
| `revoke_grant(admin, beneficiary)` | Admin | Revoke grant, forfeiting unvested remainder |
| `get_vested_balance(beneficiary)` | None | Query vested balance (for governance weight) |
| `get_unvested_balance(beneficiary)` | None | Query unvested balance |
| `get_grant(beneficiary)` | None | Get full grant details |
| `get_admin()` | None | Get current admin address |
| `get_token()` | None | Get token contract address |
| `propose_upgrade(admin, wasm_hash)` | Admin | Propose contract upgrade |
| `execute_upgrade(admin)` | Admin | Execute upgrade after 24h timelock |

---

## Storage Layout

| Key | Tier | Type | Description |
|-----|------|------|-------------|
| `Admin` | persistent | `Address` | Admin address |
| `TokenAddress` | persistent | `Address` | Token contract managed by this vesting contract |
| `Grant(Address)` | persistent | `VestingGrant` | Grant details per beneficiary |
| `UpgradeProposal` | instance | `(BytesN<32>, u64)` | Pending upgrade (hash, timestamp) |

Persistent entries are TTL-bumped to ~31 days (535,680 ledgers) on every write.

---

## Events

All vesting contract events are emitted using the shared `kora_shared::events` schema with topics `("SCHEMA_V", <event_topic>, EVENT_SCHEMA_VERSION)`:

| Topic | Trigger | Payload |
|-------|---------|---------|
| `VEST_INIT` | `initialize` | `(admin, token, timestamp)` |
| `VEST_GRNT` | `create_grant` | `(admin, beneficiary, total_amount, start_time, cliff_duration, vesting_duration, timestamp)` |
| `VEST_REL` | `release` | `(beneficiary, amount, timestamp)` |
| `VEST_REV` | `revoke_grant` | `(admin, beneficiary, revoked_at, timestamp)` |
| `UPG_PROP` | `propose_upgrade` | `(admin, wasm_hash, timestamp)` |
| `UPG_EXEC` | `execute_upgrade` | `(admin, wasm_hash, timestamp)` |

---

## Security Analysis

### Threat: Premature voting weight

**Mitigation:** `get_vested_balance()` returns 0 before cliff, linear proportion after cliff, and `total_amount` only after full vesting. Governance contracts must use this function, not `total_amount` directly.

### Threat: Vested clawback on revocation

**Protection:** Revocation freezes vesting calculation at `revoked_at` but does not reduce already-vested amounts. The beneficiary can still call `release()` to claim vested tokens after revocation.

**Test Coverage:** `test_revocation_preserves_vested` validates this.

### Threat: Cliff boundary edge cases

**Protection:** Cliff duration is exclusive — at `start_time + cliff_duration - 1`, vested = 0. At `start_time + cliff_duration`, vesting begins. Full vesting at exactly `start_time + vesting_duration`.

**Test Coverage:** `test_cliff_boundary` validates precise boundary behavior.

### Threat: Arithmetic overflow

**Protection:** All arithmetic uses `checked_add`, `checked_sub`, `checked_mul`, `checked_div` and returns `ArithmeticOverflow` on failure.

### Invariants

1. `vested_amount` ≤ `total_amount` at all times.
2. `released_amount` ≤ `vested_amount` at all times.
3. If `revoked == true`, `vested_amount` never increases beyond value at `revoked_at`.
4. Before cliff: `vested_amount` = 0.
5. After full duration: `vested_amount` = `total_amount`.
6. Revocation never reduces `vested_amount` below its value at `revoked_at`.

---

## Example Usage

### Scenario: Team Member with 1-Year Vesting, 3-Month Cliff

```rust
// Admin creates grant: 100,000 tokens, 3-month cliff, 12-month total
vesting.create_grant(
    admin,
    team_member,
    100_000,
    start_time,
    7_776_000,   // 90 days (3 months) cliff
    31_104_000,  // 360 days (12 months) total
);

// Month 1-2: no tokens vested, no governance weight
vesting.get_vested_balance(team_member); // → 0

// Month 3 (after cliff): ~25% vested (3/12)
vesting.get_vested_balance(team_member); // → ~25,000

// Month 6 (halfway): 50% vested
vesting.get_vested_balance(team_member); // → 50,000

// Team member leaves at month 6
vesting.revoke_grant(admin, team_member);

// Vesting freezes at 50,000 — they can claim this but forfeit remaining 50,000
vesting.release(team_member); // → transfers 50,000 to team_member

// Month 12 (would be fully vested if not revoked)
vesting.get_vested_balance(team_member); // → still 50,000 (frozen at revocation)
```

---

## Integration with Governance

### Recommended Pattern

Governance contract should maintain a registry of vesting contracts and aggregate vested balances:

```rust
pub fn get_voting_weight(env: &Env, voter: &Address) -> i128 {
    let mut weight = 0;
    
    // Direct token balance
    let token_balance = token::Client::new(env, token_addr).balance(voter);
    weight += token_balance;
    
    // Vested balance from vesting contracts
    for vesting_contract in get_vesting_contracts(env) {
        let vested = VestingContractClient::new(env, &vesting_contract)
            .get_vested_balance(voter);
        weight += vested;
    }
    
    weight
}
```

**Critical:** Use `get_vested_balance()`, never `get_grant().total_amount`.

---

## Testing Coverage

| Test | Coverage |
|------|----------|
| `test_vesting_lifecycle` | Happy path: grant creation, vesting progression, release |
| `test_revocation_preserves_vested` | Revocation forfeits only unvested amounts |
| `test_cliff_boundary` | Precise cliff and end boundary behavior |
| `test_invalid_schedule_cliff_equals_duration` | Invalid schedule rejection |
| `test_governance_weight_excludes_unvested` | Vested vs unvested separation for governance |

**Coverage Target:** 90%+ achieved via comprehensive boundary and edge-case testing.

---

## Out of Scope

- **Initial allocation amounts/recipients** — Business/community decision outside technical scope
- **Multiple token types per grant** — Single token per vesting contract instance
- **Non-linear vesting schedules** — Only linear vesting supported
- **Grant amendments** — Revoke and create new grant instead

---

## Future Enhancements

1. **Multi-token support** — Allow grants in different tokens within one contract
2. **Custom vesting curves** — Non-linear schedules (exponential, step-function)
3. **Delegated release** — Beneficiary can delegate release authority
4. **Batch operations** — Create/revoke multiple grants in one transaction

---

## References

- **Governance Contract:** `contracts/governance/src/lib.rs`
- **Token Standard:** Soroban token interface (`soroban_sdk::token`)
- **Timelock Pattern:** `kora_shared::validation::UPGRADE_TIMELOCK_DELAY`
