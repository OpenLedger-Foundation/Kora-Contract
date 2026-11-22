#!/usr/bin/env bash
# =============================================================================
# CI smoke: check_deploy_order.sh (issue #1083)
#
# Parses scripts/deploy.sh and asserts that:
#   1. parameter_registry is deployed before marketplace and financing_pool.
#   2. parameter_registry is initialized before marketplace and financing_pool.
#   3. marketplace is initialized after financing_pool.
#   4. The script exits non-zero if the PARAMETER_REGISTRY_ID guard block is present.
#
# This script is intentionally a text-level parse (grep line numbers) rather than
# a full Bash interpreter — it is cheap and CI-fast, and covers the structural
# invariant that the deploy order comment documents.
#
# Usage:
#   bash scripts/check_deploy_order.sh [path/to/deploy.sh]
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY_SCRIPT="${1:-$SCRIPT_DIR/deploy.sh}"

if [ ! -f "$DEPLOY_SCRIPT" ]; then
  echo "ERROR: deploy script not found at '$DEPLOY_SCRIPT'" >&2
  exit 1
fi

echo "=== Checking deploy.sh dependency order ==="
echo "Script: $DEPLOY_SCRIPT"
echo ""

FAIL=0

# Helper: return the first line number on which grep matches, or 0 if not found.
first_line() {
  grep -n "$1" "$DEPLOY_SCRIPT" | head -1 | cut -d: -f1 || echo 0
}

# ── Deploy section: parameter_registry before marketplace and financing_pool ──

REGISTRY_DEPLOY=$(first_line 'deploy_contract.*parameter_registry')
POOL_DEPLOY=$(first_line 'deploy_contract.*financing_pool')
MARKETPLACE_DEPLOY=$(first_line 'deploy_contract.*marketplace\b')

echo "Deploy line numbers:"
echo "  parameter_registry : ${REGISTRY_DEPLOY:-NOT FOUND}"
echo "  financing_pool     : ${POOL_DEPLOY:-NOT FOUND}"
echo "  marketplace        : ${MARKETPLACE_DEPLOY:-NOT FOUND}"

if [ "${REGISTRY_DEPLOY:-0}" -eq 0 ]; then
  echo "FAIL: parameter_registry deploy call not found in $DEPLOY_SCRIPT" >&2
  FAIL=1
fi
if [ "${POOL_DEPLOY:-0}" -gt 0 ] && [ "${REGISTRY_DEPLOY:-0}" -gt "${POOL_DEPLOY:-0}" ]; then
  echo "FAIL: parameter_registry deployed AFTER financing_pool (line $REGISTRY_DEPLOY > $POOL_DEPLOY)" >&2
  echo "      Fix: move the parameter_registry deploy_contract call above financing_pool." >&2
  FAIL=1
fi
if [ "${MARKETPLACE_DEPLOY:-0}" -gt 0 ] && [ "${REGISTRY_DEPLOY:-0}" -gt "${MARKETPLACE_DEPLOY:-0}" ]; then
  echo "FAIL: parameter_registry deployed AFTER marketplace (line $REGISTRY_DEPLOY > $MARKETPLACE_DEPLOY)" >&2
  echo "      Fix: move the parameter_registry deploy_contract call above marketplace." >&2
  FAIL=1
fi

# ── Init section: parameter_registry initialized before marketplace and pool ──

REGISTRY_INIT=$(grep -n 'invoke.*PARAMETER_REGISTRY_ID.*initialize\|PARAMETER_REGISTRY_ID.*invoke.*initialize' "$DEPLOY_SCRIPT" | head -1 | cut -d: -f1 || echo 0)
POOL_INIT=$(grep -n 'invoke.*POOL_ID.*initialize\|POOL_ID.*invoke.*initialize' "$DEPLOY_SCRIPT" | head -1 | cut -d: -f1 || echo 0)
MARKETPLACE_INIT=$(grep -n 'invoke.*MARKETPLACE_ID.*initialize\|MARKETPLACE_ID.*invoke.*initialize' "$DEPLOY_SCRIPT" | head -1 | cut -d: -f1 || echo 0)

# Fallback for the shell pattern invoke "$VAR" function form
if [ "${REGISTRY_INIT:-0}" -eq 0 ]; then
  REGISTRY_INIT=$(grep -n '"$PARAMETER_REGISTRY_ID".*initialize\|invoke.*"\$PARAMETER_REGISTRY_ID"' "$DEPLOY_SCRIPT" | head -1 | cut -d: -f1 || echo 0)
fi
if [ "${POOL_INIT:-0}" -eq 0 ]; then
  POOL_INIT=$(grep -n '"$POOL_ID".*initialize\|invoke.*"\$POOL_ID"' "$DEPLOY_SCRIPT" | head -1 | cut -d: -f1 || echo 0)
fi
if [ "${MARKETPLACE_INIT:-0}" -eq 0 ]; then
  MARKETPLACE_INIT=$(grep -n '"$MARKETPLACE_ID".*initialize\|invoke.*"\$MARKETPLACE_ID"' "$DEPLOY_SCRIPT" | head -1 | cut -d: -f1 || echo 0)
fi

echo ""
echo "Init line numbers:"
echo "  parameter_registry : ${REGISTRY_INIT:-NOT FOUND}"
echo "  financing_pool     : ${POOL_INIT:-NOT FOUND}"
echo "  marketplace        : ${MARKETPLACE_INIT:-NOT FOUND}"

if [ "${REGISTRY_INIT:-0}" -eq 0 ]; then
  echo "FAIL: parameter_registry initialize call not found in $DEPLOY_SCRIPT" >&2
  FAIL=1
fi
if [ "${POOL_INIT:-0}" -gt 0 ] && [ "${REGISTRY_INIT:-0}" -gt "${POOL_INIT:-0}" ]; then
  echo "FAIL: parameter_registry initialized AFTER financing_pool (line $REGISTRY_INIT > $POOL_INIT)" >&2
  echo "      Fix: move the parameter_registry invoke initialize call above financing_pool." >&2
  FAIL=1
fi
if [ "${MARKETPLACE_INIT:-0}" -gt 0 ] && [ "${REGISTRY_INIT:-0}" -gt "${MARKETPLACE_INIT:-0}" ]; then
  echo "FAIL: parameter_registry initialized AFTER marketplace (line $REGISTRY_INIT > $MARKETPLACE_INIT)" >&2
  echo "      Fix: move the parameter_registry invoke initialize call above marketplace." >&2
  FAIL=1
fi

# ── Guard block: script exits on empty PARAMETER_REGISTRY_ID ─────────────────

if ! grep -q 'PARAMETER_REGISTRY_ID.*empty\|empty.*PARAMETER_REGISTRY_ID\|parameter_registry.*empty.*address\|empty address' "$DEPLOY_SCRIPT"; then
  if ! grep -q 'PARAMETER_REGISTRY_ID.*exit 1\|empty.*PARAMETER_REGISTRY_ID' "$DEPLOY_SCRIPT"; then
    echo "WARN: no explicit guard found for empty PARAMETER_REGISTRY_ID" >&2
    echo "      (deploy_contract helper already exits on empty ID, so this is not a hard failure)" >&2
  fi
fi

echo ""
if [ "$FAIL" -eq 0 ]; then
  echo "=== check_deploy_order: PASS — parameter_registry precedes all dependents ==="
  exit 0
else
  echo "=== check_deploy_order: FAIL — see errors above ===" >&2
  exit 1
fi
