# Kora Protocol — Threat Model

This document outlines the key threat actors, their capabilities, and the protocol's defenses against each. It covers all seven contracts: `invoice_nft`, `marketplace`, `financing_pool`, `treasury`, `risk_registry`, `access_control`, and the `shared` library.

---

## Threat Actors

### 1. Malicious Investor

**Capabilities:**
- Controls a funded account on the Stellar network
- Can call any public contract function they have authorization to call
- Can attempt to fund invoices, manipulate positions, or extract excess funds

**What They Can Do:**

| Contract | Action | Mitigation |
|----------|--------|-----------|
| `marketplace` | Fund an invoice (repeatedly or partially) | `require_auth()` on investor address; position tracking prevents double-counting |
| `marketplace` | Exceed funding target | Check: `funded_amount + contribution <= asking_price`; revert if exceeded |
| `financing_pool` | Manipulate position records | Positions are keyed by investor address and marked immutable; only the pool contract can write them |
| `treasury` | Attempt to withdraw fees | Only admin can call `withdraw()` and `emergency_drain()` |

**What They Cannot Do:**

- ❌ Steal funds from the pool (funds stored in pool contract, not investor-accessible)
- ❌ Alter invoice status or metadata (only invoice_nft contract can transition state)
- ❌ Change protocol fees or parameters (only admin can call `update_fee_bps()`)
- ❌ Mint invoices or change debtor information
- ❌ Transfer another investor's position (positions are immutable once created)

**Key Defenses:**
1. **Explicit auth requirement** — `require_auth()` ensures investor must sign their own funding call
2. **Immutable position records** — investor positions cannot be changed after creation
3. **Clear state transitions** — invoice status machine prevents unauthorized state changes
4. **Separate custody** — investor funds held in dedicated pool contract, not in marketplace or treasury

---

### 2. Malicious SME (Invoice Seller)

**Capabilities:**
- Controls an SME wallet that can mint invoices
- Can list invoices, request cancellation, and attempt repayment
- Can provide false or fraudulent invoice metadata (IPFS CID)

**What They Can Do:**

| Contract | Action | Mitigation |
|----------|--------|-----------|
| `invoice_nft` | Mint invoices with false metadata | Metadata lives off-chain (IPFS); off-chain verification by verifiers via risk scoring |
| `invoice_nft` | Mint invoices with false due date | Timestamp validation: `due_date` must be in the future; verified at mint time |
| `invoice_nft` | Mint invoices with zero/negative amount | Check: `amount > 0`; revert if invalid |
| `marketplace` | List the same invoice multiple times | Invariant: one listing per invoice ID; second list attempt fails with `InvoiceAlreadyExists` |
| `marketplace` | Cancel a listing after partial funding | `cancel_listing()` allowed but marketplace tracks state; investors can still claim repayment if repaid |
| `financing_pool` | Repay with insufficient funds | Explicit transfer requirement; if sender lacks balance, token transfer fails |
| `financing_pool` | Claim yield they didn't fund | Yield is distributed per position; investor receives only their proportional share |

**What They Cannot Do:**

- ❌ Steal investor funds before full repayment (funds locked in pool until invoice is repaid or defaulted)
- ❌ Change the asking price after listing (listings are immutable post-creation)
- ❌ Claim repayment credit for funds not sent (token transfer is cryptographically verified)
- ❌ Mint an invoice in another SME's name (`require_auth()` enforces SME signature)
- ❌ Mark their own invoice as defaulted (only admin can call `mark_default()` after due date)

**Key Defenses:**
1. **Off-chain metadata** — debtor PII and invoice details verified by external verifiers, not trusted on-chain
2. **Risk scoring** — verifier-assigned risk scores guide investor decisions
3. **Explicit amount enforcement** — all financial amounts validated before state changes
4. **Immutable listings** — once listed, key parameters (asking price, funding deadline) cannot be changed
5. **Investor claim on repayment** — repayment is tracked and yield distributed proportionally

---

### 3. Compromised Admin Key

**Capabilities:**
- Can execute any admin-only function (pause, fee changes, token whitelist, default marking, admin transfer)
- Can transfer admin privilege to another address
- Can pause the entire protocol

**What They Can Do:**

| Contract | Action | Impact |
|----------|--------|--------|
| `access_control` | Pause the protocol | Blocks new activity: minting, listing, funding, recording positions; does NOT block repayment or cancellation |
| `access_control` | Unpause the protocol | Restores normal operation |
| `access_control` | Transfer admin to attacker address | Attacker gains all admin privileges indefinitely |
| `marketplace` | Change protocol fee (0–100%) | Can extract 100% of investor contributions as fees (catastrophic) |
| `marketplace` | Whitelist/revoke tokens | Can block funding in legitimate tokens or allow malicious tokens |
| `treasury` | Withdraw accumulated fees | Extracts protocol revenue (acceptable use) |
| `financing_pool` | Mark invoice as defaulted | Can falsely declare repayment default and distribute partial recovery |
| `invoice_nft` | (None directly) | Admin cannot directly modify invoice state or mint invoices |

