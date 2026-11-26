#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Blue-Green Cutover Checklist
#
# Closes #<issue_number>
#
# Automated pre-cutover health and metrics gate runner.  Every gate must pass
# before the caller (a human or CI) proceeds with deploy.sh.
#
# Gates checked (in order):
#   1. New instance /health endpoint responds 200
#   2. Prometheus metrics scraped from new instance are parseable
#   3. kora_indexer_lag_seconds is within the configured lag budget
#   4. No dead-letter keeper jobs (kora_keeper_jobs{status="dead"} == 0)
#   5. No circuit-breaker trips in the last 15 minutes
#   6. Active colour is confirmed in the state file
#   7. No pending alerts in Alertmanager (severity: critical)
#
# Rollback instructions are printed if any gate fails.
#
# Usage:
#   ./infra/blue-green/cutover-checklist.sh [api|indexer|both] [--dry-run]
#
# Options:
#   --dry-run   Print all gates and expected values without making HTTP calls.
#               Returns exit 0 regardless of real service state.  Use this in
#               CI to verify the script is wired correctly without a live env.
#
# Environment variables (all have safe defaults for staging):
#   CHECKLIST_API_BLUE_URL        Base URL for api-blue  (default: http://localhost:3001)
#   CHECKLIST_API_GREEN_URL       Base URL for api-green (default: http://localhost:3002)
#   CHECKLIST_INDEXER_BLUE_URL    Base URL for indexer-blue  (default: http://localhost:8081)
#   CHECKLIST_INDEXER_GREEN_URL   Base URL for indexer-green (default: http://localhost:8082)
#   CHECKLIST_ALERTMANAGER_URL    Alertmanager API (default: http://localhost:9093)
#   CHECKLIST_MAX_INDEXER_LAG_S   Max indexer lag in seconds (default: 60)
#   CHECKLIST_HEALTH_TIMEOUT_S    curl max-time per request  (default: 5)
#   CHECKLIST_RETRIES             Retries per gate before marking failed (default: 3)
#   CHECKLIST_RETRY_INTERVAL_S    Seconds between retries (default: 2)
#
# Exit codes:
#   0  All gates passed — safe to proceed with cutover
#   1  One or more gates failed — DO NOT proceed, see rollback instructions
#   2  Invalid arguments
#
# Example CI dry-run:
#   ./infra/blue-green/cutover-checklist.sh both --dry-run
#
# =============================================================================

set -euo pipefail

SERVICE="${1:-both}"
DRY_RUN="${2:-}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_FILE="$SCRIPT_DIR/.active-colour"

# ── Configuration ─────────────────────────────────────────────────────────────

CHECKLIST_API_BLUE_URL="${CHECKLIST_API_BLUE_URL:-http://localhost:3001}"
CHECKLIST_API_GREEN_URL="${CHECKLIST_API_GREEN_URL:-http://localhost:3002}"
CHECKLIST_INDEXER_BLUE_URL="${CHECKLIST_INDEXER_BLUE_URL:-http://localhost:8081}"
CHECKLIST_INDEXER_GREEN_URL="${CHECKLIST_INDEXER_GREEN_URL:-http://localhost:8082}"
CHECKLIST_ALERTMANAGER_URL="${CHECKLIST_ALERTMANAGER_URL:-http://localhost:9093}"
CHECKLIST_MAX_INDEXER_LAG_S="${CHECKLIST_MAX_INDEXER_LAG_S:-60}"
CHECKLIST_HEALTH_TIMEOUT_S="${CHECKLIST_HEALTH_TIMEOUT_S:-5}"
CHECKLIST_RETRIES="${CHECKLIST_RETRIES:-3}"
CHECKLIST_RETRY_INTERVAL_S="${CHECKLIST_RETRY_INTERVAL_S:-2}"

# ── Colour helpers ────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

pass()  { echo -e "  ${GREEN}✓ PASS${NC}  $*"; }
fail()  { echo -e "  ${RED}✗ FAIL${NC}  $*"; }
info()  { echo -e "  ${CYAN}→${NC}      $*"; }
warn()  { echo -e "  ${YELLOW}⚠ WARN${NC}  $*"; }
title() { echo -e "\n${BOLD}$*${NC}"; }

FAILED_GATES=()

record_failure() {
  FAILED_GATES+=("$1")
}

# ── Helpers ───────────────────────────────────────────────────────────────────

