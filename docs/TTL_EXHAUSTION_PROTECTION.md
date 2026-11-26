# TTL Exhaustion Attack Protection

Protection against deliberate or accidental TTL (Time-To-Live) expiration of critical storage entries.

---

## Problem Statement

Soroban's storage-rent model requires periodic TTL extension to keep data accessible. If critical entries (pools, positions, invoices) expire:

1. **Fund lock:** Investor funds become inaccessible until restoration
2. **Operation disruption:** Contract calls fail on archived entries
3. **MEV extraction:** Attackers front-run restoration for profit
4. **Trust damage:** Users perceive protocol as unreliable

### Attack Vectors

**Malicious neglect:**
- Attacker deliberately doesn't extend TTL on positions they control
- Forces other parties to pay restoration costs
- Disrupts protocol operations

**Accidental expiry:**
- Keeper bot downtime during critical period
- Gas price spike prevents economical extension
- Human error in manual TTL management

**Front-running:**
- Attacker monitors for near-expiry entries
- Lets them expire, then front-runs restoration
- Extracts fees or manipulates state during restoration window

---

## Defense Architecture

### Layer 1: Automatic Extension on Access

Every read or write to a critical entry automatically extends its TTL:

```rust
use kora_shared::ttl_protection::{extend_critical_entry, CriticalEntryType};

pub fn get_pool(env: Env, pool_id: u64) -> Result<Pool, Error> {
    let key = DataKey::Pool(pool_id);
    
    // Extend TTL before returning data
    extend_critical_entry(&env, &key, CriticalEntryType::Pool);
    
    env.storage()
        .persistent()
        .get(&key)
        .ok_or(Error::PoolNotFound)
}
```

**Benefit:** As long as entries are actively used, they never expire.

**Limitation:** Dormant positions (no activity for >60 days) still at risk.

### Layer 2: Permissionless Bump Functions

Anyone can extend TTL on critical entries, not just the owner:

```rust
/// Permissionless TTL extension for a pool entry
/// Anyone can call to prevent expiration (no auth required)
pub fn bump_pool_ttl(env: Env, pool_id: u64) {
    let key = DataKey::Pool(pool_id);
    extend_critical_entry(&env, &key, CriticalEntryType::Pool);
    events::emit_ttl_extended(&env, pool_id, "Pool");
}

/// Batch bump multiple pools in one transaction
pub fn bump_pools_batch(env: Env, pool_ids: Vec<u64>) -> u32 {
    let mut count = 0u32;
    for pool_id in pool_ids.iter() {
        bump_pool_ttl(env.clone(), pool_id);
        count += 1;
    }
    count
}
```

**Benefit:** Keeper bots or altruistic users can prevent neglect attacks.

**Limitation:** Requires off-chain monitoring to detect near-expiry entries.

### Layer 3: Keeper Bot Monitoring

Off-chain service monitors entry TTLs and automatically extends before expiry:

**Implementation:** `scripts/ttl_keeper.sh`

```bash
#!/bin/bash
# TTL Keeper Bot - monitors and extends critical entries

# Configuration
RPC_URL="https://soroban-rpc.stellar.org"
CONTRACT_ID="CXXXXXX..."
THRESHOLD=259200  # Bump when <15 days remaining
POLL_INTERVAL=3600  # Check every hour

while true; do
    # Query all active pools
    POOLS=$(stellar contract invoke \
        --id $CONTRACT_ID \
        --fn get_all_active_pools \
        --rpc-url $RPC_URL)
    
    for POOL_ID in $POOLS; do
        # Check remaining TTL
        ENTRY_KEY="Pool($POOL_ID)"
        TTL_INFO=$(stellar rpc getLedgerEntries \
            --keys $ENTRY_KEY \
            --rpc-url $RPC_URL)
        
        CURRENT_LEDGER=$(echo $TTL_INFO | jq '.latestLedger')
        LIVE_UNTIL=$(echo $TTL_INFO | jq '.entries[0].liveUntilLedgerSeq')
        REMAINING=$((LIVE_UNTIL - CURRENT_LEDGER))
        
        if [ $REMAINING -lt $THRESHOLD ]; then
            echo "Pool $POOL_ID at risk: $REMAINING ledgers remaining"
            
            # Extend TTL
            stellar contract invoke \
                --id $CONTRACT_ID \
                --fn bump_pool_ttl \
                --arg $POOL_ID \
                --rpc-url $RPC_URL
            
            echo "Extended TTL for pool $POOL_ID"
        fi
    done
    
    sleep $POLL_INTERVAL
done
```

