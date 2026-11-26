#!/usr/bin/env bash
# One-command community node: indexer + API + health.
# Usage: ./packaging/community-node.sh [--check]   (--check only validates config)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${KORA_ENV_FILE:-$ROOT/.env}"
REQUIRED=(STELLAR_NETWORK SOROBAN_RPC_URL TREASURY_CONTRACT_ID
  CONTRACT_INVOICE_NFT CONTRACT_MARKETPLACE CONTRACT_FINANCING_POOL
  CONTRACT_RISK_REGISTRY CONTRACT_PRICE_ORACLE CONTRACT_GOVERNANCE
  CONTRACT_ACCESS_CONTROL)

if [ ! -f "$ENV_FILE" ]; then
  echo "ERROR: $ENV_FILE not found; copy packaging/.env.example" >&2
  exit 2
fi
missing=0
for key in "${REQUIRED[@]}"; do
  if ! grep -Eq "^${key}=[^[:space:]#]" "$ENV_FILE"; then
    echo "ERROR: $key is missing or empty in $ENV_FILE" >&2
    missing=1
  fi
done
[ "$missing" -eq 0 ] || exit 2
[ "${1:-}" != "--check" ] || { echo "config OK"; exit 0; }

# The compose file's env_file is ../.env, so a custom file is validate-only.
if [ "$ENV_FILE" != "$ROOT/.env" ]; then
  echo "ERROR: KORA_ENV_FILE is only supported with --check" >&2
  exit 2
fi
COMPOSE=(docker compose --env-file "$ENV_FILE" -f "$ROOT/packaging/docker-compose.yml" --profile community)
if grep -Eq '^KORA_INDEXER_IMAGE=[^[:space:]#]' "$ENV_FILE"; then
  "${COMPOSE[@]}" pull indexer
fi
"${COMPOSE[@]}" up -d --wait
