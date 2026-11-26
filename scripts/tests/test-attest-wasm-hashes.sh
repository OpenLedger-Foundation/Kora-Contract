#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Reproducible Build Attestation Fixture Tests (#1112)
#
# Runs scripts/attest-wasm-hashes.sh against fake WASM files so the
# match / mismatch / missing paths are covered without a Rust build.
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ATTEST="$ROOT_DIR/scripts/attest-wasm-hashes.sh"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi; }

REL="target/wasm32-unknown-unknown/release"
mkdir -p "$WORK/$REL" "$WORK/releases"
printf 'fake-wasm-access-control' > "$WORK/$REL/kora_access_control.wasm"
printf 'fake-wasm-treasury'       > "$WORK/$REL/kora_treasury.wasm"

write_manifest() {
  {
    echo "$(sha "$WORK/$REL/kora_access_control.wasm")  $REL/kora_access_control.wasm"
    echo "$(sha "$WORK/$REL/kora_treasury.wasm")  $REL/kora_treasury.wasm"
  } > "$WORK/releases/v9.9.9.hashes"
}

run() {
  bash "$ATTEST" v9.9.9 --root "$WORK" --out "$WORK/out" --commit deadbeef > "$WORK/log.txt" 2>&1
}

echo "--- Test 1: matching hashes write attestation ---"
write_manifest
run
grep -q '"result": "match"' "$WORK/out/v9.9.9.attestation.json"
grep -q '"commit": "deadbeef"' "$WORK/out/v9.9.9.attestation.json"
grep -q "kora_treasury.wasm" "$WORK/out/v9.9.9.attestation.json"
cmp -s "$WORK/releases/v9.9.9.hashes" "$WORK/out/v9.9.9.hashes"
echo "✓ Attestation written on match."

echo ""
echo "--- Test 2: mismatch fails and writes no attestation ---"
rm -rf "$WORK/out"
printf 'tampered' > "$WORK/$REL/kora_treasury.wasm"
if run; then echo "FAIL: mismatch must fail"; cat "$WORK/log.txt"; exit 1; fi
grep -q "MISMATCH" "$WORK/log.txt"
[ ! -e "$WORK/out/v9.9.9.attestation.json" ] || { echo "FAIL: attestation written on mismatch"; exit 1; }
echo "✓ Mismatch fails the job."

echo ""
echo "--- Test 3: missing binary fails (no silent SKIP) ---"
printf 'fake-wasm-treasury' > "$WORK/$REL/kora_treasury.wasm"
write_manifest
rm "$WORK/$REL/kora_access_control.wasm"
if run; then echo "FAIL: missing binary must fail"; exit 1; fi
grep -q "MISSING" "$WORK/log.txt"
echo "✓ Missing binary fails."

echo ""
echo "--- Test 4: missing or empty manifest fails ---"
rm "$WORK/releases/v9.9.9.hashes"
if run; then echo "FAIL: missing manifest must fail"; exit 1; fi
: > "$WORK/releases/v9.9.9.hashes"
if run; then echo "FAIL: empty manifest must fail"; exit 1; fi
echo "✓ Missing/empty manifest fails."

echo ""
echo "--- Test 5: malformed manifest line fails ---"
printf 'fake-wasm-access-control' > "$WORK/$REL/kora_access_control.wasm"
write_manifest
echo "not-a-hash  $REL/kora_extra.wasm" >> "$WORK/releases/v9.9.9.hashes"
if run; then echo "FAIL: malformed line must fail"; exit 1; fi
echo "✓ Malformed line fails."

echo ""
echo "=== Reproducible Build Attestation Tests Passed! ==="