**Priority queue:**
1. Pools with investor funds (highest priority)
2. Active positions
3. Active invoices
4. Active listings
5. SME profiles
6. Verifier registrations

### Layer 4: Grace Period & Alerts

**Early warning system:**
- 30 days remaining: Yellow alert (routine extension)
- 15 days remaining: Orange alert (keeper should act)
- 7 days remaining: Red alert (critical, manual intervention)
- 3 days remaining: Emergency alert (all hands on deck)

**Alert channels:**
- Discord webhook for keeper bot status
- PagerDuty for critical entries < 7 days
- On-chain events for transparency

---

## TTL Extension Costs

### Cost Model

Based on Stellar's storage rent economics:

```
Cost (stroops) = BaseFee + (EntrySize × Ledgers × RentRate)

Where:
- BaseFee ≈ 100 stroops (transaction fee)
- EntrySize = bytes stored
- Ledgers = number of ledgers to extend
- RentRate ≈ 0.0001 stroops per byte per ledger
```

### Example Costs

| Entry Type | Size (bytes) | Extension (days) | Cost (XLM) |
|------------|--------------|------------------|------------|
| Pool | 2,000 | 60 | ~0.002 |
| Position | 500 | 60 | ~0.0005 |
| Invoice | 1,500 | 60 | ~0.0015 |
| SME Profile | 1,000 | 30 | ~0.0005 |

**Annual cost for 1,000 active pools:**
- Cost per pool per year: ~0.012 XLM
- Total for 1,000 pools: ~12 XLM/year
- At $0.10/XLM: ~$1.20/year

**Conclusion:** TTL extension is very inexpensive; no excuse for neglect.

---

## Contract-Specific Implementation

### Financing Pool

**Critical entries:**
- `Pool(u64)` — contains funded_amount, investor count, state
- `Positions(u64)` — map of investor positions

**Implementation:**

```rust
// In contracts/financing_pool/src/lib.rs

use kora_shared::ttl_protection::{extend_critical_entry, CriticalEntryType};

impl FinancingPoolContract {
    pub fn get_pool(env: Env, pool_id: u64) -> Result<Pool, Error> {
        let key = DataKey::Pool(pool_id);
        extend_critical_entry(&env, &key, CriticalEntryType::Pool);
        
        env.storage()
            .persistent()
            .get(&key)
            .ok_or(Error::PoolNotFound)
    }
    
    pub fn get_position(
        env: Env,
        pool_id: u64,
        investor: Address,
    ) -> Result<Position, Error> {
        let pool_key = DataKey::Pool(pool_id);
        let positions_key = DataKey::Positions(pool_id);
        
        // Extend both pool and positions entries
        extend_critical_entry(&env, &pool_key, CriticalEntryType::Pool);
        extend_critical_entry(&env, &positions_key, CriticalEntryType::Position);
        
        let positions: Map<Address, Position> = env.storage()
            .persistent()
            .get(&positions_key)
            .ok_or(Error::PoolNotFound)?;
        
        positions.get(&investor).ok_or(Error::PositionNotFound)
    }
    
    /// Permissionless TTL bump for pool
    pub fn bump_pool_ttl(env: Env, pool_id: u64) {
        extend_critical_entry(
            &env,
            &DataKey::Pool(pool_id),
            CriticalEntryType::Pool,
        );
        extend_critical_entry(
            &env,
            &DataKey::Positions(pool_id),
            CriticalEntryType::Position,
        );
    }
    
    /// Batch bump for efficiency
    pub fn bump_pools_batch(env: Env, pool_ids: Vec<u64>) -> u32 {
        let mut count = 0u32;
        for pool_id in pool_ids.iter() {
            Self::bump_pool_ttl(env.clone(), pool_id);
            count += 1;
        }
        count
    }
}
```

