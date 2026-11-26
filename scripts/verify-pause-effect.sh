#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Pause-effect verification (issue #1175)
#
# After an emergency pause (or drill), query `is_paused` on EVERY target
# contract and emit Prometheus textfile metrics proving the pause took effect:
#
#   kora_pause_effective{target="<name>",contract="<id>"}  1 paused / 0 not
#   kora_pause_probe_error{target="<name>",contract="<id>"} 1 if query failed
#   kora_pause_drill_targets_total                         number of targets
#   kora_pause_drill_unpaused_targets                      not-paused + errored
#   kora_pause_drill_last_run_timestamp_seconds            unix time
#
# Alerts KoraPauseNotEffective / KoraPauseProbeFailed in
# infra/monitoring/alerts.yml fire on these series.
#
# Targets: PAUSE_TARGETS="name=CONTRACT_ID,name=CONTRACT_ID" or --targets-file
# (one `name=CONTRACT_ID` per line, # comments allowed).
#
# Usage:
#   PAUSE_TARGETS=... ./scripts/verify-pause-effect.sh [--targets-file f] [--out metrics.prom]
#
# Env:
#   PAUSE_TARGETS     targets (see above)
#   PAUSE_METRICS_OUT output file (default: stdout); written atomically
#   PAUSE_STATE_CMD   override probe command; invoked as `$PAUSE_STATE_CMD <contract_id>`
#                     and must print true/false (used by tests & dry runs)
#   STELLAR_NETWORK   network for the default stellar CLI probe (default testnet)
#   STELLAR_SOURCE    source account for the default probe
#
# Exit: 0 all paused, 1 at least one target not paused / probe failed,
#       2 config error (no targets, malformed entry, duplicate target)
# =============================================================================
set -euo pipefail
TARGETS_FILE=""; OUT="${PAUSE_METRICS_OUT:-}"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --targets-file) TARGETS_FILE="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    -h|--help) sed -n '2,36p' "$0"; exit 0 ;;
    *) echo "ERR_USAGE: unknown argument $1" >&2; exit 2 ;;
  esac
done

raw="${PAUSE_TARGETS:-}"
if [ -n "$TARGETS_FILE" ]; then
  [ -r "$TARGETS_FILE" ] || { echo "ERR_CONFIG_MISSING: $TARGETS_FILE not readable" >&2; exit 2; }
  raw="$(grep -vE '^[[:space:]]*(#|$)' "$TARGETS_FILE" | tr '\n' ',')"
fi
IFS=',' read -r -a entries <<<"$raw"
names=(); ids=()
for e in "${entries[@]}"; do
  e="$(echo "$e" | tr -d '[:space:]')"; [ -z "$e" ] && continue
  if [[ ! "$e" =~ ^([A-Za-z0-9_-]+)=([A-Z0-9]+)$ ]]; then echo "ERR_CONFIG_SCHEMA: malformed target '$e' (want name=CONTRACT_ID)" >&2; exit 2; fi
  for n in "${names[@]:-}"; do [ "$n" = "${BASH_REMATCH[1]}" ] && { echo "ERR_CONFIG_SCHEMA: duplicate target ${BASH_REMATCH[1]}" >&2; exit 2; }; done
  names+=("${BASH_REMATCH[1]}"); ids+=("${BASH_REMATCH[2]}")
done
[ "${#names[@]}" -gt 0 ] || { echo "ERR_CONFIG_MISSING: no pause targets configured (PAUSE_TARGETS or --targets-file)" >&2; exit 2; }

probe() {
  if [ -n "${PAUSE_STATE_CMD:-}" ]; then $PAUSE_STATE_CMD "$1"
  else stellar contract invoke --id "$1" --network "${STELLAR_NETWORK:-testnet}" \
         ${STELLAR_SOURCE:+--source "$STELLAR_SOURCE"} -- is_paused; fi
}

body="# HELP kora_pause_effective 1 if is_paused returned true for the target during the drill.
# TYPE kora_pause_effective gauge
"
errs="# HELP kora_pause_probe_error 1 if the is_paused probe failed for the target.
# TYPE kora_pause_probe_error gauge
"
unpaused=0
for i in "${!names[@]}"; do
  n="${names[$i]}"; id="${ids[$i]}"; eff=0; err=0
  if state="$(probe "$id" 2>/dev/null)"; then
    state="$(echo "$state" | tr -d '[:space:]"' | tr 'A-Z' 'a-z')"
    case "$state" in true) eff=1 ;; false) ;; *) err=1 ;; esac
  else err=1; fi
  [ "$eff" -eq 1 ] || { unpaused=$((unpaused+1)); echo "NOT_PAUSED: $n ($id) state=${state:-<probe failed>}" >&2; }
  body+="kora_pause_effective{target=\"$n\",contract=\"$id\"} $eff
"
  errs+="kora_pause_probe_error{target=\"$n\",contract=\"$id\"} $err
"
done
metrics="$body$errs# HELP kora_pause_drill_targets_total Number of contracts the pause drill verified.
# TYPE kora_pause_drill_targets_total gauge
kora_pause_drill_targets_total ${#names[@]}
# HELP kora_pause_drill_unpaused_targets Targets that were not confirmed paused.
# TYPE kora_pause_drill_unpaused_targets gauge
kora_pause_drill_unpaused_targets $unpaused
# HELP kora_pause_drill_last_run_timestamp_seconds Unix time of the last verification run.
# TYPE kora_pause_drill_last_run_timestamp_seconds gauge
kora_pause_drill_last_run_timestamp_seconds $(date +%s)
"
if [ -n "$OUT" ]; then tmp="$OUT.tmp.$$"; printf '%s' "$metrics" > "$tmp"; mv "$tmp" "$OUT"; else printf '%s' "$metrics"; fi
[ "$unpaused" -eq 0 ] && echo "pause verified on ${#names[@]}/${#names[@]} targets" >&2
[ "$unpaused" -eq 0 ]
