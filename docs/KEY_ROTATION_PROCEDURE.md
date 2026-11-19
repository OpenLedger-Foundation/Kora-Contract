# Key Rotation Procedure

Secure, documented key-rotation procedure for admin multi-sig co-signers and risk-registry verifiers.

---

## Overview

This document describes how to safely rotate compromised or lost keys without protocol downtime or loss of funds/authority.

**Key principle:** Rotation is just another quorum-gated, timelocked action reusing existing multi-sig/timelock infrastructure—not a separate bespoke mechanism.

---

## Rotation Types

### 1. Multi-Sig Co-Signer Rotation

**When to use:**
- Co-signer key compromised or suspected compromise
- Co-signer lost access to private key
- Planned security rotation (e.g., quarterly key refresh)
- Organization member departure

**Timelock:** 7 days (extended timelock for meta-governance)

**Process:**

#### Step 1: Propose Rotation (On-Chain)

Any existing co-signer with quorum can propose adding/removing signers:

```rust
// Add new signer
access_control.propose_add_signer(
    &proposer,
    &new_signer_address,
);

// Remove compromised signer (separate proposal or combined)
access_control.propose_remove_signer(
    &proposer,
    &old_signer_address,
);
```

**Validation:**
- Must maintain minimum 2 signers after removal
- Proposer must be current co-signer
- New signer cannot already exist in set

#### Step 2: Quorum Approval

Other co-signers approve the proposal:

```rust
access_control.approve_proposal(
    &approver,
    &proposal_id,
);
```

**Requirement:** Must meet current threshold (e.g., 3 of 5 signers)

#### Step 3: Wait for Timelock (7 Days)

Extended cooling-off period for signer set changes.

**During this period:**
- Community can review and raise concerns
- Other signers can object
- Guardian can pause if suspicious activity detected

**Monitoring:**
- Check proposal status: `access_control.get_proposal(proposal_id)`
- Verify timelock remaining: `proposed_at + 604_800 - current_timestamp`

#### Step 4: Execute Rotation

After timelock expires, any signer executes:

```rust
access_control.execute_proposal(
    &executor,
    &proposal_id,
);
```

**Effect:**
- Old signer immediately revoked (can no longer sign)
- New signer can immediately participate in new proposals
- Threshold recalculated to maintain same percentage
- All audit logs updated with rotation details

#### Step 5: Verify & Communicate

```rust
// Verify new signer set
let config = access_control.get_multisig_config();
assert!(config.signers.contains(&new_signer));
assert!(!config.signers.contains(&old_signer));

// Check audit log
let audit_entry = access_control.get_latest_audit_entry();
```

**External communication:**
- Announce rotation in community channels
- Update documented signer list
- Revoke old key from all systems

---

### 2. Verifier Key Rotation

**When to use:**
- Verifier key compromised or suspected compromise
- Verifier lost access to private key
- Planned security rotation
- Organization key management policy change

**Timelock:** 24 hours (standard governance timelock)

**Process:**

#### Step 1: Propose Rotation (On-Chain)

Verifier or admin proposes rotation:

```rust
risk_registry.propose_verifier_key_rotation(
    &proposer, // verifier or admin
    &old_verifier_address,
    &new_verifier_address,
);
```

**Validation:**
- Proposer must be the verifier or admin
- Old key must be active verifier
- New key must not already be a verifier
- Verifier must not have pending disputes requiring old key

#### Step 2: Wait for Timelock (24 Hours)

Standard cooling-off period.

**During this period:**
- No new score submissions from old key
- Verifier status marked as "pending rotation"
- Stakes remain locked to old key

#### Step 3: Execute Rotation

After timelock, verifier or admin executes:

```rust
risk_registry.execute_verifier_key_rotation(
    &executor,
    &old_verifier_address,
);
```

**Atomic state transfer:**
1. All staked tokens transferred to new key
2. Reputation score migrated
3. Sub-account delegations transferred
4. Pending obligations transferred or resolved
5. Old key revoked