### Marketplace

**Critical entries:**
- `Listing(u64)` — active funding opportunities
- `Contribution(u64, Address)` — per-investor contributions for refunds

**Implementation:**

```rust
impl MarketplaceContract {
    pub fn get_listing(env: Env, id: u64) -> Result<Listing, Error> {
        let key = DataKey::Listing(id);
        extend_critical_entry(&env, &key, CriticalEntryType::Listing);
        
        env.storage()
            .persistent()
            .get(&key)
            .ok_or(Error::ListingNotFound)
    }
    
    pub fn bump_listing_ttl(env: Env, listing_id: u64) {
        extend_critical_entry(
            &env,
            &DataKey::Listing(listing_id),
            CriticalEntryType::Listing,
        );
    }
}
```

### Invoice NFT

**Critical entries:**
- `Invoice(u64)` — canonical invoice state machine

**Implementation:**

```rust
impl InvoiceNftContract {
    pub fn get_invoice(env: Env, id: u64) -> Result<Invoice, Error> {
        let key = DataKey::Invoice(id);
        extend_critical_entry(&env, &key, CriticalEntryType::Invoice);
        
        env.storage()
            .persistent()
            .get(&key)
            .ok_or(Error::InvoiceNotFound)
    }
    
    pub fn bump_invoice_ttl(env: Env, invoice_id: u64) {
        extend_critical_entry(
            &env,
            &DataKey::Invoice(invoice_id),
            CriticalEntryType::Invoice,
        );
    }
}
```

### Risk Registry

**Critical entries:**
- `SmeProfile(Address)` — credit history
- `Verifier(Address)` — verifier registration
- `VerifierStake(Address)` — staked bonds

**Implementation:**

```rust
impl RiskRegistryContract {
    pub fn get_sme_profile(env: Env, sme: Address) -> Result<SmeProfile, Error> {
        let key = DataKey::SmeProfile(sme.clone());
        extend_critical_entry(&env, &key, CriticalEntryType::SmeProfile);
        
        env.storage()
            .persistent()
            .get(&key)
            .ok_or(Error::SMENotRegistered)
    }
    
    pub fn bump_sme_profile_ttl(env: Env, sme: Address) {
        extend_critical_entry(
            &env,
            &DataKey::SmeProfile(sme),
            CriticalEntryType::SmeProfile,
        );
    }
    
    pub fn bump_verifier_ttl(env: Env, verifier: Address) {
        extend_critical_entry(
            &env,
            &DataKey::Verifier(verifier.clone()),
            CriticalEntryType::Verifier,
        );
        extend_critical_entry(
            &env,
            &DataKey::VerifierStake(verifier),
            CriticalEntryType::Verifier,
        );
    }
}
```

---

## Keeper Bot Architecture

### Components

**1. Entry Enumerator**
- Scans events for newly created critical entries
- Maintains list of all active pools, positions, invoices
- Filters out closed/expired entries

**2. TTL Monitor**
- Queries RPC for remaining TTL of each entry
- Prioritizes by criticality and remaining time
- Tracks historical TTL to detect anomalies

**3. Extension Executor**
- Batches extensions for gas efficiency
- Signs and submits transactions
- Retries on failure with exponential backoff

**4. Alert System**
- Discord/Slack notifications for near-expiry
- PagerDuty for critical entries
- On-chain event emission for transparency

### Dry-run rehearsal (#<keeper-dry-run>)

Operators need a safe rehearsal before burning fees on a mass extend. The
keeper exposes a read-only dry run that classifies each watched key as
**instance** (contract instance entry) or **persistent** (per-listing entries)
and reports which need a TTL bump — **without enqueuing, dispatching, or
submitting any transaction**:

- One-shot CLI: `KEEPER_DRY_RUN=1 npm start` prints the report and exits.
- HTTP: `GET /dry-run` on the running keeper.

The report flags keys below `KEEPER_TTL_THRESHOLD_LEDGERS` as `needsBump`,
surfacing archived/expired entries as `needsBump + missing` (never silently
skipped) so they can be restored. Empty/missing config fails closed with a
typed `DryRunConfigError`. Re-running the dry run is idempotent and produces an
identical report. See `docs/storage-rent-cost-model.md § Dry-run rehearsal`.

