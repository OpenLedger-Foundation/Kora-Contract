# Access Control Contract

The `access_control` contract provides protocol-wide authorization and emergency pause controls for the Kora Protocol. It is the central hub for role-based access management.

## Access Control Model

### Roles

Four roles exist in the system:

| Role | Capabilities | Assigned By | Can Be Transferred |
|------|-------------|-------------|-------------------|
| **Admin** | Full protocol control (pause, grant roles, transfer admin) | Initialization or `transfer_admin()` | Yes, via `transfer_admin()` |
| **Operator** | Reserved for future use (keeper bots, automation) | Admin via `grant_role()` | No, fixed role |
| **Verifier** | Register SMEs, assign risk scores (managed by `risk_registry`) | Admin via `grant_role()` | No, fixed role |
| **None** | No special privileges (default) | N/A | N/A |

### Role Grant and Revocation

**Granting a Role:**
- Only the Admin can grant roles (via `grant_role()`)
- Cannot grant `Role::Admin` — use `transfer_admin()` instead
- Cannot grant `Role::None` — use `revoke_role()` instead
- Cannot grant a role to the Admin's own address (prevents accidental self-overwrite)
- Role can be overridden: assigning a new role to an address with an existing role silently replaces it

**Revoking a Role:**
- Only the Admin can revoke roles (via `revoke_role()`)
- Cannot revoke the Admin's own role
- Fails if the target has no role assigned
- Removes the role entry from storage, reclaiming storage space

### Admin Transfer

The `transfer_admin()` function transfers admin privileges to a new address with strict safeguards:

- Only the current Admin can initiate transfer (via `require_auth()`)
- Cannot transfer to self (fails with `InvalidAddress`)
- Cannot transfer to an address that already holds `Operator` or `Verifier` role
  - This prevents silent role overwrites; the caller must `revoke_role()` first
- Old admin's role entry is removed (storage reclaimed)
- New admin is immediately given `Role::Admin`
- Old admin can no longer perform any privileged operations

## Public API Surface

### Initialization

```rust
pub fn initialize(env: Env, admin: Address) -> Result<(), KoraError>
```

**Purpose:** One-time initialization of the contract.

**Parameters:**
- `env` — Soroban environment
- `admin` — Address to designate as the initial admin

**Returns:** `Ok(())` on success, or:
- `KoraError::AlreadyInitialized` if already initialized

**Authorization:** None required (one-time setup).

**Security:** Can only be called once. Second calls fail regardless of caller.

---

### Pause / Unpause

```rust
pub fn pause(env: Env, admin: Address) -> Result<(), KoraError>
```

**Purpose:** Pause the entire protocol (emergency stop).

**Parameters:**
- `env` — Soroban environment
- `admin` — Caller's address (must be the current admin)

**Returns:** `Ok(())` on success, or:
- `KoraError::NotAdmin` if caller is not the admin
- `KoraError::AlreadyPaused` if protocol is already paused

**Authorization:** Requires `admin.require_auth()`.

**Security:** When paused:
- All state-mutating operations in `invoice_nft` and `marketplace` revert
- Repayments are still allowed (SMEs can always pay back)
- The pause flag is checked at each entry point

---

```rust
pub fn unpause(env: Env, admin: Address) -> Result<(), KoraError>
```

**Purpose:** Unpause the protocol and resume normal operations.

**Parameters:**
- `env` — Soroban environment
- `admin` — Caller's address (must be the current admin)

**Returns:** `Ok(())` on success, or:
- `KoraError::NotAdmin` if caller is not the admin
- `KoraError::NotPaused` if protocol is not currently paused

**Authorization:** Requires `admin.require_auth()`.

---

### Role Management

```rust
pub fn grant_role(
    env: Env,
    admin: Address,
    target: Address,
    role: Role,
) -> Result<(), KoraError>
```

**Purpose:** Assign a role to an address.

**Parameters:**
- `env` — Soroban environment
- `admin` — Caller's address (must be the current admin)
- `target` — Address to grant the role to
- `role` — The role to assign (`Operator` or `Verifier`)

**Returns:** `Ok(())` on success, or:
- `KoraError::NotAdmin` if caller is not the admin
- `KoraError::Unauthorized` if role is `Admin` or `None`, or if target is the admin

**Authorization:** Requires `admin.require_auth()`.

**Security:**
- Role is stored in persistent storage and TTL-bumped for ~30 days
- Granting a new role to an address overwrites any previous role

---

```rust
pub fn revoke_role(env: Env, admin: Address, target: Address) -> Result<(), KoraError>
```

**Purpose:** Remove a role from an address.

**Parameters:**
- `env` — Soroban environment
- `admin` — Caller's address (must be the current admin)
- `target` — Address to revoke the role from

**Returns:** `Ok(())` on success, or:
- `KoraError::NotAdmin` if caller is not the admin
- `KoraError::Unauthorized` if target is the admin
- `KoraError::RoleNotAssigned` if target has no role

