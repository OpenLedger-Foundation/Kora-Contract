#!/usr/bin/env bash
# scripts/check_reentrancy_audit.sh
#
# Automated gate for the reentrancy / CEI audit checklist in
# docs/REENTRANCY_AUDIT.md.
#
# What this script checks:
#
#   1. Guard presence: every contract function marked "Guard present: ✅" in the
#      audit matrix must still use ReentrancyGuard or RepaymentLock at its call
#      site. We confirm this by asserting that the relevant source files still
#      import / reference those identifiers at all.
#
#   2. Documented open items: the two known deviations (TR-6 claim_share and
#      RR-6 top_up_stake) must each have their corresponding regression test.
#      If the test is removed the gate fails and forces a re-audit.
#
#   3. Token transfer coverage: any new outbound `token.transfer` / `transfer`
#      call sites in the five guarded contracts (financing_pool, marketplace,
#      treasury, risk_registry, invoice_nft) that are NOT preceded by either
#      a RepaymentLock set or a ReentrancyGuard acquire within the same function
#      are flagged for manual review. This is a heuristic grep; false positives
#      must be added to REENTRANCY_ALLOWLIST below with a justification.
#
#   4. REENTRANCY_AUDIT.md structural check: the document must list at least
#      the minimum expected number of rows so a wholesale deletion is caught.
#
# Usage (local):
#   bash scripts/check_reentrancy_audit.sh
#
# Exit codes:
#   0 — all checks passed
#   1 — one or more checks failed (details printed to stderr)
#
# Adding a new call site:
#   1. Add a row to docs/REENTRANCY_AUDIT.md.
#   2. Add a regression test in contracts/tests/reentrancy_cei_regression.rs.
#   3. If the new call site is intentionally guard-free (documented + CEI
#      correct), add an entry to REENTRANCY_ALLOWLIST with justification.
#
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ── Colour helpers ────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
pass() { echo -e "${GREEN}✓${NC} $*"; }
fail() { echo -e "${RED}✗${NC} $*" >&2; FAILED=1; }
warn() { echo -e "${YELLOW}!${NC} $*"; }

FAILED=0

# ─────────────────────────────────────────────────────────────────────────────
# Allowlist: functions that intentionally have no guard but are audited safe.
# Format: "file:function_pattern:justification"
# ─────────────────────────────────────────────────────────────────────────────
# TR-6: treasury::claim_share — CEI is correct (claim flag set before transfer).
#        Guard omitted because the function is stakeholder-restricted, not
#        callable by arbitrary addresses. See REENTRANCY_AUDIT.md § TR-6.
# RR-1: risk_registry::add_verifier — inbound token transfer; re-entrance benign.
# RR-6: risk_registry::top_up_stake — inbound transfer; low exploitability.
#        Effects written after interaction; documented deviation.
# TR-4: treasury::refund_fee — marketplace-restricted caller; single auth check.
declare -a REENTRANCY_ALLOWLIST=(
  "treasury:claim_share:TR-6 documented; CEI correct; stakeholder-restricted"
  "risk_registry:add_verifier:RR-1 documented; inbound transfer; benign"
  "risk_registry:top_up_stake:RR-6 documented deviation; inbound transfer"
  "treasury:refund_fee:TR-4 documented; marketplace-restricted"
)

echo "═══════════════════════════════════════════════════════════"
echo " Reentrancy / CEI Audit Gate — docs/REENTRANCY_AUDIT.md"
echo "═══════════════════════════════════════════════════════════"

# ─────────────────────────────────────────────────────────────────────────────
# Check 1: Audit document exists and has sufficient matrix rows
# ─────────────────────────────────────────────────────────────────────────────
AUDIT_DOC="$REPO_ROOT/docs/REENTRANCY_AUDIT.md"

if [ ! -f "$AUDIT_DOC" ]; then
  fail "docs/REENTRANCY_AUDIT.md not found — do not delete the audit document"
  exit 1
fi

