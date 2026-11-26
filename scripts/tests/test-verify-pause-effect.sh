#!/usr/bin/env bash
# Tests for scripts/verify-pause-effect.sh (issue #1175).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
V="$ROOT/scripts/verify-pause-effect.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
# Mock probe: CPAUSED* -> true, CLIVE* -> false, CERR* -> fails, CJUNK* -> garbage
cat > "$T/probe" <<'P'
#!/usr/bin/env bash
case "$1" in CPAUSED*) echo true ;; CLIVE*) echo false ;; CJUNK*) echo '"maybe"' ;; *) exit 7 ;; esac
P
chmod +x "$T/probe"; export PAUSE_STATE_CMD="$T/probe"
pass=0; fail=0
run() { set +e; out="$(env "$@" bash "$V" --out "$T/m.prom" 2>&1)"; rc=$?; set -e; }
ok() { if eval "$2"; then echo "ok  - $1"; pass=$((pass+1)); else echo "FAIL- $1 (rc=$rc) $out"; fail=$((fail+1)); fi; }

run PAUSE_TARGETS="access_control=CPAUSED1,treasury=CPAUSED2,price_oracle=CPAUSED3"
ok "all targets paused -> exit 0" '[ $rc -eq 0 ]'
ok "per-target effective metric emitted" 'grep -q "kora_pause_effective{target=\"treasury\",contract=\"CPAUSED2\"} 1" "$T/m.prom"'
ok "targets_total counts every target" 'grep -qx "kora_pause_drill_targets_total 3" "$T/m.prom"'
ok "unpaused_targets is zero" 'grep -qx "kora_pause_drill_unpaused_targets 0" "$T/m.prom"'
run PAUSE_TARGETS="access_control=CPAUSED1,treasury=CPAUSED2,price_oracle=CPAUSED3"
ok "re-run is idempotent" '[ $rc -eq 0 ] && grep -qx "kora_pause_drill_unpaused_targets 0" "$T/m.prom"'

run PAUSE_TARGETS="access_control=CPAUSED1,treasury=CLIVE2"
ok "one target still live -> exit 1" '[ $rc -eq 1 ] && grep -q "NOT_PAUSED: treasury" <<<"$out"'
ok "live target reported as 0" 'grep -q "kora_pause_effective{target=\"treasury\",contract=\"CLIVE2\"} 0" "$T/m.prom"'

run PAUSE_TARGETS="access_control=CPAUSED1,treasury=CERR2"
ok "probe failure (degraded RPC) fails closed" '[ $rc -eq 1 ] && grep -q "kora_pause_probe_error{target=\"treasury\",contract=\"CERR2\"} 1" "$T/m.prom"'
run PAUSE_TARGETS="treasury=CJUNK1"
ok "unparseable state counts as not paused" '[ $rc -eq 1 ] && grep -qx "kora_pause_drill_unpaused_targets 1" "$T/m.prom"'

run PAUSE_TARGETS=""
ok "empty config fails closed with typed error" '[ $rc -eq 2 ] && grep -q ERR_CONFIG_MISSING <<<"$out"'
run PAUSE_TARGETS="treasury"
ok "malformed target fails closed" '[ $rc -eq 2 ] && grep -q ERR_CONFIG_SCHEMA <<<"$out"'
run PAUSE_TARGETS="treasury=CPAUSED1,treasury=CPAUSED2"
ok "duplicate target rejected" '[ $rc -eq 2 ] && grep -q "duplicate target" <<<"$out"'

printf '# drill targets\naccess_control=CPAUSED1\n\ntreasury=CPAUSED2\n' > "$T/targets"
set +e; out="$(bash "$V" --targets-file "$T/targets" 2>&1)"; rc=$?; set -e
ok "targets file supported, metrics to stdout" '[ $rc -eq 0 ] && grep -qx "kora_pause_drill_targets_total 2" <<<"$out"'
set +e; out="$(bash "$V" --targets-file "$T/missing" 2>&1)"; rc=$?; set -e
ok "missing targets file fails closed" '[ $rc -eq 2 ] && grep -q ERR_CONFIG_MISSING <<<"$out"'

echo "passed=$pass failed=$fail"; [ "$fail" -eq 0 ]
