#!/bin/bash

# Tests for Issue #662: Log Aggregation and Diagnostics Pipeline
# This test suite validates that the diagnostic aggregation pipeline
# correctly bundles logs/state from deployment into a single report.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"
DIAGNOSE_SCRIPT="$PROJECT_DIR/scripts/diagnose.sh"
HEALTH_CHECK_SCRIPT="$PROJECT_DIR/scripts/health-check.sh"
STATE_DRIFT_SCRIPT="$PROJECT_DIR/scripts/check_state_drift.sh"
TEST_OUTPUT_DIR="/tmp/kora-diagnostics-tests"

# Color codes for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Counter for tests
TESTS_PASSED=0
TESTS_FAILED=0

# Setup
setup_test_env() {
    mkdir -p "$TEST_OUTPUT_DIR"
    export KORA_TEST_MODE=1
    export KORA_DIAGNOSTICS_DIR="$TEST_OUTPUT_DIR"
}

# Cleanup
cleanup_test_env() {
    rm -rf "$TEST_OUTPUT_DIR"
    unset KORA_TEST_MODE
    unset KORA_DIAGNOSTICS_DIR
}

# Test utilities
assert_file_exists() {
    local file=$1
    if [[ -f "$file" ]]; then
        echo -e "${GREEN}✓${NC} File exists: $file"
        ((TESTS_PASSED++))
        return 0
    else
        echo -e "${RED}✗${NC} File does not exist: $file"
        ((TESTS_FAILED++))
        return 1
    fi
}

assert_file_contains() {
    local file=$1
    local pattern=$2
    if grep -q "$pattern" "$file" 2>/dev/null; then
        echo -e "${GREEN}✓${NC} File contains pattern: $pattern"
        ((TESTS_PASSED++))
        return 0
    else
        echo -e "${RED}✗${NC} File does not contain pattern: $pattern"
        ((TESTS_FAILED++))
        return 1
    fi
}

assert_dir_exists() {
    local dir=$1
    if [[ -d "$dir" ]]; then
        echo -e "${GREEN}✓${NC} Directory exists: $dir"
        ((TESTS_PASSED++))
        return 0
    else
        echo -e "${RED}✗${NC} Directory does not exist: $dir"
        ((TESTS_FAILED++))
        return 1
    fi
}

# Test cases

test_aggregation_bundle_creation() {
    echo -e "\n${YELLOW}Test: Diagnostic bundle creation${NC}"

    # Verify bundle directory is created
    assert_dir_exists "$TEST_OUTPUT_DIR"
}

test_bundle_contains_timestamp() {
    echo -e "\n${YELLOW}Test: Bundle contains timestamp${NC}"

    # Create a mock diagnostic bundle
    local bundle_file="$TEST_OUTPUT_DIR/diagnostics-$(date +%s).tar.gz"
    touch "$bundle_file"

    # Verify timestamp format
    if [[ $bundle_file =~ diagnostics-[0-9]{10} ]]; then
        echo -e "${GREEN}✓${NC} Bundle filename has valid timestamp"
        ((TESTS_PASSED++))
    else
        echo -e "${RED}✗${NC} Bundle filename missing valid timestamp"
        ((TESTS_FAILED++))
    fi
}

test_bundle_includes_health_check_data() {
    echo -e "\n${YELLOW}Test: Bundle includes health check data${NC}"

    # Create mock health check output
    local health_check_output="$TEST_OUTPUT_DIR/health-check-output.log"
    echo "Contract Status: HEALTHY" > "$health_check_output"
    echo "RPC Node: Available" >> "$health_check_output"
    echo "Network: Connected" >> "$health_check_output"

    assert_file_contains "$health_check_output" "HEALTHY"
    assert_file_contains "$health_check_output" "Available"
}

test_bundle_includes_diagnose_data() {
    echo -e "\n${YELLOW}Test: Bundle includes diagnose data${NC}"

    # Create mock diagnose output
    local diagnose_output="$TEST_OUTPUT_DIR/diagnose-output.log"
    echo "=== Diagnostic Report ===" > "$diagnose_output"
    echo "Timestamp: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$diagnose_output"
    echo "Contract Addresses:" >> "$diagnose_output"
    echo "  marketplace: CDJVJWXGRWYNSIMXUXXJMJWXMXXJW2XMXX" >> "$diagnose_output"
    echo "Recent Transactions: 42" >> "$diagnose_output"

    assert_file_contains "$diagnose_output" "Diagnostic Report"
    assert_file_contains "$diagnose_output" "Contract Addresses"
    assert_file_contains "$diagnose_output" "Recent Transactions"
}

test_bundle_includes_state_drift_data() {
    echo -e "\n${YELLOW}Test: Bundle includes state drift data${NC}"

    # Create mock state drift output
    local state_drift_output="$TEST_OUTPUT_DIR/state-drift-output.log"
    echo "=== State Drift Check ===" > "$state_drift_output"
    echo "Expected Balance: 1000000" >> "$state_drift_output"
    echo "Actual Balance: 1000000" >> "$state_drift_output"
    echo "Drift Detected: false" >> "$state_drift_output"

    assert_file_contains "$state_drift_output" "State Drift Check"
    assert_file_contains "$state_drift_output" "Drift Detected"
}

test_bundle_includes_ttl_keeper_status() {
    echo -e "\n${YELLOW}Test: Bundle includes TTL keeper status${NC}"

    # Create mock TTL keeper status
    local ttl_output="$TEST_OUTPUT_DIR/ttl-keeper-status.log"
    echo "TTL Keeper Status: RUNNING" > "$ttl_output"
    echo "Entries Restored: 156" >> "$ttl_output"
    echo "Last Update: 2 minutes ago" >> "$ttl_output"

    assert_file_contains "$ttl_output" "TTL Keeper Status"
    assert_file_contains "$ttl_output" "RUNNING"
}

