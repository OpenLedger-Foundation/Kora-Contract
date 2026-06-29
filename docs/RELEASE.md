# Kora Protocol — Release Process

This document describes how to release a new version of the Kora Protocol and verify that deployed contracts match their intended WASM binaries.

---

## Release Versioning

We follow [Semantic Versioning](https://semver.org/):

```
MAJOR.MINOR.PATCH
 │      │      └─ Bug fixes and non-breaking changes
 │      └──────── New features and backward-compatible changes
 └─────────────── Breaking changes (protocol, storage, or API)
```

**Examples:**
- `0.1.0` — initial release
- `0.1.1` — bug fix (non-breaking)
- `0.2.0` — new feature (backward-compatible)
- `1.0.0` — breaking change or production milestone

---

## Release Workflow

### Step 1: Prepare the Release

1. **Update version numbers** in `Cargo.toml` (all contract crates):
   ```toml
   [package]
   version = "X.Y.Z"
   ```

2. **Finalize CHANGELOG.md:**
   - Move all `[Unreleased]` sections to a new version header:
     ```markdown
     ## [X.Y.Z] — YYYY-MM-DD
     ```
   - Create a new empty `[Unreleased]` section
   - Update version links at the bottom of the file

3. **Review AUDIT_LOG.md:**
   - Confirm all findings are marked as Fixed, Planned, or Open
   - Update status for findings resolved in this release

4. **Commit version changes:**
   ```bash
   git add Cargo.toml CHANGELOG.md AUDIT_LOG.md
   git commit -m "chore: prepare release vX.Y.Z"
   ```

### Step 2: Build & Verify WASM Binaries

1. **Clean the build directory:**
   ```bash
   make clean
   ```

2. **Build and optimize all contracts:**
   ```bash
   make build-optimized
   ```

3. **Record WASM hashes:**
   ```bash
   sha256sum target/wasm32-unknown-unknown/release/kora_*.wasm > releases/vX.Y.Z.hashes
   ```

   This generates a file like:
   ```
   a1b2c3d4e5f6... target/wasm32-unknown-unknown/release/kora_access_control.wasm
   b2c3d4e5f6a7... target/wasm32-unknown-unknown/release/kora_invoice_nft.wasm
   c3d4e5f6a7b8... target/wasm32-unknown-unknown/release/kora_marketplace.wasm
   d4e5f6a7b8c9... target/wasm32-unknown-unknown/release/kora_financing_pool.wasm
   e5f6a7b8c9d0... target/wasm32-unknown-unknown/release/kora_treasury.wasm
   f6a7b8c9d0e1... target/wasm32-unknown-unknown/release/kora_risk_registry.wasm
   ```

4. **Verify all tests pass:**
   ```bash
   make fmt && make lint && cargo test --all
   ```

### Step 3: Create Git Tag

```bash
git tag -a vX.Y.Z -m "Release vX.Y.Z

$(cat <<'EOF'
Contracts:
- access_control
- invoice_nft
- marketplace
- financing_pool
- treasury
- risk_registry
- shared (library)

WASM hashes recorded in releases/vX.Y.Z.hashes
See CHANGELOG.md for changes.
See THREAT_MODEL.md for security posture.
EOF
)"

git push origin vX.Y.Z
```

### Step 4: Create GitHub Release

```bash
gh release create vX.Y.Z \
  --title "Kora Protocol vX.Y.Z" \
  --notes-file <(cat <<'EOF'
## What's New

[Summarize major features, fixes, and improvements]

## WASM Hashes

Verify that the deployed contracts match:

$(cat releases/vX.Y.Z.hashes)

## Deployment

- **Testnet:** Deploy via `make deploy-testnet`
- **Mainnet:** Deploy via `make deploy-mainnet` (requires admin key)

### Pre-Deployment Checklist

- [ ] All audit findings resolved or documented in [AUDIT_LOG.md](../AUDIT_LOG.md)
- [ ] THREAT_MODEL.md reviewed and updated
- [ ] CHANGELOG.md reflects all changes
- [ ] WASM hashes verified locally
- [ ] Security review completed
- [ ] Integration test suite passes on testnet

## Installation

1. Update your deployment manifest with new contract addresses
2. Update any client libraries that reference contract ABIs
3. Run full integration test suite before switching to mainnet

## Contributors

[List contributors if applicable]

See [CHANGELOG.md](../CHANGELOG.md) for full change log.
EOF
)" \
  releases/vX.Y.Z.hashes
```

---

## Verifying Deployed Code

To verify that a deployed contract matches the released WASM binary:

### 1. Get the Deployed Contract Hash

```bash
# Query the contract's WASM hash from the ledger
stellar contract info --id CAB...XYZ --network testnet
```

The output includes a `current_ledger_state` hash.

### 2. Compare Against Released Hashes

```bash
# Download the release hashes
curl -s https://github.com/OpenLedger-Foundation/Kora-Contract/releases/download/vX.Y.Z/vX.Y.Z.hashes \
  > released.hashes

# Compare locally-built WASM against released hashes
sha256sum target/wasm32-unknown-unknown/release/kora_access_control.wasm | \
  grep -f - released.hashes

# If no output, the hashes don't match — investigate
# If output shows the line, the WASM matches the release
```

### 3. Full Verification Script

Create `scripts/verify-deployment.sh`:

```bash
#!/usr/bin/env bash
# Verify that deployed contracts match released WASM hashes

NETWORK="${1:-testnet}"
VERSION="${2:-v0.1.0}"

CONTRACTS=(
  "access_control"
  "invoice_nft"
  "marketplace"
  "financing_pool"
  "treasury"
  "risk_registry"
)

echo "=== Verifying Kora Protocol Deployment ==="
echo "Network : $NETWORK"
echo "Version : $VERSION"
echo ""

# Download released hashes (requires internet)
RELEASE_URL="https://github.com/OpenLedger-Foundation/Kora-Contract/releases/download/$VERSION/$VERSION.hashes"
TEMP_HASHES=$(mktemp)
curl -sL "$RELEASE_URL" -o "$TEMP_HASHES" || {
  echo "ERROR: Could not download hashes from $RELEASE_URL"
  exit 1
}

echo "--- Verifying WASM binaries ---"
ALL_MATCH=true
for contract in "${CONTRACTS[@]}"; do
  WASM="target/wasm32-unknown-unknown/release/kora_$contract.wasm"
  if [ ! -f "$WASM" ]; then
    echo "❌ $contract: WASM not found at $WASM"
    ALL_MATCH=false
    continue
  fi

  HASH=$(sha256sum "$WASM" | awk '{print $1}')
  if grep -q "^$HASH" "$TEMP_HASHES"; then
    echo "✓ $contract: $HASH"
  else
    echo "❌ $contract: HASH MISMATCH"
    echo "  Got:      $HASH"
    echo "  Expected: $(grep "kora_$contract.wasm" $TEMP_HASHES | awk '{print $1}')"
    ALL_MATCH=false
  fi
done

rm -f "$TEMP_HASHES"

echo ""
if [ "$ALL_MATCH" = true ]; then
  echo "✓ All WASM binaries match release $VERSION"
  exit 0
else
  echo "❌ One or more WASM binaries do not match release $VERSION"
  exit 1
fi
```

---

## Storing WASM Hashes

**Location Options:**

1. **In Repository** (`releases/vX.Y.Z.hashes`):
   - Pros: Auditable in git history; no external dependency
   - Cons: Increases repo size over time
   - Recommendation: ✅ Primary storage

2. **GitHub Release Assets**:
   - Pros: Standard GitHub release interface; easy to download
   - Cons: Requires GitHub API access; assets are immutable (can't correct errors)
   - Recommendation: ✅ Secondary storage (created automatically)

3. **IPFS**:
   - Pros: Decentralized; permanent; content-addressable
   - Cons: Requires IPFS access; not as discoverable
   - Recommendation: 🔮 Future enhancement

**Format:**

```
<SHA256-HASH> <RELATIVE-PATH-TO-WASM>
<SHA256-HASH> <RELATIVE-PATH-TO-WASM>
...
```

Each line contains one hash and path, separated by spaces. This is the standard `sha256sum` format and can be verified with:

```bash
sha256sum -c releases/vX.Y.Z.hashes
```

---

## Promoting Across Networks

### Testnet → Mainnet

1. **Verify on Testnet First:**
   - Deploy to testnet
   - Run full integration test suite
   - Confirm end-to-end flows work (mint → list → fund → repay)
   - Wait 7+ days for community feedback

2. **Prepare Mainnet Deployment:**
   - Record mainnet contract addresses after deployment
   - Update `deployments/mainnet.json`
   - Notify all dependent services (indexers, frontend, etc.)

3. **Deploy to Mainnet:**
   ```bash
   DEPLOYER_SECRET=... make deploy-mainnet
   ```

4. **Verify Post-Deployment:**
   - Confirm all contracts are initialized
   - Verify fee parameters are correct
   - Confirm token whitelist is set
   - Run smoke tests through the marketplace

---

## Rollback & Emergency Procedures

### If a Critical Bug is Found Post-Release

1. **Do NOT immediately release a patch.**
2. **Pause the protocol** via `access_control.pause()` if needed to prevent new transactions
3. **Investigate** — root cause analysis and testing
4. **Fix & test** — create a patch branch and run full test suite
5. **Release patch** — follow the release workflow above for `vX.Y.Z+1`
6. **Deploy patch** — deploy the fixed contracts and resume operations

### If WASM Hashes Don't Match

1. **Stop deployment immediately**
2. **Check build environment:**
   - Rust version: `rustc --version` (must match lockfile)
   - Stellar CLI version: `stellar --version`
   - Clean build: `make clean && make build-optimized`
3. **Compare against source:**
   - Verify no uncommitted changes: `git status`
   - Verify correct git ref: `git rev-parse HEAD`
4. **Investigate differences:**
   - Non-deterministic builds can produce different hashes even for identical source
   - If hashes still don't match, investigate Rust/LLVM versions
   - Consider using a Docker build environment for reproducibility (planned for v2)

---

## Checklist: Release Day

- [ ] All audit findings resolved or documented
- [ ] CHANGELOG.md finalized and version bumped in Cargo.toml
- [ ] AUDIT_LOG.md updated with resolution status
- [ ] THREAT_MODEL.md reviewed for new findings
- [ ] All tests pass: `make fmt && make lint && cargo test --all`
- [ ] WASM binaries built and optimized: `make clean && make build-optimized`
- [ ] WASM hashes recorded: `sha256sum target/wasm32-unknown-unknown/release/kora_*.wasm`
- [ ] Hashes committed to git: `releases/vX.Y.Z.hashes`
- [ ] Git tag created and pushed: `git tag -a vX.Y.Z && git push origin vX.Y.Z`
- [ ] GitHub release created with hashes attached
- [ ] Release notes link to CHANGELOG.md and THREAT_MODEL.md
- [ ] Security contacts notified (if applicable)
- [ ] Community announcement prepared

---

## Future Enhancements

- **Docker reproducible builds** (v2) — ensure WASM hashes are reproducible across environments
- **IPFS hash storage** (v2) — store WASM hashes on IPFS for decentralized availability
- **Automated release CI** (v2) — GitHub Actions workflow that builds, hashes, and publishes releases
- **Signed releases** (v2) — GPG-sign release tags for authenticity
- **Contract upgrade mechanism** (v2) — on-chain upgrade with timelock and multisig approval

---

## References

- [Semantic Versioning](https://semver.org/)
- [Keep a Changelog](https://keepachangelog.com/)
- [CHANGELOG.md](../CHANGELOG.md)
- [AUDIT_LOG.md](../AUDIT_LOG.md)
- [THREAT_MODEL.md](../THREAT_MODEL.md)
- [Stellar Contract Docs](https://developers.stellar.org/docs/build/smart-contracts)

---

*Last updated: 2026-06-27*
