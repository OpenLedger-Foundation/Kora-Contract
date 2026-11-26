#!/usr/bin/env bash
# Tests config validation and the community compose profile.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
check() { KORA_ENV_FILE="$1" "$ROOT/packaging/community-node.sh" --check >/dev/null 2>&1; }
fail() { echo "FAIL: $1" >&2; exit 1; }

! check "$TMP/absent" || fail "absent env accepted"
! check "$ROOT/packaging/.env.example" || fail "empty addresses accepted"
sed -E 's/^([A-Z_]+)=$/\1=CTEST/' "$ROOT/packaging/.env.example" > "$TMP/env"
check "$TMP/env" || fail "valid env rejected"
check "$TMP/env" || fail "re-run rejected"
sed '/^SOROBAN_RPC_URL=/d' "$TMP/env" > "$TMP/norpc"
! check "$TMP/norpc" || fail "missing RPC accepted"

# Render with a copy so the repository's own .env is never touched.
mkdir "$TMP/packaging"; cp "$ROOT/packaging/docker-compose.yml" "$TMP/packaging/"; cp "$TMP/env" "$TMP/.env"
services() { docker compose --env-file "$TMP/.env" -f "$TMP/packaging/docker-compose.yml" "$@" config --services | sort | xargs; }
[ "$(services --profile community)" = "api health indexer postgres" ] || fail "profile services"
[ "$(services)" = "indexer postgres" ] || fail "default services changed"
echo "community node tests OK"
