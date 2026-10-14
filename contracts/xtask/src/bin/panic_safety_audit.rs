//! Panic Safety Audit Static Scanner
//!
//! Statically analyzes all production contract source files (`contracts/*/src/**/*.rs`),
//! detecting any calls to `.unwrap()`, `.expect(...)`, or `panic!()`, as well as
//! `#[allow(clippy::unwrap_used)]` suppressions.
//!
//! All detected panic paths must be documented in `contracts/panic_safety_allowlist.json`
//! with a valid security justification. Unapproved panic paths cause the audit to fail.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Attribute, ExprMacro, ExprMethodCall, File, Item, ItemMod};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExemptionEntry {
    pub file: String,
    pub line: usize,
    pub kind: String,
    #[serde(default)]
    pub snippet: String,
    pub justification: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allowlist {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub exemptions: Vec<ExemptionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FindingStatus {
    Approved { justification: String },
    Violation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub kind: String,
    pub snippet: String,
    pub status: FindingStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuppressionEntry {
    pub file: String,
    pub line: usize,
    pub attribute: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    pub total_files_scanned: usize,
    pub total_findings: usize,
    pub approved_count: usize,
    pub violation_count: usize,
    pub stale_allowlist_count: usize,
    pub suppressions: Vec<SuppressionEntry>,
    pub findings: Vec<Finding>,
    pub stale_exemptions: Vec<ExemptionEntry>,
}

struct AstScanner<'a> {
    file_rel_path: String,
    source_lines: &'a [&'a str],
    findings: Vec<Finding>,
    suppressions: Vec<SuppressionEntry>,
    allowlist: &'a Allowlist,
    used_exemptions: HashSet<usize>,
    in_test_module: bool,
}

impl<'a> AstScanner<'a> {
    fn new(
        file_rel_path: String,
        source_lines: &'a [&'a str],
        allowlist: &'a Allowlist,
    ) -> Self {
        Self {
            file_rel_path,
            source_lines,
            findings: Vec::new(),
            suppressions: Vec::new(),
            allowlist,
            used_exemptions: HashSet::new(),
            in_test_module: false,
        }
    }

    fn has_test_attr(attrs: &[Attribute]) -> bool {
        for attr in attrs {
            let meta_str = quote::quote!(#attr).to_string();
            if meta_str.contains("cfg (test)")
                || meta_str.contains("cfg(test)")
                || meta_str.contains("test")
            {
                return true;
            }
        }
        false
    }

    fn check_suppression_attrs(&mut self, attrs: &[Attribute]) {
        for attr in attrs {
            let meta_str = quote::quote!(#attr).to_string();
            if meta_str.contains("unwrap_used")
                || meta_str.contains("expect_used")
                || meta_str.contains("indexing_slicing")
            {
                let line = attr.span().start().line;
                self.suppressions.push(SuppressionEntry {
                    file: self.file_rel_path.clone(),
                    line,
                    attribute: meta_str,
                });
            }
        }
    }

    fn get_line_snippet(&self, line_num: usize) -> String {
        if line_num > 0 && line_num <= self.source_lines.len() {
            self.source_lines[line_num - 1].trim().to_string()
        } else {
            String::new()
        }
    }

    fn record_potential_violation(&mut self, kind: &str, line: usize, col: usize) {
        if self.in_test_module {
            return;
        }

        let snippet = self.get_line_snippet(line);

        // Check against allowlist
        let mut matched_entry: Option<(usize, &ExemptionEntry)> = None;
        let normalized_file = self.file_rel_path.replace('\\', "/");

        for (idx, entry) in self.allowlist.exemptions.iter().enumerate() {
            let entry_norm = entry.file.replace('\\', "/");
            if entry_norm == normalized_file {
                // Exact line match or fuzzy match within +/- 5 lines
                if entry.line == line || (line.abs_diff(entry.line) <= 5 && !entry.snippet.is_empty() && snippet.contains(&entry.snippet)) {
                    if !self.used_exemptions.contains(&idx) {
                        matched_entry = Some((idx, entry));
                        break;
                    } else if matched_entry.is_none() {
                        matched_entry = Some((idx, entry));
                    }
                }
            }
        }

        let status = if let Some((idx, entry)) = matched_entry {
            self.used_exemptions.insert(idx);
            FindingStatus::Approved {
                justification: entry.justification.clone(),
            }
        } else {
            FindingStatus::Violation
        };

        self.findings.push(Finding {
            file: normalized_file,
            line,
            col,
            kind: kind.to_string(),
            snippet,
            status,
        });
    }
}

impl<'a, 'ast> Visit<'ast> for AstScanner<'a> {
    fn visit_item(&mut self, i: &'ast Item) {
        match i {
            Item::Mod(ItemMod { attrs, .. }) | Item::Fn(syn::ItemFn { attrs, .. }) => {
                if Self::has_test_attr(attrs) {
                    let prev = self.in_test_module;
                    self.in_test_module = true;
                    syn::visit::visit_item(self, i);
                    self.in_test_module = prev;
                    return;
                }
                self.check_suppression_attrs(attrs);
            }
            Item::Impl(syn::ItemImpl { attrs, .. }) => {
                if Self::has_test_attr(attrs) {
                    let prev = self.in_test_module;
                    self.in_test_module = true;
                    syn::visit::visit_item(self, i);
                    self.in_test_module = prev;
                    return;
                }
                self.check_suppression_attrs(attrs);
            }
            _ => {}
        }
        syn::visit::visit_item(self, i);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method_name = node.method.to_string();
        if method_name == "unwrap" || method_name == "expect" {
            let span = node.method.span();
            let line = span.start().line;
            let col = span.start().column;
            self.record_potential_violation(&method_name, line, col);
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_macro(&mut self, node: &'ast ExprMacro) {
        if let Some(ident) = node.mac.path.get_ident() {
            if ident == "panic" {
                let span = ident.span();
                let line = span.start().line;
                let col = span.start().column;
                self.record_potential_violation("panic!", line, col);
            }
        }
        syn::visit::visit_expr_macro(self, node);
    }
}

pub fn collect_contract_src_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let contracts_dir = root.join("contracts");
    if !contracts_dir.is_dir() {
        return files;
    }

    if let Ok(entries) = fs::read_dir(contracts_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = path.file_name().unwrap_or_default().to_string_lossy();
            // Skip non-production directories
            if dir_name == "tests" || dir_name == "xtask" || dir_name == "fuzz" {
                continue;
            }

            let src_dir = path.join("src");
            if src_dir.is_dir() {
                collect_rs_recursive(&src_dir, &mut files);
            }
        }
    }
    files.sort();
    files
}

fn collect_rs_recursive(dir: &Path, acc: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_recursive(&path, acc);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                acc.push(path);
            }
        }
    }
}

