#!/usr/bin/env bash
# PolinRider / Contagious Interview indicator scanner (read-only).
# Safe by design: never executes scanned files, never installs dependencies,
# never uses secrets, and never modifies the scanned tree.
#
# Adapted for Kora-Contract (Rust / Solana workspace — build output in target/).
# Detection rules follow the TrustBridge PolinRider scanner
# (Stellar-TrustBridge/trustbridge-contract).
#
# Usage: scan-polinrider.sh [root] [repo|pr-files|self-test]
#   repo       - scan a working tree (default)
#   pr-files   - additionally fail on high-risk paths listed in $POLIN_CHANGED_FILES
#   self-test  - prove detection still works: positive fixtures must be detected
#                and the clean fixture must pass
#
# Exit codes: 0 = clean (warnings allowed), 1 = confirmed indicators, 2 = setup error.
set -euo pipefail

ROOT="${1:-.}"
MODE="${2:-repo}" # repo | pr-files | self-test
FAIL=0
WARN=0
REPORT=()

note() { REPORT+=("$1"); }
fail() { FAIL=1; note "CONFIRMED: $1"; echo "::error::$1"; }
warn() { WARN=1; note "REVIEW: $1"; echo "::warning::$1"; }

# Single-file pattern match (grep is always available; rg is optional).
pat_match() { grep -qE -- "$1" "$2" 2>/dev/null; }

cd "$ROOT"

if command -v rg >/dev/null 2>&1; then HAVE_RG=1; else HAVE_RG=0; fi

# ---- scan scope: adapted to this repository's layout -------------------
EXCLUDE_GLOBS=(
  --glob '!node_modules/**'
  --glob '!**/node_modules/**'
  --glob '!target/**'
  --glob '!**/target/**'
  --glob '!dist/**'
  --glob '!.git/**'
  --glob '!security/fixtures/**'
  # Scanner + docs intentionally mention markers — exclude to avoid self-hits
  --glob '!scripts/security/**'
  --glob '!docs/SECURITY_POLINRIDER.md'
  --glob '!.github/workflows/polinrider-security.yml'
)

# Equivalent excludes for the grep fallback (same scope, no rg needed).
GREP_EXCLUDES=(
  --exclude-dir=node_modules
  --exclude-dir=.git
  --exclude-dir=target
  --exclude-dir=dist
  --exclude-dir=fixtures
  --exclude=scan-polinrider.sh
  --exclude=SECURITY_POLINRIDER.md
  --exclude=polinrider-security.yml
)

# Directories pruned for find-based checks (bat files, fonts, configs).
PRUNE_FIND=(
  \( -name node_modules -o -name .git -o -name target -o -name dist \
     -o -path './security/fixtures' \)
)

# Build/tool config files checked for appended payloads (extreme line length).
CONFIG_FIND=(
  \( -name '*.config.js' -o -name '*.config.mjs' -o -name '*.config.cjs' \
     -o -name '*.config.ts' -o -name '*.config.mts' -o -name 'postcss.config.*' \
     -o -name 'jest.config.*' -o -name 'babel.config.*' -o -name 'webpack.config.*' \
     -o -name 'vite.config.*' -o -name 'eslint.config.*' \)
)

# ---- known PolinRider / Contagious Interview indicators ----------------
MARKERS=(
  'rmcej%otb%'
  '_\$_1e42'
  "global\['!'\]"
  'global\["_V"\]'
  'global\.i="A'
  'temp_auto_push\.bat'
  'temp_interactive_push\.bat'
  'fa-solid-400\.woff2'
  'DEV#POPPER'
  'OmniSteal'
)
MARKER_PATTERN=$(IFS='|'; echo "${MARKERS[*]}")

echo "## PolinRider security scan"
echo "Root: $(pwd)  Mode: $MODE  ripgrep: $([[ $HAVE_RG -eq 1 ]] && echo yes || echo no)"
echo

