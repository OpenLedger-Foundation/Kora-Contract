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
#   access_control → invoice_nft / risk_registry → financing_pool → treasury → marketplace
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
#
# =============================================================================

set -euo pipefail

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

POOL_ID=$(deploy_contract "financing_pool" "$WASM_DIR/kora_financing_pool.wasm")
echo "  financing_pool : $POOL_ID"

MARKETPLACE_ID=$(deploy_contract "marketplace" "$WASM_DIR/kora_marketplace.wasm")
echo "  marketplace    : $MARKETPLACE_ID"

RISK_REGISTRY_ID=$(deploy_contract "risk_registry" "$WASM_DIR/kora_risk_registry.wasm")
echo "  risk_registry  : $RISK_REGISTRY_ID"

PRICE_ORACLE_ID=$(deploy_contract "price_oracle" "$WASM_DIR/kora_price_oracle.wasm")
echo "  price_oracle   : $PRICE_ORACLE_ID"

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

invoke "$MARKETPLACE_ID" initialize \
  --admin "$ADMIN" \
  --invoice_nft "$INVOICE_NFT_ID" \
  --financing_pool "$POOL_ID" \
  --treasury "$TREASURY_ID" \
  --access_control "$ACCESS_CONTROL_ID" \
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

cat > "$DEPLOY_LOG" <<EOF
{
  "network": "$NETWORK",
  "deployed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
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
    }
  }
}
EOF

echo ""
echo "=== Deployment complete ==="
echo "Manifest saved to: $DEPLOY_LOG"