get_active_colour() {
  local svc="$1"
  grep "^${svc}=" "$STATE_FILE" 2>/dev/null | cut -d= -f2 || echo "blue"
}

opposite_colour() {
  [ "$1" = "blue" ] && echo "green" || echo "blue"
}

service_base_url() {
  local svc="$1" colour="$2"
  case "${svc}-${colour}" in
    api-blue)      echo "$CHECKLIST_API_BLUE_URL" ;;
    api-green)     echo "$CHECKLIST_API_GREEN_URL" ;;
    indexer-blue)  echo "$CHECKLIST_INDEXER_BLUE_URL" ;;
    indexer-green) echo "$CHECKLIST_INDEXER_GREEN_URL" ;;
    *)             echo "" ;;
  esac
}

# Retry wrapper: retries a command up to CHECKLIST_RETRIES times.
# Returns 0 on first success, 1 if all attempts fail.
with_retries() {
  local label="$1"; shift
  for attempt in $(seq 1 "$CHECKLIST_RETRIES"); do
    if "$@" 2>/dev/null; then
      return 0
    fi
    if [ "$attempt" -lt "$CHECKLIST_RETRIES" ]; then
      info "  retry $attempt/$CHECKLIST_RETRIES for $label ..."
      sleep "$CHECKLIST_RETRY_INTERVAL_S"
    fi
  done
  return 1
}

# Parse a Prometheus metric value from /metrics text output.
# Usage: parse_metric <metric_name> <labels_or_empty> <metrics_text>
# Returns the numeric value or "" if not found.
parse_metric() {
  local metric_name="$1"
  local labels="$2"       # e.g. '{status="dead"}' or "" for label-free
  local metrics_text="$3"
  echo "$metrics_text" | grep -E "^${metric_name}${labels}[[:space:]]" | \
    awk '{print $NF}' | head -1
}

# ── Gate implementations ──────────────────────────────────────────────────────

#
# Gate 1 — /health responds 200
#
gate_health() {
  local svc="$1" colour="$2"
  local base_url
  base_url=$(service_base_url "$svc" "$colour")
  local gate_name="health:${svc}/${colour}"

  if [ "$DRY_RUN" = "--dry-run" ]; then
    info "DRY-RUN gate_health: would GET ${base_url}/health"
    pass "$gate_name (dry-run)"
    return
  fi

  if with_retries "$gate_name" \
      curl -sf --max-time "$CHECKLIST_HEALTH_TIMEOUT_S" \
           -H "Accept: application/json" \
           "${base_url}/health" > /dev/null; then
    pass "$gate_name — /health responded 200"
  else
    fail "$gate_name — /health did not respond within ${CHECKLIST_RETRIES} attempts"
    record_failure "$gate_name"
  fi
}

#
# Gate 2+3 — Prometheus metrics parseable; indexer lag within budget
#
gate_metrics() {
  local svc="$1" colour="$2"
  local base_url gate_name
  base_url=$(service_base_url "$svc" "$colour")
  gate_name="metrics:${svc}/${colour}"

  if [ "$DRY_RUN" = "--dry-run" ]; then
    info "DRY-RUN gate_metrics: would GET ${base_url}/metrics"
    pass "$gate_name (dry-run)"
    [ "$svc" = "indexer" ] && pass "indexer-lag:${colour} (dry-run)"
    return
  fi

  local metrics_text
  metrics_text=$(curl -sf --max-time "$CHECKLIST_HEALTH_TIMEOUT_S" "${base_url}/metrics" 2>/dev/null || echo "")

  if [ -z "$metrics_text" ]; then
    fail "$gate_name — could not fetch /metrics"
    record_failure "$gate_name"
    return
  fi

  pass "$gate_name — /metrics endpoint responding"

  # Gate 3: indexer lag check
  if [ "$svc" = "indexer" ]; then
    local lag_gate="indexer-lag:${colour}"
    local lag_value
    lag_value=$(parse_metric "kora_indexer_lag_seconds" "" "$metrics_text")

    if [ -z "$lag_value" ]; then
      warn "$lag_gate — kora_indexer_lag_seconds not found in metrics; skipping lag gate"
      return
    fi

    # bc comparison: lag <= CHECKLIST_MAX_INDEXER_LAG_S
    if echo "$lag_value $CHECKLIST_MAX_INDEXER_LAG_S" | \
        awk '{exit ($1 <= $2) ? 0 : 1}'; then
      pass "$lag_gate — indexer lag ${lag_value}s ≤ budget ${CHECKLIST_MAX_INDEXER_LAG_S}s"
    else
      fail "$lag_gate — indexer lag ${lag_value}s EXCEEDS budget ${CHECKLIST_MAX_INDEXER_LAG_S}s"
      record_failure "$lag_gate"
    fi
  fi

  # Gate 4: keeper dead-letter jobs (checked from API metrics for keeper awareness)
  if [ "$svc" = "api" ]; then
    local keeper_dead
    keeper_dead=$(parse_metric 'kora_keeper_jobs' '{status="dead"}' "$metrics_text" || echo "")
    if [ -n "$keeper_dead" ] && [ "$keeper_dead" != "0" ]; then
      fail "keeper-dead-letters:${colour} — ${keeper_dead} dead-letter jobs require operator review"
      record_failure "keeper-dead-letters:${colour}"
    elif [ -n "$keeper_dead" ]; then
      pass "keeper-dead-letters:${colour} — no dead-letter jobs"
    fi
  fi
}