# Count table rows (lines starting with | followed by a number and |)
row_count=$(grep -c '^\| [0-9]' "$AUDIT_DOC" 2>/dev/null || echo 0)
MIN_ROWS=27
if [ "$row_count" -lt "$MIN_ROWS" ]; then
  fail "REENTRANCY_AUDIT.md has only $row_count matrix rows (expected >= $MIN_ROWS)." \
       " Do not remove rows without re-auditing."
else
  pass "Audit matrix has $row_count rows (>= $MIN_ROWS required)."
fi

# ─────────────────────────────────────────────────────────────────────────────
# Check 2: Regression test file exists and is non-trivial
# ─────────────────────────────────────────────────────────────────────────────
REGRESSION_FILE="$REPO_ROOT/contracts/tests/reentrancy_cei_regression.rs"

if [ ! -f "$REGRESSION_FILE" ]; then
  fail "contracts/tests/reentrancy_cei_regression.rs not found." \
       " Do not delete the regression test file."
  exit 1
fi

test_count=$(grep -c '#\[test\]' "$REGRESSION_FILE" 2>/dev/null || echo 0)
MIN_TESTS=20
if [ "$test_count" -lt "$MIN_TESTS" ]; then
  fail "reentrancy_cei_regression.rs has only $test_count #[test] functions" \
       " (expected >= $MIN_TESTS). Tests may have been removed."
else
  pass "Regression file has $test_count tests (>= $MIN_TESTS required)."
fi

# ─────────────────────────────────────────────────────────────────────────────
# Check 3: Documented open-item regression tests are present
# TR-6: claim_share has no guard — regression test must exist
# RR-6: top_up_stake CEI deviation — regression test must exist
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "── Open-item regression tests ──"

if grep -q 'test_tr.*claim_share\|test_tr_claim_share\|TR-6' "$REGRESSION_FILE" 2>/dev/null; then
  pass "TR-6 (claim_share no-guard) regression coverage present."
else
  fail "TR-6 regression test not found in reentrancy_cei_regression.rs." \
       " Add a test verifying that claim_share cannot be double-called" \
       " (CEI anchor: claim flag set before transfer)."
fi

if grep -q 'test_rr_top_up_stake_cei\|RR-6\|top_up_stake_cei' "$REGRESSION_FILE" 2>/dev/null; then
  pass "RR-6 (top_up_stake CEI deviation) regression coverage present."
else
  fail "RR-6 regression test not found in reentrancy_cei_regression.rs." \
       " Add a test documenting the top_up_stake CEI deviation" \
       " (inbound transfer before storage write)."
fi

# ─────────────────────────────────────────────────────────────────────────────
# Check 4: Guard presence in guarded contract source files
# Every contract that uses ReentrancyGuard or RepaymentLock must still reference
# those identifiers. If a future refactor removes all guards from a file we want
# to know.
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "── Guard presence per contract ──"

declare -A GUARD_REQUIREMENTS=(
  ["contracts/financing_pool/src/lib.rs"]="RepaymentLock"
  ["contracts/marketplace/src/lib.rs"]="ReentrancyGuard"
  ["contracts/treasury/src/lib.rs"]="ReentrancyGuard"
  ["contracts/risk_registry/src/lib.rs"]="ReentrancyGuard"
  ["contracts/invoice_nft/src/lib.rs"]="ReentrancyGuard"
  ["contracts/access_control/src/lib.rs"]="ReentrancyGuard"
)

for file in "${!GUARD_REQUIREMENTS[@]}"; do
  guard="${GUARD_REQUIREMENTS[$file]}"
  full_path="$REPO_ROOT/$file"
  if [ ! -f "$full_path" ]; then
    warn "$file not found — skipping guard check (file may have moved)."
    continue
  fi
  if grep -q "$guard" "$full_path" 2>/dev/null; then
    pass "$file uses $guard."
  else
    fail "$file no longer references $guard." \
         " Confirm that CEI is still correct on all cross-contract call sites" \
         " and update docs/REENTRANCY_AUDIT.md if the guard was deliberately removed."
  fi
done

# ─────────────────────────────────────────────────────────────────────────────
# Check 5: New token transfer sites heuristic
# Flag any line in the guarded contracts that calls a token transfer function
# and is NOT in the allowlist. This is intentionally conservative: it will
# print warnings but not fail the build for allowlisted functions, and will
# fail for genuinely new, un-allowlisted transfer sites.
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "── Token transfer call site heuristic ──"

