# Kora Protocol — Reproducible Build Verification

This document explains how the Kora Protocol guarantees that deployed WASM bytecode
matches the reviewed source code, and how any contributor can independently verify it.

---

## Why Reproducible Builds Matter

Investors and auditors reviewing the Kora Protocol source code need assurance that the
bytecode running on-chain is exactly what was reviewed. Without this, trust depends
entirely on the deployer's unverifiable claim.

The reproducible-build pipeline closes this gap: it proves — verifiably, via independent
rebuild — that the deployed hash matches a build from the tagged source commit.

---

## How It Works

### Toolchain Pinning (`rust-toolchain.toml`)

The workspace root contains a `rust-toolchain.toml` that pins the exact Rust version:

```toml
[toolchain]
channel = "1.75.0"
targets = ["wasm32-unknown-unknown"]
```

`rustup` reads this file automatically and installs or switches to the pinned toolchain.
This eliminates one major source of non-determinism: different compiler versions can
produce different codegen output for the same source.

### Non-Determinism Elimination Checklist

The build is deterministic because every source of variance has been addressed:

| Source | Eliminated by |
|--------|--------------|
| Compiler version | `rust-toolchain.toml` pins `1.75.0` |
| Debug timestamps | `SOURCE_DATE_EPOCH=0` in CI env |
| Incremental compilation state | `CARGO_INCREMENTAL=0` in CI env |
| Absolute paths in debug info | `--remap-path-prefix` in `RUSTFLAGS` |
| Link-time codegen variance | `lto = true` in `[profile.release]` |
| Parallel codegen units | `codegen-units = 1` in `[profile.release]` |
| Debug symbols | `debug = 0`, `strip = "symbols"` in `[profile.release]` |
| Optimisation level variance | `opt-level = "z"` in `[profile.release]` |

### The CI Pipeline (`.github/workflows/reproducible-build.yml`)

On every `vX.Y.Z` tag push (or a manual `workflow_dispatch` with a `version`
input), the reproducible-build workflow:

1. **Resolves the committed manifest.** `releases/vX.Y.Z.hashes` must already be
   committed (recorded with `scripts/record-wasm-hashes.sh`). A missing manifest
   fails the job.
2. **Builds release WASM from scratch** on the pinned `ubuntu-22.04` runner with
   the toolchain from `rust-toolchain.toml`, no build cache,
   `SOURCE_DATE_EPOCH=0`, `CARGO_INCREMENTAL=0` and path-remapping `RUSTFLAGS`
   (`make clean && make build`).
3. **Compares hashes strictly** with `scripts/attest-wasm-hashes.sh`. Any
   mismatch, missing binary, malformed or empty manifest is a hard failure
   (non-zero exit, red CI) and **nothing is attested**.
4. **Attests and uploads** only when every hash matches:
   - a Sigstore-signed SLSA build-provenance attestation for each
     `kora_*.wasm` (`actions/attest-build-provenance`), stored in the
     repository's attestations and verifiable with `gh attestation verify`;
   - a workflow artifact `wasm-attestation-vX.Y.Z` containing
     `vX.Y.Z.attestation.json` (version, commit, toolchain, runner image,
     build env, manifest SHA-256 and per-contract hashes), the verified
     `vX.Y.Z.hashes` manifest and the WASM binaries.

On pull requests that touch `releases/*.hashes` or the attestation tooling, only
the fixture tests (`scripts/tests/test-attest-wasm-hashes.sh`) run.

#### Attestation report format

```json
{
  "schema": "kora.reproducible-build.attestation/v1",
  "version": "v0.2.0",
  "commit": "<git sha>",
  "generated_at": "2026-11-22T00:00:00Z",
  "result": "match",
  "toolchain": "rustc 1.75.0 (...)",
  "platform": "Linux-x86_64",
  "runner_image": "ubuntu22@20261117.1.0",
  "build_env": {"SOURCE_DATE_EPOCH": "0", "CARGO_INCREMENTAL": "0"},
  "manifest": {"file": "releases/v0.2.0.hashes", "sha256": "<sha of manifest>"},
  "contracts": [
    {"file": "target/wasm32-unknown-unknown/release/kora_access_control.wasm", "sha256": "..."}
  ]
}
```

---

## Independent Verification (Step-by-Step)

Any contributor, auditor, or investor can independently verify a deployment.
No CI access is required.

