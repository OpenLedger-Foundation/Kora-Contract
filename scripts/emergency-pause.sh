#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Emergency Pause Script
# Issue #812: Emergency Pause Drill and Incident Response Automation
#
# PURPOSE
#   Trigger a protocol-wide emergency pause through the access_control contract,
#   collect quorum signatures from configured signers, and emit a stakeholder
#   notification the moment the pause takes effect.
#
# USAGE
#   # Dry-run (no on-chain transactions):
#   ./scripts/emergency-pause.sh --dry-run
#
#   # Single-admin environment (no multisig):
#   ADMIN_SECRET=<secret> ./scripts/emergency-pause.sh --env testnet
#
#   # Multisig environment (collect N-of-M approvals):
#   SIGNER_SECRET=<signer1_secret> ./scripts/emergency-pause.sh \
#       --env testnet --propose
#
#   SIGNER_SECRET=<signer2_secret> ./scripts/emergency-pause.sh \
#       --env testnet --approve --proposal-id <id>
#
#   SIGNER_SECRET=<signer2_secret> ./scripts/emergency-pause.sh \
#       --env testnet --execute --proposal-id <id>
#
#   # Guardian emergency pause (single guardian, no multisig required):
#   GUARDIAN_SECRET=<guardian_secret> ./scripts/emergency-pause.sh \
#       --env testnet --guardian
#
# ENVIRONMENT VARIABLES (required, or set in .env.testnet / .env.mainnet)
#   ADMIN_SECRET         — Stellar secret key for the admin address
#   SIGNER_SECRET        — Stellar secret key for a multisig signer
#   GUARDIAN_SECRET      — Stellar secret key for a guardian-role address
#   ACCESS_CONTROL       — Contract ID of the access_control contract
#   NOTIFICATION_WEBHOOK — Webhook URL for stakeholder notifications (optional)
#   STELLAR_RPC_URL      — Soroban RPC endpoint (default: Stellar testnet)
#   NETWORK_PASSPHRASE   — Stellar network passphrase
#
# OUTPUTS
#   - On-chain pause transaction hash
#   - Webhook notification payload (if NOTIFICATION_WEBHOOK is set)
#   - Timestamped log entry appended to logs/emergency-pause.log
#   - Exit code 0 on success, non-zero on any failure
#
# DEPENDENCIES
#   stellar CLI >= 20.0  (https://developers.stellar.org/docs/tools/stellar-cli)
#   curl                 (for webhook notifications)
#   jq                   (for JSON processing)
#
# SECURITY NOTES
#   - This script NEVER bypasses the quorum requirement.
#     In single-admin mode, pause() is called directly only when no multisig
#     is configured on access_control.
#   - In multisig mode, propose/approve/execute steps are separate invocations
#     so each signer operates from their own terminal/machine.
#   - Secrets are read from environment variables; never hardcoded.
# =============================================================================

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
LOG_DIR="${REPO_ROOT}/logs"
LOG_FILE="${LOG_DIR}/emergency-pause.log"
TIMESTAMP="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

ENV="testnet"
MODE=""          # --propose | --approve | --execute | --guardian | (empty = direct admin)
PROPOSAL_ID=""
DRY_RUN=false
NETWORK_PASSPHRASE="${NETWORK_PASSPHRASE:-Test SDF Network ; September 2015}"
STELLAR_RPC_URL="${STELLAR_RPC_URL:-https://soroban-testnet.stellar.org}"

# ── Colours ───────────────────────────────────────────────────────────────────

RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Logging ───────────────────────────────────────────────────────────────────

mkdir -p "${LOG_DIR}"

log() {
    local level="$1"; shift
    local msg="$*"
    echo -e "[${TIMESTAMP}] [${level}] ${msg}" | tee -a "${LOG_FILE}"
}

info()    { log "${CYAN}INFO${NC}"    "$@"; }
warn()    { log "${YELLOW}WARN${NC}"  "$@"; }
success() { log "${GREEN}OK${NC}"    "$@"; }
error()   { log "${RED}ERROR${NC}"   "$@" >&2; }
fatal()   { error "$@"; exit 1; }

# ── Argument parsing ──────────────────────────────────────────────────────────

