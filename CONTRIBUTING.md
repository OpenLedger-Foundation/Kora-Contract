# Contributing to Kora Protocol

Thank you for your interest in contributing to Kora. This is an open-source protocol with real-world impact — your contributions help close the trade finance gap for African SMEs. We hold contributions to a high standard because this code handles real money.

---

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [How to Contribute](#how-to-contribute)
- [Development Setup](#development-setup)
- [Branching Strategy](#branching-strategy)
- [Commit Conventions](#commit-conventions)
- [Pull Request Process](#pull-request-process)
- [Testing Requirements](#testing-requirements)
- [Security Vulnerabilities](#security-vulnerabilities)
- [Style Guide](#style-guide)

---

## Code of Conduct

This project follows the [Contributor Covenant](https://www.contributor-covenant.org/version/2/1/code_of_conduct/). By participating, you agree to uphold a respectful, inclusive environment. Harassment, discrimination, or bad-faith contributions will not be tolerated.

---

## How to Contribute

There are several ways to contribute:

- **Bug reports** — open a GitHub issue with a clear reproduction case
- **Feature proposals** — open a GitHub discussion before writing code for significant changes
- **Documentation** — improve clarity, fix typos, add examples
- **Contract improvements** — gas optimizations, security hardening, new features
- **Tests** — additional edge cases, fuzz tests, integration scenarios

For anything that changes protocol behavior or storage layout, open a discussion first. Breaking changes to deployed contracts require a migration plan.

---

## Development Setup

```bash
# 1. Fork and clone
git clone https://github.com/your-fork/kora-contract.git
cd kora-contract

# 2. Install toolchain
rustup target add wasm32-unknown-unknown
cargo install stellar-cli --locked

# 3. Build
make build

# 4. Run tests
make test

# 5. Lint
make lint
```

All of these must pass before opening a PR.

---

## Branching Strategy

| Branch | Purpose |
|--------|---------|
| `main` | Stable, audited code. Protected. |
| `develop` | Integration branch for upcoming releases. |
| `feat/<name>` | New features. Branch from `develop`. |
| `fix/<name>` | Bug fixes. Branch from `develop` (or `main` for hotfixes). |
| `chore/<name>` | Tooling, CI, docs. |

Never push directly to `main` or `develop`. All changes go through pull requests.

---

## Commit Conventions

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short description>

[optional body]

[optional footer]
```

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `chore`, `perf`, `security`

Scopes: `invoice_nft`, `marketplace`, `financing_pool`, `treasury`, `risk_registry`, `access_control`, `shared`, `scripts`, `docs`

Examples:

```
feat(marketplace): add partial funding support with deadline enforcement
fix(financing_pool): correct yield distribution rounding for odd share sizes
security(invoice_nft): add require_auth on set_listed caller
docs(README): add mainnet deployment instructions
```

---

## Testing & Verification

Before opening a pull request, you **must** run the full local verification suite to ensure your changes pass all checks.

### Verification Commands

Run these commands in order from the repository root:

```bash
# 1. Format all code
make fmt

# 2. Lint with clippy (treats warnings as errors)
make lint

# 3. Run all unit and integration tests
cargo test --all
```

All three must pass with no errors or warnings before opening a PR.

### Understanding AUDIT FIX Comments

When reviewing code, you may see comments marked with `// AUDIT FIX:` These indicate code changes made in response to audit findings or internal reviews.

**Format:**
```rust
// AUDIT FIX: Brief description of what was changed and why.
```

**Example:**
```rust
// AUDIT FIX: Removed duplicate sme_invoice_counted event — use sme_invoice_count_incremented instead.
```

When you find an audit fix comment:
1. Read the comment to understand the issue that was addressed
2. Verify the fix is still correct and complete
3. If adding new code in response to an audit finding, use the same convention
4. Reference the audit finding ID or GitHub issue if available

### Extending AUDIT FIX Comments

If you discover a bug or security issue and fix it:

1. Add an `// AUDIT FIX:` comment immediately above or within the fixed code
2. Briefly describe what was wrong and how it was fixed
3. In your commit message, reference the GitHub issue (e.g., `Closes #XYZ`)
4. Link to or record the issue in [AUDIT_LOG.md](AUDIT_LOG.md)

### PR Expectations

Every PR must:

- **Pass all checks** — `make fmt`, `make lint`, and `cargo test --all` with zero failures
- **Include tests** — new features need unit and integration tests; bug fixes need regression tests
- **Be clear and focused** — address one issue or feature per PR
- **Have a complete template** — fill in the PR template completely (Summary, Changes, Testing, Security Considerations, Breaking Changes)
- **Be rebaseable** — use conventional commits; avoid force-pushing after review has started

For PRs that touch storage layout, fee logic, or access control, expect longer review times and requests from two maintainers plus security sign-off.

---

## Changelog Process

We follow [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) format. Every PR that affects user-facing behavior or includes an audit fix must include a CHANGELOG entry.

### Adding Changelog Entries

Edit `CHANGELOG.md` under the `## [Unreleased]` section:

```markdown
## [Unreleased]

### Added
- New features that users should know about

### Fixed
- Bug fixes and audit fixes

### Changed
- Breaking changes or significant behavior modifications

### Deprecated
- Warnings about planned removal of features

### Security
- Security vulnerabilities fixed (or new security features)
```

### Audit Fix Entries

When an audit fix is made, add an entry to the `[Unreleased]` section under **Fixed**:

```markdown
### Fixed
- **Removed duplicate sme_invoice_counted event** — use sme_invoice_count_incremented instead (see AUDIT_LOG.md)
```

Link to [AUDIT_LOG.md](AUDIT_LOG.md) where applicable to track findings across versions.

### Release Process

When cutting a new release:

1. Move all `[Unreleased]` sections to a new version header: `## [X.Y.Z] — YYYY-MM-DD`
2. Retain an empty `## [Unreleased]` section for the next cycle
3. Update all old version links at the bottom of the file
4. Create a git tag: `git tag vX.Y.Z`

---

## Pull Request Process

1. **Branch** from `develop` using the naming convention above.
2. **Write tests** for every change. New features need both unit and integration tests. Bug fixes need a regression test.
3. **Update CHANGELOG.md** if your change affects user-facing behavior or includes an audit fix (see [Changelog Process](#changelog-process)).
4. **Run the full verification suite** locally: see [Testing & Verification](#testing--verification) above.
5. **Open the PR** against `develop`. Fill in the PR template completely.
6. **Request review** from at least one core maintainer.
7. **Address feedback** — do not force-push after review has started; add new commits instead.
8. **Squash on merge** — maintainers will squash your branch into a single clean commit on `develop`.

PRs that touch contract storage layout, fee logic, or access control require review from two maintainers and a security checklist sign-off.

### PR Template

```markdown
## Summary
<!-- What does this PR do? Why? -->

## Changes
<!-- List the files/contracts changed and what changed in each -->

## Testing
<!-- What tests were added or modified? How was this tested? -->

## Security Considerations
<!-- Does this change auth logic, storage, or fee handling? If so, explain. -->

## Breaking Changes
<!-- Does this change any public function signatures or storage keys? -->
```

---

## Testing Requirements

Every PR must maintain or improve test coverage. Specifically:

- **Unit tests** — every public contract function must have at least one happy-path and one failure-path test.
- **Integration tests** — any change to cross-contract interactions must be covered in `contracts/tests/`.
- **Edge cases** — zero amounts, expired timestamps, invalid scores, unauthorized callers, double-initialization.

Run tests with:

```bash
make test          # all tests
make test-verbose  # with stdout output
```

Tests must pass with `cargo clippy -- -D warnings` clean.

---

## Security Vulnerabilities

**Do not open a public GitHub issue for security vulnerabilities.**

Report security issues privately to: **security@kora.finance** (or the maintainer contact listed in the repository).

A log of past internal audit findings and their resolutions is maintained in [AUDIT_LOG.md](AUDIT_LOG.md).

Include:
- A description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will acknowledge within 48 hours and aim to patch within 7 days for critical issues.

See [docs/SECURITY.md](docs/SECURITY.md) for the full security policy.

---

## Style Guide

### Rust

- Follow `rustfmt` defaults (`make fmt` enforces this).
- No `unwrap()` in contract code — use `?` with typed errors from `KoraError`.
- No `panic!` in contract code — Soroban panics consume the entire transaction.
- Use `checked_add`, `checked_mul`, etc. for all arithmetic on financial values.
- All public functions must have a doc comment explaining parameters and failure modes.
- Storage keys must be defined in a `DataKey` enum using `#[contracttype]`.
- Events must be emitted via the `kora_shared::events` module — do not publish raw events inline.

### Documentation

- Write in plain English. Avoid jargon where a simpler word works.
- Code examples in docs must be runnable (or clearly marked as pseudocode).
- Keep line length under 100 characters in Markdown files.

---

## Release Process

See [docs/RELEASE.md](docs/RELEASE.md) for the complete release workflow, including:
- Semantic versioning (MAJOR.MINOR.PATCH)
- Building and recording WASM hashes
- Creating git tags and GitHub releases
- Verifying deployed code matches released binaries
- Promoting from testnet to mainnet

**TL;DR:** Version bumps go in `Cargo.toml`, changelog updates go in `CHANGELOG.md`, and WASM hashes are recorded in `releases/vX.Y.Z.hashes`.

---

## Recognition

All contributors are listed in [CONTRIBUTORS.md](CONTRIBUTORS.md). Significant contributions may be recognized with a protocol grant from the Kora Foundation.

---

*Built for African trade. Open to the world.*
