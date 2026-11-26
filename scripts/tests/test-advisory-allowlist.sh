#!/usr/bin/env bash
# Tests for scripts/security/check_advisory_allowlist.py (issue #1173).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHECK="$ROOT/scripts/security/check_advisory_allowlist.py"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
pass=0; fail=0
expect() { # expect <exit> <stderr-substring> <desc> -- args...
  local want="$1" needle="$2" desc="$3"; shift 4
  set +e; out="$(python3 "$CHECK" "$@" 2>&1)"; rc=$?; set -e
  if [ "$rc" -eq "$want" ] && grep -q -- "$needle" <<<"$out"; then echo "ok  - $desc"; pass=$((pass+1))
  else echo "FAIL- $desc (rc=$rc) $out"; fail=$((fail+1)); fi
}
w() { printf '%s\n' "$2" > "$T/$1"; }

expect 0 "allowlist OK" "repo config is valid" -- --deny "$ROOT/deny.toml" --audit "$ROOT/.cargo/audit.toml"

w d.toml '[advisories]
ignore = [{ id = "RUSTSEC-2024-0001", reason = "unreachable; owner=sec; expires=2027-01-01; #1173" }]'
w a.toml '[advisories]
ignore = ["RUSTSEC-2024-0001"]'
expect 0 "allowlist OK" "documented, unexpired, in-sync exception passes" -- --deny "$T/d.toml" --audit "$T/a.toml" --today 2026-11-24
expect 0 "allowlist OK" "re-running is idempotent" -- --deny "$T/d.toml" --audit "$T/a.toml" --today 2026-11-24
expect 1 "ERR_EXCEPTION_EXPIRED" "expired exception fails" -- --deny "$T/d.toml" --audit "$T/a.toml" --today 2027-01-02

w a2.toml '[advisories]
ignore = []'
expect 1 "ERR_ALLOWLIST_DRIFT" "drift between deny.toml and audit.toml fails" -- --deny "$T/d.toml" --audit "$T/a2.toml" --today 2026-11-24

w d2.toml '[advisories]
ignore = [{ id = "RUSTSEC-2024-0001", reason = "" }]'
expect 1 "ERR_MISSING_REASON" "missing reason fails" -- --deny "$T/d2.toml" --audit "$T/a.toml"

w d3.toml '[advisories]
ignore = ["RUSTSEC-2024-0001"]'
expect 1 "ERR_MISSING_REASON" "bare string exception (no rationale) fails" -- --deny "$T/d3.toml" --audit "$T/a.toml"

w d4.toml '[advisories]
ignore = [{ id = "CVE-2024-1", reason = "x expires=2099-01-01" }]'
expect 1 "ERR_BAD_ADVISORY_ID" "malformed advisory id fails" -- --deny "$T/d4.toml" --audit "$T/a2.toml"

w empty.toml ''
expect 1 "ERR_CONFIG_SCHEMA" "empty config fails closed" -- --deny "$T/empty.toml" --audit "$T/a.toml"
expect 1 "ERR_CONFIG_MISSING" "missing config fails closed" -- --deny "$T/nope.toml" --audit "$T/a.toml"

echo "passed=$pass failed=$fail"; [ "$fail" -eq 0 ]