usage() {
    grep '^# ' "${BASH_SOURCE[0]}" | sed 's/^# //' | head -50
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --env)           ENV="$2";          shift 2 ;;
        --propose)       MODE="propose";    shift   ;;
        --approve)       MODE="approve";    shift   ;;
        --execute)       MODE="execute";    shift   ;;
        --guardian)      MODE="guardian";   shift   ;;
        --proposal-id)   PROPOSAL_ID="$2";  shift 2 ;;
        --dry-run)       DRY_RUN=true;      shift   ;;
        --help|-h)       usage                       ;;
        *) fatal "Unknown argument: $1"              ;;
    esac
done

# ── Load per-environment config ───────────────────────────────────────────────

ENV_FILE="${REPO_ROOT}/.env.${ENV}"
if [[ -f "${ENV_FILE}" ]]; then
    # shellcheck disable=SC1090
    source "${ENV_FILE}"
    info "Loaded environment config from ${ENV_FILE}"
fi

# ── Validate required variables ───────────────────────────────────────────────

if [[ "${DRY_RUN}" == "true" ]]; then
  ACCESS_CONTROL="${ACCESS_CONTROL:-CDUMMYACCESSCONTROL0000000000000000000000000000000000000000000}"
  ADMIN_SECRET="${ADMIN_SECRET:-SDUMMYADMINSECRET00000000000000000000000000000000000000000000}"
  GUARDIAN_SECRET="${GUARDIAN_SECRET:-SDUMMYGUARDIANSECRET00000000000000000000000000000000000000000}"
  SIGNER_SECRET="${SIGNER_SECRET:-SDUMMYSIGNERSECRET0000000000000000000000000000000000000000000}"
fi
: "${ACCESS_CONTROL:?ACCESS_CONTROL contract ID must be set}"

# Determine which secret to use based on mode
case "${MODE}" in
    guardian)
        : "${GUARDIAN_SECRET:?GUARDIAN_SECRET must be set for --guardian mode}"
        ACTIVE_SECRET="${GUARDIAN_SECRET}"
        ;;
    propose|approve|execute)
        : "${SIGNER_SECRET:?SIGNER_SECRET must be set for multisig mode}"
        ACTIVE_SECRET="${SIGNER_SECRET}"
        ;;
    "")
        : "${ADMIN_SECRET:?ADMIN_SECRET must be set for direct admin mode}"
        ACTIVE_SECRET="${ADMIN_SECRET}"
        ;;
esac

# ── Derive addresses from secrets ────────────────────────────────────────────

get_address() {
    stellar keys address --secret-key "$1" 2>/dev/null || echo "UNKNOWN"
}

ACTIVE_ADDRESS="$(get_address "${ACTIVE_SECRET}")"
info "Active signer address: ${ACTIVE_ADDRESS}"

# ── Stellar CLI invocation helper ─────────────────────────────────────────────

stellar_invoke() {
    local fn_name="$1"; shift
    local args=("$@")

    local cmd=(
        stellar contract invoke
        --id        "${ACCESS_CONTROL}"
        --source    "${ACTIVE_SECRET}"
        --rpc-url   "${STELLAR_RPC_URL}"
        --network-passphrase "${NETWORK_PASSPHRASE}"
        --
        "${fn_name}"
        "${args[@]}"
    )

    if [[ "${DRY_RUN}" == "true" ]]; then
        info "[DRY-RUN] Would execute: ${cmd[*]}"
        echo "DRY_RUN_TXHASH_PLACEHOLDER"
        return 0
    fi

    info "Invoking: ${fn_name} ${args[*]}"
    "${cmd[@]}"
}

# ── Verify current pause state ────────────────────────────────────────────────

check_pause_state() {
    local state
    state="$(stellar_invoke is_paused)" || true
    echo "${state}"
}

# ── Notification helper ───────────────────────────────────────────────────────

send_notification() {
    local event="$1"
    local details="$2"

    if [[ -z "${NOTIFICATION_WEBHOOK:-}" ]]; then
        warn "NOTIFICATION_WEBHOOK not set — skipping stakeholder notification"
        return 0
    fi

    local payload
    payload="$(jq -n \
        --arg event   "${event}" \
        --arg details "${details}" \
        --arg env     "${ENV}" \
        --arg ts      "${TIMESTAMP}" \
        --arg actor   "${ACTIVE_ADDRESS}" \
        --arg contract "${ACCESS_CONTROL}" \
        '{
            event:    $event,
            details:  $details,
            env:      $env,
            timestamp: $ts,
            actor:    $actor,
            contract: $contract
        }')"

    if [[ "${DRY_RUN}" == "true" ]]; then
        info "[DRY-RUN] Would POST notification: ${payload}"
        return 0
    fi

    local http_code
    http_code="$(curl -s -o /dev/null -w "%{http_code}" \
        -X POST \
        -H "Content-Type: application/json" \
        -d "${payload}" \
        "${NOTIFICATION_WEBHOOK}")" || true

    if [[ "${http_code}" =~ ^2 ]]; then
        success "Stakeholder notification sent (HTTP ${http_code})"
    else
        warn "Notification webhook returned HTTP ${http_code} — check manually"
    fi
}