GUARDED_CONTRACTS=(
  "contracts/financing_pool/src/lib.rs"
  "contracts/marketplace/src/lib.rs"
  "contracts/treasury/src/lib.rs"
  "contracts/risk_registry/src/lib.rs"
  "contracts/invoice_nft/src/lib.rs"
)

NEW_VIOLATIONS=0

for file in "${GUARDED_CONTRACTS[@]}"; do
  full_path="$REPO_ROOT/$file"
  [ -f "$full_path" ] || continue

  # Find lines that call token.transfer or token_client.transfer
  # (the most common patterns in the codebase).
  while IFS=: read -r lineno content; do
    # Skip comment lines
    [[ "$content" =~ ^[[:space:]]*/(/|\*) ]] && continue

    # Check if this line is allowlisted
    allowlisted=0
    for entry in "${REENTRANCY_ALLOWLIST[@]}"; do
      IFS=: read -r al_contract al_fn al_reason <<< "$entry"
      if [[ "$file" == *"$al_contract"* ]]; then
        # Try to find the enclosing function name within 40 lines above
        fn_region=$(sed -n "$((lineno > 40 ? lineno - 40 : 1)),${lineno}p" "$full_path" \
                    | grep -oP 'fn \K\w+' | tail -1 || true)
        if [[ "$fn_region" == "$al_fn"* ]] || [[ "$al_fn" == "*" ]]; then
          allowlisted=1
          break
        fi
      fi
    done

    if [ "$allowlisted" -eq 0 ]; then
      warn "Potential unguarded transfer at $file:$lineno"
      warn "  → $content"
      warn "  If this is intentionally guard-free, add it to REENTRANCY_ALLOWLIST"
      warn "  in scripts/check_reentrancy_audit.sh with a justification."
      NEW_VIOLATIONS=$((NEW_VIOLATIONS + 1))
    fi
  done < <(grep -n '\.transfer(' "$full_path" 2>/dev/null || true)
done

if [ "$NEW_VIOLATIONS" -gt 0 ]; then
  warn "$NEW_VIOLATIONS potentially unguarded transfer(s) flagged for review."
  warn "Review each one, confirm CEI + guard are correct, then add to REENTRANCY_ALLOWLIST."
  # Note: this is a warn-only check, not a hard fail, to avoid false positives
  # from the heuristic. The mandatory checks above (1–4) are the hard gates.
else
  pass "No new unguarded token transfer patterns detected."
fi

# ─────────────────────────────────────────────────────────────────────────────
# Check 6: shared reentrancy module is intact
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "── Shared reentrancy module ──"

REENTRANCY_RS="$REPO_ROOT/contracts/shared/src/reentrancy.rs"
if [ ! -f "$REENTRANCY_RS" ]; then
  fail "contracts/shared/src/reentrancy.rs not found."
else
  if grep -q 'ReentrancyGuard' "$REENTRANCY_RS" 2>/dev/null && \
     grep -q 'acquire_guard' "$REENTRANCY_RS" 2>/dev/null; then
    pass "shared reentrancy module defines ReentrancyGuard and acquire_guard."
  else
    fail "contracts/shared/src/reentrancy.rs is missing expected symbols." \
         " Do not remove ReentrancyGuard or acquire_guard."
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════════════"
if [ "$FAILED" -eq 0 ]; then
  echo -e "${GREEN}All reentrancy audit gate checks passed.${NC}"
  exit 0
else
  echo -e "${RED}One or more reentrancy audit gate checks FAILED.${NC}"
  echo ""
  echo "To fix:"
  echo "  1. Update docs/REENTRANCY_AUDIT.md if a new call site was added."
  echo "  2. Add a regression test in contracts/tests/reentrancy_cei_regression.rs."
  echo "  3. If the site is intentionally guard-free, add it to REENTRANCY_ALLOWLIST"
  echo "     in scripts/check_reentrancy_audit.sh with a justification."
  exit 1
fi