### Prerequisites

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install stellar CLI (pinned version)
cargo install stellar-cli --locked --version 21.4.1
```

### Steps

```bash
# 1. Checkout the exact tagged commit
git clone https://github.com/OpenLedger-Foundation/Kora-Contract.git
cd Kora-Contract
git checkout v0.2.0   # replace with the version you want to verify

# 2. Confirm toolchain (rust-toolchain.toml is read automatically)
rustup show active-toolchain
# Expected: 1.75.0-x86_64-unknown-linux-gnu (or your platform equivalent)

# 3. Clean rebuild with the same flags CI uses
export SOURCE_DATE_EPOCH=0 CARGO_INCREMENTAL=0
export RUSTFLAGS="--remap-path-prefix=$(pwd)=/build --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"
make clean build

# 4. Download the release's published hashes
curl -sL https://github.com/OpenLedger-Foundation/Kora-Contract/releases/download/v0.2.0/v0.2.0.hashes \
  -o expected.hashes

# 5. Verify
sha256sum -c expected.hashes
# Expected output: each line prints OK
# kora_access_control.wasm: OK
# kora_invoice_nft.wasm: OK
# ...

# 6. Or use the strict comparison script (fails on any mismatch or missing
#    binary and writes attestation/v0.2.0.attestation.json on success)
./scripts/attest-wasm-hashes.sh v0.2.0
```

### Verifying the CI Attestation

You can also check that the binaries you are about to deploy (or that were
deployed) are exactly the ones CI rebuilt and matched against the committed
hashes, without rebuilding yourself:

```bash
# Find the attestation run for the tag and download its artifact (requires gh CLI)
RUN_ID=$(gh run list --repo OpenLedger-Foundation/Kora-Contract \
  --workflow reproducible-build.yml --branch v0.2.0 --status success \
  --limit 1 --json databaseId --jq '.[0].databaseId')
gh run download "$RUN_ID" --repo OpenLedger-Foundation/Kora-Contract \
  --name wasm-attestation-v0.2.0 --dir wasm-attestation

# Verify the Sigstore-signed provenance for each binary
for wasm in wasm-attestation/target/wasm32-unknown-unknown/release/kora_*.wasm; do
  gh attestation verify "$wasm" --repo OpenLedger-Foundation/Kora-Contract
done