#### Step 4: Verify & Resume Operations

```rust
// Verify rotation complete
let verifier_status = risk_registry.get_verifier_status(&new_verifier_address);
assert_eq!(verifier_status, VerifierStatus::Active);

// Old key should be revoked
let old_status = risk_registry.get_verifier_status(&old_verifier_address);
assert_eq!(old_status, VerifierStatus::Removed);

// Verify stake transferred
let new_stake = risk_registry.get_verifier_stake(&new_verifier_address);
assert_eq!(new_stake, expected_stake);
```

**Resume operations:**
- Verifier can immediately submit scores with new key
- All existing attestations remain valid
- Reputation history preserved

---

### 3. Admin Key Rotation

**When to use:**
- Admin key compromised
- Planned rotation per security policy
- Organization admin change

**Timelock:** 48 hours (two-step transfer)

**Process:**

#### Step 1: Propose Transfer

Current admin proposes new admin:

```rust
contract.propose_admin_transfer(
    &current_admin,
    &new_admin_address,
);
```

#### Step 2: Wait & Review (48 Hours)

Guardian or multi-sig can object during this period if transfer is suspicious.

#### Step 3: New Admin Claims

New admin accepts transfer:

```rust
contract.claim_admin(&new_admin_address);
```

**Effect:**
- Old admin immediately loses all admin privileges
- New admin gains full admin authority
- All contracts updated atomically

---

## Emergency Rotation (Compromised Key)

If a key is actively compromised and being used maliciously:

### Immediate Actions

1. **Pause Protocol** (Guardian):
   ```rust
   access_control.emergency_pause(&guardian);
   ```

2. **Initiate Fast-Track Rotation:**
   - Use remaining honest quorum to propose rotation
   - Reduce timelock to minimum safe period (12 hours) via guardian override
   - Execute as soon as timelock expires

3. **Monitor Malicious Activity:**
   - Track all transactions from compromised key
   - Document for incident response
   - Prepare for potential rollback if necessary

### Post-Emergency Cleanup

1. Execute full key rotation as documented above
2. Review all actions taken by compromised key
3. Revert any malicious parameter changes
4. Restore normal timelock periods
5. Conduct security audit
6. Update incident response documentation

---

## Safe Minimum Rules

### Multi-Sig Rotation

**Minimum signer count:** 2

**Validation during rotation:**
```rust
// Before executing removal
assert!(current_signer_count - removal_count >= 2);

// Before executing combined add/remove
assert!(current_signer_count - removal_count + addition_count >= 2);
```

**Threshold adjustment:**
- Maintains same approval percentage
- Minimum 2 approvals required
- Maximum = new signer count

Example:
- 3 of 5 signers (60%) → rotate to 4 signers → new threshold = 3 (75%, but maintains security)
- 2 of 3 signers (66%) → rotate to 5 signers → new threshold = 3 (60%)

### Verifier Rotation

**Pending obligations check:**
- No open disputes where verifier is challenger/defendant
- No pending stake slashing
- No active score update cooldowns

**Bond transfer:**
- Full stake amount must transfer atomically
- Cannot rotate while slash is in progress
- Cannot rotate while removal is pending

---

## Testing Rotation

### Pre-Production Testing

Always test rotation on testnet before mainnet:

```bash
# 1. Deploy test environment
make deploy-testnet

# 2. Set up test signers
./scripts/setup-test-multisig.sh

# 3. Execute rotation
./scripts/test-key-rotation.sh

# 4. Verify all state transferred
./scripts/verify-rotation.sh
```

### Test Scenarios

1. **Normal rotation** (no pressure)
2. **Rotation under load** (active proposals pending)
3. **Emergency rotation** (compromised key)
4. **Multiple simultaneous rotations** (multiple signers)
5. **Failed rotation** (insufficient quorum)
6. **Rotation with pending obligations** (should fail)

---

## Off-Chain Coordination

Key rotation requires coordination beyond on-chain mechanics:

### Before Rotation

