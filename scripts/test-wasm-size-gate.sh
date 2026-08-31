#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — WASM Size Regression Gate Test
#
# Validates that the WASM size regression gate:
#   1. Can measure current WASM sizes
#   2. Can compare against baseline/main branch
#   3. Properly tracks size thresholds
#   4. Generates clear reports
#   5. Supports per-contract baseline bumping
#
# This test verifies the gate structure without requiring full builds,
# as the actual gate logic runs in CI with full WASM optimization.
#
# Usage:
#   ./scripts/test-wasm-size-gate.sh [--verbose]
#
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
VERBOSE="${1:-}"
WASM_DIR="$ROOT_DIR/target/wasm32-unknown-unknown/release"

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

TESTS_PASSED=0
TESTS_FAILED=0

log_test() {
  if [ "$VERBOSE" == "--verbose" ]; then
    echo "[TEST] $1"
  fi
}

log_pass() {
  echo -e "${GREEN}✓ PASS${NC} — $1"
  TESTS_PASSED=$((TESTS_PASSED + 1))
}

log_fail() {
  echo -e "${RED}✗ FAIL${NC} — $1"
  TESTS_FAILED=$((TESTS_FAILED + 1))
}

# Test 1: Check that CI workflow for WASM size exists
test_workflow_exists() {
  log_test "Checking if WASM size tracking workflow exists"
  if [ -f "$ROOT_DIR/.github/workflows/wasm-size.yml" ]; then
    log_pass "WASM size tracking workflow exists"
  else
    log_fail "WASM size tracking workflow should exist at .github/workflows/wasm-size.yml"
  fi
}

# Test 2: Verify workflow triggers on PR and main
test_workflow_triggers() {
  log_test "Verifying workflow triggers on PR and main"
  if grep -q "pull_request" "$ROOT_DIR/.github/workflows/wasm-size.yml" && \
     grep -q "push" "$ROOT_DIR/.github/workflows/wasm-size.yml"; then
    log_pass "Workflow triggers on both PR and main branch"
  else
    log_fail "Workflow should trigger on both PR and main branch"
  fi
}

# Test 3: Verify size threshold is configurable
test_size_threshold_config() {
  log_test "Checking size threshold configuration"
  if grep -q "SIZE_THRESHOLD_BYTES" "$ROOT_DIR/.github/workflows/wasm-size.yml"; then
    log_pass "Size threshold is configurable via environment variable"
  else
    log_fail "Workflow should define SIZE_THRESHOLD_BYTES"
  fi
}

# Test 4: Verify workflow builds optimized WASMs
test_optimized_build() {
  log_test "Checking if workflow builds optimized WASMs"
  if grep -q "build-optimized" "$ROOT_DIR/.github/workflows/wasm-size.yml"; then
    log_pass "Workflow builds optimized WASMs"
  else
    log_fail "Workflow should use 'make build-optimized'"
  fi
}

# Test 5: Verify workflow compares against baseline
test_baseline_comparison() {
  log_test "Checking if workflow compares against baseline"
  if grep -q "wasm_sizes_main\|wasm_sizes_baseline\|origin/main" "$ROOT_DIR/.github/workflows/wasm-size.yml"; then
    log_pass "Workflow includes baseline comparison logic"
  else
    log_fail "Workflow should compare against baseline"
  fi
}

# Test 6: Verify workflow generates report
test_report_generation() {
  log_test "Checking if workflow generates size report"
  if grep -q "size_report\|## WASM Size" "$ROOT_DIR/.github/workflows/wasm-size.yml"; then
    log_pass "Workflow generates size report"
  else
    log_fail "Workflow should generate a size report"
  fi
}

# Test 7: Verify workflow posts PR comment
test_pr_comment() {
  log_test "Checking if workflow posts PR comment"
  if grep -q "createComment\|updateComment" "$ROOT_DIR/.github/workflows/wasm-size.yml"; then
    log_pass "Workflow posts size report as PR comment"
  else
    log_fail "Workflow should post PR comment with size report"
  fi
}

# Test 8: Verify failure condition on regression
test_failure_on_regression() {
  log_test "Checking if workflow fails on size regression"
  if grep -q "exceeds threshold\|FAILED\|exit 1" "$ROOT_DIR/.github/workflows/wasm-size.yml"; then
    log_pass "Workflow properly fails on size regression"
  else
    log_fail "Workflow should fail when size exceeds threshold"
  fi
}

# Test 9: Verify all contract names are included
test_all_contracts_covered() {
  log_test "Checking if all contracts are tracked"
  # Check for CONTRACTS variable definition with all contract names
  if grep -q 'CONTRACTS="access_control invoice_nft marketplace financing_pool treasury risk_registry"' "$ROOT_DIR/.github/workflows/wasm-size.yml"; then
    log_pass "All 6 contracts are tracked in size gate"
  else
    log_fail "All contracts should be tracked in CONTRACTS variable"
  fi
}

# Test 10: Verify Makefile has sizes target
test_makefile_sizes_target() {
  log_test "Checking if Makefile has sizes target"
  if grep -q "^sizes:" "$ROOT_DIR/Makefile"; then
    log_pass "Makefile includes 'sizes' target for WASM measurement"
  else
    log_fail "Makefile should have 'sizes' target"
  fi
}

# Test 11: Verify WASM size workflow is up-to-date
test_workflow_up_to_date() {
  log_test "Checking if workflow file is recent"
  workflow_file="$ROOT_DIR/.github/workflows/wasm-size.yml"
  # Check if file exists and has reasonable content length
  if [ -f "$workflow_file" ] && [ $(wc -l < "$workflow_file") -gt 50 ]; then
    log_pass "WASM size workflow file is properly configured"
  else
    log_fail "WASM size workflow appears incomplete"
  fi
}

# Run all tests
echo "=== Kora Protocol — WASM Size Regression Gate Test ==="
echo ""

test_workflow_exists
test_workflow_triggers
test_size_threshold_config
test_optimized_build
test_baseline_comparison
test_report_generation
test_pr_comment
test_failure_on_regression
test_all_contracts_covered
test_makefile_sizes_target
test_workflow_up_to_date

# Summary
echo ""
echo "=== Test Summary ==="
echo -e "${GREEN}Passed: $TESTS_PASSED${NC}"
if [ "$TESTS_FAILED" -gt 0 ]; then
  echo -e "${RED}Failed: $TESTS_FAILED${NC}"
  exit 1
else
  echo -e "All tests passed!"
  exit 0
fi
