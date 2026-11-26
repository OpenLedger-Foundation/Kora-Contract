#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Key Rotation Dry-Run / Rehearsal Script
# Issue #1110: Key rotation procedure dry-run script for admin/oracle roles
#
# PURPOSE
#   Rehearse the admin / oracle-feeder rotations described in
#   docs/KEY_ROTATION_PROCEDURE.md. The script validates the CURRENT key
#   against on-chain state, validates the NEXT key, prints the exact rotation
#   plan (contract calls, signers, order) and writes an audit log. It is
#   DRY-RUN BY DEFAULT: nothing is submitted unless --execute is passed.
#
# USAGE
#   # Admin rotation rehearsal (read-only):
#   ./scripts/key-rotation-dry-run.sh --env testnet --role admin \
#       --contract treasury --current G...OLD --next G...NEW
#
#   # Oracle feeder rotation rehearsal (read-only):
#   ./scripts/key-rotation-dry-run.sh --env testnet --role oracle \
#       --contract price_oracle --signer G...ORACLE_ADMIN \
#       --current G...OLD_FEEDER --next G...NEW_FEEDER
#
#   # Submit the steps signed by the current key holder (never the default):
#   ROTATION_SIGNER_SECRET=<secret> ./scripts/key-rotation-dry-run.sh \
#       --env testnet --role admin --contract treasury \
#       --current G...OLD --next G...NEW --execute
#
# OPTIONS
#   --env <name>          testnet (default) | futurenet | local | mainnet
#   --role <role>         admin | oracle
#   --contract <name>     access_control | treasury | risk_registry |
#                         parameter_registry | price_oracle
#   --contract-id <id>    Contract ID (defaults to <NAME>_CONTRACT from env /
#                         scripts/contracts.env)
#   --current <G...>      Key currently holding the role
#   --next <G...>         Key that will hold the role after rotation
#   --signer <G...>       Oracle admin authorising feeder changes (oracle role)
#   --execute             Submit the current-key-signed steps (default: dry-run)
#   --accept-unverified   Allow --execute when the contract exposes no getter
#                         to verify the current key on-chain (oracle feeders,
#                         price_oracle admin)
#   --log-file <path>     Audit log path (default: logs/key-rotation.log)
#
# ENVIRONMENT VARIABLES
#   STELLAR_RPC_URL                   Soroban RPC endpoint
#   NETWORK_PASSPHRASE                Stellar network passphrase
#   ROTATION_SIGNER_SECRET            Secret of the signing key (--execute only)
#   KORA_MAINNET_ROTATION_CONFIRM     Must equal "I_UNDERSTAND_MAINNET_ROTATION"
#                                     for ANY run against --env mainnet
#   <NAME>_CONTRACT                   e.g. TREASURY_CONTRACT, PRICE_ORACLE_CONTRACT
#
# AUDIT LOG FORMAT
#   One JSON object per line (JSON Lines) appended to the log file:
#   {"ts":"…","run_id":"…","env":"…","mode":"dry-run|execute","role":"…",
#    "contract":"…","contract_id":"…","current":"…","next":"…",
#    "step":"…","status":"…","detail":"…"}
#
# EXIT CODES
#   0  plan valid (dry-run) / steps submitted (execute)
#   1  usage or input validation error
#   2  on-chain state does not match the expected current key
#   3  mainnet run refused (missing confirmation)
#   4  execute refused or submission failed
#
# DEPENDENCIES
#   stellar CLI >= 20.0 (read-only simulation via --send=no)
#
# SECURITY NOTES
#   - Dry-run only performs read-only simulations (`--send=no`).
#   - Secrets are only read in --execute mode and never logged.
#   - Steps that must be signed by the NEXT key (accept_admin) are printed for
#     the new key holder to run from their own machine; this script never
#     handles both keys at once.
# =============================================================================

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
LOG_FILE="${REPO_ROOT}/logs/key-rotation.log"
TIMESTAMP="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
RUN_ID="rot-$(date -u +%Y%m%dT%H%M%SZ)-$$"

MAINNET_CONFIRM_VALUE="I_UNDERSTAND_MAINNET_ROTATION"

ENV="testnet"
ROLE=""
CONTRACT=""
CONTRACT_ID=""
CURRENT_KEY=""
NEXT_KEY=""
SIGNER_KEY=""
EXECUTE=false
ACCEPT_UNVERIFIED=false

# ── Colours ───────────────────────────────────────────────────────────────────

RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Logging ───────────────────────────────────────────────────────────────────

info()    { echo -e "[${TIMESTAMP}] [${CYAN}INFO${NC}] $*"; }
warn()    { echo -e "[${TIMESTAMP}] [${YELLOW}WARN${NC}] $*"; }
success() { echo -e "[${TIMESTAMP}] [${GREEN}OK${NC}] $*"; }
error()   { echo -e "[${TIMESTAMP}] [${RED}ERROR${NC}] $*" >&2; }

# Strip characters that would break the JSON line (inputs are validated
# identifiers/addresses; only free-text details need this).
json_safe() { printf '%s' "$1" | tr -d '"\\' | tr '\n\r\t' '   '; }

audit() {
    local step="$1" status="$2" detail="${3:-}"
    mkdir -p "$(dirname "${LOG_FILE}")"
    printf '{"ts":"%s","run_id":"%s","env":"%s","mode":"%s","role":"%s","contract":"%s","contract_id":"%s","current":"%s","next":"%s","step":"%s","status":"%s","detail":"%s"}\n' \
        "$(date -u +"%Y-%m-%dT%H:%M:%SZ")" "${RUN_ID}" "$(json_safe "${ENV}")" "${MODE_LABEL:-dry-run}" \
        "$(json_safe "${ROLE}")" "$(json_safe "${CONTRACT}")" "$(json_safe "${CONTRACT_ID}")" \
        "$(json_safe "${CURRENT_KEY}")" "$(json_safe "${NEXT_KEY}")" \
        "$(json_safe "${step}")" "$(json_safe "${status}")" "$(json_safe "${detail}")" \
        >> "${LOG_FILE}"
}

fail() {
    local code="$1" step="$2"; shift 2
    error "$*"
    audit "${step}" "refused" "$*"
    exit "${code}"
}

usage() {
    sed -n '2,80p' "${BASH_SOURCE[0]}" | grep '^#' | sed 's/^# \{0,1\}//'
    exit 0
}

# ── Argument parsing ──────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --env)               ENV="${2:-}";          shift 2 ;;
        --role)              ROLE="${2:-}";         shift 2 ;;
        --contract)          CONTRACT="${2:-}";     shift 2 ;;
        --contract-id)       CONTRACT_ID="${2:-}";  shift 2 ;;
        --current)           CURRENT_KEY="${2:-}";  shift 2 ;;
        --next)              NEXT_KEY="${2:-}";     shift 2 ;;
        --signer)            SIGNER_KEY="${2:-}";   shift 2 ;;
        --execute)           EXECUTE=true;          shift   ;;
        --dry-run)           EXECUTE=false;         shift   ;;
        --accept-unverified) ACCEPT_UNVERIFIED=true; shift  ;;
        --log-file)          LOG_FILE="${2:-}";     shift 2 ;;
        --help|-h)           usage ;;
        *) error "Unknown argument: $1"; exit 1 ;;
    esac
done

MODE_LABEL="dry-run"
[[ "${EXECUTE}" == "true" ]] && MODE_LABEL="execute"

# ── Network selection & mainnet guard ─────────────────────────────────────────

case "${ENV}" in
    testnet)
        STELLAR_RPC_URL="${STELLAR_RPC_URL:-https://soroban-testnet.stellar.org}"
        NETWORK_PASSPHRASE="${NETWORK_PASSPHRASE:-Test SDF Network ; September 2015}" ;;
    futurenet)
        STELLAR_RPC_URL="${STELLAR_RPC_URL:-https://rpc-futurenet.stellar.org}"
        NETWORK_PASSPHRASE="${NETWORK_PASSPHRASE:-Test SDF Future Network ; October 2022}" ;;
    local)
        STELLAR_RPC_URL="${STELLAR_RPC_URL:-http://localhost:8000/soroban/rpc}"
        NETWORK_PASSPHRASE="${NETWORK_PASSPHRASE:-Standalone Network ; February 2017}" ;;
    mainnet)
        if [[ "${KORA_MAINNET_ROTATION_CONFIRM:-}" != "${MAINNET_CONFIRM_VALUE}" ]]; then
            fail 3 "mainnet-guard" \
                "Refusing mainnet run: set KORA_MAINNET_ROTATION_CONFIRM=${MAINNET_CONFIRM_VALUE} to proceed"
        fi
        : "${STELLAR_RPC_URL:?STELLAR_RPC_URL must be set explicitly for mainnet}"
        NETWORK_PASSPHRASE="${NETWORK_PASSPHRASE:-Public Global Stellar Network ; September 2015}"
        audit "mainnet-guard" "confirmed" "KORA_MAINNET_ROTATION_CONFIRM present" ;;
    *) fail 1 "input" "Unknown --env '${ENV}' (testnet|futurenet|local|mainnet)" ;;