# ── Pause verification loop ───────────────────────────────────────────────────

wait_for_pause_confirmation() {
  if [[ "${DRY_RUN}" == "true" ]]; then
    info "[DRY-RUN] Simulating pause confirmation verification..."
    success "[DRY-RUN] Protocol pause verification simulated successfully"
    return 0
  fi
    local max_attempts=10
    local attempt=0
    info "Verifying pause took effect..."
    while [[ ${attempt} -lt ${max_attempts} ]]; do
        local state
        state="$(check_pause_state)"
        if [[ "${state}" == *"true"* ]]; then
            success "Protocol is PAUSED (confirmed on-chain)"
            return 0
        fi
        attempt=$((attempt + 1))
        warn "Pause not yet confirmed (attempt ${attempt}/${max_attempts}) — retrying in 3s"
        sleep 3
    done
    fatal "Protocol pause could not be confirmed after ${max_attempts} attempts"
}

# ── Record drill results ──────────────────────────────────────────────────────

record_drill_result() {
    local phase="$1"
    local txhash="$2"
    local elapsed="$3"

    cat >> "${LOG_FILE}" <<EOF

=== DRILL RECORD: ${TIMESTAMP} ===
  Phase:     ${phase}
  Network:   ${ENV}
  Actor:     ${ACTIVE_ADDRESS}
  Contract:  ${ACCESS_CONTROL}
  TxHash:    ${txhash}
  Elapsed:   ${elapsed}s
  Mode:      ${MODE:-direct-admin}
EOF
    info "Drill result recorded in ${LOG_FILE}"
}

# ── Main execution ────────────────────────────────────────────────────────────

START_TIME="$(date +%s)"

echo ""
echo -e "${BOLD}${RED}============================================================${NC}"
echo -e "${BOLD}${RED}  KORA PROTOCOL — EMERGENCY PAUSE${NC}"
echo -e "${BOLD}${RED}  Network: ${ENV} | Mode: ${MODE:-direct-admin}${NC}"
if [[ "${DRY_RUN}" == "true" ]]; then
    echo -e "${BOLD}${YELLOW}  *** DRY RUN — No transactions will be submitted ***${NC}"
fi
echo -e "${BOLD}${RED}============================================================${NC}"
echo ""

log "INFO" "=== Emergency Pause started: network=${ENV} mode=${MODE:-direct-admin} actor=${ACTIVE_ADDRESS} ==="

