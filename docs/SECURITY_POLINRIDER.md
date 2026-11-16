# PolinRider malware protection (Kora-Contract)

This repository includes a **read-only** scanner for indicators associated with the **PolinRider** (Contagious Interview / Famous Chollima–linked) supply-chain campaign, adapted to this Rust/Solana workspace.

## What we look for

Confirmed fail conditions:

- Obfuscation / loader markers such as `rmcej%otb%`, `$_1e42`, `global['!']`, `global["_V"]`, `global.i="A…"`
- Fake font payloads (`.woff`/`.woff2`/etc. that are text/JavaScript instead of real font binaries)
- `.vscode/tasks.json` with `runOn: folderOpen` executing `node` on a non-JS asset (classic PolinRider trigger)
- `.gitignore` entries for `temp_auto_push.bat` / `temp_interactive_push.bat`
- Those batch helper filenames in the tree
- Unexpected `dist/setup.js`
- Build configs with extreme line length **and** obfuscation markers (inline config-injection variant)
- `package.json` lifecycle scripts that curl/wget-pipe to shell or `node -e` remote payloads (where a `package.json` exists)
- Broader heuristics: `Function("return…")`, `eval(atob…)`, `child_process….exec`, exfil endpoints (`webhook.site`, `ngrok.io`, `pastebin.com/raw`)

Warnings (human review, do not auto-fail unless combined with confirmed markers):

- `folderOpen` tasks without a non-JS `node` target
- Unexpected `.bat`/`.cmd` files
- Extreme config line length without markers

## Files

| Path | Purpose |
| --- | --- |
| `scripts/security/scan-polinrider.sh` | Read-only scanner (`repo`, `pr-files`, `self-test` modes) |
| `.github/workflows/polinrider-security.yml` | Workflow **PolinRider Security Scan** |
| `security/fixtures/polinrider/` | Safe test fixtures (see below) |
| `docs/SECURITY_POLINRIDER.md` | This document |

## Relationship to existing security tooling

This repository already runs `security.yml` (IAM policy lint) and the full Rust CI
(`cargo check`, `clippy`, `rustfmt`, unbounded-loop guard, dependency-cycle guard).
PolinRider scanning is **additional** — the existing workflows and `docs/SECURITY.md`,
`THREAT_MODEL.md` are untouched. Rust build output (`target/`) is excluded from scans.

## CI workflow

Workflow: `.github/workflows/polinrider-security.yml`
Required check name: **`PolinRider Security Scan`**

Runs on:

- Every pull request (`opened`, `synchronize`, `reopened`, `ready_for_review`)
- Pushes to `main`
- Manual `workflow_dispatch` (`repo` or `self-test`) — used to re-validate the scanner on demand

Safety guarantees:

- Actions pinned to full commit SHA (`actions/checkout@11d5960a326750d5838078e36cf38b85af677262` = v4.4.0)
- `permissions: contents: read` only; `persist-credentials: false`
- No dependency install from the PR; no execution of scanned code
- No secrets or tokens used by the scanner
- Fixtures under `security/fixtures/polinrider/` are excluded from production scans

## Test fixtures (intentional — not malware)

`security/fixtures/polinrider/` contains **harmless, clearly-labelled** fixtures that intentionally match detection rules:

- `fa-solid-400.woff2` — a *text* file with a `.woff2` name and marker strings (fake-font payload shape)
- `tasks.json` — VS Code `folderOpen` task running `node` on the fake font (classic trigger)
- `gitignore.snippet` — the `temp_auto_push.bat` / `temp_interactive_push.bat` fingerprint
- `tailwind.config.js` — single-line >2000-char config with an obfuscation marker (config-append variant)
- `clean-sample.js` — **negative** fixture: must always pass (guards against over-blocking)

`self-test` mode copies these into a temp directory (never in-place) and asserts:

1. the positive fixtures are **detected** (nested scan must fail), and
2. the clean fixture **passes** (nested scan must succeed).

## Local usage

```bash
# Clean tree scan
./scripts/security/scan-polinrider.sh .

# Prove detection still works using the checked-in fixtures
./scripts/security/scan-polinrider.sh . self-test
```

Both commands were validated locally: clean scan exits 0, self-test exits 0 only when fixtures are detected and the clean fixture passes.

## Investigating false positives

1. Open the Actions log for **PolinRider Security Scan**.
2. Identify the `CONFIRMED:` / `::error::` lines.
3. If a legitimate file matches a marker string by coincidence, fix the file or refine the scanner in a reviewed PR — do **not** disable or weaken the required check.
4. Review every finding before classifying it as a false positive; fixtures under `security/fixtures/polinrider/` are the only intentionally-matching files.

## Limitations

- The history advisory job is informational (pickaxe on recent commits); attackers can rewrite history.
- Binary packing / novel obfuscators may evade static markers — treat unexpected `folderOpen` tasks and text “fonts” as high signal.
- The scanner does not rotate credentials or quarantine developer machines. If you opened an infected tree in VS Code with auto-tasks enabled, treat the machine as compromised and rotate secrets from a clean device.

## Credential exposure

This tooling **does not** rotate credentials. If malware was confirmed on a machine that had GitHub/npm/cloud access:

1. Stop using that machine for privileged work.
2. From a clean device, rotate GitHub, npm, cloud, SSH, wallet, and CI secrets.
3. Review recent publishes and force-pushes on your accounts.
