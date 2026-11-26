#!/usr/bin/env bash
# =============================================================================
# Kora Protocol — Key Rotation Dry-Run Fixture Tests (#1110)
#
# Exercises scripts/key-rotation-dry-run.sh against a stubbed `stellar` CLI so
# no network access or real keys are needed. The stub records every
# invocation; tests assert the dry-run path only issues read-only simulations.
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
ROTATE="$ROOT_DIR/scripts/key-rotation-dry-run.sh"

WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT

KEY_A="G$(printf 'A%.0s' $(seq 1 55))"
KEY_B="G$(printf 'B%.0s' $(seq 1 55))"
KEY_C="G$(printf 'C%.0s' $(seq 1 55))"
SECRET_X="S$(printf 'X%.0s' $(seq 1 55))"
CONTRACT_ID="C$(printf 'D%.0s' $(seq 1 55))"

# ── Stub stellar CLI ──────────────────────────────────────────────────────────
mkdir -p "$WORK_DIR/bin"
cat > "$WORK_DIR/bin/stellar" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$STUB_CALLS"
case "$*" in
  *"-- get_admin"*) echo "\"${STUB_ONCHAIN_ADMIN}\"" ;;
  *) echo "stub: unexpected call" >&2; exit 9 ;;
esac
EOF
chmod +x "$WORK_DIR/bin/stellar"
export PATH="$WORK_DIR/bin:$PATH"
export STUB_CALLS="$WORK_DIR/calls.log"

run_rotate() {
    # Usage: run_rotate <expected-exit> <args...>
    local expected="$1"; shift
    local rc=0
    "$ROTATE" --log-file "$WORK_DIR/audit.log" --contract-id "$CONTRACT_ID" "$@" \
        > "$WORK_DIR/out.txt" 2>&1 || rc=$?
    if [[ "$rc" -ne "$expected" ]]; then
        echo "FAIL: expected exit $expected, got $rc for: $*"
        cat "$WORK_DIR/out.txt"
        exit 1
    fi
}

reset() { : > "$STUB_CALLS"; : > "$WORK_DIR/audit.log"; }

# 1. Dry-run prints plan without mutating
echo "--- Test 1: admin dry-run prints plan, read-only ---"
reset
STUB_ONCHAIN_ADMIN="$KEY_A" run_rotate 0 \
    --env testnet --role admin --contract treasury --current "$KEY_A" --next "$KEY_B"
grep -q "DRY RUN" "$WORK_DIR/out.txt"
grep -q "propose_admin --admin $KEY_A --new_admin $KEY_B" "$WORK_DIR/out.txt"
grep -q "accept_admin --new_admin $KEY_B" "$WORK_DIR/out.txt"
if grep -v -- "--send=no" "$STUB_CALLS" | grep -q .; then
    echo "FAIL: dry-run issued a non-simulated stellar call"; cat "$STUB_CALLS"; exit 1
fi
grep -q '"mode":"dry-run"' "$WORK_DIR/audit.log"
grep -q '"step":"complete","status":"dry-run-ok"' "$WORK_DIR/audit.log"
echo "✓ Plan printed, only --send=no simulations, audit log written."

# 2. Mismatched on-chain role
echo ""
echo "--- Test 2: mismatched on-chain admin is rejected ---"
reset
STUB_ONCHAIN_ADMIN="$KEY_C" run_rotate 2 \
    --env testnet --role admin --contract treasury --current "$KEY_A" --next "$KEY_B"
grep -q '"step":"verify-onchain","status":"mismatch"' "$WORK_DIR/audit.log"
echo "✓ Mismatch exits 2 and is audited."

# 3. Mainnet refused without confirmation env
echo ""
echo "--- Test 3: mainnet refused without confirm env ---"
reset
STUB_ONCHAIN_ADMIN="$KEY_A" STELLAR_RPC_URL="http://stub" run_rotate 3 \
    --env mainnet --role admin --contract treasury --current "$KEY_A" --next "$KEY_B"
if [[ -s "$STUB_CALLS" ]]; then
    echo "FAIL: mainnet guard should trip before any RPC call"; exit 1
fi
echo "✓ Mainnet refused before touching the network."

# 4. Mainnet dry-run allowed with confirmation env
echo ""
echo "--- Test 4: mainnet dry-run with confirm env ---"
reset
KORA_MAINNET_ROTATION_CONFIRM=I_UNDERSTAND_MAINNET_ROTATION \
STUB_ONCHAIN_ADMIN="$KEY_A" STELLAR_RPC_URL="http://stub" run_rotate 0 \
    --env mainnet --role admin --contract treasury --current "$KEY_A" --next "$KEY_B"
grep -q '"step":"mainnet-guard","status":"confirmed"' "$WORK_DIR/audit.log"
echo "✓ Confirmed mainnet dry-run succeeds."

# 5. Current == next rejected
echo ""
echo "--- Test 5: identical current/next keys rejected ---"
reset
STUB_ONCHAIN_ADMIN="$KEY_A" run_rotate 1 \
    --env testnet --role admin --contract treasury --current "$KEY_A" --next "$KEY_A"
echo "✓ Identical keys rejected."

# 6. Secret key passed instead of public key is rejected and never logged
echo ""
echo "--- Test 6: secret key input rejected and redacted ---"
reset
STUB_ONCHAIN_ADMIN="$KEY_A" run_rotate 1 \
    --env testnet --role admin --contract treasury --current "$KEY_A" --next "$SECRET_X"
if grep -q "$SECRET_X" "$WORK_DIR/audit.log" "$WORK_DIR/out.txt"; then
    echo "FAIL: secret key leaked into output/audit log"; exit 1
fi
echo "✓ Secret rejected and redacted."

# 7. Oracle feeder rotation: unverified, add-before-remove plan
echo ""
echo "--- Test 7: oracle feeder dry-run ---"
reset
run_rotate 0 \
    --env testnet --role oracle --contract price_oracle \
    --signer "$KEY_C" --current "$KEY_A" --next "$KEY_B"
add_line=$(grep -n "add_feeder" "$WORK_DIR/out.txt" | head -1 | cut -d: -f1)
rm_line=$(grep -n "remove_feeder" "$WORK_DIR/out.txt" | head -1 | cut -d: -f1)
[[ "$add_line" -lt "$rm_line" ]] || { echo "FAIL: add_feeder must precede remove_feeder"; exit 1; }
grep -q '"status":"unverified"' "$WORK_DIR/audit.log"
echo "✓ Oracle plan adds new feeder before removing old one."

# 8. Execute refuses unverified oracle rotation without explicit override
echo ""
echo "--- Test 8: execute refused when current key is unverified ---"
reset
ROTATION_SIGNER_SECRET="$SECRET_X" run_rotate 4 \
    --env testnet --role oracle --contract price_oracle \
    --signer "$KEY_C" --current "$KEY_A" --next "$KEY_B" --execute
if [[ -s "$STUB_CALLS" ]]; then
    echo "FAIL: refused execute must not call stellar"; exit 1
fi
echo "✓ Unverified execute refused."

# 9. Execute without signer secret refused
echo ""
echo "--- Test 9: execute without ROTATION_SIGNER_SECRET refused ---"
reset
STUB_ONCHAIN_ADMIN="$KEY_A" run_rotate 4 \
    --env testnet --role admin --contract treasury --current "$KEY_A" --next "$KEY_B" --execute
echo "✓ Execute without secret refused."

echo ""
echo "=== Key Rotation Dry-Run Tests Passed! ==="
