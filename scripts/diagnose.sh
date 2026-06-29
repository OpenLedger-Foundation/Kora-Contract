#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Cross-Contract State Diagnostic Tool
#
# Iterates all minted invoices and checks for state inconsistencies across
# invoice_nft, marketplace, and financing_pool.  Reports each mismatch with
# contract, invoice_id, field, expected value, and actual value.
#
# Usage:
#   ./scripts/diagnose.sh [OPTIONS]
#
# Options:
#   --network       testnet|mainnet|standalone  (default: testnet)
#   --env-file      Path to env file            (default: scripts/contracts.env or .env)
#   --max-invoices  Stop after N invoices        (default: 100)
#   --dry-run       Test connectivity only, no consistency checks
#   --help          Show this help text
#
# Contract addresses (command-line flags override env file):
#   --invoice-nft       INVOICE_NFT_CONTRACT address
#   --marketplace       MARKETPLACE_CONTRACT address
#   --financing-pool    FINANCING_POOL_CONTRACT address
#   --treasury          TREASURY_CONTRACT address
#   --access-control    ACCESS_CONTROL_CONTRACT address
#   --risk-registry     RISK_REGISTRY_CONTRACT address
#
# Environment variables (read from env file or shell):
#   INVOICE_NFT_CONTRACT, MARKETPLACE_CONTRACT, FINANCING_POOL_CONTRACT,
#   TREASURY_CONTRACT, ACCESS_CONTROL_CONTRACT, RISK_REGISTRY_CONTRACT
#
# Exit codes:
#   0  No mismatches detected
#   1  One or more mismatches detected
#   2  Fatal error (missing dependency, bad args, connectivity failure)
#
# Prerequisites:
#   - stellar CLI  (https://developers.stellar.org/docs/tools/stellar-cli)
#   - jq           (https://stedolan.github.io/jq/)
#
# =============================================================================
#
# GitHub Actions scheduled-run snippet (copy into .github/workflows/):
#
# name: Kora State Diagnostic
# on:
#   schedule:
#     - cron: '0 */6 * * *'   # every 6 hours
#   workflow_dispatch:
#
# jobs:
#   diagnose:
#     runs-on: ubuntu-latest
#     steps:
#       - uses: actions/checkout@v4
#
#       - name: Install stellar CLI
#         run: |
#           curl -sSL https://github.com/stellar/stellar-cli/releases/latest/download/stellar-cli-x86_64-unknown-linux-gnu.tar.gz \
#             | tar -xz -C /usr/local/bin
#
#       - name: Install jq
#         run: sudo apt-get install -y jq
#
#       - name: Run diagnostics
#         env:
#           INVOICE_NFT_CONTRACT:    ${{ secrets.INVOICE_NFT_CONTRACT }}
#           MARKETPLACE_CONTRACT:    ${{ secrets.MARKETPLACE_CONTRACT }}
#           FINANCING_POOL_CONTRACT: ${{ secrets.FINANCING_POOL_CONTRACT }}
#           TREASURY_CONTRACT:       ${{ secrets.TREASURY_CONTRACT }}
#           ACCESS_CONTROL_CONTRACT: ${{ secrets.ACCESS_CONTROL_CONTRACT }}
#           RISK_REGISTRY_CONTRACT:  ${{ secrets.RISK_REGISTRY_CONTRACT }}
#         run: |
#           chmod +x scripts/diagnose.sh
#           ./scripts/diagnose.sh --network testnet --max-invoices 200
#
# =============================================================================

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────

NETWORK="testnet"
MAX_INVOICES=100
DRY_RUN=false
ENV_FILE=""

# Contract address overrides (empty = read from env)
ARG_INVOICE_NFT=""
ARG_MARKETPLACE=""
ARG_FINANCING_POOL=""
ARG_TREASURY=""
ARG_ACCESS_CONTROL=""
ARG_RISK_REGISTRY=""

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Argument parsing ──────────────────────────────────────────────────────────

usage() {
  sed -n '/^# Usage:/,/^# =====/{/^# =====/d; s/^# \{0,2\}//; p}' "$0"
  exit 0
}

