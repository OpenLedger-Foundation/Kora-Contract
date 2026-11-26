#!/usr/bin/env python3
"""Validate the RustSec advisory allowlist (issue #1173).

Fails closed (non-zero exit, typed error code) when:
  * deny.toml or .cargo/audit.toml is missing or has no [advisories] table
  * an allowlisted advisory ID is malformed (must be RUSTSEC-YYYY-NNNN)
  * a deny.toml exception lacks a `reason` or a `expires = YYYY-MM-DD` marker
  * an exception has expired
  * the two allowlists disagree

Usage: check_advisory_allowlist.py [--deny deny.toml] [--audit .cargo/audit.toml] [--today YYYY-MM-DD]
"""
import argparse
import datetime as dt
import re
import sys
import tomllib

ID_RE = re.compile(r"^RUSTSEC-\d{4}-\d{4}$")
EXPIRES_RE = re.compile(r"expires[=: ]+(\d{4}-\d{2}-\d{2})")

E_MISSING, E_SCHEMA, E_BAD_ID, E_NO_REASON, E_EXPIRED, E_DRIFT = (
    "ERR_CONFIG_MISSING", "ERR_CONFIG_SCHEMA", "ERR_BAD_ADVISORY_ID",
    "ERR_MISSING_REASON", "ERR_EXCEPTION_EXPIRED", "ERR_ALLOWLIST_DRIFT")


def load(path):
    try:
        with open(path, "rb") as fh:
            data = tomllib.load(fh)
    except FileNotFoundError:
        raise SystemExit(f"{E_MISSING}: {path} not found")
    except tomllib.TOMLDecodeError as exc:
        raise SystemExit(f"{E_SCHEMA}: {path}: {exc}")
    adv = data.get("advisories")
    if not isinstance(adv, dict) or "ignore" not in adv:
        raise SystemExit(f"{E_SCHEMA}: {path} has no [advisories].ignore list")
    return adv["ignore"]


def check(deny_path, audit_path, today):
    errors = []
    deny_ids = set()
    for entry in load(deny_path):
        if isinstance(entry, str):
            errors.append(f"{E_NO_REASON}: {entry} in {deny_path} must be a table with id + reason")
            deny_ids.add(entry)
            continue
        adv_id, reason = entry.get("id", ""), entry.get("reason", "").strip()
        deny_ids.add(adv_id)
        if not ID_RE.match(adv_id):
            errors.append(f"{E_BAD_ID}: {adv_id!r} in {deny_path}")
        if not reason:
            errors.append(f"{E_NO_REASON}: {adv_id} in {deny_path}")
            continue
        m = EXPIRES_RE.search(reason)
        if not m:
            errors.append(f"{E_NO_REASON}: {adv_id} reason must include 'expires=YYYY-MM-DD'")
        elif dt.date.fromisoformat(m.group(1)) < today:
            errors.append(f"{E_EXPIRED}: {adv_id} expired {m.group(1)}")
    audit_ids = set()
    for adv_id in load(audit_path):
        if not isinstance(adv_id, str) or not ID_RE.match(adv_id):
            errors.append(f"{E_BAD_ID}: {adv_id!r} in {audit_path}")
        audit_ids.add(adv_id)
    for missing in sorted(deny_ids ^ audit_ids):
        errors.append(f"{E_DRIFT}: {missing} present in only one of {deny_path} / {audit_path}")
    return errors


def main(argv=None):
    p = argparse.ArgumentParser()
    p.add_argument("--deny", default="deny.toml")
    p.add_argument("--audit", default=".cargo/audit.toml")
    p.add_argument("--today", default=dt.date.today().isoformat())
    a = p.parse_args(argv)
    errors = check(a.deny, a.audit, dt.date.fromisoformat(a.today))
    for e in errors:
        print(e, file=sys.stderr)
    if errors:
        return 1
    print("advisory allowlist OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