# ---------- self-test: prove detection against checked-in fixtures ------
if [[ "$MODE" == "self-test" ]]; then
  FIX=security/fixtures/polinrider
  [[ -d "$FIX" ]] || { echo "Missing fixtures at $FIX"; exit 2; }
  SCRIPT="$(cd "$(dirname "$0")" && pwd)/scan-polinrider.sh"
  TMP=$(mktemp -d)
  TMP_CLEAN=$(mktemp -d)
  trap 'rm -rf "$TMP" "$TMP_CLEAN"' EXIT

  # Positive case: real indicator fixtures must be detected.
  mkdir -p "$TMP/public/fonts" "$TMP/.vscode"
  cp "$FIX/fa-solid-400.woff2" "$TMP/public/fonts/"
  cp "$FIX/tasks.json" "$TMP/.vscode/tasks.json"
  cp "$FIX/tailwind.config.js" "$TMP/"
  { cat "$FIX/gitignore.snippet"; echo; } > "$TMP/.gitignore"
  echo "Running nested scan against POSITIVE fixture tree..."
  if bash "$SCRIPT" "$TMP" repo; then
    echo "SELF-TEST FAILED: expected detection in positive fixtures"
    exit 1
  else
    echo "OK: positive fixtures correctly detected as malware indicators"
  fi

  # Negative case: the clean fixture must NOT be flagged (no over-blocking).
  mkdir -p "$TMP_CLEAN"
  cp "$FIX/clean-sample.js" "$TMP_CLEAN/"
  echo "Running nested scan against CLEAN fixture tree..."
  if bash "$SCRIPT" "$TMP_CLEAN" repo; then
    echo "SELF-TEST PASSED: clean fixture passes, malicious fixtures detected"
    exit 0
  else
    echo "SELF-TEST FAILED: clean fixture was incorrectly flagged"
    exit 1
  fi
fi

# 1) Marker grep
if [[ "$HAVE_RG" -eq 1 ]]; then
  HITS=$(rg -n --hidden "${EXCLUDE_GLOBS[@]}" -e "$MARKER_PATTERN" . 2>/dev/null || true)
else
  HITS=$(grep -RInE "${GREP_EXCLUDES[@]}" -e "$MARKER_PATTERN" . 2>/dev/null || true)
fi
if [[ -n "${HITS}" ]]; then
  fail "PolinRider code markers found in repository content"
  echo "$HITS" | head -50
fi

# 2) VS Code folderOpen auto-run (classic PolinRider trigger)
if [[ -f .vscode/tasks.json ]]; then
  if pat_match 'folderOpen' .vscode/tasks.json; then
    if grep -qiE -- 'node.*\.(woff2|woff|ttf|otf|eot|png|jpg|svg)' .vscode/tasks.json 2>/dev/null; then
      fail ".vscode/tasks.json runs node on a non-JS asset with folderOpen (PolinRider pattern)"
    else
      warn ".vscode/tasks.json uses runOn folderOpen — verify the command is legitimate"
    fi
  fi
fi

# 3) .gitignore fingerprint
if [[ -f .gitignore ]] && pat_match 'temp_auto_push|temp_interactive_push' .gitignore; then
  fail ".gitignore references temp_auto_push / temp_interactive_push (PolinRider fingerprint)"
fi

# 4) Batch helpers
while IFS= read -r -d '' f; do
  base=$(basename "$f")
  case "$base" in
    temp_auto_push.bat|temp_interactive_push.bat)
      fail "Malicious helper batch file present: $f"
      ;;
    *)
      warn "Unexpected Windows batch file in tree: $f — verify legitimacy"
      ;;
  esac
done < <(find . "${PRUNE_FIND[@]}" -prune -o \
  \( -iname '*.bat' -o -iname '*.cmd' \) -print0 2>/dev/null)

# 5) Fake fonts (text/JavaScript instead of real font binaries)
while IFS= read -r -d '' f; do
  ft=$(file -b "$f" 2>/dev/null || echo unknown)
  case "$ft" in
    *ASCII*|*UTF-8*|*text*|*JavaScript*|*JSON*|*empty*)
      if pat_match "$MARKER_PATTERN" "$f"; then
        fail "Font-named file contains PolinRider markers and is text/JS: $f ($ft)"
      else
        fail "Font-named file is text/JS (not a real font): $f ($ft)"
      fi
      ;;
  esac
done < <(find . "${PRUNE_FIND[@]}" -prune -o \
  \( -iname '*.woff2' -o -iname '*.woff' -o -iname '*.ttf' -o -iname '*.otf' -o -iname '*.eot' \) -print0 2>/dev/null)

# 6) Common malware drop path
if [[ -f dist/setup.js ]]; then
  fail "Unexpected dist/setup.js present (common malware drop path)"
fi

