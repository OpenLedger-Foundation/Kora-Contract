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

## Operational limits

The current implementation caps a deployment at 50 nomination IDs to keep
selection iteration bounded. Deploy a fresh instance for a later nomination
cycle. Configure a meaningful nomination window and per-address cap in the
staking token's smallest units. The nomination contract and risk registry are
separate; deployments must publish the accepted candidate ID/address for the
normal admin/governance onboarding step.

## Stake accounting and withdrawal rules

**Stake math:** `total_stake` on a `Nomination` always equals the sum of all
individual supporter stakes recorded under `DataKey::Stake(nomination_id,
supporter)`. Multiple supporters contribute independently; each is capped by
`per_address_cap` **per nomination**, not globally. A supporter who saturates
the cap on one nomination can contribute the full cap to a separate nomination.

**Withdraw eligibility:** A supporter may reclaim their stake only under one of
these conditions:

| Condition | Status trigger |
|-----------|---------------|
| Nomination window expired | `NominationStatus::Open` and `now >= created_at + window` |
| Candidate declined | `NominationStatus::Declined` (immediately, no window wait) |
| Selected-candidate response window expired | `NominationStatus::Selected` or `Accepted` and `now >= selected_at + window` |
| Nomination already expired | `NominationStatus::Expired` |

Withdrawing inside an active window returns the typed `NominationError::NotStale`
error. Withdrawing zero stake (the supporter never contributed) returns
`NominationError::InvalidAmount`. Both are typed contract errors — no panics.

**Boundary at exactly the window:** A nomination is considered stale when
`now >= created_at + window` (inclusive). Withdrawal at the exact boundary
second succeeds; one second before fails with `NotStale`.

**Double-nomination rules:** A supporter may back multiple distinct candidates
simultaneously. The per-address cap is enforced per nomination independently.
Submitting zero or negative stake to either `nominate` or `stake` returns
`InvalidAmount`. The nomination counter advances monotonically and is bounded to
`MAX_NOMINATIONS = 50` per deployment.

## Test coverage

The following test cases are in `contracts/verifier_nomination/src/lib.rs`:

| Test | What it asserts |
|------|----------------|
| `enforces_per_address_cap_for_each_nomination` | cap saturates at `per_address_cap` |
| `permits_withdrawal_when_nomination_window_is_stale` | withdrawal after window returns exact staked amount |
| `declined_candidate_unlocks_support_and_accepted_candidate_does_not` | declined → withdrawable; accepted → locked |
| `stake_math_total_equals_sum_of_contributors` | `total_stake == sum(individual stakes)` |
| `stake_cap_independent_per_nomination` | hitting cap on one nomination does not block another |
| `withdraw_returns_exact_staked_amount` | returned amount equals staked amount exactly |
| `multiple_supporters_withdraw_independently_after_window` | each supporter recovers their own share |
| `early_withdraw_inside_window_rejected` | `NotStale` returned while window is open |
| `early_withdraw_one_second_before_boundary_rejected` | boundary -1s returns `NotStale` |
| `withdraw_at_exact_window_boundary_accepted` | boundary exactly succeeds |
| `withdraw_after_decline_available_immediately` | decline bypasses window wait |
| `double_nomination_different_candidates_allowed` | same supporter, two nominations |
| `nomination_id_counter_advances_monotonically` | ids increase |
| `nominate_zero_stake_rejected_typed_error` | zero stake → `InvalidAmount` |
| `stake_zero_to_existing_nomination_rejected` | zero stake call → `InvalidAmount` |
| `nominate_negative_stake_rejected` | negative stake → `InvalidAmount` |
| `withdraw_zero_stake_returns_invalid_amount` | non-contributor withdraw → `InvalidAmount` |
| `select_candidate_no_eligible_returns_typed_error` | no nominations → `NoEligibleCandidate` |
| `nomination_below_minimum_stake_not_selectable` | below `minimum_stake` → `NoEligibleCandidate` |