esac

# ── Contract registry ─────────────────────────────────────────────────────────
# Maps each supported contract to its on-chain getter and rotation entrypoints.
#   ADMIN_GETTER  — read-only fn returning the admin address ("" = none exposed)
#   ADMIN_PROPOSE — fn signed by the CURRENT admin: <fn> <current-arg-name>
#   ADMIN_ACCEPT  — fn signed by the NEXT admin ("" = single-step rotation)

ADMIN_GETTER=""; ADMIN_PROPOSE_FN=""; ADMIN_PROPOSE_ARG=""; ADMIN_ACCEPT_FN=""
case "${CONTRACT}" in
    access_control)
        ADMIN_GETTER="get_admin"; ADMIN_PROPOSE_FN="rotate_admin"
        ADMIN_PROPOSE_ARG="current_admin"; ADMIN_ACCEPT_FN="" ;;
    treasury)
        ADMIN_GETTER="get_admin"; ADMIN_PROPOSE_FN="propose_admin"
        ADMIN_PROPOSE_ARG="admin"; ADMIN_ACCEPT_FN="accept_admin" ;;
    parameter_registry)
        ADMIN_GETTER="get_admin"; ADMIN_PROPOSE_FN="propose_admin"
        ADMIN_PROPOSE_ARG="admin"; ADMIN_ACCEPT_FN="accept_admin" ;;
    risk_registry)
        ADMIN_GETTER="get_admin"; ADMIN_PROPOSE_FN="transfer_admin"
        ADMIN_PROPOSE_ARG="admin"; ADMIN_ACCEPT_FN="accept_admin" ;;
    price_oracle)
        ADMIN_GETTER=""; ADMIN_PROPOSE_FN="transfer_admin"
        ADMIN_PROPOSE_ARG="admin"; ADMIN_ACCEPT_FN="" ;;
    "") fail 1 "input" "--contract is required" ;;
    *)  fail 1 "input" "Unsupported --contract '${CONTRACT}'" ;;
esac

case "${ROLE}" in
    admin) ;;
    oracle)
        [[ "${CONTRACT}" == "price_oracle" ]] \
            || fail 1 "input" "--role oracle is only valid with --contract price_oracle" ;;
    "") fail 1 "input" "--role is required (admin|oracle)" ;;
    *)  fail 1 "input" "Unsupported --role '${ROLE}' (admin|oracle)" ;;
esac

# ── Resolve contract ID ───────────────────────────────────────────────────────

if [[ -z "${CONTRACT_ID}" ]]; then
    CONTRACTS_ENV="${REPO_ROOT}/scripts/contracts.env"
    if [[ -f "${CONTRACTS_ENV}" ]]; then
        # shellcheck disable=SC1090
        source "${CONTRACTS_ENV}"
    fi
    CONTRACT_VAR="$(echo "${CONTRACT}" | tr '[:lower:]' '[:upper:]')_CONTRACT"
    CONTRACT_ID="${!CONTRACT_VAR:-}"
    [[ -n "${CONTRACT_ID}" ]] \
        || fail 1 "input" "No contract ID: pass --contract-id or set ${CONTRACT_VAR}"
fi

# ── Key validation (current vs next) ──────────────────────────────────────────

is_public_key() { [[ "$1" =~ ^G[A-Z2-7]{55}$ ]]; }
is_secret_key() { [[ "$1" =~ ^S[A-Z2-7]{55}$ ]]; }

validate_key() {
    local label="$1" value="$2"
    [[ -n "${value}" ]] || fail 1 "validate-keys" "--${label} is required"
    if is_secret_key "${value}"; then
        # Do not echo or log the value: it is a secret.
        CURRENT_KEY="<redacted>"; NEXT_KEY="<redacted>"
        fail 1 "validate-keys" "--${label} looks like a SECRET key; pass the public G... address"
    fi
    is_public_key "${value}" \
        || fail 1 "validate-keys" "--${label} '${value}' is not a valid Stellar public key (G...)"
}

if [[ ! "${CONTRACT_ID}" =~ ^C[A-Z2-7]{55}$ ]]; then
    fail 1 "input" "Contract ID '${CONTRACT_ID}' is not a valid Soroban contract ID (C...)"