# 7) Suspicious package.json lifecycle scripts
if [[ -f package.json ]] && command -v python3 >/dev/null 2>&1; then
  LIFE=$(python3 - <<'PY' 2>/dev/null || true
import json, re
try:
    d = json.load(open("package.json"))
except Exception:
    d = {}
scripts = d.get("scripts") or {}
bad = re.compile(r"(curl\s+.*\|\s*(ba)?sh|wget\s+.*\|\s*(ba)?sh|node\s+-e|eval\(|child_process|invoke-expression)", re.I)
for k, v in scripts.items():
    if any(x in k.lower() for x in ("preinstall", "postinstall", "install", "prepare", "prepublish")):
        if bad.search(str(v)):
            print(f"{k}={v}")
PY
)
  if [[ -n "$LIFE" ]]; then
    fail "package.json contains suspicious lifecycle script that downloads/executes remote code"
    echo "$LIFE"
  fi
fi

# 8) Extreme line length in build configs (payload-append variant)
while IFS= read -r -d '' f; do
  if awk 'length($0)>2000 { found=1 } END { exit found?2:0 }' "$f"; then
    :
  else
    if pat_match "$MARKER_PATTERN|global\[|eval\(" "$f"; then
      fail "Build config has extreme line length with obfuscation markers: $f"
    else
      warn "Build config has extreme line length (>2000 chars): $f — verify no appended payload"
    fi
  fi
done < <(find . "${PRUNE_FIND[@]}" -prune -o "${CONFIG_FIND[@]}" -print0 2>/dev/null)

# 9) Broader supply-chain / malware heuristics (not PolinRider-only)
if [[ "$HAVE_RG" -eq 1 ]]; then
  BROADER=$(rg -n --hidden "${EXCLUDE_GLOBS[@]}" \
    -e 'Function\s*\(\s*["'"'"']return' \
    -e 'atob\s*\([^)]+\)\s*;?\s*eval' \
    -e 'eval\s*\(\s*atob' \
    -e 'child_process["'"'"']?\s*\]?\s*\.\s*exec' \
    -e 'webhook\.site|ngrok\.io/|pastebin\.com/raw' \
    -g '!*.md' -g '!*.txt' . 2>/dev/null || true)
else
  BROADER=$(grep -RInE "${GREP_EXCLUDES[@]}" \
    --exclude='*.md' --exclude='*.txt' \
    -e 'Function[[:space:]]*\([[:space:]]*["'"'"']return' \
    -e 'atob[[:space:]]*\([^)]+\)[[:space:]]*;?[[:space:]]*eval' \
    -e 'eval[[:space:]]*\([[:space:]]*atob' \
    -e "child_process[\"']?[[:space:]]*\]?[[:space:]]*\.[[:space:]]*exec" \
    -e 'webhook\.site|ngrok\.io/|pastebin\.com/raw' \
    . 2>/dev/null || true)
fi
if [[ -n "${BROADER}" ]]; then
  fail "Suspicious remote-exec / obfuscation patterns found (broader malware heuristics)"
  echo "$BROADER" | head -40
fi

# 10) PR changed-files mode — high-risk paths fail closed on PRs
if [[ "$MODE" == "pr-files" && -n "${POLIN_CHANGED_FILES:-}" ]]; then
  while IFS= read -r f; do
    [[ -z "$f" || ! -f "$f" ]] && continue
    case "$f" in
      .vscode/tasks.json|dist/setup.js|*fa-solid-400.woff2|temp_auto_push.bat|temp_interactive_push.bat)
        fail "PR introduces/modifies high-risk malware path: $f"
        ;;
    esac
    if pat_match "$MARKER_PATTERN" "$f"; then
      fail "Malware marker in changed file: $f"
    fi
  done <<< "$POLIN_CHANGED_FILES"
fi

echo
echo "### Summary"
if [[ ${#REPORT[@]} -eq 0 ]]; then
  echo "No findings recorded."
else
  printf '%s\n' "${REPORT[@]}"
fi
echo

if [[ "$FAIL" -ne 0 ]]; then
  echo "MALWARE DETECTED — Do not merge. Review the security report and remove the malicious code."
  echo "::error::MALWARE DETECTED — Do not merge. Review the security report and remove the malicious code."
  exit 1
fi

if [[ "$WARN" -ne 0 ]]; then
  echo "Scan completed with items needing human review (see warnings). No confirmed PolinRider markers."
  exit 0
fi

echo "Scan clean: no confirmed PolinRider indicators found."
exit 0
