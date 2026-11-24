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

### The CI Pipeline (`.github/workflows/release.yml`)

On every `vX.Y.Z` tag push, the release workflow:

1. **Build #1 (canonical):** Builds all contracts from scratch in a clean environment.
   Records SHA-256 hashes. Uploads WASMs as a GitHub Actions artifact.

2. **Build #2 (verification):** A separate job checks out the same commit fresh,
   rebuilds all contracts from scratch, then downloads the Build #1 hashes and compares.
   Any mismatch causes a hard failure (non-zero exit, red CI).

3. **Publish:** Only after both builds confirm identical hashes does the workflow create
   the GitHub Release and attach the `vX.Y.Z.hashes` manifest and a
   `vX.Y.Z-verified-manifest.txt` containing the full verification report.

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

# 3. Clean rebuild with non-determinism flags
SOURCE_DATE_EPOCH=0 CARGO_INCREMENTAL=0 make clean build-optimized

# 4. Download the release's published hashes
curl -sL https://github.com/OpenLedger-Foundation/Kora-Contract/releases/download/v0.2.0/v0.2.0.hashes \
  -o expected.hashes

# 5. Verify
sha256sum -c expected.hashes
# Expected output: each line prints OK
# kora_access_control.wasm: OK
# kora_invoice_nft.wasm: OK
# ...

# 6. Or use the convenience script
./scripts/record-wasm-hashes.sh v0.2.0 --verify
```

### Interpreting Results

- **All `OK`:** The WASM you built from source is byte-identical to what was deployed.
  The audited source code and the on-chain bytecode are the same.

- **Any `FAILED`:** Something differs. Common causes:
  - Wrong toolchain — run `rustc --version` and compare to `rust-toolchain.toml`
  - Uncommitted changes — run `git status` and `git diff`
  - Wrong git ref — run `git rev-parse HEAD` and compare to the release commit
  - Different platform LLVM backend — some platforms produce different output;
    use the same OS as CI (Ubuntu 22.04 / `ubuntu-latest`) for guaranteed match

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
2. Clean rebuild: `make clean && SOURCE_DATE_EPOCH=0 CARGO_INCREMENTAL=0 make build-optimized`
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
| `.github/workflows/release.yml` | CI pipeline: dual-build verification + release |
| `deployments/<network>.json` | Deployment manifest with per-contract hashes |
| `releases/vX.Y.Z.hashes` | Committed hash manifest per release |

---

*Last updated: 2026-09-27*