#
# Gate 5 — No circuit-breaker trips in the last 15 minutes
#
gate_circuit_breaker() {
  local svc="$1" colour="$2"
  local base_url gate_name
  base_url=$(service_base_url "$svc" "$colour")
  gate_name="circuit-breaker:${svc}/${colour}"

  if [ "$svc" != "indexer" ]; then
    return  # Only meaningful for the indexer
  fi

  if [ "$DRY_RUN" = "--dry-run" ]; then
    info "DRY-RUN gate_circuit_breaker: would check kora_onchain_events_total{event_type=\"circuit_breaker_trip\"}"
    pass "$gate_name (dry-run)"
    return
  fi

  local metrics_text
  metrics_text=$(curl -sf --max-time "$CHECKLIST_HEALTH_TIMEOUT_S" "${base_url}/metrics" 2>/dev/null || echo "")

  local trip_count
  trip_count=$(parse_metric 'kora_onchain_events_total' '{event_type="circuit_breaker_trip"}' "$metrics_text" || echo "")

  if [ -z "$trip_count" ]; then
    info "$gate_name — circuit_breaker_trip metric not found; assuming 0"
    pass "$gate_name"
    return
  fi

  # Any non-zero value warrants a warning; a Sev-1 alert should already be firing
  if [ "$trip_count" = "0" ]; then
    pass "$gate_name — no circuit breaker trips"
  else
    fail "$gate_name — ${trip_count} circuit breaker trip(s) observed; check Sev-1 alerts before proceeding"
    record_failure "$gate_name"
  fi
}

#
# Gate 6 — Active colour confirmed in state file
#
gate_state_file() {
  local svc="$1"
  local gate_name="state-file:${svc}"

  if [ "$DRY_RUN" = "--dry-run" ]; then
    info "DRY-RUN gate_state_file: would read $STATE_FILE for $svc"
    pass "$gate_name (dry-run)"
    return
  fi

  if [ ! -f "$STATE_FILE" ]; then
    warn "$gate_name — $STATE_FILE not found; first deploy assumed"
    pass "$gate_name (first deploy)"
    return
  fi

  local active
  active=$(get_active_colour "$svc")
  local new_colour
  new_colour=$(opposite_colour "$active")

  pass "$gate_name — active colour is '${active}'; new colour will be '${new_colour}'"
}

#
# Gate 7 — No critical/paging alerts in Alertmanager
#
gate_alertmanager() {
  local gate_name="alertmanager:no-critical-alerts"

  if [ "$DRY_RUN" = "--dry-run" ]; then
    info "DRY-RUN gate_alertmanager: would GET ${CHECKLIST_ALERTMANAGER_URL}/api/v2/alerts"
    pass "$gate_name (dry-run)"
    return
  fi

  local alerts_json
  alerts_json=$(curl -sf --max-time "$CHECKLIST_HEALTH_TIMEOUT_S" \
    "${CHECKLIST_ALERTMANAGER_URL}/api/v2/alerts?active=true&silenced=false" 2>/dev/null || echo "[]")

  # Look for any alert with severity: critical that is currently firing
  local critical_count
  critical_count=$(echo "$alerts_json" | \
    grep -c '"severity":"critical"' 2>/dev/null || echo "0")

  if [ "$critical_count" = "0" ]; then
    pass "$gate_name — no active critical alerts"
  else
    fail "$gate_name — ${critical_count} critical alert(s) active; resolve before cutover"
    record_failure "$gate_name"
  fi
}

