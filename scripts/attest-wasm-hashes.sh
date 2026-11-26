#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Reproducible Build Hash Comparison & Attestation (#1112)
# =============================================================================
#
# Usage:
#   ./scripts/attest-wasm-hashes.sh <version> [--root <dir>] [--manifest <file>]
#                                   [--out <dir>] [--commit <sha>]
#
# Compares freshly built WASM binaries against the committed hash manifest
# (releases/<version>.hashes, `sha256sum` format) and, ONLY if every entry
# matches, writes an attestation report:
#
#   <out>/<version>.attestation.json   machine-readable report
#   <out>/<version>.hashes             copy of the verified manifest
#
# Unlike `record-wasm-hashes.sh --verify`, this is strict: a missing binary,
# a malformed line, an empty manifest, or any mismatch fails with exit 1 and
# no attestation is written.
#
# Options:
#   --root <dir>       Directory manifest paths are relative to (default: repo root)
#   --manifest <file>  Manifest path (default: <root>/releases/<version>.hashes)
#   --out <dir>        Output directory (default: <root>/attestation)
#   --commit <sha>     Commit recorded in the report (default: git rev-parse HEAD)
#
# Exit codes: 0 all match (attestation written), 1 mismatch/missing/invalid.
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

VERSION="${1:-}"
[ $# -gt 0 ] && shift
MANIFEST=""
OUT_DIR=""
COMMIT=""

while [ $# -gt 0 ]; do
  case "$1" in
    --root)     ROOT_DIR="$(cd "$2" && pwd)"; shift 2 ;;
    --manifest) MANIFEST="$2"; shift 2 ;;
    --out)      OUT_DIR="$2"; shift 2 ;;
    --commit)   COMMIT="$2"; shift 2 ;;
    *) echo "ERROR: Unknown argument: $1"; exit 1 ;;
  esac
done

if [ -z "$VERSION" ]; then
  echo "ERROR: Version required. Usage: $0 <version> [--root dir] [--manifest file] [--out dir]"
  exit 1
fi
if [[ ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "ERROR: Version must be in format vX.Y.Z, got: $VERSION"
  exit 1
fi

MANIFEST="${MANIFEST:-$ROOT_DIR/releases/$VERSION.hashes}"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/attestation}"
if [ -z "$COMMIT" ]; then
  COMMIT="$(git -C "$ROOT_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
fi

if [ ! -f "$MANIFEST" ]; then
  echo "ERROR: No committed hash manifest for $VERSION at $MANIFEST"
  echo "       Record it with: ./scripts/record-wasm-hashes.sh $VERSION"
  exit 1
fi

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

echo "=== Kora Protocol — Reproducible Build Attestation ==="
echo "Version  : $VERSION"
echo "Commit   : $COMMIT"
echo "Manifest : $MANIFEST"
echo ""

PASS=true
CHECKED=0
ENTRIES_JSON=""

while IFS= read -r line || [ -n "$line" ]; do
  # Skip blank lines and comments.
  [[ -z "${line//[[:space:]]/}" || "$line" =~ ^[[:space:]]*# ]] && continue

  if [[ ! "$line" =~ ^([0-9a-f]{64})[[:space:]]+\*?(.+)$ ]]; then
    echo "  ✗ malformed manifest line: $line"
    PASS=false
    continue
  fi
  expected="${BASH_REMATCH[1]}"
  rel_path="${BASH_REMATCH[2]}"
  wasm="$ROOT_DIR/$rel_path"
  name="$(basename "$rel_path")"
  CHECKED=$((CHECKED + 1))

  if [ ! -f "$wasm" ]; then
    echo "  ✗ $name: MISSING (expected at $rel_path)"
    PASS=false
    continue
  fi

  actual="$(sha256_of "$wasm")"
  if [ "$actual" = "$expected" ]; then
    echo "  ✓ $name: $actual"
    [ -n "$ENTRIES_JSON" ] && ENTRIES_JSON="$ENTRIES_JSON,"
    ENTRIES_JSON="$ENTRIES_JSON
    {\"file\": \"$rel_path\", \"sha256\": \"$actual\"}"
  else
    echo "  ✗ $name: MISMATCH"
    echo "      Expected (committed): $expected"
    echo "      Got      (rebuilt) : $actual"
    PASS=false
  fi
done < "$MANIFEST"

echo ""
if [ "$CHECKED" -eq 0 ]; then
  echo "✗ FAIL: Manifest $MANIFEST contains no hash entries."
  exit 1
fi

if [ "$PASS" != "true" ]; then
  echo "✗ FAIL: Rebuilt WASM does not match the committed hashes — no attestation written."
  echo ""
  echo "Investigate (see docs/REPRODUCIBLE_BUILD.md#what-to-do-if-hashes-dont-match):"
  echo "  1. Toolchain: rustup show active-toolchain (must match rust-toolchain.toml)"
  echo "  2. Build env: SOURCE_DATE_EPOCH=0 CARGO_INCREMENTAL=0, clean target/"
  echo "  3. Platform : canonical hashes are produced on ubuntu-22.04 x86_64"
  echo "  4. Source   : git status / git rev-parse HEAD vs the release tag"
  exit 1
fi

RUSTC_VERSION="$(rustc --version 2>/dev/null || echo 'unknown')"
PLATFORM="$(uname -s 2>/dev/null || echo unknown)-$(uname -m 2>/dev/null || echo unknown)"
RUNNER_IMAGE="${ImageOS:-local}${ImageVersion:+@$ImageVersion}"
GENERATED_AT="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
MANIFEST_SHA="$(sha256_of "$MANIFEST")"

mkdir -p "$OUT_DIR"
cp "$MANIFEST" "$OUT_DIR/$VERSION.hashes"
REPORT="$OUT_DIR/$VERSION.attestation.json"

cat > "$REPORT" <<EOF
{
  "schema": "kora.reproducible-build.attestation/v1",
  "version": "$VERSION",
  "commit": "$COMMIT",
  "generated_at": "$GENERATED_AT",
  "result": "match",
  "toolchain": "$RUSTC_VERSION",
  "platform": "$PLATFORM",
  "runner_image": "$RUNNER_IMAGE",
  "build_env": {
    "SOURCE_DATE_EPOCH": "${SOURCE_DATE_EPOCH:-unset}",
    "CARGO_INCREMENTAL": "${CARGO_INCREMENTAL:-unset}"
  },
  "manifest": {"file": "releases/$VERSION.hashes", "sha256": "$MANIFEST_SHA"},
  "contracts": [$ENTRIES_JSON
  ]
}
EOF

echo "✓ PASS: $CHECKED contract(s) reproduced byte-identically."
echo "Attestation written to: $REPORT"
