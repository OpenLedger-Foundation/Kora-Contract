# Protocol-Parameter Governance

Protocol-wide tunable parameters are changed through a lightweight on-chain governance process
rather than by a unilateral single-admin call. The process reuses two existing primitives:

- **B2 multisig** — proposing, voting, and executing are all gated by the configured multisig
  signer set (`configure_multisig`).
- **B1 timelock** — an executed proposal must clear a cooling-off period
  (`GOVERNANCE_TIMELOCK_DELAY`, ~24h, mirroring the upgrade timelock) before it takes effect.

## Governed parameters

| `ParameterKey`   | Meaning                                  | Allowed range |
|------------------|------------------------------------------|---------------|
| `FeeBps`         | Protocol fee in basis points             | `0..=10_000`  |
| `LatePenaltyBps` | Late-repayment penalty in basis points   | `0..=10_000`  |
| `MaxRiskScore`   | Ceiling for accepted invoice risk scores | `0..=100`     |

## Flow

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

## Reading values

- `get_parameter(key) -> Option<u32>` — the current governed value (or `None` if never set).
- `get_parameter_proposal(proposal_id) -> ParameterProposal` — inspect a proposal's votes/state.

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

The signer set and threshold are intentionally reused from the B2 multisig so governance starts
gated by the same trusted set, leaving room to widen stakeholder participation later.