die() { echo "ERROR: $*" >&2; exit 2; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --help|-h)          usage ;;
    --network)          NETWORK="${2:?--network requires a value}";        shift 2 ;;
    --env-file)         ENV_FILE="${2:?--env-file requires a value}";      shift 2 ;;
    --max-invoices)     MAX_INVOICES="${2:?--max-invoices requires a value}"; shift 2 ;;
    --dry-run)          DRY_RUN=true; shift ;;
    --invoice-nft)      ARG_INVOICE_NFT="${2:?--invoice-nft requires a value}";      shift 2 ;;
    --marketplace)      ARG_MARKETPLACE="${2:?--marketplace requires a value}";      shift 2 ;;
    --financing-pool)   ARG_FINANCING_POOL="${2:?--financing-pool requires a value}"; shift 2 ;;
    --treasury)         ARG_TREASURY="${2:?--treasury requires a value}";            shift 2 ;;
    --access-control)   ARG_ACCESS_CONTROL="${2:?--access-control requires a value}"; shift 2 ;;
    --risk-registry)    ARG_RISK_REGISTRY="${2:?--risk-registry requires a value}";  shift 2 ;;
    *) die "Unknown argument: $1. Run with --help for usage." ;;
  esac
done

# ── Validate network ──────────────────────────────────────────────────────────

case "$NETWORK" in
  testnet)
    RPC_URL="https://soroban-testnet.stellar.org"
    NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
    ;;
  mainnet)
    RPC_URL="https://soroban-mainnet.stellar.org"
    NETWORK_PASSPHRASE="Public Global Stellar Network ; September 2015"
    ;;
  standalone)
    RPC_URL="${STANDALONE_RPC_URL:-http://localhost:8000/soroban/rpc}"
    NETWORK_PASSPHRASE="${STANDALONE_PASSPHRASE:-Standalone Network ; February 2017}"
    ;;
  *)
    die "Unknown network '$NETWORK'. Use testnet, mainnet, or standalone."
    ;;
esac

# ── Dependency checks ─────────────────────────────────────────────────────────

command -v stellar >/dev/null 2>&1 || die "'stellar' CLI not found. Install from https://developers.stellar.org/docs/tools/stellar-cli"
command -v jq      >/dev/null 2>&1 || die "'jq' not found. Install via your package manager."

# ── Load env file ─────────────────────────────────────────────────────────────

load_env() {
  local file="$1"
  if [[ -f "$file" ]]; then
    echo "  Loading env from: $file"
    # shellcheck disable=SC1090
    set -o allexport
    source "$file"
    set +o allexport
  fi
}

if [[ -n "$ENV_FILE" ]]; then
  [[ -f "$ENV_FILE" ]] || die "Env file not found: $ENV_FILE"
  load_env "$ENV_FILE"
else
  # Try default locations
  if [[ -f "$SCRIPT_DIR/contracts.env" ]]; then
    load_env "$SCRIPT_DIR/contracts.env"
  elif [[ -f "$ROOT_DIR/.env" ]]; then
    load_env "$ROOT_DIR/.env"
  fi
fi

# ── Resolve contract addresses ────────────────────────────────────────────────

# Command-line flags take precedence over env vars
INVOICE_NFT_CONTRACT="${ARG_INVOICE_NFT:-${INVOICE_NFT_CONTRACT:-}}"
MARKETPLACE_CONTRACT="${ARG_MARKETPLACE:-${MARKETPLACE_CONTRACT:-}}"
FINANCING_POOL_CONTRACT="${ARG_FINANCING_POOL:-${FINANCING_POOL_CONTRACT:-}}"
TREASURY_CONTRACT="${ARG_TREASURY:-${TREASURY_CONTRACT:-}}"
ACCESS_CONTROL_CONTRACT="${ARG_ACCESS_CONTROL:-${ACCESS_CONTROL_CONTRACT:-}}"
RISK_REGISTRY_CONTRACT="${ARG_RISK_REGISTRY:-${RISK_REGISTRY_CONTRACT:-}}"

# These three are required for consistency checks
[[ -n "$INVOICE_NFT_CONTRACT" ]]    || die "INVOICE_NFT_CONTRACT not set. Use --invoice-nft or set in env file."
[[ -n "$MARKETPLACE_CONTRACT" ]]    || die "MARKETPLACE_CONTRACT not set. Use --marketplace or set in env file."
[[ -n "$FINANCING_POOL_CONTRACT" ]] || die "FINANCING_POOL_CONTRACT not set. Use --financing-pool or set in env file."