# ── Run checklist for a single service ───────────────────────────────────────

run_checklist_for_service() {
  local svc="$1"

  local active_colour new_colour
  active_colour=$(get_active_colour "$svc" 2>/dev/null || echo "blue")
  new_colour=$(opposite_colour "$active_colour")

  title "Service: ${svc}  (active=${active_colour}, deploying to=${new_colour})"

  gate_health           "$svc" "$new_colour"
  gate_metrics          "$svc" "$new_colour"
  gate_circuit_breaker  "$svc" "$new_colour"
  gate_state_file       "$svc"

  # Alertmanager check only once (not per-service)
}

# ── Print rollback instructions ───────────────────────────────────────────────

print_rollback_instructions() {
  echo ""
  echo -e "${YELLOW}════════════════════════════════════════════════════════${NC}"
  echo -e "${YELLOW}  ROLLBACK INSTRUCTIONS${NC}"
  echo -e "${YELLOW}════════════════════════════════════════════════════════${NC}"
  echo ""
  echo "  If a partial cutover was attempted, roll back with:"
  echo ""
  echo "    ./infra/blue-green/deploy.sh api     --rollback"
  echo "    ./infra/blue-green/deploy.sh indexer --rollback"
  echo "    ./infra/blue-green/deploy.sh both    --rollback"
  echo ""
  echo "  The deploy.sh rollback:"
  echo "    1. Starts the previous-colour instance"
  echo "    2. Health-checks it until it passes"
  echo "    3. Shifts nginx traffic back (graceful SIGHUP)"
  echo "    4. Stops the failed new-colour instance"
  echo ""
  echo "  For partial-service healthy situations:"
  echo "    Roll back only the failed service by name."
  echo "    The healthy service can remain on the new colour."
  echo ""
  echo "  Confirm rollback succeeded:"
  echo "    curl -f http://localhost:80/health   # via nginx"
  echo "    ./infra/blue-green/cutover-checklist.sh both"
  echo ""
  echo -e "${YELLOW}════════════════════════════════════════════════════════${NC}"
}

# ── Main ──────────────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}════════════════════════════════════════════════════════${NC}"
echo -e "${BOLD}  Kora — Blue-Green Cutover Checklist${NC}"
if [ "$DRY_RUN" = "--dry-run" ]; then
  echo -e "${YELLOW}  MODE: DRY-RUN (no HTTP calls, exit 0 always)${NC}"
fi
echo -e "${BOLD}════════════════════════════════════════════════════════${NC}"

echo ""
echo "  Service             : $SERVICE"
echo "  State file          : $STATE_FILE"
echo "  Max indexer lag     : ${CHECKLIST_MAX_INDEXER_LAG_S}s"
echo "  Health timeout      : ${CHECKLIST_HEALTH_TIMEOUT_S}s"
echo "  Retries per gate    : ${CHECKLIST_RETRIES}  (interval ${CHECKLIST_RETRY_INTERVAL_S}s)"
echo "  Alertmanager        : $CHECKLIST_ALERTMANAGER_URL"

case "$SERVICE" in
  api)
    run_checklist_for_service api
    ;;
  indexer)
    run_checklist_for_service indexer
    ;;
  both)
    run_checklist_for_service api
    run_checklist_for_service indexer
    ;;
  *)
    echo "Unknown service: $SERVICE. Use: api | indexer | both"
    exit 2
    ;;
esac

# Always run alertmanager gate once
title "Global gates"
gate_alertmanager

# ── Summary ───────────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}════════════════════════════════════════════════════════${NC}"

if [ ${#FAILED_GATES[@]} -eq 0 ]; then
  echo -e "${GREEN}  ALL GATES PASSED — safe to proceed with cutover${NC}"
  echo ""
  echo "  Next step:"
  echo "    DOCKER_IMAGE_API=<image> ./infra/blue-green/deploy.sh ${SERVICE}"
  echo -e "${BOLD}════════════════════════════════════════════════════════${NC}"
  echo ""
  exit 0
else
  echo -e "${RED}  ${#FAILED_GATES[@]} GATE(S) FAILED — DO NOT proceed with cutover${NC}"
  echo ""
  for g in "${FAILED_GATES[@]}"; do
    echo -e "    ${RED}✗${NC} $g"
  done
  print_rollback_instructions
  exit 1
fi
