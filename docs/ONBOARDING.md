# Contributor Onboarding Guide

This guide walks a new contributor from zero to a merged first pull request. It
was validated by actually following it end-to-end on a clean checkout (see
[Known Current Blockers](#known-current-blockers) for what that run found and
fixed, and what's still open).

If you're looking for something to work on rather than how to set up, skip to
[`GOOD_FIRST_ISSUES.md`](GOOD_FIRST_ISSUES.md).

---

## 1. Local Setup

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) — installed via `rustup`,
  **not** your OS package manager. The repo pins an exact toolchain in
  [`rust-toolchain.toml`](../rust-toolchain.toml); `rustup` will install and
  switch to it automatically the first time you run `cargo` in this directory.
- [stellar CLI](https://developers.stellar.org/docs/tools/stellar-cli) — only
  needed for `make build-optimized` / `make deploy-testnet`. Skip it if you're
  just building and testing.
- Docker + VS Code with the Dev Containers extension — optional, but the
  fastest path if you don't want to manage Rust versions yourself. See
  [`CONTRIBUTING.md` § Development Setup](../CONTRIBUTING.md#development-setup).

### Get the code building

```bash
git clone https://github.com/<your-fork>/Kora-Contract.git
cd Kora-Contract

# rustup reads rust-toolchain.toml and installs the pinned version automatically
rustup target add wasm32-unknown-unknown

make check      # cargo check --all — fastest way to confirm your toolchain works
make build      # cargo build --target wasm32-unknown-unknown --release
make test       # cargo test --all
make lint       # cargo clippy --all --all-targets -- -D warnings
make fmt        # cargo fmt --all
```

Run these **in this order** the first time: `check` fails fast if your
toolchain is wrong, before you pay for a full `build`.

### Orient yourself in the codebase

Read, in order:

1. [`README.md`](../README.md) § Project Structure — one paragraph per
   contract, what it owns.
2. [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) — how the contracts call each
   other (invoice_nft → marketplace → financing_pool → treasury).
3. Pick **one** contract under `contracts/<name>/src/lib.rs` relevant to the
   issue you're picking up and read it top to bottom before editing. Each
   contract crate is self-contained: storage keys (`DataKey` enum), errors
   (`<Contract>Error` enum), and entrypoints (`impl <Contract>Contract`) all
   live in that one file.
4. [`contracts/shared/src`](../contracts/shared/src) — types, errors, events,
   and validation helpers every contract crate depends on. `errors.rs`'s
   `KoraError` enum is the master error registry; `kora-xtask
   check-error-variants` (see below) keeps it honest.

### Governance and escrow contract inventory

These contracts are workspace members and participate in workspace formatting,
linting, and tests. CI also runs their unit tests separately with at most two
matrix jobs in parallel (issues #912 and #911).

| Directory | Cargo package | Typed Rust client |
| --- | --- | --- |
| `contracts/governance` | `kora-governance` | `GovernanceContractClient` |
| `contracts/governance_voting` | `kora-governance-voting` | `GovernanceVotingContractClient` |
| `contracts/parameter_registry` | `kora-parameter-registry` | `ParameterRegistryContractClient` |
| `contracts/dispute_escrow` | `kora-dispute-escrow` | `DisputeEscrowContractClient` |
| `contracts/secondary_market` | `kora-secondary-market` | `SecondaryMarketContractClient` |

The governance crates depend on shared types and (for voting and registry)
access control; they do not depend on each other. Parameter registry is an
on-chain contract even when its values are queried by off-chain services.
All four release WASM artifacts are covered by the size-budget gate. See each
contract README for address configuration and use `cargo +stable test -p
<package>` to isolate its unit tests. No compile or test results are claimed
by the #912/#911 onboarding change; verification was restricted to static review.

**`secondary_market`** (`contracts/secondary_market`, `kora-secondary-market`) is a
first-class workspace member as of issue #910. CI builds it natively and for
`wasm32-unknown-unknown`, runs its unit tests (`cargo test -p kora-secondary-market`),
and the WASM size-budget gate covers its release artifact. Add it to your local
build whenever you touch settlement or secondary-market logic.

### Before you touch code

- [`CONTRIBUTING.md`](../CONTRIBUTING.md) covers commit conventions, branching
  (`feat/<name>` off `develop`), the PR template, and the changelog process —
  read it once, it's not optional.
- If you're adding a `KoraError` variant reference from a contract crate, add
  the matching variant to `contracts/shared/src/errors.rs` in the same
  change, then run:

  ```bash
  cargo run -p kora-xtask --bin check-error-variants
  ```

  CI fails the build otherwise (see [`CONTRIBUTING.md` § Style
  Guide](../CONTRIBUTING.md#style-guide)).

- If you're adding or changing a public contract entrypoint (`pub fn` in a
  `#[contractimpl]` block), add the matching row to
  [`docs/ACCESS_MATRIX.md`](ACCESS_MATRIX.md) in the same change, then run:

  ```bash
  cargo run -p kora-xtask --bin check_access_matrix
  ```

  The checker parses the matrix tables from that file (no hardcoded copy —
  the markdown is the single source of truth), supports
  `--contract <name>` and `--verbose`, and CI fails the build on any drift
  or undocumented entrypoint. See the "New Entrypoint Checklist" in
  [`docs/ACCESS_MATRIX.md`](ACCESS_MATRIX.md).

---

## 2. Making Your First Contribution

1. Find an issue via [`GOOD_FIRST_ISSUES.md`](GOOD_FIRST_ISSUES.md) or the
   [`good first issue`](https://github.com/OpenLedger-Foundation/Kora-Contract/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22)
   label. Comment on it before starting so it doesn't get picked up twice.
2. Branch off `develop`: `git checkout -b fix/<short-description> develop`.
3. Make the change. Keep it scoped to the issue — a bug fix doesn't need
   surrounding refactors.
4. Add or update tests. Every public contract function needs at least one
   happy-path and one failure-path test (see [`CONTRIBUTING.md` § Testing
   Requirements](../CONTRIBUTING.md#testing-requirements)).
5. Run the full verification suite locally before opening the PR:
   ```bash
   make fmt && make lint && cargo test --all
   ```
6. Open the PR against `develop`, fill in the template completely, and link
   the issue it closes.
7. Add yourself to [`CONTRIBUTORS.md`](../CONTRIBUTORS.md) in the same PR.

---

## Known Current Blockers

This guide was validated by running the setup steps above end-to-end rather
than assumed. That run surfaced real, current problems on `main` — some were
small enough to fix as part of building this guide (see the PR that
introduced this file for the exact commits), others are separate, larger
issues now tracked for follow-up. Listing them here so the next contributor
doesn't waste an afternoon thinking they broke something:

- **Toolchain / lockfile mismatch.** `rust-toolchain.toml` pins Rust
  **1.75.0** (for byte-reproducible WASM output — see
  [`docs/REPRODUCIBLE_BUILD.md`](REPRODUCIBLE_BUILD.md)), but `Cargo.lock` is
  in lockfile format v4, which requires Cargo **≥1.78**. Cargo 1.75 cannot
  even parse the committed lockfile — `cargo check` fails before touching any
  source. Until a maintainer decides how to reconcile the pin with the
  lockfile (bump the pin, or re-pin the lockfile against 1.75-compatible
  dependency versions), **use a newer stable toolchain locally** to get a
  working build:
  ```bash
  rustup install stable
  cargo +stable check --all
  ```
  Understand that WASM built this way won't byte-for-byte match the
  officially pinned reproducible-build target — fine for iterating on logic
  and tests, not for anything release-related.
- **`contracts/financing_pool` (and its dependents `marketplace`,
  `fractionalizer`) currently fail to compile** — a large, pre-existing set of
  errors unrelated to this guide (malformed doc comments in a formal
  verification model, among others). Out of scope here; needs its own
  triage issue rather than a rushed fix. `contracts/tests` (the integration
  suite) and `contracts/fuzz` depend on these crates and won't build until
  it's resolved either.
- **`contracts/referral_rewards` expects `invoice_nft` to expose a referral
  feature that doesn't exist yet** (`mint_invoice_with_referrer`,
  `get_sme_referrer`, `get_first_mint`, an `InvalidReferrer` error variant).
  This is a real missing feature, not a bug in the existing code — it needs
  design (anti-self-referral, cycle detection) before implementation, not a
  mechanical fix.

- **Several `#[cfg(test)] mod tests` blocks carry the same kind of merge
  duplication independently of the production-code issues above** — spot
  checks found duplicate test functions and tests referencing entrypoints
  that were never implemented (a two-step admin-transfer flow, an audit-log
  getter) in `invoice_nft`, `risk_registry`, and `treasury` at least.
  `cargo check` / `make build` (production code) is clean for all three;
  `cargo test` for each currently is not. This looks scattered across more
  than these three crates and needs its own audit pass — a good candidate
  for the kind of split described in `GOOD_FIRST_ISSUES.md`'s worked example,
  crate by crate.

Everything else — `kora-shared`, `access_control`, `risk_registry`,
`price_oracle`, `treasury`, `invoice_nft`, `dispute_resolution`, `tranche`,
`verifier_nomination`, and `xtask` — **builds cleanly** (`make check` /
`make build`, i.e. `cargo check --all` / `cargo build`) as of this guide's
validation pass, using a newer-than-pinned toolchain per the note above.
Given the test-module issue just above, don't assume a full `cargo test
--all` is green right now — run tests scoped to the crate you're actually
touching (`cargo test -p <crate>`) and treat unrelated failures elsewhere as
pre-existing, not something your change broke.
