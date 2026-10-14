# Security Lint Violation Fixes - Implementation Guide

This document outlines the systematic approach to fixing all security lint violations across the codebase.

## Summary of Violations Found

Based on initial analysis, the main violation categories are:

1. **Test code using `.unwrap()`** (acceptable with `#[cfg(test)]` guard)
2. **Production code using `.unwrap()` and `.expect()`** (MUST FIX)
3. **Direct indexing/slicing** (MUST FIX)
4. **Unchecked arithmetic operations** (MUST FIX)
5. **Missing error propagation** (MUST FIX)

## Fix Priority

### P0 (Critical - Production Contract Code)
- contracts/marketplace/src/lib.rs
- contracts/financing_pool/src/lib.rs
- contracts/treasury/src/lib.rs
- contracts/access_control/src/lib.rs
- contracts/risk_registry/src/lib.rs
- contracts/invoice_nft/src/lib.rs

### P1 (Important - Shared Libraries)
- contracts/shared/src/validation.rs
- contracts/shared/src/types.rs

### P2 (Test Code - Add Suppressions)
- contracts/*/tests/*.rs
- contracts/tests/*.rs

### P3 (Tooling)
- contracts/xtask/src/main.rs
- services/indexer/src/lib.rs

## Systematic Fix Patterns

### Pattern 1: Test Code `.unwrap()` → Add Suppression

**Before:**
```rust
#[test]
fn test_something() {
    let env = Env::default();
    let contract_id = env.register_contract(None, Contract).unwrap();
}
```

**After:**
```rust
#[test]
#[allow(clippy::unwrap_used)] // Test setup code
fn test_something() {
    let env = Env::default();
    let contract_id = env.register_contract(None, Contract).unwrap();
}
```

### Pattern 2: Production `.unwrap()` → Proper Error Handling

**Before:**
```rust
pub fn get_listing(env: Env, id: u64) -> Listing {
    env.storage().persistent().get(&DataKey::Listing(id)).unwrap()
}
```

**After:**
```rust
pub fn get_listing(env: Env, id: u64) -> Result<Listing, KoraError> {
    env.storage()
        .persistent()
        .get(&DataKey::Listing(id))
        .ok_or(KoraError::ListingNotFound)
}
```

### Pattern 3: Test Assertions `.unwrap_err().unwrap()` → Keep with Suppression

**Before:**
```rust
#[test]
fn test_error() {
    let result = contract.try_invalid_operation();
    assert_eq!(result.unwrap_err().unwrap(), Error::InvalidAmount);
}
```

**After:**
```rust
#[test]
#[allow(clippy::unwrap_used)] // Test error assertions
fn test_error() {
    let result = contract.try_invalid_operation();
    assert_eq!(result.unwrap_err().unwrap(), Error::InvalidAmount);
}
```

### Pattern 4: Storage `.get().unwrap()` → `.ok_or()`

**Before:**
```rust
let config: Config = env.storage().instance().get(&DataKey::Config).unwrap();
```

**After:**
```rust
let config: Config = env.storage()
    .instance()
    .get(&DataKey::Config)
    .ok_or(KoraError::NotInitialized)?;
```

### Pattern 5: Map `.get().unwrap()` → `.ok_or()`

**Before:**
```rust
let position = positions.get(&investor).unwrap();
```

**After:**
```rust
let position = positions
    .get(&investor)
    .ok_or(KoraError::PositionNotFound)?;
```

### Pattern 6: Vec Indexing → `.get().ok_or()`

**Before:**
```rust
let first = vec[0];
let slice = &data[start..end];
```

**After:**
```rust
let first = vec.get(0).ok_or(Error::EmptyVec)?;
let slice = data.get(start..end).ok_or(Error::InvalidRange)?;
```

### Pattern 7: Arithmetic → Checked Operations

**Before:**
```rust
let total = amount1 + amount2;
let share = amount * bps / 10_000;
```

**After:**
```rust
let total = amount1.checked_add(amount2)
    .ok_or(Error::ArithmeticOverflow)?;
let share = bps_of(amount, bps)?; // Use safe helper
```

### Pattern 8: Division → Check for Zero

**Before:**
```rust
let average = total / count;
```

**After:**
```rust
if count == 0 {
    return Err(Error::DivisionByZero);
}
#[allow(clippy::integer_arithmetic)] // Guaranteed non-zero by check above
let average = total / count;
```

## Implementation Steps

### Step 1: Add Test Suppressions (Bulk)

Add to top of all test modules:
```rust
#![cfg(test)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::indexing_slicing)]
```

### Step 2: Fix Shared Library Code

Priority: `contracts/shared/src/validation.rs`

Ensure all helper functions use checked arithmetic:
```rust
pub fn safe_add(a: i128, b: i128) -> Result<i128, CommonError> {
    a.checked_add(b).ok_or(CommonError::ArithmeticOverflow)
}

pub fn bps_of(amount: i128, bps: u32) -> Result<i128, CommonError> {
    require_valid_bps_range(bps)?;
    let numerator = amount.checked_mul(bps as i128)
        .ok_or(CommonError::ArithmeticOverflow)?;
    numerator.checked_div(10_000)
        .ok_or(CommonError::ArithmeticOverflow)
}
```

### Step 3: Fix Each Contract Module

For each contract in P0 list:

1. Search for all `.unwrap()` calls
2. Determine if production or test code
3. Apply appropriate fix pattern
4. Update function signatures to return `Result<T, Error>`
5. Propagate errors with `?` operator
6. Update callers

### Step 4: Update Contract Interfaces

Many public contract functions currently return `T` and need to return `Result<T, Error>`:

**Before:**
```rust
pub fn get_admin(env: Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}
```

**After:**
```rust
pub fn get_admin(env: Env) -> Result<Address, KoraError> {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(KoraError::NotInitialized)
}
```

Note: This is a **breaking API change**. All callers must be updated.

### Step 5: Fix Cross-Contract Calls

When contract A calls contract B, handle new Result types:

**Before:**
```rust
let nft_client = InvoiceNftContractClient::new(&env, &nft_addr);
let invoice = nft_client.get_invoice(&id);
```

**After:**
```rust
let nft_client = InvoiceNftContractClient::new(&env, &nft_addr);
let invoice = nft_client.try_get_invoice(&id)?;
```

### Step 6: Fix Arithmetic Operations

Search for: `+`, `-`, `*`, `/`, `%` in non-test code

Replace with checked operations or use helpers from `kora_shared::validation`.

## Verification

After fixes, verify:

```bash
# 1. Lint passes
make lint

# 2. All tests still pass
cargo test --workspace

# 3. Contracts build
make build

# 4. Coverage maintained
make coverage
```

## Exceptions and Justified Suppressions

Some code may legitimately need suppressions. Document with inline comments:

```rust
// JUSTIFICATION: BPS values are validated to be <= 10_000 on input,
// making this division safe and the result guaranteed to fit in i128.
#[allow(clippy::integer_arithmetic)]
let amount = principal * bps / 10_000;
```

Acceptable justifications:
- Mathematical invariants guarantee safety
- Test/mock code only
- Soroban SDK API requires specific pattern
- External crate limitation

Unacceptable justifications:
- "Too much work to fix properly"
- "This probably won't overflow"
- "Works fine in practice"

## Rollout Plan

1. **Week 1:** Fix shared libraries + add test suppressions
2. **Week 2:** Fix 3 P0 contracts (marketplace, financing_pool, treasury)
3. **Week 3:** Fix remaining P0 contracts
4. **Week 4:** Fix P1 libraries, verify all tests pass
5. **Week 5:** Enable lints in CI, monitor for new violations

## Tracking

Create GitHub issues for each contract:
- [ ] #<NUM> Fix security lints: contracts/marketplace
- [ ] #<NUM> Fix security lints: contracts/financing_pool
- [ ] #<NUM> Fix security lints: contracts/treasury
- [ ] #<NUM> Fix security lints: contracts/access_control
- [ ] #<NUM> Fix security lints: contracts/risk_registry
- [ ] #<NUM> Fix security lints: contracts/invoice_nft
- [ ] #<NUM> Fix security lints: contracts/shared

## Automated Assistance

Consider using `cargo-fix` for some mechanical fixes:

```bash
# Fix some lints automatically (be careful, review changes)
cargo fix --allow-dirty --broken-code

# Or per-package
cargo fix -p kora-marketplace --allow-dirty
```

**Warning:** `cargo fix` can't handle all patterns. Manual review required.

## Post-Fix Monitoring

After fixes are complete:
1. CI enforces `make lint` on every PR
2. Any new `.unwrap()` in production code blocked by lint
3. Quarterly audit to verify no regressions
4. Security review before mainnet deployment

## Automated Panic Safety Audit (`panic-safety-audit`)

An AST-based static scanner is integrated into `contracts/xtask` to ensure no unapproved `.unwrap()`, `.expect()`, or `panic!` calls exist in production contract crates (`contracts/*/src/`):

```bash
# Run panic safety audit scanner locally
cargo run -p kora-xtask --bin panic-safety-audit

# Custom report outputs
cargo run -p kora-xtask --bin panic-safety-audit -- --report-path target/panic-safety-audit-report.json --markdown-path target/panic-safety-audit-report.md
```

### Validation Invariants:
1. **Zero Unapproved Panics**: Any `.unwrap()` or `.expect()` in non-test production code without an entry in `contracts/panic_safety_allowlist.json` will fail the audit and CI.
2. **Mandatory Justification**: Every allowlist entry must specify file, line, kind, and a clear security justification explaining why the operation is guaranteed safe.
3. **No Stale Exemptions**: If code refactoring removes an unwrap, the corresponding entry in `contracts/panic_safety_allowlist.json` must be removed.
4. **CI Enforcement**: The audit runs automatically in `.github/workflows/security.yml` under the `panic-safety-audit` job.

## Questions?

See `docs/SECURITY_LINTS.md` for detailed lint explanations.
Open issues with `security` label for questions or exceptions.

