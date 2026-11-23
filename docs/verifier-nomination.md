# Verifier Nomination and Staking

`verifier_nomination` collects community support for prospective risk-registry
verifiers. It does not register verifiers or bypass the existing risk-registry
admin approval and stake checks.

## Flow

1. Initialize with the staking token, minimum total support, per-address cap,
   and nomination window.
2. `nominate(supporter, candidate, amount)` opens a nomination and escrows the
   first support stake. `stake` adds support to an open nomination.
3. `select_candidate` selects the open, unexpired nomination with the highest
   total stake that meets the minimum. Selection is permissionless and bounded
   to 50 nominations per deployment.
4. The selected candidate calls `accept_onboarding` or `decline_onboarding`.
   Only an accepted nomination is reported by `is_eligible`; an operator must
   still submit the candidate to `risk_registry.add_verifier` for formal
   approval and registry staking.
5. Supporters can withdraw after the nomination window, after the nominee
   declines, or after the selected-candidate response window expires.

Each supporter is capped independently per nomination. Stakes are held in the
contract and returned to the same supporter address. Selection does not transfer
or slash support. A declined or timed-out nomination cannot be selected again.
When support is withdrawn from a timed-out accepted nomination, it is marked
expired so eligibility is revoked before the backing is returned.

## Post-slash renomination cool-down

A verifier that has been slashed must wait out a configurable cool-down before
it can be nominated again. Instant renomination after a slash would let a
penalized verifier immediately re-enter the selection pool and defeat the
penalty.

- The cool-down is configured at initialization (in ledgers) and stored on the
  contract. A zero cool-down disables the restriction.
- When a slash is recorded for a candidate, the contract stores the ledger at
  which the cool-down expires (`slash_ledger + cool_down`).
- `nominate` and `stake` reject a candidate whose cool-down has not yet elapsed
  with a typed `CoolDownActive` error (fail closed). The check is applied to the
  candidate address, not the supporter.
- Renomination is allowed once the cool-down has elapsed. The boundary is
  inclusive of the expiry ledger: a nomination at exactly the expiry ledger
  succeeds, while a nomination one ledger before expiry is rejected.
- The cool-down is idempotent: re-recording a slash for the same candidate
  refreshes the expiry to the later of the existing and new expiry, so repeated
  slashes cannot shorten an active cool-down.

## Operational limits

The current implementation caps a deployment at 50 nomination IDs to keep
selection iteration bounded. Deploy a fresh instance for a later nomination
cycle. Configure a meaningful nomination window and per-address cap in the
staking token's smallest units. Choose a cool-down long enough to make a slash
meaningful relative to the nomination window; a cool-down shorter than the
window lets a slashed verifier re-enter before the prior cycle settles. The
nomination contract and risk registry are separate; deployments must publish
the accepted candidate ID/address for the normal admin/governance onboarding
step.
