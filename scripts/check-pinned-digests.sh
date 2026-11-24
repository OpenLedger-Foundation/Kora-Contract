#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Pinned base-image digest gate (issue #1174)
#
# Fails when any `FROM` in the given Dockerfiles uses a floating tag instead of
# an immutable `@sha256:<64 hex>` digest. Build-stage aliases (FROM builder)
# and `scratch` are allowed.
#
# Usage: scripts/check-pinned-digests.sh [Dockerfile ...]
#   default: packaging/Dockerfile packaging/Dockerfile.wasm
# Exit codes: 0 ok, 1 unpinned image found, 2 config error (no/missing files)
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ "$#" -eq 0 ]; then
  set -- "$ROOT/packaging/Dockerfile" "$ROOT/packaging/Dockerfile.wasm"
fi
bad=0
for f in "$@"; do
  if [ ! -s "$f" ]; then echo "ERR_CONFIG_MISSING: $f missing or empty" >&2; exit 2; fi
  stages=" "
  froms=0
  while read -r lineno image alias; do
    froms=$((froms+1))
    if [[ "$image" == "scratch" || "$stages" == *" $image "* ]]; then :
    elif [[ "$image" =~ @sha256:[0-9a-f]{64}$ ]]; then :
    else echo "ERR_UNPINNED_IMAGE: $f:$lineno '$image' is not pinned by @sha256 digest" >&2; bad=1; fi
    [ -n "$alias" ] && stages="$stages$alias "
  done < <(grep -inE '^[[:space:]]*FROM[[:space:]]' "$f" \
    | sed -E 's/^([0-9]+):[[:space:]]*[Ff][Rr][Oo][Mm][[:space:]]+(--[^[:space:]]+[[:space:]]+)*([^[:space:]]+)([[:space:]]+[Aa][Ss][[:space:]]+([^[:space:]]+))?.*/\1 \3 \5/')
  if [ "$froms" -eq 0 ]; then echo "ERR_CONFIG_SCHEMA: $f has no FROM instruction" >&2; exit 2; fi
done
[ "$bad" -eq 0 ] && echo "all base images pinned by digest"
exit "$bad"
