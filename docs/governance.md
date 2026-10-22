# Protocol-Parameter Governance

Protocol-wide tunable parameters are changed through a lightweight on-chain governance process
rather than by a unilateral single-admin call. The process reuses two existing primitives:

- **B2 multisig** — proposing, voting, and executing are all gated by the configured multisig
  signer set (`configure_multisig`).
- **B1 timelock** — an executed proposal must clear a cooling-off period
  (`GOVERNANCE_TIMELOCK_DELAY`, ~24h, mirroring the upgrade timelock) before it takes effect.

---

## Governed parameters

| `ParameterKey`   | Meaning                                  | Allowed range |
|------------------|------------------------------------------|---------------|
| `FeeBps`         | Protocol fee in basis points             | `0..=10_000`  |
| `LatePenaltyBps` | Late-repayment penalty in basis points   | `0..=10_000`  |
| `MaxRiskScore`   | Ceiling for accepted invoice risk scores | `0..=100`     |

---

## Standard Governance Flow

```
propose_parameter_change ──▶ vote_parameter_change ──▶ execute_parameter_change
   (signer; auto-votes)        (other signers)            (quorum + timelock)
```

1. **`propose_parameter_change(proposer, key, new_value)`** — a multisig signer proposes a new
   value. The value is range-checked and the proposer's vote is recorded automatically. Returns a
   `proposal_id`.
2. **`vote_parameter_change(signer, proposal_id)`** — other signers vote in favour. Each signer
   may vote once; already-executed proposals are rejected.
3. **`execute_parameter_change(caller, proposal_id)`** — once approvals reach the multisig
   `threshold` **and** `created_at + GOVERNANCE_TIMELOCK_DELAY` has elapsed, the new value is
   committed on-chain under `Parameter(key)`.

---

## Fast-Track Governance for Critical Security Fixes

A distinct fast-track path allows time-sensitive security fixes to reach on-chain execution
faster than the standard 24-hour timelock, while still requiring meaningful multi-party sign-off.

### Purpose

The standard governance process is deliberately slow by design, but becomes a liability when a
critical security vulnerability is discovered and the normal path would leave the protocol exposed.
The fast-track path resolves this tension.

### Eligibility

Fast-track proposals are **strictly scoped to confirmed critical security fixes** only.

**Requirements:**
- Must be linked to a confirmed **critical-severity bug bounty report**
- Proposal must be within size constraints (e.g., 1000 bytes max)
- Requires higher multi-sig threshold than routine actions (e.g., 75% vs 50%)
- Shorter but **non-zero** timelock (e.g., 2 hours vs 24 hours)

**Out of Scope:**
- Non-security parameter changes (even if urgent)
- Business-driven updates
- Feature additions

### Flow

```
create_fast_track_proposal ──▶ vote_fast_track ──▶ execute_fast_track_proposal
   (linked to bug report)       (higher threshold)    (shorter timelock)
                                                              │
                                                              ▼
                                                   publish_fast_track_disclosure
                                                   (mandatory post-hoc)
```

1. **`register_critical_bug_report(admin, report_id)`** — Admin registers a confirmed critical
   bug from the bug bounty triage process. This is the eligibility trigger.

2. **`create_fast_track_proposal(proposer, target, action_hash, bug_report_id, size_bytes)`** — 
   Create fast-track proposal. Validates:
   - Bug report ID is registered as critical
   - Proposal size ≤ max allowed (prevents bundling unrelated changes)
   - Proposer has sufficient stake

3. **`vote_fast_track(voter, proposal_id, support)`** — Signers vote. Same mechanism as standard,
   but higher threshold enforced at execution.

4. **`execute_fast_track_proposal(proposal_id)`** — Execute after:
   - Shorter timelock elapsed (e.g., 2 hours)
   - Higher threshold reached (e.g., 75% approval)
   - No target changes detected

5. **`publish_fast_track_disclosure(admin, proposal_id)`** — Mandatory post-hoc public disclosure
   and community review. Triggers event logging for transparency.

### Security Safeguards

| Safeguard | Purpose |
|-----------|---------|
| **Bug bounty link** | Prevents misuse for non-security changes |
| **Size constraint** | Prevents bundling unrelated changes with security fix |
| **Higher threshold** | Requires broader consensus than routine actions |
| **Non-zero timelock** | Minimum review period (e.g., 2h) before execution |
| **Mandatory disclosure** | Post-hoc transparency and community review |
| **Prominent logging** | All fast-track usage audited on-chain |

### Example

