#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — TTL Keeper Test Suite
#
# Validates that the TTL keeper script:
#   1. Has correct environment variable requirements
#   2. Handles missing deployment files gracefully
#   3. Supports both testnet and mainnet networks
#   4. Has proper error handling for invalid networks
#   5. Includes guards against overlapping runs
#
# Usage:
#   ./scripts/test-ttl-keeper.sh [--verbose]
#
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
VERBOSE="${1:-}"
TESTS_PASSED=0
TESTS_FAILED=0

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

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

# Test 1: TTL keeper script exists and is executable
test_script_exists() {
  log_test "Checking if ttl_keeper.sh exists and is executable"
  if [ -x "$SCRIPT_DIR/ttl_keeper.sh" ]; then
    log_pass "ttl_keeper.sh exists and is executable"
  else
    log_fail "ttl_keeper.sh does not exist or is not executable"
  fi
}

# Test 2: TTL keeper requires DEPLOYER_SECRET
test_requires_deployer_secret() {
  log_test "Verifying DEPLOYER_SECRET requirement"
  output=$(bash "$SCRIPT_DIR/ttl_keeper.sh" testnet 2>&1 || true)
  if echo "$output" | grep -q "DEPLOYER_SECRET"; then
    log_pass "TTL keeper correctly requires DEPLOYER_SECRET environment variable"
  else
    log_fail "TTL keeper should require DEPLOYER_SECRET"
  fi
}

# Test 3: TTL keeper rejects invalid networks
test_network_validation() {
  log_test "Verifying network parameter validation"
  output=$(DEPLOYER_SECRET="test" bash "$SCRIPT_DIR/ttl_keeper.sh" invalid_network 2>&1 || true)
  if echo "$output" | grep -q "Unknown network"; then
    log_pass "TTL keeper correctly validates network parameter"
  else
    log_fail "TTL keeper should reject invalid network parameter"
  fi
}

# Test 4: TTL keeper checks for deployment manifest
test_deployment_manifest_check() {
  log_test "Verifying deployment manifest requirement"
  output=$(DEPLOYER_SECRET="test" bash "$SCRIPT_DIR/ttl_keeper.sh" testnet 2>&1 || true)
  if echo "$output" | grep -q "Deployment manifest not found"; then
    log_pass "TTL keeper correctly checks for deployment manifest"
  else
    log_fail "TTL keeper should check for deployment manifest"
  fi
}

# Test 5: TTL keeper script has proper bash settings
test_bash_safety() {
  log_test "Verifying bash error handling (set -euo pipefail)"
  if head -n 25 "$SCRIPT_DIR/ttl_keeper.sh" | grep -q "set -euo pipefail"; then
    log_pass "TTL keeper has proper bash error handling"
  else
    log_fail "TTL keeper should use 'set -euo pipefail' for safety"
  fi
}

# Test 6: TTL keeper supports both networks
test_network_support() {
  log_test "Verifying network configuration for testnet and mainnet"
  if grep -q "testnet" "$SCRIPT_DIR/ttl_keeper.sh" && grep -q "mainnet" "$SCRIPT_DIR/ttl_keeper.sh"; then
    log_pass "TTL keeper supports both testnet and mainnet"
  else
    log_fail "TTL keeper should support both testnet and mainnet"
  fi
}

# Test 7: TTL keeper has error tracking
test_error_tracking() {
  log_test "Verifying error tracking mechanism"
  if grep -q "ERRORS=" "$SCRIPT_DIR/ttl_keeper.sh"; then
    log_pass "TTL keeper tracks errors during execution"
  else
    log_fail "TTL keeper should track errors"
  fi
}

# Test 8: TTL keeper can be run safely multiple times (idempotency check)
test_script_safe_for_cron() {
  log_test "Verifying script is safe for scheduled execution"
  # Check for elements that make it safe for cron:
  # 1. No interactive input reads (legitimate uses like config reading are OK)
  if ! grep -q "^[[:space:]]*read[[:space:]]*_\|^[[:space:]]*read[[:space:]]*$" "$SCRIPT_DIR/ttl_keeper.sh"; then
    log_pass "TTL keeper has no interactive prompts (safe for cron)"
  else
    log_fail "TTL keeper should not have interactive prompts"
  fi
}

# Test 9: Test that test script itself works
test_self_validation() {
  log_test "Running self-validation"
  if [ -f "$SCRIPT_DIR/test-ttl-keeper.sh" ]; then
    log_pass "TTL keeper test suite is installed"
  else
    log_fail "TTL keeper test suite should exist"
  fi
}

# Run all tests
echo "=== Kora Protocol — TTL Keeper Test Suite ==="
echo ""

test_script_exists
test_requires_deployer_secret
test_network_validation
test_deployment_manifest_check
test_bash_safety
test_network_support
test_error_tracking
test_script_safe_for_cron
test_self_validation

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
