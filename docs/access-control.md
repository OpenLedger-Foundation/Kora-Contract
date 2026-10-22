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
- The `pause` flag is read by `invoice_nft` and `marketplace` via cross-contract calls
- The calling contract's address is used as the authorized signer (implicit trust)
- No circular dependencies exist between contracts

### 7. Council Rotation & Term-Limit Enforcement (#845)
- **Defined Term Limits:** Council seats possess configurable term lengths (`configure_council_terms`) preventing indefinite tenure.
- **Rotation Elections:** Expiry triggers a structured election (`initiate_council_rotation`, `vote_council_rotation`, `conclude_council_rotation`).
- **Quorum Safety:** An expiring seat's authority only lapses once a replacement is confirmed; safe minimum quorum is preserved throughout transitions.
- **Failed-Election Fallback:** If an election fails to reach quorum or confirm a successor, an automatic term extension grace period activates, preventing any gap in protocol authority.
- **Key Rotation Integration:** Leverages the key-rotation procedures for secure credential handoff.

## Known Limitations (v1)

### Single Admin Key
- If the admin's private key is compromised, an attacker has full protocol control
- Mitigations planned for v2:
  - Multisig admin (threshold signature scheme)
  - Timelock on sensitive admin operations (48–72 hour delay)
  - Hardware security module for key storage

### No Role-Based View Permissions
- All view functions (`is_paused()`, `get_role()`, `get_admin()`) are public
- There is no way to restrict who can query this information
- This is acceptable because the pause flag and role assignments are not secret

### No Audit Trail
- No events are emitted on all role changes (this will be fixed in a future issue)
- An audit trail of admin actions would improve forensics and accountability

## Interaction with Other Contracts

### `invoice_nft`
- Calls `access_control.is_paused()` before minting and state transitions
- Reverts with `KoraError::ProtocolPaused` if paused

### `marketplace`
- Calls `access_control.is_paused()` before listing and funding
- Reverts with `KoraError::ProtocolPaused` if paused

### `risk_registry`
- Checks if caller is a verifier by calling `access_control.get_role()`
- Only verifiers can register SMEs and assign risk scores

### `financing_pool`
- Calls `access_control.is_paused()` before repayment and yield distribution
- Reverts with `KoraError::ProtocolPaused` if paused