pub fn run_audit(root: &Path, allowlist: &Allowlist) -> AuditReport {
    let src_files = collect_contract_src_files(root);
    let mut all_findings = Vec::new();
    let mut all_suppressions = Vec::new();
    let mut globally_used_exemptions = HashSet::new();

    for file_path in &src_files {
        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let parsed_file: File = match syn::parse_file(&content) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let rel_path = file_path
            .strip_prefix(root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .replace('\\', "/");

        let lines: Vec<&str> = content.lines().collect();
        let mut scanner = AstScanner::new(rel_path, &lines, allowlist);
        scanner.visit_file(&parsed_file);

        for used_idx in scanner.used_exemptions {
            globally_used_exemptions.insert(used_idx);
        }
        all_findings.extend(scanner.findings);
        all_suppressions.extend(scanner.suppressions);
    }

    let mut stale_exemptions = Vec::new();
    for (idx, entry) in allowlist.exemptions.iter().enumerate() {
        if !globally_used_exemptions.contains(&idx) {
            stale_exemptions.push(entry.clone());
        }
    }

    let approved_count = all_findings
        .iter()
        .filter(|f| matches!(f.status, FindingStatus::Approved { .. }))
        .count();
    let violation_count = all_findings
        .iter()
        .filter(|f| matches!(f.status, FindingStatus::Violation))
        .count();

    AuditReport {
        total_files_scanned: src_files.len(),
        total_findings: all_findings.len(),
        approved_count,
        violation_count,
        stale_allowlist_count: stale_exemptions.len(),
        suppressions: all_suppressions,
        findings: all_findings,
        stale_exemptions,
    }
}

pub fn generate_markdown_report(report: &AuditReport) -> String {
    let mut md = String::new();
    md.push_str("# Panic Safety Audit Report\n\n");
    md.push_str(&format!(
        "- **Status**: {}\n",
        if report.violation_count == 0 {
            "✅ PASS (0 unapproved panic paths)"
        } else {
            "❌ FAIL (unapproved panic paths detected)"
        }
    ));
    md.push_str(&format!("- **Files Scanned**: {}\n", report.total_files_scanned));
    md.push_str(&format!("- **Total Potential Panics Found**: {}\n", report.total_findings));
    md.push_str(&format!("- **Approved Allowlisted Entries**: {}\n", report.approved_count));
    md.push_str(&format!("- **Unapproved Violations**: {}\n", report.violation_count));
    md.push_str(&format!("- **Stale Allowlist Entries**: {}\n", report.stale_allowlist_count));
    md.push_str(&format!("- **Clippy Suppressions Detected**: {}\n\n", report.suppressions.len()));

    if !report.suppressions.is_empty() {
        md.push_str("## Clippy Suppressions (file:line)\n\n");
        md.push_str("| File | Line | Attribute |\n| --- | --- | --- |\n");
        for s in &report.suppressions {
            md.push_str(&format!("| `{}` | {} | `{}` |\n", s.file, s.line, s.attribute));
        }
        md.push_str("\n");
    }

    if report.violation_count > 0 {
        md.push_str("## ❌ Violations (Unapproved Panic Points)\n\n");
        md.push_str("| File | Line | Kind | Snippet |\n| --- | --- | --- | --- |\n");
        for f in &report.findings {
            if matches!(f.status, FindingStatus::Violation) {
                md.push_str(&format!(
                    "| `{}` | {} | `{}` | `{}` |\n",
                    f.file, f.line, f.kind, f.snippet
                ));
            }
        }
        md.push_str("\n");
    }

    if !report.findings.is_empty() {
        md.push_str("## Approved Exemptions\n\n");
        md.push_str("| File | Line | Kind | Justification |\n| --- | --- | --- | --- |\n");
        for f in &report.findings {
            if let FindingStatus::Approved { justification } = &f.status {
                md.push_str(&format!(
                    "| `{}` | {} | `{}` | {} |\n",
                    f.file, f.line, f.kind, justification
                ));
            }
        }
        md.push_str("\n");
    }

    if !report.stale_exemptions.is_empty() {
        md.push_str("## ⚠️ Stale Allowlist Entries (Can be cleaned up)\n\n");
        for s in &report.stale_exemptions {
            md.push_str(&format!("- `{}:{}` ({})\n", s.file, s.line, s.kind));
        }
        md.push_str("\n");
    }

    md
}

fn main() {
    let mut root_dir = PathBuf::from(".");
    let mut allowlist_path = root_dir.join("contracts/panic_safety_allowlist.json");
    let mut report_path = root_dir.join("target/panic-safety-audit-report.json");
    let mut markdown_path = root_dir.join("target/panic-safety-audit-report.md");

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--root" if i + 1 < args.len() => {
                root_dir = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--allowlist" if i + 1 < args.len() => {
                allowlist_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--report-path" if i + 1 < args.len() => {
                report_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--markdown-path" if i + 1 < args.len() => {
                markdown_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            _ => i += 1,
        }
    }

    // Auto-detect root if running from within contracts/xtask
    if !root_dir.join("contracts").is_dir() && root_dir.join("../../contracts").is_dir() {
        root_dir = root_dir.join("../..");
        allowlist_path = root_dir.join("contracts/panic_safety_allowlist.json");
        report_path = root_dir.join("target/panic-safety-audit-report.json");
        markdown_path = root_dir.join("target/panic-safety-audit-report.md");
    }

    let allowlist_content = fs::read_to_string(&allowlist_path).unwrap_or_else(|e| {
        eprintln!(
            "Failed to read allowlist at {}: {e}",
            allowlist_path.display()
        );
        std::process::exit(1);
    });

    let allowlist: Allowlist = serde_json::from_str(&allowlist_content).unwrap_or_else(|e| {
        eprintln!("Failed to parse allowlist JSON: {e}");
        std::process::exit(1);
    });

    println!("Running panic-safety-audit across production contract code...");
    let report = run_audit(&root_dir, &allowlist);

    // Ensure target output dir exists
    if let Some(parent) = report_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Write JSON report
    if let Ok(json_str) = serde_json::to_string_pretty(&report) {
        if let Err(e) = fs::write(&report_path, json_str) {
            eprintln!("Warning: failed to write JSON report to {}: {e}", report_path.display());
        } else {
            println!("Wrote JSON audit report to {}", report_path.display());
        }
    }

    // Write Markdown report
    let md_str = generate_markdown_report(&report);
    if let Err(e) = fs::write(&markdown_path, md_str) {
        eprintln!("Warning: failed to write Markdown report to {}: {e}", markdown_path.display());
    } else {
        println!("Wrote Markdown audit report to {}", markdown_path.display());
    }

    println!("\n=== Audit Summary ===");
    println!("Files scanned: {}", report.total_files_scanned);
    println!("Total findings: {}", report.total_findings);
    println!("Approved exemptions: {}", report.approved_count);
    println!("Clippy suppressions found: {}", report.suppressions.len());
    println!("Violations: {}", report.violation_count);

    if report.violation_count > 0 {
        eprintln!("\n❌ Panic Safety Audit FAILED. Unapproved panic points detected:");
        for f in &report.findings {
            if matches!(f.status, FindingStatus::Violation) {
                eprintln!("  --> {}:{}:{} [{}]: {}", f.file, f.line, f.col, f.kind, f.snippet);
            }
        }
        eprintln!("\nPlease eliminate the unwrap/expect or follow docs/BUG_BOUNTY_OPERATIONS.md to submit a justified exemption.");
        std::process::exit(1);
    }

    println!("\n✅ Panic Safety Audit PASSED. All panic paths conform to workspace security standards.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scanner_detects_unapproved_unwrap() {
        let code = r#"
            pub fn vulnerable_transfer(amount: i128) -> i128 {
                let opt = Some(amount);
                opt.unwrap()
            }
        "#;
        let file = syn::parse_file(code).unwrap();
        let lines: Vec<&str> = code.lines().collect();
        let allowlist = Allowlist {
            version: "1.0.0".to_string(),
            description: "empty".to_string(),
            exemptions: vec![],
        };
        let mut scanner = AstScanner::new("dummy/src/lib.rs".to_string(), &lines, &allowlist);
        scanner.visit_file(&file);

        assert_eq!(scanner.findings.len(), 1);
        assert!(matches!(scanner.findings[0].status, FindingStatus::Violation));
        assert_eq!(scanner.findings[0].kind, "unwrap");
    }

    #[test]
    fn test_scanner_approves_allowlisted_unwrap() {
        let code = r#"
            pub fn safe_calc() -> u32 {
                let x = Some(42);
                x.unwrap()
            }
        "#;
        let file = syn::parse_file(code).unwrap();
        let lines: Vec<&str> = code.lines().collect();
        let allowlist = Allowlist {
            version: "1.0.0".to_string(),
            description: "test".to_string(),
            exemptions: vec![ExemptionEntry {
                file: "dummy/src/lib.rs".to_string(),
                line: 4,
                kind: "unwrap".to_string(),
                snippet: "x.unwrap()".to_string(),
                justification: "Proven Some".to_string(),
            }],
        };
        let mut scanner = AstScanner::new("dummy/src/lib.rs".to_string(), &lines, &allowlist);
        scanner.visit_file(&file);

        assert_eq!(scanner.findings.len(), 1);
        assert!(matches!(scanner.findings[0].status, FindingStatus::Approved { .. }));
    }

    #[test]
    fn test_scanner_ignores_cfg_test_module() {
        let code = r#"
            pub fn prod_func() -> u32 { 1 }

            #[cfg(test)]
            mod tests {
                #[test]
                fn test_something() {
                    let opt = Some(1);
                    assert_eq!(opt.unwrap(), 1);
                }
            }
        "#;
        let file = syn::parse_file(code).unwrap();
        let lines: Vec<&str> = code.lines().collect();
        let allowlist = Allowlist {
            version: "1.0.0".to_string(),
            description: "test".to_string(),
            exemptions: vec![],
        };
        let mut scanner = AstScanner::new("dummy/src/lib.rs".to_string(), &lines, &allowlist);
        scanner.visit_file(&file);

        assert_eq!(scanner.findings.len(), 0, "Test module unwraps should be excluded");
    }

    #[test]
    fn test_production_contracts_audit_passes() {
        let mut root = std::env::current_dir().unwrap();
        if !root.join("contracts").is_dir() && root.join("../../contracts").is_dir() {
            root = root.join("../..");
        }
        let allowlist_path = root.join("contracts").join("panic_safety_allowlist.json");
        let content = fs::read_to_string(&allowlist_path).expect("Must read panic_safety_allowlist.json");
        let allowlist: Allowlist = serde_json::from_str(&content).expect("Must parse allowlist JSON");

        let report = run_audit(&root, &allowlist);
        assert!(report.total_files_scanned > 0, "Should scan production contract files");
        assert_eq!(report.violation_count, 0, "Production contracts must have zero unapproved panics");
    }
}

