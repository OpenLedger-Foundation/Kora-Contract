#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — WASM Size Budget Checker (#795)
# Checks built WASMs against per-contract budgets in wasm-budgets.json.
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

BUDGET_FILE="${1:-$ROOT_DIR/wasm-budgets.json}"
WASM_DIR="${2:-$ROOT_DIR/target/wasm32-unknown-unknown/release}"
PR_LABELS="${3:-}"

echo "=== Kora Protocol — WASM Size Budget Checker ==="

if [ ! -f "$BUDGET_FILE" ]; then
  echo "ERROR: Budget configuration file not found: $BUDGET_FILE"
  exit 1
fi

CONTRACTS="access_control invoice_nft risk_registry treasury financing_pool marketplace price_oracle tranche dispute_resolution dispute_escrow governance governance_voting parameter_registry secondary_market"
HAS_OVERAGE=0
HAS_MISSING=0

echo "| Contract | Size (bytes) | Budget (bytes) | Status |"
echo "|---|---|---|---|"

for c in $CONTRACTS; do
  wasm="$WASM_DIR/kora_${c}.optimized.wasm"
  [ -f "$wasm" ] || wasm="$WASM_DIR/kora_${c}.wasm"

  if [ ! -f "$wasm" ]; then
    if [ "${REQUIRE_ALL_WASMS:-0}" = "1" ]; then
      echo "ERROR: Missing release WASM for kora_${c}"
      HAS_MISSING=1
    fi
    continue
  fi

  sz=$(wc -c < "$wasm" | tr -d ' \n\r')

  budget=$(python3 -c "import json; d=json.load(open('$BUDGET_FILE')); print(d['budgets'].get('$c', d.get('default_budget_bytes', 122880)))")

  if [ "$sz" -gt "$budget" ]; then
    overage=$((sz - budget))
    echo "| \`kora_${c}\` | $sz B | $budget B | ❌ OVER BUDGET (+${overage} B) |"
    HAS_OVERAGE=1
  else
    under=$((budget - sz))
    echo "| \`kora_${c}\` | $sz B | $budget B | ✓ OK (${under} B under) |"
  fi
done

if [ "$HAS_MISSING" -eq 1 ]; then
  echo "FAILED: Required release artifacts are missing."
  exit 1
fi

if [ "$HAS_OVERAGE" -eq 1 ]; then
  echo ""
  echo "Checking WASM size increase approval labels..."
  if echo "$PR_LABELS" | grep -q "wasm-size-increase-approved"; then
    echo "✓ OVERRIDE APPROVED: Label 'wasm-size-increase-approved' present."
    exit 0
  else
    echo "❌ FAILED: One or more contracts exceeded their WASM size budget."
    echo "To request a budget increase, apply label 'wasm-size-increase-approved' to PR and update wasm-budgets.json."
    exit 1
  fi
fi

echo ""
echo "✓ All contracts within WASM size budgets."
exit 0
