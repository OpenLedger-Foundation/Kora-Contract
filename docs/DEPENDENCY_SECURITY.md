# Dependency security checks

Every pull request runs the `Security` workflow, including `cargo audit` and
`cargo deny check`. The checks include transitive dependencies and fail the
workflow on vulnerable advisories, denied licenses, or denied crate sources.
Configure the repository's branch protection to require both the `audit` and
`deny` jobs from the `Security` workflow; workflow YAML cannot enable branch
protection by itself.

The `Scheduled dependency advisories` workflow checks the current `main` branch
daily. It runs the same advisory checks and creates or comments on the open
`[Security] Scheduled dependency advisory findings` issue when a scan fails.
It also exits unsuccessfully after creating the issue so the failed workflow
run remains visible. Repository Actions settings must allow the workflow's
`GITHUB_TOKEN` to write issues.

## Exceptions

Do not suppress an advisory solely to make CI pass. An exception in
`deny.toml` must identify the exact advisory, include a documented reachability
and impact assessment, name an owner, provide an expiry date, and link a
tracking issue. A security owner must review it. Do not use wildcard ignores;
remove the exception when the dependency is upgraded or the exception expires.
Revisit all active exceptions during dependency updates.

## Safe validation procedure

To verify the PR gate without leaving a vulnerable dependency in a branch,
temporarily add a known-vulnerable direct or transitive dependency in a
disposable test branch and confirm the `audit` job fails with its RustSec
advisory. Remove the test dependency and lockfile change before opening a real
PR. Do not merge or publish the fixture branch.

To exercise the scheduled workflow's issue-delivery path without manipulating
dependencies, run `Scheduled dependency advisories` with
`simulate_finding=true`. Confirm it creates or comments on the distinct
`[Security Test] Scheduled dependency alert delivery` issue, then close that
test issue. A real finding still creates/updates the normal security tracking
issue and fails the workflow.
## Advisory allowlist

> Issue #1173.

The `advisory-gate` job in `.github/workflows/security.yml` runs on every PR,
push to `main`, and weekly. It fails the build on any RustSec advisory that is
not explicitly allowlisted:

1. `scripts/security/check_advisory_allowlist.py` validates the allowlist and
   fails closed with typed errors:
   - `ERR_CONFIG_MISSING` / `ERR_CONFIG_SCHEMA` — `deny.toml` or
     `.cargo/audit.toml` missing, empty, or without `[advisories].ignore`.
   - `ERR_BAD_ADVISORY_ID` — ID is not `RUSTSEC-YYYY-NNNN`.
   - `ERR_MISSING_REASON` — `deny.toml` entry lacks a `reason` with an
     `expires=YYYY-MM-DD` marker (bare string IDs are rejected).
   - `ERR_EXCEPTION_EXPIRED` — the exception's expiry date has passed.
   - `ERR_ALLOWLIST_DRIFT` — `deny.toml` and `.cargo/audit.toml` disagree.
2. `cargo audit` (reads `.cargo/audit.toml`, `deny = ["warnings"]`).
3. `cargo deny check advisories` (reads `deny.toml`).

To accept an advisory, add it to **both** files:

```toml
# deny.toml
[advisories]
ignore = [
  { id = "RUSTSEC-2024-XXXX", reason = "Unreachable: <analysis>; owner=@sec; expires=2027-01-31; tracked in #NNN" },
]

# .cargo/audit.toml
[advisories]
ignore = ["RUSTSEC-2024-XXXX"]
```

Run locally: `python3 scripts/security/check_advisory_allowlist.py && cargo audit && cargo deny check advisories`.
Tests: `bash scripts/tests/test-advisory-allowlist.sh`.
