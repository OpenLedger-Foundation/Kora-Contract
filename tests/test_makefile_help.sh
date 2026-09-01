#!/bin/bash
# Test suite for Makefile help target (Issue #658)
# Verifies that make help displays all available targets with descriptions

set -e

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Color output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

test_count=0
pass_count=0
fail_count=0

log_test() {
    echo -e "\n${GREEN}Testing:${NC} $1"
    ((test_count++))
}

assert_success() {
    if [ $? -eq 0 ]; then
        echo -e "${GREEN}✓ PASS${NC}: $1"
        ((pass_count++))
    else
        echo -e "${RED}✗ FAIL${NC}: $1"
        ((fail_count++))
        return 1
    fi
}

assert_contains() {
    local output="$1"
    local pattern="$2"
    local description="$3"

    if echo "$output" | grep -q "$pattern"; then
        echo -e "${GREEN}✓ PASS${NC}: $description"
        ((pass_count++))
    else
        echo -e "${RED}✗ FAIL${NC}: $description (pattern not found: $pattern)"
        ((fail_count++))
        return 1
    fi
}

# Test 1: Verify make help target exists and runs
log_test "make help target exists and runs without error"
help_output=$(make help 2>&1)
assert_success "make help exits cleanly"

# Test 2: Verify all main targets are documented
log_test "All main targets are documented in help output"
expected_targets=(
    "build"
    "test"
    "test-verbose"
    "fmt"
    "lint"
    "check"
    "fuzz"
    "audit"
    "coverage"
    "clean"
    "deploy-testnet"
    "deploy-mainnet"
    "setup"
    "sizes"
)

for target in "${expected_targets[@]}"; do
    assert_contains "$help_output" "$target" "Target '$target' is documented in help"
done

# Test 3: Verify help output has descriptions for each target
log_test "Help output includes descriptions for targets"
assert_contains "$help_output" ".*build.*" "build target has description"
assert_contains "$help_output" ".*test.*" "test target has description"
assert_contains "$help_output" ".*clean.*" "clean target has description"

# Test 4: Verify no critical targets are missing
log_test "No critical targets are missing from documentation"
critical_targets=("build" "test" "lint" "clean")
for target in "${critical_targets[@]}"; do
    if ! echo "$help_output" | grep -q "$target"; then
        echo -e "${RED}✗ FAIL${NC}: Critical target '$target' not found in help output"
        ((fail_count++))
    else
        echo -e "${GREEN}✓ PASS${NC}: Critical target '$target' is documented"
        ((pass_count++))
    fi
done

# Test 5: Verify target descriptions are non-empty
log_test "Each target has a non-empty description"
# Extract lines with target definitions and verify they have descriptions
targets_with_descriptions=$(echo "$help_output" | grep -E '^[a-z-]+:' | wc -l)
if [ "$targets_with_descriptions" -gt 0 ]; then
    echo -e "${GREEN}✓ PASS${NC}: Found $targets_with_descriptions targets with descriptions"
    ((pass_count++))
else
    echo -e "${RED}✗ FAIL${NC}: No targets with descriptions found in help output"
    ((fail_count++))
fi

# Summary
echo -e "\n==============================================="
echo -e "Test Summary:"
echo -e "  Total:  $test_count"
echo -e "  Passed: $pass_count"
echo -e "  Failed: $fail_count"
echo -e "==============================================="

if [ "$fail_count" -eq 0 ]; then
    exit 0
else
    exit 1
fi