fi

validate_key "current" "${CURRENT_KEY}"
validate_key "next" "${NEXT_KEY}"
[[ "${CURRENT_KEY}" != "${NEXT_KEY}" ]] \
    || fail 1 "validate-keys" "--current and --next are identical; nothing to rotate"

if [[ "${ROLE}" == "oracle" ]]; then
    validate_key "signer" "${SIGNER_KEY}"
    [[ "${SIGNER_KEY}" != "${NEXT_KEY}" ]] \
        || fail 1 "validate-keys" "Oracle admin (--signer) must not become a feeder"
fi
audit "validate-keys" "ok" "current and next keys are well-formed and distinct"

# ── On-chain verification (read-only) ─────────────────────────────────────────

stellar_read() {
    local fn_name="$1"; shift
    stellar contract invoke \
        --id "${CONTRACT_ID}" \
        --source-account "${CURRENT_KEY}" \
        --rpc-url "${STELLAR_RPC_URL}" \
        --network-passphrase "${NETWORK_PASSPHRASE}" \
        --send=no \
        -- "${fn_name}" "$@"
}

VERIFIED=false
if [[ "${ROLE}" == "admin" && -n "${ADMIN_GETTER}" ]]; then
    command -v stellar >/dev/null 2>&1 \
        || fail 1 "verify-onchain" "stellar CLI not found; required for read-only verification"
    info "Reading ${CONTRACT}.${ADMIN_GETTER}() from ${ENV} (simulation only)..."
    if ! ONCHAIN_RAW="$(stellar_read "${ADMIN_GETTER}" 2>&1)"; then
        fail 2 "verify-onchain" "Could not read ${ADMIN_GETTER}: $(json_safe "${ONCHAIN_RAW}")"
    fi
    ONCHAIN_ADMIN="$(printf '%s' "${ONCHAIN_RAW}" | tr -d '"[:space:]')"
    if [[ "${ONCHAIN_ADMIN}" != "${CURRENT_KEY}" ]]; then
        audit "verify-onchain" "mismatch" "on-chain admin is ${ONCHAIN_ADMIN}"
        error "On-chain role mismatch: ${CONTRACT} admin is ${ONCHAIN_ADMIN}, expected ${CURRENT_KEY}"
        error "Do NOT proceed. Confirm which key holds the role (a prior rotation may be"
        error "half-complete, or the wrong contract ID / network was used)."
        exit 2
    fi
    VERIFIED=true
    success "On-chain admin matches --current"
    audit "verify-onchain" "ok" "on-chain admin matches current key"
elif [[ "${ROLE}" == "oracle" ]]; then
    warn "price_oracle exposes no public feeder getter; current feeder cannot be verified on-chain"
    audit "verify-onchain" "unverified" "price_oracle has no feeder getter"
else
    warn "${CONTRACT} exposes no admin getter; current admin cannot be verified on-chain"
    audit "verify-onchain" "unverified" "${CONTRACT} has no admin getter"
fi

# ── Build rotation plan ───────────────────────────────────────────────────────
# Each entry: "<signer>|<fn>|<args>"; signer is CURRENT, NEXT or SIGNER.

PLAN=()
if [[ "${ROLE}" == "admin" ]]; then
    PLAN+=("CURRENT|${ADMIN_PROPOSE_FN}|--${ADMIN_PROPOSE_ARG} ${CURRENT_KEY} --new_admin ${NEXT_KEY}")
    if [[ -n "${ADMIN_ACCEPT_FN}" ]]; then
        PLAN+=("NEXT|${ADMIN_ACCEPT_FN}|--new_admin ${NEXT_KEY}")
    fi
    [[ -n "${ADMIN_GETTER}" ]] && PLAN+=("READ|${ADMIN_GETTER}|(expect ${NEXT_KEY})")
else
    # Add before remove so price submission never drops below min fresh feeders.
    PLAN+=("SIGNER|add_feeder|--admin ${SIGNER_KEY} --feeder ${NEXT_KEY}")
    PLAN+=("READ|get_min_fresh_feeders|(confirm quorum still reachable)")
    PLAN+=("SIGNER|remove_feeder|--admin ${SIGNER_KEY} --feeder ${CURRENT_KEY}")
fi

