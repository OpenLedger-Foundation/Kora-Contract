# PolinRider test fixtures — INTENTIONAL, NOT MALWARE

These files are **harmless test fixtures** used by `scripts/security/scan-polinrider.sh`
to prove the scanner still detects PolinRider indicators (self-test mode). They are
**not** executed, and they are **excluded** from normal repository scans.

| File | Role | Expected result |
| --- | --- | --- |
| `fa-solid-400.woff2` | Positive: text payload with a font extension + marker strings | must be detected |
| `tasks.json` | Positive: VS Code `folderOpen` task running `node` on the fake font | must be detected |
| `gitignore.snippet` | Positive: `temp_auto_push.bat` / `temp_interactive_push.bat` fingerprint | must be detected |
| `tailwind.config.js` | Positive: >2000-char single line with an obfuscation marker | must be detected |
| `clean-sample.js` | **Negative**: benign file | must **not** be flagged |

Run `./scripts/security/scan-polinrider.sh . self-test` to verify: the self-test only
passes when the positive fixtures are detected **and** the clean fixture passes.

Never "fix" these fixtures to make a scan green — that would weaken detection.
Full documentation: `docs/SECURITY_POLINRIDER.md`.
