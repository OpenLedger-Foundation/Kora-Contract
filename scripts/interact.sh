#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Example Interactions
# Demonstrates how to interact with deployed contracts via stellar CLI.
#
# Usage:
#   source scripts/interact.sh testnet
#   kora_mint_invoice <sme_address> <amount> <due_date_unix>
# =============================================================================

set -euo pipefail

# ── Version guard ─────────────────────────────────────────────────────────────
# Minimum required stellar CLI version (major.minor.patch).
STELLAR_MIN_VERSION="20.0.0"

_version_gte() {
  # Returns 0 (true) if $1 >= $2, using sort -V semantics.
  [ "$(printf '%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]
}

if ! command -v stellar &>/dev/null; then
  echo "ERROR: 'stellar' CLI not found in PATH." >&2
  echo "       Install it with: cargo install stellar-cli --locked" >&2
  echo "       Minimum required version: ${STELLAR_MIN_VERSION}" >&2
  exit 1
fi

STELLAR_VERSION=$(stellar --version 2>&1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n1)
if [ -z "$STELLAR_VERSION" ]; then
  echo "WARNING: Could not parse stellar CLI version. Proceeding anyway." >&2
elif ! _version_gte "$STELLAR_VERSION" "$STELLAR_MIN_VERSION"; then
  echo "ERROR: stellar CLI version ${STELLAR_VERSION} is below the minimum required ${STELLAR_MIN_VERSION}." >&2
  echo "       Upgrade with: cargo install stellar-cli --locked" >&2
  exit 1
fi

# ── Network setup ─────────────────────────────────────────────────────────────
NETWORK="${1:-testnet}"
MANIFEST="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/deployments/$NETWORK.json"

if [ ! -f "$MANIFEST" ]; then
  echo "No deployment manifest found at $MANIFEST. Run deploy.sh first." >&2
  exit 1
fi

# Load contract addresses from manifest
ACCESS_CONTROL=$(jq -r '.contracts.access_control.address' "$MANIFEST")
INVOICE_NFT=$(jq -r '.contracts.invoice_nft.address' "$MANIFEST")
MARKETPLACE=$(jq -r '.contracts.marketplace.address' "$MANIFEST")
POOL=$(jq -r '.contracts.financing_pool.address' "$MANIFEST")
TREASURY=$(jq -r '.contracts.treasury.address' "$MANIFEST")
RISK_REGISTRY=$(jq -r '.contracts.risk_registry.address' "$MANIFEST")

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
    echo "ERROR: Unknown network '${NETWORK}'. Supported: testnet, mainnet" >&2
    exit 1
    ;;
esac

SOURCE="${DEPLOYER_SECRET:?Set DEPLOYER_SECRET}"

# ── Core invoke helper ────────────────────────────────────────────────────────
_invoke() {
  local contract="$1"; local fn="$2"; shift 2
  stellar contract invoke \
    --id "$contract" \
    --source-account "$SOURCE" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- "$fn" "$@"
}

# ── Protocol Admin ────────────────────────────────────────────────────────────

kora_pause()   { _invoke "$ACCESS_CONTROL" pause   --admin "$1"; }
kora_unpause() { _invoke "$ACCESS_CONTROL" unpause --admin "$1"; }

kora_whitelist_token() {
  # $1 = admin, $2 = token_address
  _invoke "$MARKETPLACE" whitelist_token --admin "$1" --token "$2"
}

kora_add_verifier() {
  # $1 = admin, $2 = verifier_address
  _invoke "$RISK_REGISTRY" add_verifier --admin "$1" --verifier "$2"
}

# ── SME Flows ─────────────────────────────────────────────────────────────────

kora_register_sme() {
  # $1 = verifier, $2 = sme_address, $3 = risk_score (0-100)
  _invoke "$RISK_REGISTRY" register_sme \
    --verifier "$1" --sme "$2" --risk_score "$3"
}

kora_mint_invoice() {
  # $1=sme $2=debtor_hash(hex) $3=amount $4=currency $5=due_date $6=ipfs_cid $7=risk_score
  _invoke "$INVOICE_NFT" mint_invoice \
    --sme "$1" \
    --debtor_hash "$2" \
    --amount "$3" \
    --currency "$4" \
    --due_date "$5" \
    --ipfs_cid "$6" \
    --risk_score "$7"
}

kora_list_invoice() {
  # $1=seller $2=invoice_id $3=asking_price $4=face_value $5=token $6=deadline
  _invoke "$MARKETPLACE" list_invoice \
    --seller "$1" \
    --invoice_id "$2" \
    --asking_price "$3" \
    --face_value "$4" \
    --token "$5" \
    --funding_deadline "$6"
}

# ── Investor Flows ────────────────────────────────────────────────────────────

kora_fund_invoice() {
  # $1=investor $2=invoice_id $3=amount
  _invoke "$MARKETPLACE" fund_invoice \
    --investor "$1" \
    --invoice_id "$2" \
    --amount "$3"
}

# ── Repayment ─────────────────────────────────────────────────────────────────

kora_repay() {
  # $1=payer $2=invoice_id $3=token $4=amount
  _invoke "$POOL" repay \
    --payer "$1" \
    --invoice_id "$2" \
    --token "$3" \
    --amount "$4"
}

# ── Views ─────────────────────────────────────────────────────────────────────

kora_get_invoice() {
  stellar contract invoke \
    --id "$INVOICE_NFT" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- get_invoice --invoice_id "$1"
}

kora_get_pool() {
  stellar contract invoke \
    --id "$POOL" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- get_pool --invoice_id "$1"
}

kora_get_sme_profile() {
  stellar contract invoke \
    --id "$RISK_REGISTRY" \
    --rpc-url "$RPC_URL" \
    --network-passphrase "$NETWORK_PASSPHRASE" \
    -- get_sme_profile --sme "$1"
}

echo "Kora Protocol interaction helpers loaded (stellar CLI ${STELLAR_VERSION})."
echo "Contracts on $NETWORK:"
echo "  access_control : $ACCESS_CONTROL"
echo "  invoice_nft    : $INVOICE_NFT"
echo "  marketplace    : $MARKETPLACE"
echo "  financing_pool : $POOL"
echo "  treasury       : $TREASURY"
echo "  risk_registry  : $RISK_REGISTRY"