1. **Generate new key securely:**
   - Use hardware wallet or HSM
   - Verify key generation entropy
   - Store backup in secure location
   - Test key signing capability

2. **Communicate with team:**
   - Notify all other signers/verifiers
   - Schedule rotation window
   - Prepare rollback plan
   - Document reason for rotation

3. **Prepare infrastructure:**
   - Update key in off-chain systems
   - Test new key with dummy transactions
   - Verify key has appropriate permissions
   - Update monitoring/alerting

### During Rotation

1. **Monitor proposal:**
   - Track approval progress
   - Watch for objections
   - Monitor timelock countdown
   - Stay available for emergency response

2. **Coordinate execution:**
   - Designate executor
   - Prepare execution transaction
   - Verify gas/fee availability
   - Plan for retry if first attempt fails

### After Rotation

1. **Verify on-chain:**
   - Check new key in signer/verifier set
   - Verify old key revoked
   - Confirm state transferred correctly
   - Review audit logs

2. **Update off-chain systems:**
   - Revoke old key from all systems
   - Activate new key in monitoring
   - Update documentation
   - Test new key functionality

3. **Communicate:**
   - Announce completion
   - Update public signer list
   - File security report if compromise
   - Schedule retrospective

---

## Troubleshooting

### Rotation Proposal Fails

**Error: Timelock not elapsed**
```
Solution: Wait for full timelock period. Check current time and proposed_at timestamp.
```

**Error: Minimum quorum violation**
```
Solution: Add new signers before removing old ones, or increase new signer count.
```

**Error: Pending obligations**
```
Solution: Resolve all pending disputes, wait for cooldowns to expire, then retry.
```

### Rotation Executed But State Inconsistent

**Symptom: Old key still appears in some queries**
```
Solution: Check cache invalidation. May need to wait for next ledger. Query persistent storage directly.
```

**Symptom: Stake not transferred**
```
Solution: Verify atomic transfer logic. Check both old and new key balances. Review transaction logs.
```

### Emergency: Need to Rotate Immediately

**Option 1: Guardian pause + fast-track**
```rust
access_control.emergency_pause(&guardian);
access_control.propose_rotation_with_reduced_timelock(&guardian, &new_key, 12_hours);
```

**Option 2: Recovery proposal (30 day timelock, high bar)**
```rust
access_control.propose_recovery(&recovery_address);
// Requires extraordinary justification and transparency
```

---

## Security Checklist

Before executing any rotation:

- [ ] New key generated securely (hardware wallet/HSM)
- [ ] New key tested on testnet
- [ ] All other signers/verifiers notified
- [ ] Reason for rotation documented
- [ ] Timelock period appropriate for risk level
- [ ] Minimum quorum maintained
- [ ] Pending obligations resolved
- [ ] Rollback plan prepared
- [ ] Monitoring configured for new key
- [ ] Post-rotation verification script ready

After executing rotation:

- [ ] New key verified in on-chain state
- [ ] Old key confirmed revoked
- [ ] State transfer complete (stakes, approvals, etc.)
- [ ] Audit log entry created
- [ ] Off-chain systems updated
- [ ] Community notification sent
- [ ] Old key destroyed/archived securely
- [ ] Incident report filed (if compromise)

---

## Related Documentation

- `contracts/shared/src/key_rotation.rs` — Rotation framework code
- `contracts/access_control/src/lib.rs` — Multi-sig rotation implementation
- `contracts/risk_registry/src/lib.rs` — Verifier rotation implementation
- `docs/INCIDENT_RESPONSE.md` — Emergency procedures
- `docs/incident-response-templates/README.md` — Internal and external-safe incident messages
- `docs/MULTISIG_OPERATIONS.md` — Day-to-day multi-sig operations

---

## Questions & Support

For rotation questions or assistance:
- **Emergency:** Contact guardian immediately
- **Planned rotation:** Open issue with `security` and `rotation` labels
- **Policy questions:** See `docs/GOVERNANCE.md`