**What They Cannot Do:**

- ❌ Change historic invoice status (all state transitions emit immutable events; historical records are on-chain and permanent)
- ❌ Retroactively alter repayments or yields (all transfers are cryptographically final once confirmed)
- ❌ Access investor funds directly (only via `withdraw()` and `emergency_drain()` on treasury, which holds only fees)
- ❌ Modify the source code (off-chain; would require redeployment to a new contract address)

**Mitigations in Place (v1):**

1. **Pause mechanism limits damage** — pausing stops new activity but allows repayments to continue
2. **Fee bounds** — max fee is 10,000 bps (100%), enforced in `update_fee_bps()` validation
3. **Admin transfer** — single admin key is a known operational risk; all actions are auditable via event logs
4. **Event transparency** — every privileged action emits an event; off-chain monitoring can detect suspicious activity

**Planned Mitigations (v2):**

- **Multisig requirement (B2):** Replace single admin with M-of-N threshold signature scheme
- **Timelock on admin actions:** Delay sensitive operations (fee changes, admin transfer, pause) by 48 hours to allow stakeholder objection
- **Emergency freeze:** Different key can call `emergency_pause()` without the ability to unpause

---

### 4. Malicious Third-Party Contract

**Capabilities:**
- Deployed by attacker to any address on Stellar
- Can be whitelisted as a token (if admin is tricked or compromised)
- Can call public functions of the Kora contracts

**What They Can Do:**

| Scenario | Threat | Mitigation |
|----------|--------|-----------|
| Whitelisted as a funding token | Mint unlimited tokens to self; drain pool funding | Token whitelist requires admin approval; admin must verify contract code before whitelisting |
| Reentrancy attack | Call back into a Kora contract mid-transaction | Soroban's synchronous execution model prevents true reentrancy; state is atomic within a single contract |
| Callback via `require_auth()` | Bypass authorization checks | `require_auth(address)` validates the cryptographic signature; attacker cannot forge another user's signature |
| Token transfer failure | Cause protocol to fail mid-operation | All external token transfers use explicit error handling; failures revert the transaction |

**What They Cannot Do:**