### Deployment

```bash
# 1. Build keeper bot
cd services/ttl-keeper
npm install
npm run build

# 2. Configure
cp .env.example .env
# Edit .env with RPC URL, contract IDs, signing key

# 3. Run as service
pm2 start ttl-keeper.js --name "kora-ttl-keeper"
pm2 save
pm2 startup

# 4. Monitor
pm2 logs kora-ttl-keeper
```

### Cost Management

**Fee estimation:**
```typescript
const estimateBatchCost = (entryCount: number): number => {
  const baseFee = 100; // stroops
  const perEntryFee = 50; // stroops
  return baseFee + (perEntryFee * entryCount);
};

// Batch up to 100 entries per transaction
const MAX_BATCH_SIZE = 100;
```

**Budget tracking:**
```typescript
interface Budget {
  dailyLimit: number; // stroops
  spent: number;
  lastReset: number;
}

// Reset daily
if (Date.now() - budget.lastReset > 86400000) {
  budget.spent = 0;
  budget.lastReset = Date.now();
}

// Check before extending
if (budget.spent + estimatedCost > budget.dailyLimit) {
  alert("Keeper budget exceeded");
  return;
}
```

---

## Testing TTL Protection

### Test Scenarios

**1. Normal operation:**
```rust
#[test]
fn test_auto_extension_on_access() {
    let env = Env::default();
    let contract = deploy(&env);
    
    // Create pool
    let pool_id = contract.create_pool(...);
    
    // Access multiple times
    for _ in 0..10 {
        contract.get_pool(pool_id);
    }
    
    // TTL should be extended (verify via RPC query off-chain)
}
```

**2. Near-expiry recovery:**
```rust
#[test]
fn test_permissionless_bump() {
    let env = Env::default();
    let contract = deploy(&env);
    
    let pool_id = create_dormant_pool(&env);
    
    // Simulate near-expiry (can't set TTL directly, but test the function)
    let keeper = Address::generate(&env);
    contract.bump_pool_ttl(pool_id);
    
    // Verify still accessible
    let pool = contract.get_pool(pool_id);
    assert!(pool.is_ok());
}
```

**3. Batch efficiency:**
```rust
#[test]
fn test_batch_bump_efficiency() {
    let env = Env::default();
    let contract = deploy(&env);
    
    let pool_ids: Vec<u64> = (1..=50).collect();
    
    // Measure resource usage
    let start = env.budget();
    contract.bump_pools_batch(pool_ids.clone());
    let used = env.budget() - start;
    
    // Should be more efficient than individual calls
    assert!(used < individual_bump_cost * 50);
}
```

---

## Incident Response

### TTL Expiry Emergency

If a critical entry expires despite protections:

**1. Immediate assessment:**
```bash
# Check which entries expired
stellar rpc getLedgerEntries \
  --keys "Pool(123)" \
  --rpc-url $RPC

# If liveUntilLedgerSeq < currentLedger: EXPIRED
```

**2. Restore entry:**
```bash
# Submit restoration transaction
stellar contract invoke \
  --id $CONTRACT_ID \
  --fn restore_pool \
  --arg 123 \
  --rpc-url $RPC \
  --source $ADMIN_KEY
```

**3. Post-incident:**
- Review keeper bot logs
- Identify root cause (bot down? gas spike? bug?)
- Implement fix
- Document in incident log
- Communicate to affected users

---

## Related Documentation

- `docs/storage-rent-cost-model.md` — TTL cost economics
- `scripts/ttl_keeper.sh` — Reference keeper implementation
- `contracts/shared/src/ttl_protection.rs` — Protection framework
- `docs/ARCHITECTURE.md § Storage Layout` — Per-contract storage keys

---

## Monitoring Checklist

- [ ] Keeper bot deployed and running
- [ ] Monitoring covers all critical entry types
- [ ] Alert thresholds configured (30d, 15d, 7d, 3d)
- [ ] Discord/PagerDuty webhooks active
- [ ] Daily budget limits set
- [ ] Backup keeper on standby
- [ ] Manual intervention runbook prepared
- [ ] Quarterly TTL audit scheduled