```rust
// 1. Bug bounty system confirms critical vulnerability
governance.register_critical_bug_report(admin, 1234);

// 2. Create fast-track proposal (2h timelock, not 24h)
let proposal_id = governance.create_fast_track_proposal(
    proposer,
    target_contract,
    fix_action_hash,
    1234,  // bug report ID
    500,   // proposal size in bytes
);

// 3. Multi-sig votes (75% threshold required, not 50%)
governance.vote_fast_track(signer1, proposal_id, true);
governance.vote_fast_track(signer2, proposal_id, true);
governance.vote_fast_track(signer3, proposal_id, true);

// 4. Execute after 2h timelock
governance.execute_fast_track_proposal(proposal_id);

// 5. Publish mandatory disclosure
governance.publish_fast_track_disclosure(admin, proposal_id);
```

---

## Reading values

- `get_parameter(key) -> Option<u32>` — the current governed value (or `None` if never set).
- `get_parameter_proposal(proposal_id) -> ParameterProposal` — inspect a proposal's votes/state.
- `get_fast_track_proposal(proposal_id) -> Option<FastTrackProposal>` — inspect fast-track proposal.

---

## Errors

| Error | Cause |
|-------|-------|
| `NotMultisigSigner` / `SignerNotFound` | caller is not in the signer set |
| `InvalidParameterValue` | proposed value is out of range |
| `ParameterProposalNotFound` | unknown proposal id |
| `ParameterProposalAlreadyExecuted` | proposal already executed |
| `AlreadyVoted` | signer already voted on this proposal |
| `GovernanceThresholdNotMet` | not enough approvals to execute |
| `GovernanceTimelockNotElapsed` | timelock has not yet elapsed |
| `NotEligibleForFastTrack` | fast-track not enabled or proposal ineligible |
| `NoBugBountyReportLinked` | bug report not registered as critical |
| `ProposalTooLarge` | proposal exceeds size limit |
| `FastTrackThresholdNotMet` | higher fast-track threshold not reached |
| `FastTrackTimelockNotElapsed` | fast-track timelock (2h) not yet elapsed |

---

## Configuration

Fast-track governance is configured via `configure_fast_track`:

```rust
governance.configure_fast_track(
    admin,
    true,   // enabled
    7500,   // 75% threshold (vs 50% standard)
    7200,   // 2h timelock (vs 24h standard)
    1000,   // max 1000 bytes proposal size
);
```

---

The signer set and threshold are intentionally reused from the B2 multisig so governance starts
gated by the same trusted set, leaving room to widen stakeholder participation later.

## Impact Simulation Service

`services/governance-sandbox` exposes `simulateImpact(change, snapshot)` for
proposal previews. Its concentration-cap plugin reports listings over the new
cap and investors within the configured proximity band. Its fee-tier plugin
reports affected listings and the estimated fee delta using funded amounts.
Unrecognized parameter types return a generic before/after diff. Every result
includes the source snapshot timestamp and is explicitly a current-state
estimate, not a guarantee of execution impact. The service receives snapshots
through an adapter so deployments can source indexed protocol state.

## Signature-Based Off-Chain Voting

`services/offchain-voting` provides Stellar Ed25519 vote verification, stake
weight lookup through the same governance source adapter used for on-chain
eligibility, proposal-scoped signatures, and deterministic SHA-256 tally
summaries for on-chain anchoring. Proposal IDs are included in the signed
message, so a signature cannot be replayed for another proposal. The adapter
must refuse off-chain voting whenever on-chain voting is open and must verify
the proposal is designated for the off-chain path. Its repository adapter must
provide atomic mode reservation and vote insertion in persistent storage; the
included in-memory store is for tests/development only. Result anchoring and
optional execution are idempotent on-chain adapter operations and must apply the
governance contract's quorum and threshold rules. The contract defaults each
proposal to on-chain voting; an authenticated admin may designate off-chain
voting before any on-chain votes are cast. On-chain voting is then rejected.
After the voting deadline, the admin-authenticated adapter may anchor one tally
hash and its `for`/`against` weights. Execution uses those anchored weights with
the same quorum and approval threshold as on-chain voting. The hash is publicly
readable through `get_offchain_result`; the authorized anchor adapter is
responsible for deriving it from the canonical tally.

## Multi-Sig Council Rotation & Term-Limit Enforcement (#845)

To prevent indefinite unreviewed tenure and ensure periodic community oversight:
- **Council Term Expiry:** Seats are governed by configurable term limits (`configure_council_terms`).
- **Rotation Elections:** When a term expires, a structured rotation election is conducted (`initiate_council_rotation`, `vote_council_rotation`).
- **Safe Fallback & Quorum Preservation:** An expiring seat continues to serve during the transition until a successor is confirmed or an automatic grace period extension applies if an election does not produce a confirmed candidate.
- **Credential Handoff:** Integrates with the protocol's secure key-rotation framework for atomic signer replacement.