- ❌ Steal funds by forging a signature (Stellar's ed25519 is cryptographically secure)
- ❌ Execute cross-contract calls without being explicitly invoked (Soroban contracts are passive; no unsolicited callbacks)
- ❌ Access storage of other contracts (each contract's storage is isolated and sandboxed by Soroban)
- ❌ Modify contract code post-deployment (Soroban is upgradeable, but only via explicit `contract.upgrade()` with admin key)

**Defenses (B16 Allowlist):**

The `marketplace` contract enforces a strict token allowlist:

```rust
pub fn whitelist_token(admin: Address, token: Address) -> Result<(), KoraError> {
    admin.require_auth();
    env.storage().persistent().set(&DataKey::WhitelistedToken(token), &true);
    Ok(())
}

pub fn fund_invoice(investor: Address, invoice_id: u64, amount: i128) -> Result<(), KoraError> {
    // ... validation ...
    if !env.storage().persistent().has(&DataKey::WhitelistedToken(token.clone())) {
        return Err(KoraError::TokenNotWhitelisted);
    }
    // ... proceed with funding ...
}
```

**Admin Responsibility:**
- Before whitelisting a token, verify:
  1. Token contract is audited and deployed by a known entity (e.g., Circle for USDC)
  2. Token contract source code is publicly available
  3. No recent suspicious activity on the token contract
- Maintain an up-to-date list of whitelisted tokens in deployment docs and off-chain systems

---

## Attack Vectors

### Vector 1: Flash Loan Manipulation

**Threat:** Attacker borrows a large amount of a token, funds invoices with it, and repays the loan in the same transaction, leaving no net liability.

**Impact:** Could artificially inflate pool sizes or game yield distribution if logic relied on cumulative state.

**Mitigation:**
- Positions are tracked at the time of funding; yield is calculated from the position snapshot, not cumulative balances
- Token balance checks are explicit (sender must actually transfer funds)
- Soroban's synchronous execution ensures all token transfers settle before the transaction completes

### Vector 2: Invoice Metadata Tampering

**Threat:** Attacker mints an invoice with a valid on-chain structure but fraudulent IPFS CID, pointing to false invoice details.

**Impact:** Investor funds an invoice believing it has one debtor/amount, but reality differs off-chain.

**Mitigation:**
- Investors must verify IPFS CID and metadata before funding
- Risk scoring by verifiers provides a secondary check; high-risk invoices earn lower yields, discouraging fraud
- Debtor information is hashed (SHA-256); verifier-provided risk score is tied to that hash, not to mutable metadata

### Vector 3: Fee Extraction

**Threat:** Compromised admin changes marketplace fee to 100%, extracting all investor contributions.

**Impact:** Investors receive net zero funding to the pool; SMEs receive zero liquidity.

**Mitigation:**
- Fee changes are infrequent and observable via events; monitoring bots can alert stakeholders
- Max fee is bounded at 10,000 bps (100%), enforced in code
- Planned mitigation (v2): timelock + multisig approval on fee changes (B2)

### Vector 4: Default Mark Manipulation

**Threat:** Admin marks invoices as defaulted falsely, distributing partial recovery and blocking legitimate repayment.

**Impact:** Investors lose funds; SMEs lose reputation; protocol loses legitimacy.

**Mitigation:**
- Default marking requires `ledger.timestamp > invoice.due_date`; cannot be done prematurely
- Once marked defaulted, invoice is immutable (state machine prevents re-transition)
- All defaults are logged via events; auditable by independent parties
- Planned mitigation (v2): governance vote or multisig approval on defaults above a certain amount (B2)

### Vector 5: Storage Exhaustion

**Threat:** Attacker mints millions of invoices to exhaust storage quotas and slow the protocol.

**Impact:** Legitimate invoices cannot be created; network becomes congested.

**Mitigation:**
- Each invoice mint requires the SME's signature and account funding; attacker must spend real capital
- Soroban storage is metered and paid for by the contract; costs scale with data volume
- No known upper limit in Soroban, but economic cost acts as a natural brake

---

## Assumptions

1. **Stellar Network Security:** We trust Stellar's validator consensus and ed25519 cryptography. A compromise of Stellar's core would compromise Kora.

2. **Soroban Runtime Security:** We trust Soroban's contract execution isolation, storage sandboxing, and event immutability. A runtime bug could bypass our defenses.

3. **Token Contract Safety:** Whitelisted tokens (e.g., Circle's USDC) are assumed to be safe and non-malicious. Careful admin review before whitelisting is required.

4. **IPFS Availability:** Invoice metadata is stored on IPFS. If IPFS content is unavailable, investors cannot verify invoice details; governance must decide how to handle disputes.

5. **Verifier Integrity:** Risk scores are assigned by trusted verifiers (e.g., credit bureaus). If a verifier is compromised, they could assign false scores; multi-verifier checks are recommended for high-value invoices.

6. **Off-Chain Verification:** The protocol relies on off-chain entities (verifiers, investors) to detect and report fraud. No fully on-chain fraud detection is in place.

---

## Security Improvements Planned

| Improvement | Issue | Target Release |
|-------------|-------|-----------------|
| Multisig admin (B2) | Single admin is a SPOF | v2.0.0 (Q3 2026) |
| Timelock on sensitive operations | Fast admin changes allow no objection period | v2.0.0 (Q3 2026) |
| Emergency pause key | Separate key for pause (without unpause power) | v2.0.0 (Q3 2026) |
| Governance vote on defaults | Admin default marking is centralized | v2.0.0 (Q3 2026) |
| Reputation NFTs | Track SME repayment history on-chain | v2.0.0 (Q3 2026) |
| Keeper network | Automated default detection and TTL management | v2.0.0 (Q3 2026) |

---

## Verification Checklist

Use this checklist before deploying to mainnet:

- [ ] All public contract functions have explicit `require_auth()` or equivalent access control
- [ ] All financial calculations use `checked_*` arithmetic (no silent overflows)
- [ ] All external calls (token transfers, cross-contract invokes) have error handling
- [ ] Storage keys are immutable and versioned for future upgrades
- [ ] Events are emitted for all state transitions and privileged actions
- [ ] Rate-limiting or spam prevention is in place for frequently-called functions
- [ ] Admin functions are tested with both authorized and unauthorized callers
- [ ] Fee bounds are enforced in code, not just documentation
- [ ] Token whitelist is maintained in code and off-chain deployment documentation
- [ ] Pause mechanism is tested to confirm it blocks only state-mutating operations, not repayments

---

## Contact & Reporting

**Security vulnerabilities:** Report privately to **security@kora.finance** (see [SECURITY.md](docs/SECURITY.md))

**Threat model feedback:** Open a GitHub discussion or pull request to this file.

**Audit findings:** Log in [AUDIT_LOG.md](AUDIT_LOG.md) with status, mitigation, and verification steps.

---

*Last updated: 2026-06-27*