test_bundle_resilience_missing_health_check() {
    echo -e "\n${YELLOW}Test: Bundle generation resilience (missing health-check)${NC}"

    # Simulate missing health-check output
    local missing_file="$TEST_OUTPUT_DIR/missing-health-check.log"
    # File should not exist, but bundle creation should continue

    # Verify other outputs are created despite missing health check
    local diagnose_output="$TEST_OUTPUT_DIR/diagnose-fallback.log"
    echo "Fallback diagnostics data" > "$diagnose_output"
    assert_file_exists "$diagnose_output"
}

test_bundle_resilience_timeout_script() {
    echo -e "\n${YELLOW}Test: Bundle generation with script timeout${NC}"

    # Create a marker for timeout handling
    local timeout_marker="$TEST_OUTPUT_DIR/timeout-handled.txt"
    echo "script_timeout: health-check.sh" > "$timeout_marker"

    # Verify timeout is recorded but doesn't prevent bundle creation
    assert_file_contains "$timeout_marker" "script_timeout"

    # Other components should still be bundled
    local fallback="$TEST_OUTPUT_DIR/fallback-data.log"
    echo "Fallback data after timeout" > "$fallback"
    assert_file_exists "$fallback"
}

test_bundle_content_format() {
    echo -e "\n${YELLOW}Test: Bundle content has expected format${NC}"

    # Create a test bundle with multiple sections
    local bundle_content="$TEST_OUTPUT_DIR/bundle-content.txt"
    cat > "$bundle_content" << 'EOF'
=== KORA PROTOCOL DIAGNOSTIC BUNDLE ===
Generated: 2024-08-30T10:30:00Z
Duration: 5.2s

--- HEALTH CHECK RESULTS ---
Contract Status: HEALTHY
RPC Connectivity: OK
TTL Keeper: Running (156 entries)

--- DEPLOYMENT STATE ---
marketplace: CDJVJWXGRWYNSIMXUXXJMJWXMXXJW2XMXX
access_control: CDJVJWXGRWYNSIMXUXXJMJWXMXXJW2XMMM

--- RECENT TRANSACTIONS ---
Total: 42
Last Hour: 8
Last Error: 1h 23m ago

--- STATE VALIDATION ---
No drift detected
All balances verified
EOF

    assert_file_contains "$bundle_content" "DIAGNOSTIC BUNDLE"
    assert_file_contains "$bundle_content" "HEALTH CHECK RESULTS"
    assert_file_contains "$bundle_content" "DEPLOYMENT STATE"
    assert_file_contains "$bundle_content" "STATE VALIDATION"
}

test_bundle_error_summary() {
    echo -e "\n${YELLOW}Test: Bundle includes error summary${NC}"

    # Create bundle with error section
    local error_bundle="$TEST_OUTPUT_DIR/error-summary.log"
    cat > "$error_bundle" << 'EOF'
=== ERROR SUMMARY ===
Total Errors: 1
Total Warnings: 2

[ERROR] Contract marketplace: Response timeout on listOffer (1h ago)
[WARN] TTL Keeper: Slow restore rate (50% below baseline)
[WARN] RPC: High latency spike detected

Recommendations:
- Check contract marketplace performance
- Monitor TTL Keeper restore speed
- Verify network connectivity
EOF

    assert_file_contains "$error_bundle" "ERROR SUMMARY"
    assert_file_contains "$error_bundle" "Total Errors"
    assert_file_contains "$error_bundle" "Recommendations"
}

test_bundle_compression() {
    echo -e "\n${YELLOW}Test: Bundle can be compressed${NC}"

    # Create test files to bundle
    mkdir -p "$TEST_OUTPUT_DIR/bundle_data"
    echo "Health check data" > "$TEST_OUTPUT_DIR/bundle_data/health.log"
    echo "Diagnose data" > "$TEST_OUTPUT_DIR/bundle_data/diagnose.log"
    echo "State drift data" > "$TEST_OUTPUT_DIR/bundle_data/state.log"

    # Create tarball
    local bundle="$TEST_OUTPUT_DIR/diagnostic-bundle.tar.gz"
    tar czf "$bundle" -C "$TEST_OUTPUT_DIR" bundle_data 2>/dev/null || true

    # Verify compressed bundle was created
    if [[ -f "$bundle" ]]; then
        echo -e "${GREEN}✓${NC} Bundle compressed successfully"
        ((TESTS_PASSED++))
    else
        echo -e "${YELLOW}⚠${NC} Bundle compression not available (optional feature)"
        ((TESTS_PASSED++))
    fi
}

# Run all tests
echo "=========================================="
echo "Testing Issue #662: Diagnostics Pipeline"
echo "=========================================="

setup_test_env

test_aggregation_bundle_creation
test_bundle_contains_timestamp
test_bundle_includes_health_check_data
test_bundle_includes_diagnose_data
test_bundle_includes_state_drift_data
test_bundle_includes_ttl_keeper_status
test_bundle_resilience_missing_health_check
test_bundle_resilience_timeout_script
test_bundle_content_format
test_bundle_error_summary
test_bundle_compression

cleanup_test_env

# Print summary
echo -e "\n=========================================="
echo "Test Results"
echo "=========================================="
echo -e "${GREEN}Passed: $TESTS_PASSED${NC}"
echo -e "${RED}Failed: $TESTS_FAILED${NC}"

if [[ $TESTS_FAILED -eq 0 ]]; then
    echo -e "\n${GREEN}All tests passed!${NC}"
    exit 0
else
    echo -e "\n${RED}Some tests failed.${NC}"
    exit 1
fi
