#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Deployment Script
# Deploys all contracts to Stellar Soroban (testnet or mainnet).
#
# Usage:
#   ./scripts/deploy.sh [testnet|mainnet]
#
# Prerequisites:
#   - stellar CLI installed (https://developers.stellar.org/docs/tools/stellar-cli)
#   - DEPLOYER_SECRET env var set (or use --source flag)
#   - Contracts built: make build
#
# Dependency order (must match constructor requirements):
#   access_control → invoice_nft / risk_registry / dispute_resolution / debtor_attestation
#   → financing_pool → dispute_escrow → marketplace
# =============================================================================
# Environment Variables (all optional — defaults shown)
# =============================================================================
#
# Protocol Economics
#   TREASURY_FEE_BPS          Treasury protocol fee in basis points (default: 50  = 0.5%)
#   MARKETPLACE_FEE_BPS       Marketplace protocol fee in basis points (default: 50 = 0.5%)
#   MARKETPLACE_REFERRER_BPS  Fraction of marketplace fee paid to referrer in bps (default: 0 = off)
#   LATE_PENALTY_BPS          Late-repayment penalty in basis points (default: 200 = 2%)
#   MAX_POSITION_BPS          Max per-investor share of any pool in basis points (default: 5000 = 50%)
#
# Oracle
#   ORACLE_BASE_CURRENCY      Symbol used as the oracle's base quote currency (default: USDC)
#
# Admin
#   DEPLOYER_SECRET           Required. Stellar secret key / account alias for the deployer.
#                             The derived public key is used as the initial admin for all contracts.
#   DEPLOY_DISPUTE_ESCROW     Optional: true to deploy repayment escrow (default: false).
#                             Requires pool escrow hooks before custody can be used;
#                             see docs/dispute-escrow.md.
#
# =============================================================================

set -euo pipefail
umask 077

NETWORK="${1:-testnet}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
WASM_DIR="$ROOT_DIR/target/wasm32-unknown-unknown/release"
DEPLOY_LOG="$ROOT_DIR/deployments/$NETWORK.json"

# ── Protocol parameter defaults ───────────────────────────────────────────────

TREASURY_FEE_BPS="${TREASURY_FEE_BPS:-50}"
MARKETPLACE_FEE_BPS="${MARKETPLACE_FEE_BPS:-50}"
MARKETPLACE_REFERRER_BPS="${MARKETPLACE_REFERRER_BPS:-0}"
LATE_PENALTY_BPS="${LATE_PENALTY_BPS:-200}"
MAX_POSITION_BPS="${MAX_POSITION_BPS:-5000}"
ORACLE_BASE_CURRENCY="${ORACLE_BASE_CURRENCY:-USDC}"
DEPLOY_DISPUTE_ESCROW="${DEPLOY_DISPUTE_ESCROW:-false}"
case "$DEPLOY_DISPUTE_ESCROW" in
  true|false) ;;
  *) echo "ERROR: DEPLOY_DISPUTE_ESCROW must be true or false." >&2; exit 1 ;;
esac

# ── Vesting configuration ──────────────────────────────────────────────────────
# VESTING_ADMIN: Admin address for vesting contract (default: deployer/admin)
VESTING_ADMIN="${VESTING_ADMIN:-$ADMIN}"
# VESTING_TOKEN: Token contract address for vesting (required, e.g., USDC token contract)
VESTING_TOKEN="${VESTING_TOKEN:-}"
# VESTING_GRANTS: JSON array of grant configurations, e.g.:
# '[{"beneficiary":"G...","total_amount":"10000000000","start_time":1704067200,"cliff_duration":15778800,"vesting_duration":31557600}]'
VESTING_GRANTS="${VESTING_GRANTS:-}"

# ── Governance voting configuration ────────────────────────────────────────────
GOVERNANCE_VOTING_ADMIN="${GOVERNANCE_VOTING_ADMIN:-$ADMIN}"

# ── Network config ────────────────────────────────────────────────────────────