# ── Helpers ───────────────────────────────────────────────────────────────────

MISMATCHES=0

# Invoke a read-only contract function; echoes raw stdout; returns 1 on failure.
# Usage: contract_query <contract_id> <function> [args...]
contract_query() {
  local contract_id="$1"
  local fn="$2"
  shift 2
  stellar contract invoke \
    --network "$NETWORK" \
    --id "$contract_id" \
    -- "$fn" "$@" 2>/dev/null
}

# Print a formatted mismatch line and increment counter.
# Usage: report_mismatch <contract> <invoice_id> <field> <expected> <actual>
report_mismatch() {
  local contract="$1"
  local invoice_id="$2"
  local field="$3"
  local expected="$4"
  local actual="$5"

  MISMATCHES=$((MISMATCHES + 1))
  printf "  MISMATCH  contract=%-20s  invoice_id=%-4s  field=%-30s  expected=%-25s  actual=%s\n" \
    "$contract" "$invoice_id" "$field" "$expected" "$actual"
}

# ── Header ────────────────────────────────────────────────────────────────────

echo "============================================================"
echo "  Kora Protocol — Cross-Contract State Diagnostic"
echo "============================================================"
echo "  Network          : $NETWORK"
echo "  RPC URL          : $RPC_URL"
echo "  invoice_nft      : ${INVOICE_NFT_CONTRACT}"
echo "  marketplace      : ${MARKETPLACE_CONTRACT}"
echo "  financing_pool   : ${FINANCING_POOL_CONTRACT}"
echo "  treasury         : ${TREASURY_CONTRACT:-<not set>}"
echo "  access_control   : ${ACCESS_CONTROL_CONTRACT:-<not set>}"
echo "  risk_registry    : ${RISK_REGISTRY_CONTRACT:-<not set>}"
echo "  Max invoices     : $MAX_INVOICES"
echo "  Dry-run          : $DRY_RUN"
echo "------------------------------------------------------------"
echo ""

# ── Dry-run: connectivity check ───────────────────────────────────────────────

echo "--- Connectivity check ---"

check_connectivity() {
  local label="$1"
  local contract_id="$2"
  local fn="$3"
  shift 3

  printf "  %-20s ..." "$label"
  if contract_query "$contract_id" "$fn" "$@" >/dev/null 2>&1; then
    echo " OK"
    return 0
  else
    echo " UNREACHABLE"
    return 1
  fi
}

CONNECTIVITY_OK=true

check_connectivity "invoice_nft"    "$INVOICE_NFT_CONTRACT"    "next_id"    || CONNECTIVITY_OK=false
check_connectivity "marketplace"    "$MARKETPLACE_CONTRACT"    "next_id"    || CONNECTIVITY_OK=false
check_connectivity "financing_pool" "$FINANCING_POOL_CONTRACT" "next_id"    || CONNECTIVITY_OK=false

if [[ -n "${TREASURY_CONTRACT:-}" ]]; then
  check_connectivity "treasury"       "$TREASURY_CONTRACT"       "next_id"  || CONNECTIVITY_OK=false
fi
if [[ -n "${ACCESS_CONTROL_CONTRACT:-}" ]]; then
  check_connectivity "access_control" "$ACCESS_CONTROL_CONTRACT" "next_id"  || CONNECTIVITY_OK=false
fi
if [[ -n "${RISK_REGISTRY_CONTRACT:-}" ]]; then
  check_connectivity "risk_registry"  "$RISK_REGISTRY_CONTRACT"  "next_id"  || CONNECTIVITY_OK=false
fi

echo ""

if [[ "$DRY_RUN" == "true" ]]; then
  if [[ "$CONNECTIVITY_OK" == "true" ]]; then
    echo "Dry-run complete. All reachable contracts responded."
    exit 0
  else
    echo "Dry-run complete. One or more contracts were UNREACHABLE." >&2
    exit 2
  fi
fi

# ── Fetch next_id (total invoice count) ───────────────────────────────────────

echo "--- Fetching invoice count from invoice_nft ---"

