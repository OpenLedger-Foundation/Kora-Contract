#!/usr/bin/env bash
# Tests for scripts/check-pinned-digests.sh (issue #1174).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHECK="$ROOT/scripts/check-pinned-digests.sh"; FX="$ROOT/scripts/tests/fixtures/dockerfiles"
pass=0; fail=0
expect() { local want="$1" needle="$2" desc="$3"; shift 3
  set +e; out="$(bash "$CHECK" "$@" 2>&1)"; rc=$?; set -e
  if [ "$rc" -eq "$want" ] && grep -q -- "$needle" <<<"$out"; then echo "ok  - $desc"; pass=$((pass+1))
  else echo "FAIL- $desc (rc=$rc) $out"; fail=$((fail+1)); fi; }
expect 0 "pinned by digest" "repo Dockerfiles are digest-pinned"
expect 0 "pinned by digest" "re-run is idempotent"
expect 0 "pinned by digest" "stage aliases and scratch are allowed" "$FX/pinned-multistage.Dockerfile"
expect 1 "ERR_UNPINNED_IMAGE" "floating tag is rejected" "$FX/unpinned.Dockerfile"
expect 1 "ERR_UNPINNED_IMAGE" "truncated digest is rejected" "$FX/short-digest.Dockerfile"
expect 2 "ERR_CONFIG_SCHEMA" "Dockerfile without FROM fails closed" "$FX/no-from.Dockerfile"
expect 2 "ERR_CONFIG_MISSING" "missing Dockerfile fails closed" "$FX/does-not-exist"
grep -q '^channel = "1.75.0"' "$ROOT/rust-toolchain.toml" && grep -q 'FROM rust:1.75.0-' "$ROOT/packaging/Dockerfile.wasm" \
  && { echo "ok  - WASM builder tag matches rust-toolchain.toml"; pass=$((pass+1)); } \
  || { echo "FAIL- WASM builder tag drifted from rust-toolchain.toml"; fail=$((fail+1)); }
echo "passed=$pass failed=$fail"; [ "$fail" -eq 0 ]
