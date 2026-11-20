# Dispute state-machine fuzzing (#919)

The `fuzz_dispute_resolution` target drives arbitrary opcode sequences through
registered native Soroban contracts. It uses `Env` test utilities, a mock
access-control contract, and the production resolution client. A separate
production escrow client, Stellar token contract, and mock financing pool
exercise fund custody and release. No escrow callback is added to resolution.

The model mirrors the stored flags as Absent, Open, Upheld, and Rejected.
Evidence is replaceable while Open. Both resolved outcomes are absorbing:
further evidence or resolution returns `DisputeAlreadyResolved`, and opening
again returns `DisputeAlreadyOpen`. Rejection must preserve every field of
every invoice, including evidence, timestamps, challenger, and outcome. The
model checks both `get_dispute` and `has_open_dispute` after each operation.

Actors include a configured signer, an admin who is deliberately not a signer,
the payer/challenger, and an outsider. Missing authorization uses an empty
authorization set and requires a raw Soroban Auth error. Other failures must
match the exact expected contract code; unexpected host errors, conversion
errors, and panics fail the target. No panic-catching wrapper suppresses findings.

The escrow companion checks each stored field, payer/pool/escrow balances,
conservation of the initial 1,000 token units, and exactly one mock pool
settlement per resolved invoice. Repeated opens, adjudications, and timeout
refunds cannot move money twice. Timeout is permissionless, including when
the opcode omits authorization. Independent operations also assert that a
resolution-only mutation cannot alter escrow funds or settlement counts.

## Input format and regression seeds

Four bytes encode `(opcode, invoice, actor/auth, argument)`. Invoice selectors
choose four reusable IDs; actor low bits choose the four actors and bit 7
omits authorization. At most 24 complete operations execute.

| Opcode | Operation | Argument |
| --- | --- | --- |
| 0 | Open resolution dispute | Ignored |
| 1 | Submit evidence | Modulo 3: CIDv1, CIDv0, invalid CID |
| 2 | Resolve | Bit 0: upheld |
| 3 | Advance timestamp | Modulo 5: 0, 1, window−1, window, window+1 seconds |
| 4 | Configure access fixture | Bit 0: paused; bit 1: disable signer membership |
| 5 | Open escrow | Modulo 5: −1, 0, 1, 100, 101 units; payer fixed to actor 2 |
| 6 | Adjudicate escrow | Bit 0: pay pool instead of refund |
| 7 | Refund escrow on timeout | Ignored, permissionless |
| 8 | Attempt reinitialization | Ignored |
| 9 | Read | Ignored; all states read after every opcode |

Binary seeds live in `contracts/fuzz/corpus/dispute_resolution/`:

- `terminal-upheld`: resolution absorption and escrow pool payment cannot repeat.
- `terminal-rejected`: rejected resolution, replaced evidence, escrow refund.
- `auth-and-roles`: absent authorization, non-signer admin, wrong challenger,
  invalid CID, eligible investor and unauthorized escrow resolver.
- `deadline-boundaries`: resolution accepts exactly seven days; escrow
  adjudication expires at exactly seven days and timeout refund becomes valid.
- `pause-and-config`: paused opening, signer removal/reinstatement, repeated
  initialization, and unknown-invoice operations.
- `outsider-claim-blocked`: outsider (actor 3) attempts `resolve_dispute` on an
  open escrow with full auth and again with empty auth; both must fail with
  `Unauthorized` and a raw Soroban auth error respectively. Timeout by outsider
  is permissionless and returns `TimeoutNotReached` (not a finding).
- `outsider-resolve-blocked`: outsider attempts `resolve_dispute` on an open
  resolution dispute; must fail with `NotGovernance` (auth) or a raw Soroban auth
  error (no auth). No fund movement occurs.
- `empty-auth-escrow`: escrow open attempted with bit-7 set (empty auth) for
  both the payer and investor actors; both must produce raw Soroban auth errors
  before any storage write. A subsequent authenticated open succeeds, confirming
  the escrow contract state is clean after rejected calls.
- `paused-outsider-resolution`: protocol paused, then outsider attempts to open a
  resolution dispute with and without auth; `ProtocolPaused` beats the outsider
  role check only when auth is present. Unpause restores normal operation.

The normal `kora-fuzz` tests replay these seeds and 32 deterministic random
walks. The ignored smoke test adds `FUZZ_ITERS` walks (default 10,000).
The libFuzzer target uses the same `run` implementation:

```sh
cargo test -p kora-fuzz --test smoke dispute_invariants
cargo test -p kora-fuzz --test smoke fuzz_dispute_resolution -- --ignored
cd contracts/fuzz/fuzz
cargo +nightly fuzz run fuzz_dispute_resolution ../corpus/dispute_resolution
```

## Coverage gaps and validation status

### Covered (as of #1080)

- **Outsider cannot call `resolve_dispute` on escrow**: `outsider-claim-blocked`
  asserts `Unauthorized` on authenticated outsider attempt and a Soroban auth
  error on unauthenticated attempt. Balance conservation invariant confirms no
  fund movement in either case.
- **Outsider cannot call `resolve_dispute` on resolution**: `outsider-resolve-blocked`
  asserts `NotGovernance` on authenticated outsider attempt and a Soroban auth
  error on unauthenticated attempt.
- **Empty auth (bit-7 set) on escrow open**: `empty-auth-escrow` exercises both
  payer and investor actors with no-auth; both must produce raw Soroban auth errors
  before any storage write. The model confirms the escrow slot remains `None`.
- **Paused-protocol interaction with outsider**: `paused-outsider-resolution`
  confirms error ordering — `ProtocolPaused` precedes the outsider role check
  when auth is present; raw auth error when auth is absent.

### Remaining gaps

- Resolution has `open_dispute`, `submit_evidence`, and `resolve_dispute`;
  there is no `challenge` or `withdraw` entry point. Evidence supplies the
  challenge mutation; escrow adjudication/timeout performs fund release.
- Resolution does not invoke escrow. The fuzz companion uses a mock pool;
  production pool callbacks are covered by the separate
  [cross-contract harness](dispute-escrow.md#cross-contract-scenarios-914).
  That harness explicitly orchestrates the two adjudication invocations.
- The access mock exposes signer membership and pause state; it does not
  exercise multisig proposal thresholds, approvals, timelocks, or execution.
  The oracle follows actual signer-membership enforcement, not stronger
  governance behavior described in some existing comments.
- Initial fixture deployment is initialized and successful. Upgrades, storage
  expiration/archival, initialization races, malicious token/pool reentrancy,
  pool accounting failures, and failed token-transfer rollback are not covered.
- Ledger sequence is fixed while timestamps advance. Amounts are bounded near
  zero and the fixture's 100-unit remaining balance. The target does not probe
  `u64::MAX` timestamp overflow or production network resource limits.

These changes were reviewed statically only. No build, formatter, test, or
fuzzer was executed, as requested in `task.txt`; CI must establish compilation
and runtime results before merge.