case "$NETWORK" in
  testnet)
    RPC_URL="https://soroban-testnet.stellar.org"
    NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
    ;;
  mainnet)
    RPC_URL="https://soroban-mainnet.stellar.org"
    NETWORK_PASSPHRASE="Public Global Stellar Network ; September 2015"
    ;;
  *)
    echo "ERROR: Unknown network '$NETWORK'. Use 'testnet' or 'mainnet'."
    exit 1
    ;;
esac

SOURCE="${DEPLOYER_SECRET:-}"
if [ -z "$SOURCE" ]; then
  echo "ERROR: Set DEPLOYER_SECRET environment variable."
  exit 1
fi

mkdir -p "$ROOT_DIR/deployments"

# ── Helpers ───────────────────────────────────────────────────────────────────

# deploy_contract <display-name> <wasm-path>
# Prints the contract ID to stdout. Aborts with a clear message on any failure.
deploy_contract() {
  local name="$1"
  local wasm="$2"

  if [ ! -f "$wasm" ]; then
    echo "ERROR: WASM not found for '$name': $wasm" >&2
    echo "       Run 'make build' before deploying." >&2
    exit 1
  fi

  echo "  Deploying $name..." >&2
  local contract_id
  contract_id=$(stellar contract deploy \
    --wasm "$wasm" \
    --source "$SOURCE" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE") \
    || { echo "ERROR: 'stellar contract deploy' failed for '$name'." >&2; exit 1; }

  if [ -z "$contract_id" ]; then
    echo "ERROR: Deploy of '$name' returned an empty contract ID." >&2
    exit 1
  fi

  echo "$contract_id"
}

# invoke <contract-id> <function> [args…]
# Aborts with a clear message on failure, identifying the contract and function.
invoke() {
  local contract_id="$1"
  local fn="$2"
  shift 2
  stellar contract invoke \
    --id "$contract_id" \
    --source "$SOURCE" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- "$fn" "$@" \
    || { echo "ERROR: invoke '$fn' on contract '$contract_id' failed." >&2; exit 1; }
}

# ── Pre-flight checks ─────────────────────────────────────────────────────────

echo "=== Kora Protocol Deployment ==="
echo "Network : $NETWORK"
echo "RPC     : $RPC_URL"
echo ""

ADMIN=$(stellar keys address "$SOURCE" 2>/dev/null || echo "$SOURCE")
echo "Admin   : $ADMIN"
echo ""

echo "--- Protocol parameters ---"
echo "  TREASURY_FEE_BPS       : $TREASURY_FEE_BPS"
echo "  MARKETPLACE_FEE_BPS    : $MARKETPLACE_FEE_BPS"
echo "  MARKETPLACE_REFERRER_BPS: $MARKETPLACE_REFERRER_BPS"
echo "  LATE_PENALTY_BPS       : $LATE_PENALTY_BPS"
echo "  MAX_POSITION_BPS       : $MAX_POSITION_BPS"
echo "  ORACLE_BASE_CURRENCY   : $ORACLE_BASE_CURRENCY"
echo ""

echo "--- Deploying contracts ---"

ACCESS_CONTROL_ID=$(deploy_contract "access_control" "$WASM_DIR/kora_access_control.wasm")
echo "  access_control : $ACCESS_CONTROL_ID"

INVOICE_NFT_ID=$(deploy_contract "invoice_nft" "$WASM_DIR/kora_invoice_nft.wasm")
echo "  invoice_nft    : $INVOICE_NFT_ID"

RISK_REGISTRY_ID=$(deploy_contract "risk_registry" "$WASM_DIR/kora_risk_registry.wasm")
echo "  risk_registry  : $RISK_REGISTRY_ID"

TREASURY_ID=$(deploy_contract "treasury" "$WASM_DIR/kora_treasury.wasm")
echo "  treasury       : $TREASURY_ID"

# Deploy vesting contract after treasury
VESTING_ID=$(deploy_contract "vesting" "$WASM_DIR/kora_vesting.wasm")
echo "  vesting        : $VESTING_ID"

DISPUTE_RESOLUTION_ID=$(deploy_contract "dispute_resolution" "$WASM_DIR/kora_dispute_resolution.wasm")
echo "  dispute_resolution: $DISPUTE_RESOLUTION_ID"

DEBTOR_ATTESTATION_ID=$(deploy_contract "debtor_attestation" "$WASM_DIR/kora_debtor_attestation.wasm")
echo "  debtor_attestation: $DEBTOR_ATTESTATION_ID"

POOL_ID=$(deploy_contract "financing_pool" "$WASM_DIR/kora_financing_pool.wasm")
echo "  financing_pool : $POOL_ID"

PARAMETER_REGISTRY_ID=$(deploy_contract "parameter_registry" "$WASM_DIR/kora_parameter_registry.wasm")
echo "  parameter_registry : $PARAMETER_REGISTRY_ID"

MARKETPLACE_ID=$(deploy_contract "marketplace" "$WASM_DIR/kora_marketplace.wasm")
echo "  marketplace    : $MARKETPLACE_ID"

PRICE_ORACLE_ID=$(deploy_contract "price_oracle" "$WASM_DIR/kora_price_oracle.wasm")
echo "  price_oracle   : $PRICE_ORACLE_ID"

GOVERNANCE_VOTING_ID=$(deploy_contract "governance_voting" "$WASM_DIR/kora_governance_voting.wasm")
echo "  governance_voting: $GOVERNANCE_VOTING_ID"

if [ "$DEPLOY_DISPUTE_ESCROW" = "true" ]; then
  DISPUTE_ESCROW_ID=$(deploy_contract "dispute_escrow" "$WASM_DIR/kora_dispute_escrow.wasm")
  echo "  dispute_escrow : $DISPUTE_ESCROW_ID"
fi

echo ""
echo "--- Initializing contracts (dependency order) ---"

invoke "$ACCESS_CONTROL_ID" initialize --admin "$ADMIN"
echo "  access_control initialized"

invoke "$INVOICE_NFT_ID" initialize --admin "$ADMIN" --access_control "$ACCESS_CONTROL_ID"
echo "  invoice_nft initialized"

invoke "$TREASURY_ID" initialize \
  --admin "$ADMIN" \
  --fee_bps "$TREASURY_FEE_BPS"
echo "  treasury initialized (fee: ${TREASURY_FEE_BPS} bps)"

# Initialize vesting contract
if [ -z "$VESTING_TOKEN" ]; then
  echo "ERROR: VESTING_TOKEN environment variable must be set (token contract address for vesting)." >&2
  exit 1
fi
invoke "$VESTING_ID" initialize \
  --admin "$VESTING_ADMIN" \
  --token "$VESTING_TOKEN"
echo "  vesting initialized (admin: $VESTING_ADMIN, token: $VESTING_TOKEN)"

# Create vesting grants if configured
if [ -n "$VESTING_GRANTS" ]; then
  echo "  Creating vesting grants..."
  # Parse JSON grants and create each one
  echo "$VESTING_GRANTS" | jq -c '.[]' | while IFS= read -r grant; do
    beneficiary=$(echo "$grant" | jq -r '.beneficiary')
    total_amount=$(echo "$grant" | jq -r '.total_amount')
    start_time=$(echo "$grant" | jq -r '.start_time')
    cliff_duration=$(echo "$grant" | jq -r '.cliff_duration')
    vesting_duration=$(echo "$grant" | jq -r '.vesting_duration')
    
    invoke "$VESTING_ID" create_grant \
      --admin "$VESTING_ADMIN" \
      --beneficiary "$beneficiary" \
      --total_amount "$total_amount" \
      --start_time "$start_time" \
      --cliff_duration "$cliff_duration" \
      --vesting_duration "$vesting_duration"
    echo "    Grant created for $beneficiary: $total_amount tokens"
  done
fi

# Transfer token custody to vesting contract
if [ -n "$VESTING_GRANTS" ]; then
  echo "  Transferring token custody to vesting contract..."
  # Calculate total allocation from grants
  TOTAL_ALLOCATION=$(echo "$VESTING_GRANTS" | jq '[.[] | .total_amount | tonumber] | add')
  if [ -z "$TOTAL_ALLOCATION" ] || [ "$TOTAL_ALLOCATION" = "null" ] || [ "$TOTAL_ALLOCATION" = "0" ]; then
    echo "ERROR: Total vesting allocation is zero or invalid." >&2
    exit 1
  fi
  echo "    Total allocation: $TOTAL_ALLOCATION tokens"
  
  # Determine source account for transfer (default to deployer/admin)
  VESTING_SOURCE="${VESTING_SOURCE:-$SOURCE}"
  
  # Verify source has sufficient balance
  SOURCE_BALANCE=$(stellar contract invoke \
    --id "$VESTING_TOKEN" \
    --source "$SOURCE" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- balance --id "$VESTING_SOURCE" 2>/dev/null || echo "0")
  
  if [ "$SOURCE_BALANCE" -lt "$TOTAL_ALLOCATION" ]; then
    echo "ERROR: Insufficient balance in source account ($VESTING_SOURCE)." >&2
    echo "       Required: $TOTAL_ALLOCATION, Available: $SOURCE_BALANCE" >&2
    exit 1
  fi
  
  # Verify token decimals match expected (vesting contract uses raw amounts)
  TOKEN_DECIMALS=$(stellar contract invoke \
    --id "$VESTING_TOKEN" \
    --source "$SOURCE" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- decimals 2>/dev/null || echo "0")
  
  if [ "$TOKEN_DECIMALS" = "0" ]; then
    echo "WARNING: Could not verify token decimals. Proceeding with transfer." >&2
  else
    echo "    Token decimals: $TOKEN_DECIMALS"
  fi
  
  # Transfer tokens to vesting contract
  echo "    Transferring $TOTAL_ALLOCATION from $VESTING_SOURCE to $VESTING_ID..."
  invoke "$VESTING_TOKEN" "transfer" \
    --from "$VESTING_SOURCE" \
    --to "$VESTING_ID" \
    --amount "$TOTAL_ALLOCATION"
  echo "    Token transfer complete."
  
  # Verify on-chain balance
  VESTING_BALANCE=$(stellar contract invoke \
    --id "$VESTING_TOKEN" \
    --source "$SOURCE" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- balance --id "$VESTING_ID" 2>/dev/null || echo "0")
  
  if [ "$VESTING_BALANCE" -lt "$TOTAL_ALLOCATION" ]; then
    echo "ERROR: Vesting contract balance mismatch after transfer." >&2
    echo "       Expected at least: $TOTAL_ALLOCATION, Actual: $VESTING_BALANCE" >&2
    exit 1
  fi
  echo "    Verified vesting contract balance: $VESTING_BALANCE"
fi

invoke "$DISPUTE_RESOLUTION_ID" initialize \
  --admin "$ADMIN" \
  --access_control "$ACCESS_CONTROL_ID"
echo "  dispute_resolution initialized"

# Initialize debtor_attestation
invoke "$DEBTOR_ATTESTATION_ID" initialize \
  --admin "$DEBTOR_ATTESTATION_ADMIN" \
  --max_attestation_age "$DEBTOR_ATTESTATION_MAX_AGE"
echo "  debtor_attestation initialized (max age: ${DEBTOR_ATTESTATION_MAX_AGE}s)"

# Add initial relays if provided
if [ -n "$DEBTOR_ATTESTATION_RELAYS" ]; then
  IFS=',' read -ra RELAYS <<< "$DEBTOR_ATTESTATION_RELAYS"
  for relay in "${RELAYS[@]}"; do
    relay=$(echo "$relay" | xargs)  # trim whitespace
    if [ -n "$relay" ]; then
      invoke "$DEBTOR_ATTESTATION_ID" add_relay --relay "$relay"
      echo "  debtor_attestation relay added: $relay"
    fi
  done
fi

invoke "$POOL_ID" initialize \
  --admin "$ADMIN" \
  --invoice_nft "$INVOICE_NFT_ID" \
  --risk_registry "$RISK_REGISTRY_ID" \
  --treasury "$TREASURY_ID" \
  --access_control "$ACCESS_CONTROL_ID" \
  --late_penalty_bps "$LATE_PENALTY_BPS" \
  --price_oracle "$PRICE_ORACLE_ID" \
  --max_position_bps "$MAX_POSITION_BPS"
echo "  financing_pool initialized (late penalty: ${LATE_PENALTY_BPS} bps, max position: ${MAX_POSITION_BPS} bps)"

invoke "$PARAMETER_REGISTRY_ID" initialize --admin "$ADMIN"
echo "  parameter_registry initialized"

# Seed parameter_registry with the initial fee values so that
# marketplace reads them as governed defaults.
invoke "$PARAMETER_REGISTRY_ID" admin_set_parameter \
  --admin "$ADMIN" \
  --key "MarketplaceFeeBps" \
  --value "$MARKETPLACE_FEE_BPS"
invoke "$PARAMETER_REGISTRY_ID" admin_set_parameter \
  --admin "$ADMIN" \
  --key "ReferrerSplitBps" \
  --value "$MARKETPLACE_REFERRER_BPS"
echo "  parameter_registry seeded with initial fee values"

# Initialize governance_voting contract
invoke "$GOVERNANCE_VOTING_ID" initialize \
  --admin "$GOVERNANCE_VOTING_ADMIN" \
  --access_control "$ACCESS_CONTROL_ID"
echo "  governance_voting initialized (admin: $GOVERNANCE_VOTING_ADMIN)"

# Set vesting contract in governance_voting for voting weight computation
invoke "$GOVERNANCE_VOTING_ID" set_vesting \
  --admin "$GOVERNANCE_VOTING_ADMIN" \
  --vesting_address "$VESTING_ID"
echo "  governance_voting vesting pointer set to $VESTING_ID"

if [ "$DEPLOY_DISPUTE_ESCROW" = "true" ]; then
  invoke "$DISPUTE_ESCROW_ID" initialize --admin "$ADMIN" --pool "$POOL_ID"
  echo "  dispute_escrow initialized; pool escrow hooks must be wired before use"
fi

invoke "$MARKETPLACE_ID" initialize \
  --admin "$ADMIN" \
  --invoice_nft "$INVOICE_NFT_ID" \
  --financing_pool "$POOL_ID" \
  --treasury "$TREASURY_ID" \
  --access_control "$ACCESS_CONTROL_ID" \
  --price_oracle "$PRICE_ORACLE_ID" \
  --risk_registry "$RISK_REGISTRY_ID" \
  --parameter_registry "$PARAMETER_REGISTRY_ID" \
  --fee_bps "$MARKETPLACE_FEE_BPS" \
  --referrer_split_bps "$MARKETPLACE_REFERRER_BPS"
echo "  marketplace initialized (fee: ${MARKETPLACE_FEE_BPS} bps, referrer split: ${MARKETPLACE_REFERRER_BPS} bps)"

invoke "$RISK_REGISTRY_ID" initialize \
  --admin "$ADMIN" \
  --invoice_nft "$INVOICE_NFT_ID"
echo "  risk_registry initialized"

invoke "$PRICE_ORACLE_ID" initialize \
  --admin "$ADMIN" \
  --base_currency "$ORACLE_BASE_CURRENCY"
echo "  price_oracle initialized (base: $ORACLE_BASE_CURRENCY)"

# ── Write deployment manifest ─────────────────────────────────────────────────

AC_HASH=$(sha256sum "$WASM_DIR/kora_access_control.wasm" | awk '{print $1}')
INVOICE_HASH=$(sha256sum "$WASM_DIR/kora_invoice_nft.wasm" | awk '{print $1}')
TREASURY_HASH=$(sha256sum "$WASM_DIR/kora_treasury.wasm" | awk '{print $1}')
POOL_HASH=$(sha256sum "$WASM_DIR/kora_financing_pool.wasm" | awk '{print $1}')
MARKETPLACE_HASH=$(sha256sum "$WASM_DIR/kora_marketplace.wasm" | awk '{print $1}')
RISK_HASH=$(sha256sum "$WASM_DIR/kora_risk_registry.wasm" | awk '{print $1}')
PRICE_ORACLE_HASH=$(sha256sum "$WASM_DIR/kora_price_oracle.wasm" | awk '{print $1}')
VESTING_HASH=$(sha256sum "$WASM_DIR/kora_vesting.wasm" | awk '{print $1}')
GOVERNANCE_VOTING_HASH=$(sha256sum "$WASM_DIR/kora_governance_voting.wasm" | awk '{print $1}')
DEBTOR_ATTESTATION_HASH=$(sha256sum "$WASM_DIR/kora_debtor_attestation.wasm" | awk '{print $1}')
ESCROW_MANIFEST_ENTRY=""
if [ "$DEPLOY_DISPUTE_ESCROW" = "true" ]; then
  DISPUTE_ESCROW_HASH=$(sha256sum "$WASM_DIR/kora_dispute_escrow.wasm" | awk '{print $1}')
  ESCROW_MANIFEST_ENTRY=", \"dispute_escrow\": {\"address\": \"$DISPUTE_ESCROW_ID\", \"wasm_hash\": \"$DISPUTE_ESCROW_HASH\"}"
fi

TIMESTAMP=$(date -u +%Y%m%d%H%M%S)
GIT_SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
MANIFEST_DIR="$ROOT_DIR/deployments/manifests"
mkdir -p "$MANIFEST_DIR"
VERSIONED_MANIFEST="$MANIFEST_DIR/${NETWORK}-${TIMESTAMP}-${GIT_SHA}.json"

TEMP_MANIFEST=$(mktemp)
cat > "$TEMP_MANIFEST" <<EOF
{
  "network": "$NETWORK",
  "deployed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "commit_sha": "$GIT_SHA",
  "admin": "$ADMIN",
  "parameters": {
    "treasury_fee_bps": $TREASURY_FEE_BPS,
    "marketplace_fee_bps": $MARKETPLACE_FEE_BPS,
    "marketplace_referrer_bps": $MARKETPLACE_REFERRER_BPS,
    "late_penalty_bps": $LATE_PENALTY_BPS,
    "max_position_bps": $MAX_POSITION_BPS,
    "oracle_base_currency": "$ORACLE_BASE_CURRENCY"
  },
  "contracts": {
    "access_control": {
      "address": "$ACCESS_CONTROL_ID",
      "wasm_hash": "$AC_HASH"
    },
    "invoice_nft": {
      "address": "$INVOICE_NFT_ID",
      "wasm_hash": "$INVOICE_HASH"
    },
    "treasury": {
      "address": "$TREASURY_ID",
      "wasm_hash": "$TREASURY_HASH"
    },
    "financing_pool": {
      "address": "$POOL_ID",
      "wasm_hash": "$POOL_HASH"
    },
    "marketplace": {
      "address": "$MARKETPLACE_ID",
      "wasm_hash": "$MARKETPLACE_HASH"
    },
    "risk_registry": {
      "address": "$RISK_REGISTRY_ID",
      "wasm_hash": "$RISK_HASH"
    },
    "price_oracle": {
      "address": "$PRICE_ORACLE_ID",
      "wasm_hash": "$PRICE_ORACLE_HASH"
    },
    "vesting": {
      "address": "$VESTING_ID",
      "wasm_hash": "$VESTING_HASH"
    },
    "governance_voting": {
      "address": "$GOVERNANCE_VOTING_ID",
      "wasm_hash": "$GOVERNANCE_VOTING_HASH"
    },
    "debtor_attestation": {
      "address": "$DEBTOR_ATTESTATION_ID",
      "wasm_hash": "$DEBTOR_ATTESTATION_HASH"
    }
    $ESCROW_MANIFEST_ENTRY
  }
}
EOF

cp "$TEMP_MANIFEST" "$VERSIONED_MANIFEST"
cp "$TEMP_MANIFEST" "$DEPLOY_LOG"
rm -f "$TEMP_MANIFEST"

echo ""
echo "=== Deployment complete ==="
echo "Manifest saved to: $DEPLOY_LOG"
echo "Versioned manifest: $VERSIONED_MANIFEST"