NEXT_ID_RAW=$(contract_query "$INVOICE_NFT_CONTRACT" "next_id") || \
  die "Could not call next_id on invoice_nft ($INVOICE_NFT_CONTRACT). Is the contract deployed?"

# next_id returns a plain u32 integer (possibly quoted)
NEXT_ID=$(echo "$NEXT_ID_RAW" | tr -d '"' | tr -d '[:space:]')

if ! [[ "$NEXT_ID" =~ ^[0-9]+$ ]]; then
  die "next_id returned unexpected value: '$NEXT_ID_RAW'"
fi

if [[ "$NEXT_ID" -le 1 ]]; then
  echo "  next_id=$NEXT_ID — no invoices minted yet. Nothing to check."
  echo ""
  echo "OK: No invoices to inspect."
  exit 0
fi

LAST_ID=$(( NEXT_ID - 1 ))
TOTAL=$LAST_ID
if [[ "$TOTAL" -gt "$MAX_INVOICES" ]]; then
  TOTAL=$MAX_INVOICES
  echo "  next_id=$NEXT_ID (capped at --max-invoices $MAX_INVOICES; checking IDs 1–$TOTAL)"
else
  echo "  next_id=$NEXT_ID (checking IDs 1–$TOTAL)"
fi
echo ""

# ── Per-invoice consistency checks ───────────────────────────────────────────

echo "--- Checking cross-contract state consistency ---"
echo ""

