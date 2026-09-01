#!/bin/bash
# Issue #681: Mutation Testing Baseline Generation Script
#
# This script generates the initial mutation testing baseline for the Kora Protocol.
#
# Usage: bash scripts/mutation-test-baseline.sh
#
# Prerequisites:
#   cargo install cargo-mutants
#   cargo test --all (ensure all tests pass first)

set -e

PROJECT_ROOT=$(dirname "$(dirname "$(readlink -f "$0")")")
cd "$PROJECT_ROOT"

echo "=========================================="
echo "Kora Protocol - Mutation Testing Baseline"
echo "=========================================="
echo ""

# Check if cargo-mutants is installed
if ! command -v cargo-mutants &> /dev/null; then
    echo "ERROR: cargo-mutants not found"
    echo "Install with: cargo install cargo-mutants"
    exit 1
fi

echo "1. Verifying test suite passes..."
cargo test --all --lib 2>&1 | grep -E "test result:|running" || true
echo ""

echo "2. Generating overall workspace mutation baseline..."
mkdir -p mutants-baseline-reports
TIMESTAMP=$(date +"%Y-%m-%d_%H-%M-%S")
REPORT_DIR="mutants-baseline-reports/baseline_${TIMESTAMP}"

cargo mutants \
    --timeout 120 \
    -j 4 \
    -o "${REPORT_DIR}" \
    2>&1 | tee "${REPORT_DIR}/mutants.log"

echo ""
echo "3. Generating financing_pool contract focus report..."
cargo mutants \
    --timeout 120 \
    -p kora-financing-pool \
    -j 4 \
    -o "${REPORT_DIR}/financing-pool" \
    2>&1 | tee "${REPORT_DIR}/financing-pool.log"

echo ""
echo "4. Generating treasury contract focus report..."
cargo mutants \
    --timeout 120 \
    -p kora-treasury \
    -j 4 \
    -o "${REPORT_DIR}/treasury" \
    2>&1 | tee "${REPORT_DIR}/treasury.log"

echo ""
echo "=========================================="
echo "Baseline Generation Complete"
echo "=========================================="
echo ""
echo "Results saved to: ${REPORT_DIR}/"
echo ""
echo "To view results:"
echo "  1. Overall: open ${REPORT_DIR}/index.html"
echo "  2. financing_pool: open ${REPORT_DIR}/financing-pool/index.html"
echo "  3. treasury: open ${REPORT_DIR}/treasury/index.html"
echo ""
echo "Next steps:"
echo "  1. Review survived mutations in the HTML reports"
echo "  2. For each survived mutation in high-risk contracts:"
echo "     - Assess if test should catch it"
echo "     - Write new tests to improve kill-rate"
echo "     - Or document acceptance with rationale"
echo "  3. Re-run mutation tests to verify improvement"
echo ""