case "${MODE}" in

    # ── Guardian emergency pause (fastest path) ───────────────────────────────
    guardian)
        info "Guardian emergency pause: no multisig required"
        TXHASH="$(stellar_invoke emergency_pause_by_guardian \
            --guardian "${ACTIVE_ADDRESS}")"
        ELAPSED=$(( $(date +%s) - START_TIME ))
        success "Guardian pause transaction submitted: ${TXHASH}"
        wait_for_pause_confirmation
        send_notification \
            "PROTOCOL_PAUSED_BY_GUARDIAN" \
            "Guardian ${ACTIVE_ADDRESS} triggered emergency pause on ${ENV}. TxHash: ${TXHASH}"
        record_drill_result "guardian-pause" "${TXHASH}" "${ELAPSED}"
        ;;

    # ── Multisig: Step 1 — Propose Pause ─────────────────────────────────────
    propose)
        info "Proposing Pause action through multisig..."
        # AdminAction::Pause is represented as the string "Pause" in the CLI
        TXHASH="$(stellar_invoke propose_action \
            --proposer "${ACTIVE_ADDRESS}" \
            --action   '{"Pause": null}')"
        ELAPSED=$(( $(date +%s) - START_TIME ))
        success "Pause proposal submitted: ${TXHASH}"

        # Extract the proposal ID from the transaction result
        RETURNED_ID="$(echo "${TXHASH}" | grep -oP '"id":\s*\K[0-9]+' | head -1 || echo "SEE_TX")"
        success "Proposal ID: ${RETURNED_ID} — share with other signers to collect approvals"
        send_notification \
            "PAUSE_PROPOSED" \
            "Pause proposal ${RETURNED_ID} created by ${ACTIVE_ADDRESS} on ${ENV}. Collect approvals. TxHash: ${TXHASH}"
        record_drill_result "propose-pause" "${TXHASH}" "${ELAPSED}"
        echo ""
        echo -e "${YELLOW}Next step: other signers run:${NC}"
        echo -e "  SIGNER_SECRET=<key> ./scripts/emergency-pause.sh --env ${ENV} --approve --proposal-id ${RETURNED_ID}"
        ;;

    # ── Multisig: Step 2 — Approve ────────────────────────────────────────────
    approve)
        [[ -z "${PROPOSAL_ID}" ]] && fatal "--proposal-id is required for --approve"
        info "Approving proposal ${PROPOSAL_ID}..."
        TXHASH="$(stellar_invoke approve_action \
            --approver    "${ACTIVE_ADDRESS}" \
            --proposal_id "${PROPOSAL_ID}")"
        ELAPSED=$(( $(date +%s) - START_TIME ))
        success "Approval submitted: ${TXHASH}"
        send_notification \
            "PAUSE_APPROVAL_COLLECTED" \
            "Signer ${ACTIVE_ADDRESS} approved proposal ${PROPOSAL_ID} on ${ENV}. TxHash: ${TXHASH}"
        record_drill_result "approve-pause" "${TXHASH}" "${ELAPSED}"
        echo ""
        echo -e "${YELLOW}Once threshold is reached, execute with:${NC}"
        echo -e "  SIGNER_SECRET=<key> ./scripts/emergency-pause.sh --env ${ENV} --execute --proposal-id ${PROPOSAL_ID}"
        ;;

    # ── Multisig: Step 3 — Execute ────────────────────────────────────────────
    execute)
        [[ -z "${PROPOSAL_ID}" ]] && fatal "--proposal-id is required for --execute"
        info "Executing proposal ${PROPOSAL_ID} (pause action)..."
        TXHASH="$(stellar_invoke execute_action \
            --executor    "${ACTIVE_ADDRESS}" \
            --proposal_id "${PROPOSAL_ID}")"
        ELAPSED=$(( $(date +%s) - START_TIME ))
        success "Execute transaction submitted: ${TXHASH}"
        wait_for_pause_confirmation
        send_notification \
            "PROTOCOL_PAUSED_MULTISIG" \
            "Protocol PAUSED via multisig proposal ${PROPOSAL_ID} on ${ENV}. Executor: ${ACTIVE_ADDRESS}. TxHash: ${TXHASH}"
        record_drill_result "execute-pause" "${TXHASH}" "${ELAPSED}"
        ;;

    # ── Direct admin pause (no multisig configured) ───────────────────────────
    "")
        info "Direct admin pause (single-key, no multisig)..."
        warn "Ensure no multisig is configured; if multisig is active, use --propose/--approve/--execute instead"
        TXHASH="$(stellar_invoke pause \
            --admin "${ACTIVE_ADDRESS}")"
        ELAPSED=$(( $(date +%s) - START_TIME ))
        success "Pause transaction submitted: ${TXHASH}"
        wait_for_pause_confirmation
        send_notification \
            "PROTOCOL_PAUSED_DIRECT" \
            "Protocol PAUSED by admin ${ACTIVE_ADDRESS} on ${ENV}. TxHash: ${TXHASH}"
        record_drill_result "direct-pause" "${TXHASH}" "${ELAPSED}"
        ;;

    *)
        fatal "Unknown mode: ${MODE}"
        ;;
esac

TOTAL_ELAPSED=$(( $(date +%s) - START_TIME ))
echo ""
echo -e "${GREEN}${BOLD}=== Pause procedure complete ===${NC}"
echo -e "  Total elapsed: ${TOTAL_ELAPSED}s"
echo -e "  Log:           ${LOG_FILE}"
echo ""

# ── Repayment exemption reminder ──────────────────────────────────────────────
echo -e "${CYAN}REMINDER: While paused, the following remain LIVE:${NC}"
echo -e "  • repay()          — SMEs can still repay outstanding invoices"
echo -e "  • cancel_listing() — investors can exit unfunded positions"
echo -e "  • set_repaid()     — pool can mark invoices as repaid"
echo ""
echo -e "${CYAN}The following are BLOCKED:${NC}"
echo -e "  • mint_invoice, list_invoice, fund_invoice"
echo -e "  • record_position, release_funds"
echo -e "  • mark_default"
echo ""

exit 0
