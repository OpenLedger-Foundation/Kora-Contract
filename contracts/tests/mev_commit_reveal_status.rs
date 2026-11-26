//! MEV: commit-reveal funding window status guard (issue #1181)
//!
//! `docs/MEV_RESISTANCE_NOTES.md` §3.4 declares commit-reveal funding as
//! **N/A** for Kora: `marketplace::fund_invoice` is single-step. A
//! half-specified MEV mitigation confuses integrators, so this suite keeps the
//! docs and the code in lock-step:
//!
//! - the N/A declaration and its rationale must stay in the MEV notes;
//! - the marketplace must not grow a commit/reveal entrypoint or storage key
//!   while the docs still say N/A;
//! - `fund_invoice` must stay pause-guarded and documented as such, since with
//!   no pending commitments the pause is the only ordering-related halt.
//!
//! Run: `cargo test -p kora-tests --test mev_commit_reveal_status`

#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

const MEV_NOTES: &str = include_str!("../../docs/MEV_RESISTANCE_NOTES.md");
const SECURITY: &str = include_str!("../../docs/SECURITY.md");

/// Identifier fragments that would indicate a commit-reveal funding flow.
const COMMIT_REVEAL_MARKERS: &[&str] = &[
    "fn commit_funding",
    "fn reveal_funding",
    "fn commit_fund",
    "fn reveal_fund",
    "FundingCommitment",
    "RevealDeadline",
    "CommitRevealWindow",
];

fn marketplace_sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in fs::read_dir(dir).expect("read marketplace/src").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src = fs::read_to_string(&path).expect("read source");
                out.push((path, src));
            }
        }
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../marketplace/src");
    let mut out = Vec::new();
    walk(&dir, &mut out);
    assert!(!out.is_empty(), "no marketplace sources found — fail closed");
    out
}

fn section(doc: &str, heading: &str) -> String {
    let start = doc
        .find(heading)
        .unwrap_or_else(|| panic!("missing section `{heading}` in MEV notes"));
    let rest = &doc[start + heading.len()..];
    let end = rest.find("\n## ").or_else(|| rest.find("\n### ")).unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn mev_notes_declare_commit_reveal_not_applicable_with_rationale() {
    let s = section(MEV_NOTES, "### 3.4 Commit-Reveal Funding Window");
    assert!(s.contains("Status: N/A"), "status line missing");
    assert!(s.contains("**Rationale:**"), "rationale missing");
    for anchor in ["fund_invoice", "PriorityWindow", "MaxInvestorShareBps", "mev_commit_reveal_status.rs"] {
        assert!(s.contains(anchor), "rationale must reference `{anchor}`");
    }
}

#[test]
fn attack_surface_table_lists_commit_reveal_as_na() {
    let row = MEV_NOTES
        .lines()
        .find(|l| l.starts_with("| Commit-reveal funding window"))
        .expect("attack-surface table row missing");
    assert!(row.contains("N/A"));
}

#[test]
fn marketplace_has_no_commit_reveal_entrypoints_while_docs_say_na() {
    for (path, src) in marketplace_sources() {
        for marker in COMMIT_REVEAL_MARKERS {
            assert!(
                !src.contains(marker),
                "{} contains `{marker}` but docs/MEV_RESISTANCE_NOTES.md §3.4 says \
                 commit-reveal is N/A — document the window (params, pause, expiry) \
                 and update this guard",
                path.display()
            );
        }
    }
}

#[test]
fn fund_invoice_is_single_step_and_pause_guarded() {
    let lib = marketplace_sources()
        .into_iter()
        .find(|(p, _)| p.ends_with("lib.rs"))
        .map(|(_, s)| s)
        .expect("marketplace lib.rs");
    let start = lib.find("pub fn fund_invoice(").expect("fund_invoice entrypoint");
    let body = &lib[start..];
    let next = body[1..].find("pub fn ").map_or(body.len(), |i| i + 1);
    let body = &body[..next];
    assert!(
        body.contains("require_not_paused"),
        "fund_invoice must halt under pause (no pending commitments to drain)"
    );
    assert!(
        SECURITY
            .lines()
            .any(|l| l.contains("**marketplace**") && l.contains("`fund_invoice`") && l.contains('✓')),
        "SECURITY.md pause matrix must mark fund_invoice ✓ YES"
    );
}

#[test]
fn guard_markers_would_detect_a_commit_reveal_flow() {
    // Negative path: the guard actually fires on a synthetic two-phase flow.
    let synthetic = "pub fn commit_funding(env: Env) {} pub fn reveal_funding(env: Env) {}";
    assert!(COMMIT_REVEAL_MARKERS.iter().any(|m| synthetic.contains(m)));
    let single_step = "pub fn fund_invoice(env: Env) {}";
    assert!(!COMMIT_REVEAL_MARKERS.iter().any(|m| single_step.contains(m)));
}