# Confirm the report says "match" and the manifest is the committed one
grep '"result": "match"' wasm-attestation/v0.2.0.attestation.json
sha256sum releases/v0.2.0.hashes   # equals .manifest.sha256 in the report
```

`gh attestation verify` proves the binary was produced by this repository's
`reproducible-build.yml` workflow at the attested commit. Combine it with
`stellar contract fetch --id <CONTRACT_ID> | sha256sum` (or the
`wasm_hash` in `deployments/<network>.json`) to tie the on-chain bytecode to
the attested hash.

### Interpreting Results

- **All `OK`:** The WASM you built from source is byte-identical to what was deployed.
  The audited source code and the on-chain bytecode are the same.

- **Any `FAILED`:** Something differs. Common causes:
  - Wrong toolchain — run `rustc --version` and compare to `rust-toolchain.toml`
  - Uncommitted changes — run `git status` and `git diff`
  - Wrong git ref — run `git rev-parse HEAD` and compare to the release commit
  - Different platform LLVM backend — some platforms produce different output;
    use the same OS as CI (Ubuntu 22.04 x86_64) for guaranteed match
  - Different `RUSTFLAGS` — the path remaps above are part of the canonical build

### Platform Drift

Canonical hashes are produced on `ubuntu-22.04` x86_64. Known drift sources:

- **Host OS / architecture:** macOS and `aarch64` hosts can emit different
  bytes even with the same toolchain. Verify on Linux x86_64 (a container such
  as `ubuntu:22.04` is sufficient) before reporting a mismatch.
- **Runner image updates:** the workflow pins `ubuntu-22.04` rather than
  `ubuntu-latest`; the image version is recorded in `runner_image` in the
  attestation report so a drift can be traced to an image rollover.
- **Toolchain bumps:** changing `rust-toolchain.toml` changes the hashes; a new
  manifest must be recorded and committed for the next release.
- **Optimized artifacts:** hashes cover the raw `kora_*.wasm` from `make build`.
  `stellar contract optimize` output depends on the CLI version and is not part
  of the reproducibility claim.

---

## Hash Files

Hash manifests are stored in two places:

1. **`releases/vX.Y.Z.hashes`** — committed to the repository for full audit trail
2. **GitHub Release assets** — attached to each release for external download

Format: standard `sha256sum` output, verifiable with `sha256sum -c`.

```
a1b2c3...  target/wasm32-unknown-unknown/release/kora_access_control.wasm
b2c3d4...  target/wasm32-unknown-unknown/release/kora_invoice_nft.wasm
...
```

---

## Deployment Manifests

When a contract is deployed, `scripts/deploy.sh` records SHA-256 hashes in the
deployment manifest (`deployments/<network>.json`):

```json
{
  "contracts": {
    "access_control": {
      "address": "CA...XYZ",
      "wasm_hash": "a1b2c3..."
    }
  }
}
```

Cross-reference: the `wasm_hash` in the deployment manifest must match the corresponding
line in `releases/vX.Y.Z.hashes`. If they differ, the deployed binary was not built from
the tagged source.

---

## What to Do If Hashes Don't Match

1. **Do not deploy** until the mismatch is resolved.
2. Clean rebuild: `make clean && SOURCE_DATE_EPOCH=0 CARGO_INCREMENTAL=0 make build` (with the `RUSTFLAGS` path remaps from the Independent Verification steps)
3. Confirm toolchain matches `rust-toolchain.toml`
4. Confirm no uncommitted changes: `git status`
5. Confirm correct git ref: `git rev-parse HEAD` vs the release tag's commit SHA
6. If the issue persists on the same platform, open an issue — it may indicate a
   Rust/LLVM regression in reproducibility for the pinned version.

---

## Pinned builder image

> Issue #1174.

Floating tags (`rust:1.75`, `debian:bookworm-slim`) can be re-pointed upstream,
silently changing the toolchain. Every `FROM` in `packaging/` is therefore
pinned by immutable digest:

| File | Image |
|------|-------|
| `packaging/Dockerfile.wasm` | `rust:1.75.0-slim-bookworm@sha256:70c2a016…1f74` (matches `rust-toolchain.toml`) |
| `packaging/Dockerfile` (indexer) | `rust:1.78-slim-bookworm@sha256:0fea9676…2dc9`, `debian:bookworm-slim@sha256:e67eeead…ce0e` |

Build canonical WASM in the pinned container:

```bash
docker build -f packaging/Dockerfile.wasm -o target/wasm-docker .
cat target/wasm-docker/SHA256SUMS
```

The `pinned-digests` job in `.github/workflows/security.yml` runs
`scripts/check-pinned-digests.sh`, which fails with `ERR_UNPINNED_IMAGE` for any
tag-only `FROM` and fails closed (`ERR_CONFIG_MISSING`/`ERR_CONFIG_SCHEMA`, exit 2)
on missing or FROM-less Dockerfiles. Tests: `bash scripts/tests/test-pinned-digests.sh`
(also asserts the WASM builder tag matches `rust-toolchain.toml`).

**Rotating the digest:** when bumping `rust-toolchain.toml`, resolve the new
digest (`docker buildx imagetools inspect rust:<ver>-slim-bookworm`), update
`packaging/Dockerfile.wasm` and this table in the same PR, and record the new
WASM hashes.

---

## Related Files

| File | Purpose |
|------|---------|
| `rust-toolchain.toml` | Pins Rust toolchain version for reproducibility |
| `packaging/Dockerfile.wasm` | Digest-pinned WASM builder container |
| `scripts/check-pinned-digests.sh` | CI gate: every `FROM` must be digest-pinned |
| `Cargo.toml` `[profile.release]` | LTO, codegen-units=1, strip symbols |
| `scripts/record-wasm-hashes.sh` | Record and verify hashes locally |
| `.github/workflows/reproducible-build.yml` | CI: rebuild, compare to committed hashes, attest |
| `scripts/attest-wasm-hashes.sh` | Strict hash comparison + attestation report |
| `scripts/tests/test-attest-wasm-hashes.sh` | Fixture tests for the comparison script |
| `deployments/<network>.json` | Deployment manifest with per-contract hashes |
| `releases/vX.Y.Z.hashes` | Committed hash manifest per release |

---

*Last updated: 2026-11-22*