**Authorization:** Requires `admin.require_auth()`.

**Security:** The role entry is removed from storage (not set to `Role::None`), reclaiming space.

---

```rust
pub fn transfer_admin(
    env: Env,
    current_admin: Address,
    new_admin: Address,
) -> Result<(), KoraError>
```

**Purpose:** Transfer admin privileges to a new address.

**Parameters:**
- `env` — Soroban environment
- `current_admin` — Caller's address (must be the current admin)
- `new_admin` — Address to receive admin privileges

**Returns:** `Ok(())` on success, or:
- `KoraError::NotAdmin` if caller is not the admin
- `KoraError::InvalidAddress` if `new_admin == current_admin`
- `KoraError::Unauthorized` if `new_admin` already holds a non-Admin role

**Authorization:** Requires `current_admin.require_auth()`.

**Security:**
- Old admin's role entry is removed (role revoked)
- New admin is immediately granted `Role::Admin`
- Old admin has no remaining privileges

---

### Views (Read-Only)

```rust
pub fn is_paused(env: Env) -> bool
```

**Purpose:** Check if the protocol is currently paused.

**Returns:** `true` if paused, `false` otherwise.

**Security:** No authorization check (public view).

---

```rust
pub fn get_role(env: Env, address: Address) -> Role
```

**Purpose:** Query the role assigned to an address.

**Parameters:**
- `env` — Soroban environment
- `address` — Address to query

**Returns:** The assigned `Role`, or `Role::None` if the address has no role.

**Security:** No authorization check (public view).

---

```rust
pub fn get_admin(env: Env) -> Result<Address, KoraError>
```

**Purpose:** Get the current admin address.

**Returns:** The admin's address, or `KoraError::NotInitialized` if the contract has not been initialized.

**Security:** No authorization check (public view).

---

### Paginated Role Enumeration

```rust
pub fn get_role_members(
    env: Env,
    role: Role,
    start: u32,
    limit: u32,
) -> Result<RoleMembersPage, KoraError>
```

**Purpose:** Enumerate the addresses holding a given role in bounded pages, so admin UIs and off-chain tooling never perform an unbounded role walk.

**Parameters:**
- `env` — Soroban environment
- `role` — The role to enumerate (`Operator` or `Verifier`)
- `start` — Zero-based index of the first member to return (the cursor)
- `limit` — Maximum number of members to return in this page

**Returns:** A `RoleMembersPage` containing:
- `members` — up to `limit` addresses starting at `start`
- `next` — `Some(next_start)` when more members remain, or `None` on the final page

or a typed error:
- `KoraError::InvalidPageLimit` if `limit` is `0` or exceeds the maximum page size
- `KoraError::Unauthorized` if `role` is `Admin` or `None` (only enumerable roles are exposed)

**Authorization:** No authorization check (public view).

**Security:**
- Page size is capped at `MAX_ROLE_PAGE_SIZE`; oversized requests fail closed with `InvalidPageLimit` rather than silently truncating
- Enumeration is read-only and does not mutate role storage or TTLs
- An empty role (or a `start` at/after the end) returns an empty `members` list with `next == None`
- Re-issuing the same `(role, start, limit)` query is idempotent and returns identical results

**Pagination pattern:**

```rust
let mut cursor = 0u32;
loop {
    let page = contract.get_role_members(env.clone(), Role::Verifier, cursor, 50)?;
    for member in page.members.iter() {
        // process member
    }
    match page.next {
        Some(next) => cursor = next,
        None => break,
    }
}
```

---

## Security Invariants

### 1. Admin Uniqueness
- Exactly one admin exists at any time
- The admin is set during `initialize()` and can only be changed via `transfer_admin()`
- A failed `transfer_admin()` leaves the original admin in place

### 2. Role Integrity
- Only the admin can grant or revoke roles (via `require_auth()`)
- Role storage uses persistent keys to prevent collisions
- Revoking a role removes it from storage (not just set to `Role::None`)

### 3. Pause Enforcement
- The pause flag is stored in instance storage (tied to the contract instance)
- When paused, all state-mutating operations in dependent contracts revert
- Repayments bypass the pause (SMEs can always repay)

### 4. Authorization Ordering
- All mutating functions check authorization via `require_auth()` **before** modifying state
- If auth fails, the entire transaction is reverted (no partial state changes)

### 5. Storage TTL
- Roles are stored in persistent storage and TTL-bumped to ~30 days on write
- The protocol operator or a keeper bot must periodically extend TTL to prevent data loss
- Instance storage (admin, paused flag) is tied to the contract instance and does not expire

### 6. Cross-Contract Safety
- The `pause` flag is read by `invoice_nft` and `marketplace` before any state-mutating operation
- Dependent contracts treat a paused protocol as a hard stop (fail closed)
- Role enumeration is read-only and never bypasses pause or auth checks on mutating paths