echo ""
echo -e "${BOLD}============================================================${NC}"
echo -e "${BOLD}  KORA KEY ROTATION PLAN — ${ROLE} @ ${CONTRACT}${NC}"
echo -e "${BOLD}  Network: ${ENV} | Mode: ${MODE_LABEL} | Run: ${RUN_ID}${NC}"
if [[ "${EXECUTE}" != "true" ]]; then
    echo -e "${BOLD}${YELLOW}  *** DRY RUN — No transactions will be submitted ***${NC}"
fi
echo -e "${BOLD}============================================================${NC}"
echo "  Contract ID : ${CONTRACT_ID}"
echo "  Current key : ${CURRENT_KEY}"
echo "  Next key    : ${NEXT_KEY}"
echo "  Verified    : ${VERIFIED}"
echo ""

STEP_NO=0
for entry in "${PLAN[@]}"; do
    STEP_NO=$((STEP_NO + 1))
    IFS='|' read -r signer fn args <<< "${entry}"
    case "${signer}" in
        CURRENT) who="current key (${CURRENT_KEY})" ;;
        NEXT)    who="next key holder (${NEXT_KEY})" ;;
        SIGNER)  who="oracle admin (${SIGNER_KEY})" ;;
        READ)    who="read-only check" ;;
    esac
    echo "  ${STEP_NO}. ${fn} ${args}"
    echo "       signed by: ${who}"
    audit "plan-step-${STEP_NO}" "planned" "${fn} ${args} [${signer}]"
done
echo ""
echo "  Procedure reference: docs/KEY_ROTATION_PROCEDURE.md#rehearsal-dry-run-script"
echo ""

if [[ "${EXECUTE}" != "true" ]]; then
    success "Dry-run complete — no state was mutated. Audit log: ${LOG_FILE}"
    audit "complete" "dry-run-ok" "plan printed; nothing submitted"
    exit 0
fi

# ── Execute (explicit opt-in only) ────────────────────────────────────────────

if [[ "${VERIFIED}" != "true" && "${ACCEPT_UNVERIFIED}" != "true" ]]; then
    fail 4 "execute" "Current key not verified on-chain; re-run with --accept-unverified after manual confirmation"
fi
[[ -n "${ROTATION_SIGNER_SECRET:-}" ]] \
    || fail 4 "execute" "ROTATION_SIGNER_SECRET must be set for --execute"

EXPECTED_SIGNER="${CURRENT_KEY}"
[[ "${ROLE}" == "oracle" ]] && EXPECTED_SIGNER="${SIGNER_KEY}"
DERIVED_SIGNER="$(stellar keys address --secret-key "${ROTATION_SIGNER_SECRET}" 2>/dev/null || echo "UNKNOWN")"
[[ "${DERIVED_SIGNER}" == "${EXPECTED_SIGNER}" ]] \
    || fail 4 "execute" "ROTATION_SIGNER_SECRET does not belong to ${EXPECTED_SIGNER}"

STEP_NO=0
for entry in "${PLAN[@]}"; do
    STEP_NO=$((STEP_NO + 1))
    IFS='|' read -r signer fn args <<< "${entry}"
    if [[ "${signer}" == "NEXT" ]]; then
        warn "Step ${STEP_NO} (${fn}) must be signed by the next key holder from their own machine:"
        echo "  stellar contract invoke --id ${CONTRACT_ID} --source <NEXT_KEY_SECRET> \\"
        echo "    --rpc-url ${STELLAR_RPC_URL} --network-passphrase \"${NETWORK_PASSPHRASE}\" -- ${fn} ${args}"
        audit "execute-step-${STEP_NO}" "handoff" "${fn} awaits next key holder"
        continue
    fi
    [[ "${signer}" == "READ" ]] && continue
    info "Submitting step ${STEP_NO}: ${fn}"
    # shellcheck disable=SC2086
    if OUT="$(stellar contract invoke \
            --id "${CONTRACT_ID}" \
            --source "${ROTATION_SIGNER_SECRET}" \
            --rpc-url "${STELLAR_RPC_URL}" \
            --network-passphrase "${NETWORK_PASSPHRASE}" \
            -- "${fn}" ${args} 2>&1)"; then
        success "Step ${STEP_NO} submitted"
        audit "execute-step-${STEP_NO}" "submitted" "${fn}"
    else
        audit "execute-step-${STEP_NO}" "failed" "${fn}: ${OUT}"
        error "Step ${STEP_NO} (${fn}) failed — stopping. Remaining steps NOT submitted."
        exit 4
    fi
done

audit "complete" "execute-ok" "current-key steps submitted"
success "Submitted. Run the dry-run again after the next key accepts to verify the new state."
exit 0
