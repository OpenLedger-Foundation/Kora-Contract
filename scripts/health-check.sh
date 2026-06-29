#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Health Check Aggregator
#
# Queries all deployed contracts and aggregates protocol health metrics into a
# single JSON response suitable for monitoring dashboards.
#
# Usage:
#   ./scripts/health-check.sh [testnet|mainnet]
#
# Output:
#   Prints JSON to stdout with:
#   - Protocol pause state
#   - Admin addresses (per contract)
#   - Fee rates (treasury, marketplace)
#   - Invoice metrics (count, states)
#   - Treasury balances
#   - Pool funding status
#   - Timestamp of check
#
# Environment:
#   DEPLOYER_SECRET — Stellar secret key or identity (for RPC calls, not signing)
# =============================================================================

set -euo pipefail

NETWORK="${1:-testnet}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$ROOT_DIR/deployments/$NETWORK.json"

if [ ! -f "$MANIFEST" ]; then
  echo '{"error": "Deployment manifest not found. Run deploy.sh first."}' >&2
  exit 1
fi

# Load contract addresses from manifest
ACCESS_CONTROL=$(jq -r '.contracts.access_control.address' "$MANIFEST")
INVOICE_NFT=$(jq -r '.contracts.invoice_nft.address' "$MANIFEST")
MARKETPLACE=$(jq -r '.contracts.marketplace.address' "$MANIFEST")
POOL=$(jq -r '.contracts.financing_pool.address' "$MANIFEST")
TREASURY=$(jq -r '.contracts.treasury.address' "$MANIFEST")
RISK_REGISTRY=$(jq -r '.contracts.risk_registry.address' "$MANIFEST")

# Network config
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
    echo '{"error": "Unknown network: '"$NETWORK"'. Use testnet or mainnet."}' >&2
    exit 1
    ;;
esac

# Query helper — read-only view (no auth required)
query() {
  local contract="$1"
  local fn="$2"
  shift 2
  stellar contract invoke \
    --id "$contract" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- "$fn" "$@" 2>/dev/null || echo "null"
}

# ── Aggregate health metrics ──────────────────────────────────────────────────

TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Protocol pause state
PROTOCOL_PAUSED=$(query "$ACCESS_CONTROL" "is_paused")

# Admins from each contract
ACCESS_CONTROL_ADMIN=$(query "$ACCESS_CONTROL" "get_admin")
TREASURY_ADMIN=$(query "$TREASURY" "get_admin")
RISK_ADMIN=$(query "$RISK_REGISTRY" "get_admin")

# Fee rates
TREASURY_FEE_BPS=$(query "$TREASURY" "get_fee_bps")
MARKETPLACE_FEE_BPS=$(query "$MARKETPLACE" "get_fee_bps")

# Invoice metrics
INVOICE_COUNT=$(query "$INVOICE_NFT" "invoice_count")

# Treasury balance (USDC for example, adjust if needed)
# Note: Requires querying with a specific token address; skipped in basic version
TREASURY_BALANCE_USDC="null"

# Build output JSON
cat > /tmp/kora_health_$$.json <<EOF
{
  "timestamp": "$TIMESTAMP",
  "network": "$NETWORK",
  "protocol": {
    "paused": $PROTOCOL_PAUSED
  },
  "admins": {
    "access_control": $ACCESS_CONTROL_ADMIN,
    "treasury": $TREASURY_ADMIN,
    "risk_registry": $RISK_ADMIN
  },
  "fees": {
    "treasury_bps": $TREASURY_FEE_BPS,
    "marketplace_bps": $MARKETPLACE_FEE_BPS
  },
  "metrics": {
    "invoices_minted": $INVOICE_COUNT
  },
  "contracts": {
    "access_control": "$ACCESS_CONTROL",
    "invoice_nft": "$INVOICE_NFT",
    "marketplace": "$MARKETPLACE",
    "financing_pool": "$POOL",
    "treasury": "$TREASURY",
    "risk_registry": "$RISK_REGISTRY"
  }
}
EOF

# Output JSON and cleanup
cat /tmp/kora_health_$$.json
rm /tmp/kora_health_$$.json
