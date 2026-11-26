"""Tests for scripts/classify_incident_sev.py (issue #1176).

Run: python3 -m unittest scripts/tests/test_classify_incident_sev.py
"""
import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "scripts"))
import classify_incident_sev as c  # noqa: E402


def run(*argv):
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        rc = c.main(list(argv))
    return rc, out.getvalue(), err.getvalue()


class ClassifyTests(unittest.TestCase):
    def test_active_exploit_is_sev1_and_prompts_pause(self):
        r = c.classify(["active_exploit"])
        self.assertEqual((r["severity"], r["level"]), ("Sev-1", "Critical"))
        self.assertTrue(any("emergency-pause.sh" in a for a in r["actions"]))

    def test_paused_prompts_pause_verification(self):
        r = c.classify(["protocol_paused"])
        self.assertEqual(r["severity"], "Sev-1")
        self.assertEqual(r["actions"], ["Verify pause on all targets: scripts/verify-pause-effect.sh"])

    def test_highest_severity_wins(self):
        r = c.classify(["ui_bug", "indexer_lag", "exploitable_vulnerability"])
        self.assertEqual((r["severity"], r["level"]), ("Sev-2", "High"))
        self.assertEqual(r["drivers"], ["exploitable_vulnerability"])
        self.assertIn(f"{c.TEMPLATES}/indexer-lag-internal.md", r["templates"])

    def test_degraded_dependency_is_sev3(self):
        r = c.classify(["rpc_degraded"])
        self.assertEqual((r["severity"], r["level"]), ("Sev-3", "Medium"))
        self.assertEqual(r["actions"], [])

    def test_low(self):
        self.assertEqual(c.classify(["docs_error"])["level"], "Low")

    def test_key_compromise_adds_scenario_templates(self):
        r = c.classify(["key_compromise"])
        self.assertIn(f"{c.TEMPLATES}/key-compromise-internal.md", r["templates"])

    def test_idempotent_and_dedup(self):
        self.assertEqual(c.classify(["indexer_lag", "INDEXER_LAG "]), c.classify(["indexer_lag"]))

    def test_every_referenced_template_exists(self):
        names = {t for lv in c.LEVELS.values() for t in lv[3]}
        names |= {t for ts in c.EXTRA_TEMPLATES.values() for t in ts}
        for n in names:
            self.assertTrue(os.path.isfile(os.path.join(ROOT, c.TEMPLATES, n)), n)

    def test_empty_input_fails_closed(self):
        rc, _, err = run()
        self.assertEqual(rc, 2)
        self.assertIn("ERR_NO_SYMPTOMS", err)

    def test_unknown_symptom_fails_closed(self):
        rc, _, err = run("--symptom", "aliens")
        self.assertEqual(rc, 2)
        self.assertIn("ERR_UNKNOWN_SYMPTOM", err)

    def test_missing_checklist_fails_closed(self):
        rc, _, err = run("--checklist", "/nonexistent/checklist.md")
        self.assertEqual(rc, 2)
        self.assertIn("ERR_CONFIG_MISSING", err)

    def test_markdown_checklist_only_counts_checked(self):
        with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False) as fh:
            fh.write("# symptoms\n- [x] keeper_degraded\n- [ ] funds_at_risk\n\nrpc_degraded # plain\n")
        try:
            rc, out, _ = run("--checklist", fh.name, "--json")
        finally:
            os.unlink(fh.name)
        self.assertEqual(rc, 0)
        r = json.loads(out)
        self.assertEqual(r["severity"], "Sev-3")
        self.assertEqual(r["symptoms"], ["keeper_degraded", "rpc_degraded"])

    def test_empty_checklist_fails_closed(self):
        with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False) as fh:
            fh.write("- [ ] funds_at_risk\n")
        try:
            rc, _, err = run("--checklist", fh.name)
        finally:
            os.unlink(fh.name)
        self.assertEqual(rc, 2)
        self.assertIn("ERR_NO_SYMPTOMS", err)

    def test_list_and_text_output(self):
        self.assertEqual(run("--list")[0], 0)
        rc, out, _ = run("--symptom", "funds_at_risk")
        self.assertEqual(rc, 0)
        self.assertIn("Suggested severity: Sev-1 / Critical", out)


if __name__ == "__main__":
    unittest.main()
