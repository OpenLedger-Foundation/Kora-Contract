#!/usr/bin/env python3
"""Suggest an incident severity from a symptoms checklist (issue #1176).

Maps observed symptoms to the severity levels defined in
docs/INCIDENT_RESPONSE.md#severity-levels and points at the matching templates
in docs/incident-response-templates/. The highest-severity symptom wins; the
Incident Commander always makes the final call.

Usage:
  scripts/classify_incident_sev.py --symptom funds_at_risk --symptom protocol_paused
  scripts/classify_incident_sev.py --checklist symptoms.txt [--json]
  scripts/classify_incident_sev.py --list

Checklist file: one symptom id per line, `- [x] id` markdown checkboxes are also
accepted (unchecked `- [ ]` lines are ignored), `#` comments allowed.

Exit: 0 suggestion printed, 2 config/input error (no symptoms, unknown symptom,
missing checklist file).
"""
import argparse
import json
import re
import sys

TEMPLATES = "docs/incident-response-templates"

# symptom id -> (sev rank 1..4, description)
SYMPTOMS = {
    "active_exploit": (1, "Exploit transactions observed on-chain"),
    "funds_at_risk": (1, "User or treasury funds are draining or at imminent risk"),
    "protocol_paused": (1, "Emergency pause was triggered"),
    "pause_not_effective": (1, "Pause did not take effect on every target contract"),
    "oracle_manipulation": (1, "Price oracle returned manipulated/out-of-band values used on-chain"),
    "key_compromise": (1, "Admin, guardian, or signer key suspected compromised"),
    "exploitable_vulnerability": (2, "Confirmed exploitable bug, not yet exploited"),
    "public_disclosure": (2, "Critical bug publicly disclosed"),
    "bounty_report_critical": (2, "Bug bounty report rated critical/high"),
    "state_inconsistency": (3, "Contract state inconsistent with expected invariants, no fund loss"),
    "indexer_lag": (3, "Indexer behind chain head beyond SLO"),
    "keeper_degraded": (3, "Keeper/TTL extension jobs failing or delayed"),
    "rpc_degraded": (3, "Upstream RPC/dependency degraded or unavailable"),
    "ui_bug": (4, "Cosmetic UI issue, no security impact"),
    "docs_error": (4, "Documentation error"),
}

LEVELS = {
    1: ("Sev-1", "Critical", "Immediate (< 1 hour)", ["sev-1-pause-internal.md", "sev-1-pause-external.md"]),
    2: ("Sev-2", "High", "4-8 hours", ["sev-2-internal.md", "sev-2-external.md"]),
    3: ("Sev-3", "Medium", "24-48 hours", ["sev-3-internal.md", "sev-3-external.md"]),
    4: ("Sev-3", "Low", "Best effort", ["sev-3-internal.md"]),
}

# Scenario-specific templates appended when a symptom is present.
EXTRA_TEMPLATES = {
    "key_compromise": ["key-compromise-internal.md", "key-compromise-external.md"],
    "indexer_lag": ["indexer-lag-internal.md", "indexer-lag-external.md"],
    "bounty_report_critical": ["bounty-disclosure-internal.md", "bounty-disclosure-external.md"],
}

CHECKED = re.compile(r"^-\s*\[[xX]\]\s*(\S+)")
UNCHECKED = re.compile(r"^-\s*\[\s\]")


class InputError(Exception):
    """Typed input error; maps to exit code 2."""


def parse_checklist(path):
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.read().splitlines()
    except FileNotFoundError:
        raise InputError(f"ERR_CONFIG_MISSING: checklist {path} not found")
    out = []
    for line in lines:
        line = line.split("#", 1)[0].strip()
        if not line or UNCHECKED.match(line):
            continue
        m = CHECKED.match(line)
        out.append(m.group(1) if m else line)
    return out


def classify(symptoms):
    symptoms = sorted({s.strip().lower() for s in symptoms if s.strip()})
    if not symptoms:
        raise InputError("ERR_NO_SYMPTOMS: provide at least one symptom (see --list)")
    unknown = [s for s in symptoms if s not in SYMPTOMS]
    if unknown:
        raise InputError(f"ERR_UNKNOWN_SYMPTOM: {', '.join(unknown)} (see --list)")
    rank = min(SYMPTOMS[s][0] for s in symptoms)
    sev, level, response, templates = LEVELS[rank]
    templates = list(templates)
    for s in symptoms:
        templates += [t for t in EXTRA_TEMPLATES.get(s, []) if t not in templates]
    if rank == 1 and "protocol_paused" not in symptoms:
        actions = ["Evaluate emergency pause now: scripts/emergency-pause.sh",
                   "Verify pause on all targets: scripts/verify-pause-effect.sh"]
    elif "protocol_paused" in symptoms:
        actions = ["Verify pause on all targets: scripts/verify-pause-effect.sh"]
    else:
        actions = []
    return {
        "severity": sev,
        "level": level,
        "response_time": response,
        "drivers": [s for s in symptoms if SYMPTOMS[s][0] == rank],
        "symptoms": symptoms,
        "templates": [f"{TEMPLATES}/{t}" for t in templates],
        "actions": actions,
        "note": "Suggestion only - the Incident Commander confirms the final severity.",
    }


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--symptom", action="append", default=[])
    p.add_argument("--checklist")
    p.add_argument("--json", action="store_true")
    p.add_argument("--list", action="store_true")
    a = p.parse_args(argv)
    if a.list:
        for k, (r, d) in sorted(SYMPTOMS.items(), key=lambda kv: (kv[1][0], kv[0])):
            print(f"{LEVELS[r][0]:6} {k:28} {d}")
        return 0
    try:
        syms = list(a.symptom) + (parse_checklist(a.checklist) if a.checklist else [])
        r = classify(syms)
    except InputError as exc:
        print(exc, file=sys.stderr)
        return 2
    if a.json:
        print(json.dumps(r, indent=2))
    else:
        print(f"Suggested severity: {r['severity']} / {r['level']} (respond: {r['response_time']})")
        print(f"Driven by: {', '.join(r['drivers'])}")
        print("Templates:")
        for t in r["templates"]:
            print(f"  - {t}")
        for act in r["actions"]:
            print(f"Action: {act}")
        print(r["note"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