for INVOICE_ID in $(seq 1 "$TOTAL"); do

  # ── 1. Fetch invoice from invoice_nft ──────────────────────────────────────

  INVOICE_RAW=$(contract_query "$INVOICE_NFT_CONTRACT" "get_invoice" \
    --invoice_id "$INVOICE_ID" 2>&1) || {
    echo "  [invoice $INVOICE_ID] WARN: get_invoice failed — skipping (may not exist)"
    continue
  }

  # Parse status field; handle both {"status":"Listed"} and {"status":{"Listed":null}}
  INVOICE_STATUS=$(echo "$INVOICE_RAW" | jq -r '
    if .status | type == "string" then .status
    elif .status | type == "object" then (.status | keys[0])
    else "UNKNOWN"
    end' 2>/dev/null) || INVOICE_STATUS="UNKNOWN"

  if [[ "$INVOICE_STATUS" == "UNKNOWN" || -z "$INVOICE_STATUS" ]]; then
    echo "  [invoice $INVOICE_ID] WARN: could not parse status — skipping. Raw: $INVOICE_RAW"
    continue
  fi

  # ── 2. Fetch marketplace listing ───────────────────────────────────────────

  LISTING_RAW=$(contract_query "$MARKETPLACE_CONTRACT" "get_listing" \
    --invoice_id "$INVOICE_ID" 2>&1) || LISTING_RAW=""

  LISTING_EXISTS=false
  LISTING_ACTIVE="N/A"
  LISTING_FUNDED=0
  LISTING_ASKING=0

  if [[ -n "$LISTING_RAW" ]] && echo "$LISTING_RAW" | jq -e . >/dev/null 2>&1; then
    LISTING_EXISTS=true
    LISTING_ACTIVE=$(echo "$LISTING_RAW"  | jq -r '.is_active // "UNKNOWN"' 2>/dev/null || echo "UNKNOWN")
    LISTING_FUNDED=$(echo "$LISTING_RAW"  | jq -r '.funded_amount // 0'    2>/dev/null || echo 0)
    LISTING_ASKING=$(echo "$LISTING_RAW"  | jq -r '.asking_price // 0'     2>/dev/null || echo 0)
  fi

  # ── 3. Fetch financing pool ────────────────────────────────────────────────

  POOL_RAW=$(contract_query "$FINANCING_POOL_CONTRACT" "get_pool" \
    --invoice_id "$INVOICE_ID" 2>&1) || POOL_RAW=""

  POOL_EXISTS=false
  POOL_CLOSED="N/A"
  POOL_DEFAULT="N/A"

  if [[ -n "$POOL_RAW" ]] && echo "$POOL_RAW" | jq -e . >/dev/null 2>&1; then
    POOL_EXISTS=true
    POOL_CLOSED=$(echo "$POOL_RAW"  | jq -r '.is_closed    // "UNKNOWN"' 2>/dev/null || echo "UNKNOWN")
    POOL_DEFAULT=$(echo "$POOL_RAW" | jq -r '.is_defaulted // "UNKNOWN"' 2>/dev/null || echo "UNKNOWN")
  fi

  # ── 4. Consistency rules ───────────────────────────────────────────────────

  # Rule 1: Invoice is Listed but no marketplace listing exists
  if [[ "$INVOICE_STATUS" == "Listed" && "$LISTING_EXISTS" == "false" ]]; then
    report_mismatch "marketplace" "$INVOICE_ID" \
      "listing_exists" \
      "true (invoice status=Listed)" \
      "false (no listing record)"
  fi

  # Rule 2: Invoice is Funded but no financing pool record
  if [[ "$INVOICE_STATUS" == "Funded" && "$POOL_EXISTS" == "false" ]]; then
    report_mismatch "financing_pool" "$INVOICE_ID" \
      "pool_exists" \
      "true (invoice status=Funded)" \
      "false (no pool record)"
  fi

  # Rule 3: Listing is_active=false (fully funded) but invoice still Listed
  if [[ "$LISTING_ACTIVE" == "false" && "$INVOICE_STATUS" == "Listed" ]]; then
    report_mismatch "invoice_nft" "$INVOICE_ID" \
      "status" \
      "Funded (listing is_active=false)" \
      "Listed"
  fi

  # Rule 4: Listing funded_amount >= asking_price but no pool created
  if [[ "$LISTING_EXISTS" == "true" && "$POOL_EXISTS" == "false" ]] && \
     [[ "$LISTING_ASKING" -gt 0 ]] && \
     [[ "$LISTING_FUNDED" -ge "$LISTING_ASKING" ]]; then
    report_mismatch "financing_pool" "$INVOICE_ID" \
      "pool_exists" \
      "true (funded_amount=${LISTING_FUNDED} >= asking_price=${LISTING_ASKING})" \
      "false (no pool record)"
  fi

  # Rule 5: Invoice is Repaid but pool is_closed=false
  if [[ "$INVOICE_STATUS" == "Repaid" && "$POOL_EXISTS" == "true" && "$POOL_CLOSED" == "false" ]]; then
    report_mismatch "financing_pool" "$INVOICE_ID" \
      "is_closed" \
      "true (invoice status=Repaid)" \
      "false"
  fi

  # Rule 6: Invoice is Defaulted but pool is not reflecting default
  if [[ "$INVOICE_STATUS" == "Defaulted" && "$POOL_EXISTS" == "true" && "$POOL_DEFAULT" == "false" ]]; then
    report_mismatch "financing_pool" "$INVOICE_ID" \
      "is_defaulted" \
      "true (invoice status=Defaulted)" \
      "false"
  fi

  # Rule 7: Pool is closed but invoice is still Funded (not advanced to Repaid/Defaulted)
  if [[ "$POOL_CLOSED" == "true" && "$INVOICE_STATUS" == "Funded" ]]; then
    report_mismatch "invoice_nft" "$INVOICE_ID" \
      "status" \
      "Repaid or Defaulted (pool is_closed=true)" \
      "Funded"
  fi

  # Rule 8: Listing active=true but invoice has progressed beyond Listed
  if [[ "$LISTING_ACTIVE" == "true" ]] && \
     { [[ "$INVOICE_STATUS" == "Funded" ]] || [[ "$INVOICE_STATUS" == "Repaid" ]] || \
       [[ "$INVOICE_STATUS" == "Defaulted" ]]; }; then
    report_mismatch "marketplace" "$INVOICE_ID" \
      "is_active" \
      "false (invoice status=$INVOICE_STATUS)" \
      "true"
  fi

done

# ── Summary ───────────────────────────────────────────────────────────────────

echo ""
echo "============================================================"
echo "  Diagnostic Summary"
echo "============================================================"
echo "  Invoices inspected : $TOTAL"
echo "  Mismatches found   : $MISMATCHES"
echo "------------------------------------------------------------"

if [[ "$MISMATCHES" -eq 0 ]]; then
  echo "  RESULT: OK — no cross-contract state mismatches detected."
  echo "============================================================"
  exit 0
else
  echo "  RESULT: ALERT — $MISMATCHES mismatch(es) detected (see above)."
  echo "============================================================"
  exit 1
fi
