# Repo-wide merge corruption on `main` — triage report

**Date:** 2026-08-29
**Branch:** `test/economic-attack-suite`
**Base:** `main` @ `1772f38` (PR #562)

## Summary

`main` does not compile. `cargo check --workspace` fails on `kora-shared` before any
downstream crate is even reached. The corruption is not a single bad commit — it is the
**re-introduction** of a class of merge damage that a previous commit
(`67a44fa` — *"fix: resolve repo-wide merge corruption across contract crates"*) had
already cleaned up. `67a44fa` is an ancestor of `main`, but the long-lived feature
branches merged after it (the `#435` / `#436` / `#437` marketplace line, and the
`invoice-nft` feature branches) each carried a stale/conflicted copy of the shared
files and re-merged the damage.

CI's `check` job (`cargo check --workspace`, `.github/workflows/ci.yml`) must currently
be red on `main`.

## Corruption pattern

Every affected file is a **valid file with an orphaned duplicate fragment concatenated
onto it** — a second copy of the file (or a large span of it) appended after the real
content, leaving an unclosed `{` / `enum` / `fn` at the seam. This is the signature of a
merge resolved by concatenating both sides instead of picking one.

## Affected files

| File | Lines | Damage |
|---|---:|---|
| `contracts/financing_pool/Cargo.toml` | — | duplicate `kora-risk-registry` dependency key — **fixed on this branch** |
| `Cargo.lock` | — | `kora-price-oracle` package listed 3× (2 stale variants carrying a removed `kora-treasury` dep) — **fixed on this branch** |
| `contracts/shared/src/audit.rs` | 138 | two concatenated file versions; unclosed `chain_checksum` fn + duplicate `MAX_AUDIT_LOG_SIZE`. Orphan fragment (old `AuditEntry` + `chain_checksum`, unreferenced anywhere) — **fixed on this branch** (pure deletion) |
| `contracts/shared/src/errors.rs` | 275 | `CommonError` (line 111) unclosed; a **second full `KoraError` enum** (line 129) jammed in after it with ~20 duplicate discriminants. Needs reconciliation — the second enum is the newer/more complete one. |
| `contracts/shared/src/events.rs` | 895 | brace imbalance (+1); unclosed fn mid-file |
| `contracts/access_control/src/lib.rs` | 2634 | brace imbalance (+4); corruption mid-file (single top-level enum defs, so seam is inside a fn or the `tests` module at line 1504) |
| `contracts/invoice_nft/src/lib.rs` | 7450 | **entire file duplicated** — real copy is lines 1–3627, a second `#![no_std]` + full copy starts at line 3628. The `use` block itself also has duplicated import lists (nested corruption). |
| `contracts/risk_registry/src/lib.rs` | 2964 | brace imbalance (+2); corruption mid-file (seam inside a fn or `tests` at line 1274) |
| `contracts/treasury/src/lib.rs` | 2410 | duplicate `use soroban_sdk::{…}` (lines 10 and 17, second is a superset); brace imbalance (+7); likely more duplicated spans |

Brace-balance probe used (naive, counts braces in strings/comments too, but the
non-zero results all correspond to real damage):

```sh
for f in $(git ls-files 'contracts/**/*.rs'); do
  awk 'BEGIN{o=0}{for(i=1;i<=length($0);i++){c=substr($0,i,1);
    if(c=="{")o++;else if(c=="}")o--}}END{if(o!=0)print FILENAME": "o}' "$f"
done
```

## Recommended fix

1. Use `67a44fa` as the reference for the shape of the shared files
   (`errors.rs`, `events.rs`, `audit.rs`) — it is the last known-good reconstruction.
2. For each file: identify the seam, delete the orphaned fragment, then **re-add only the
   genuinely new variants / functions / entrypoints** that landed in features merged
   after `67a44fa` (debtor score cooldown, two-phase cancellation, per-investor
   concentration cap `#435`, investor compliance gate `#436`, `amend_listing` `#437`,
   per-token exposure cap, batch funding, cross-currency funding, fee clawback, the
   invoice-nft protocol-config / exposure-reconciliation / dispute work, etc.).
3. `contracts/invoice_nft/src/lib.rs` is the simplest: truncate at line 3627 and
   diff the two halves to confirm nothing unique lives only in the second copy.
4. Gate with `cargo check --workspace` + `cargo test --all` before merging.
5. Add a CI guard: fail the build if any tracked `.rs` file has unbalanced braces, or
   if `git grep -c 'pub enum KoraError' contracts/shared/src/errors.rs` != 1.

## What this branch contains

Only the three mechanical, zero-risk fixes needed to get *past* the manifest/lockfile
layer (`financing_pool/Cargo.toml`, `Cargo.lock`, `shared/src/audit.rs`) plus this
report. The workspace **still does not compile** — `errors.rs` and the four `lib.rs`
files above are untouched and need the reconstruction described above.

The originally-assigned work (economic-attack integration test suite:
flash-fund-then-cancel, price-window sandwiching, partial-funding griefing) is **blocked**
on this and has not been started.
