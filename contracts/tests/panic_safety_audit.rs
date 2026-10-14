//! Panic Safety Audit Integration Test (Issue #921)
//!
//! Validates that production paths in deployed contract crates avoid unwrap,
//! expect, and unapproved panic invocations.
//!
//! Every exception must be justified and recorded in `contracts/panic_safety_allowlist.json`.

use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;
use syn::{Attribute, ExprMacro, ExprMethodCall, File, Item, ItemMod};

#[derive(Debug, Clone, Deserialize)]
struct ExemptionEntry {
    file: String,
    line: usize,
    kind: String,
    #[serde(default)]
    snippet: String,
    justification: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Allowlist {
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    exemptions: Vec<ExemptionEntry>,
}

#[derive(Debug, Clone)]
struct Finding {
    file: String,
    line: usize,
    kind: String,
    snippet: String,
    is_approved: bool,
}

struct TestAstScanner<'a> {
    file_rel_path: String,
    source_lines: &'a [&'a str],
    findings: Vec<Finding>,
    allowlist: &'a Allowlist,
    used_exemptions: HashSet<usize>,
    in_test_module: bool,
}

impl<'a> TestAstScanner<'a> {
    fn new(file_rel_path: String, source_lines: &'a [&'a str], allowlist: &'a Allowlist) -> Self {
        Self {
            file_rel_path,
            source_lines,
            findings: Vec::new(),
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

    fn get_snippet(&self, line: usize) -> String {
        if line > 0 && line <= self.source_lines.len() {
            self.source_lines[line - 1].trim().to_string()
        } else {
            String::new()
        }
    }

    fn check_finding(&mut self, kind: &str, line: usize) {
        if self.in_test_module {
            return;
        }

        let snippet = self.get_snippet(line);
        let normalized_file = self.file_rel_path.replace('\\', "/");

        let mut matched_idx = None;
        for (idx, entry) in self.allowlist.exemptions.iter().enumerate() {
            let entry_file = entry.file.replace('\\', "/");
            if entry_file == normalized_file {
                if entry.line == line || (line.abs_diff(entry.line) <= 5 && !entry.snippet.is_empty() && snippet.contains(&entry.snippet)) {
                    if !self.used_exemptions.contains(&idx) {
                        matched_idx = Some(idx);
                        break;
                    } else if matched_idx.is_none() {
                        matched_idx = Some(idx);
                    }
                }
            }
        }

        let is_approved = if let Some(idx) = matched_idx {
            self.used_exemptions.insert(idx);
            true
        } else {
            false
        };

        self.findings.push(Finding {
            file: normalized_file,
            line,
            kind: kind.to_string(),
            snippet,
            is_approved,
        });
    }
}

impl<'a, 'ast> Visit<'ast> for TestAstScanner<'a> {
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
            }
            Item::Impl(syn::ItemImpl { attrs, .. }) => {
                if Self::has_test_attr(attrs) {
                    let prev = self.in_test_module;
                    self.in_test_module = true;
                    syn::visit::visit_item(self, i);
                    self.in_test_module = prev;
                    return;
                }
            }
            _ => {}
        }
        syn::visit::visit_item(self, i);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let name = node.method.to_string();
        if name == "unwrap" || name == "expect" {
            // Find line from text if span-locations isn't active
            let line = node.method.span().start().line;
            self.check_finding(&name, line);
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_macro(&mut self, node: &'ast ExprMacro) {
        if let Some(ident) = node.mac.path.get_ident() {
            if ident == "panic" {
                let line = ident.span().start().line;
                self.check_finding("panic!", line);
            }
        }
        syn::visit::visit_expr_macro(self, node);
    }
}

fn find_workspace_root() -> PathBuf {
    let mut current = std::env::current_dir().expect("must get cwd");
    while !current.join("Cargo.toml").is_file() || !current.join("contracts").is_dir() {
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    current
}

fn collect_production_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let contracts_dir = root.join("contracts");
    if let Ok(entries) = fs::read_dir(&contracts_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name == "tests" || name == "xtask" || name == "fuzz" {
                continue;
            }
            let src = path.join("src");
            if src.is_dir() {
                collect_rs_files(&src, &mut files);
            }
        }
    }
    files.sort();
    files
}

fn collect_rs_files(dir: &Path, acc: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, acc);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                acc.push(path);
            }
        }
    }
}

fn load_allowlist(root: &Path) -> Allowlist {
    let allowlist_path = root.join("contracts/panic_safety_allowlist.json");
    assert!(
        allowlist_path.is_file(),
        "Allowlist must exist at {}",
        allowlist_path.display()
    );
    let content = fs::read_to_string(&allowlist_path).expect("read allowlist");
    serde_json::from_str(&content).expect("parse allowlist JSON")
}

#[test]
fn test_production_code_has_zero_unapproved_panics() {
    let root = find_workspace_root();
    let allowlist = load_allowlist(&root);
    let sources = collect_production_sources(&root);
    assert!(!sources.is_empty(), "Must find production contract sources");

    let mut violations = Vec::new();
    for file_path in &sources {
        let content = fs::read_to_string(file_path).expect("read source");
        let parsed: File = syn::parse_file(&content).expect("parse rust AST");
        let rel_path = file_path
            .strip_prefix(&root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .replace('\\', "/");

        let lines: Vec<&str> = content.lines().collect();
        let mut scanner = TestAstScanner::new(rel_path, &lines, &allowlist);
        scanner.visit_file(&parsed);

        for finding in scanner.findings {
            if !finding.is_approved {
                violations.push(finding);
            }
        }
    }

    if !violations.is_empty() {
        eprintln!("\n❌ Panic Safety Audit FAILED. Found unapproved panic paths:");
        for v in &violations {
            eprintln!("  --> {}:{} [{}]: {}", v.file, v.line, v.kind, v.snippet);
        }
        panic!(
            "{} unapproved panic points found. Eliminate them or document them in contracts/panic_safety_allowlist.json according to docs/BUG_BOUNTY_OPERATIONS.md",
            violations.len()
        );
    }
}

#[test]
fn test_allowlist_integrity_and_justifications() {
    let root = find_workspace_root();
    let allowlist = load_allowlist(&root);

    assert!(
        !allowlist.exemptions.is_empty(),
        "Allowlist should not be empty"
    );

    for (idx, entry) in allowlist.exemptions.iter().enumerate() {
        assert!(
            !entry.file.is_empty(),
            "Exemption #{} file path cannot be empty",
            idx
        );
        let full_path = root.join(&entry.file);
        assert!(
            full_path.is_file(),
            "Exemption #{} references non-existent file: {}",
            idx,
            entry.file
        );
        assert!(
            entry.line > 0,
            "Exemption #{} line must be positive: {}",
            idx,
            entry.line
        );
        assert!(
            entry.justification.trim().len() >= 15,
            "Exemption #{} for {} must have a thorough security justification (at least 15 chars), got '{}'",
            idx,
            entry.file,
            entry.justification
        );
    }
}

#[test]
fn test_scanner_detects_synthetic_unapproved_unwrap() {
    let root = find_workspace_root();
    let allowlist = load_allowlist(&root);

    let synthetic_code = r#"
        pub fn dangerous_production_function(x: Option<i32>) -> i32 {
            x.unwrap()
        }
    "#;

    let parsed: File = syn::parse_file(synthetic_code).expect("parse synthetic");
    let lines: Vec<&str> = synthetic_code.lines().collect();
    let mut scanner = TestAstScanner::new("contracts/synthetic/src/lib.rs".into(), &lines, &allowlist);
    scanner.visit_file(&parsed);

    let unapproved: Vec<_> = scanner.findings.into_iter().filter(|f| !f.is_approved).collect();
    assert_eq!(
        unapproved.len(),
        1,
        "Scanner must detect synthetic unapproved unwrap"
    );
    assert_eq!(unapproved[0].kind, "unwrap");
}

#[test]
fn test_scanner_ignores_test_module_unwraps() {
    let root = find_workspace_root();
    let allowlist = load_allowlist(&root);

    let synthetic_test_code = r#"
        #[cfg(test)]
        mod test {
            #[test]
            fn test_something() {
                let x = Some(42);
                assert_eq!(x.unwrap(), 42);
            }
        }
    "#;

    let parsed: File = syn::parse_file(synthetic_test_code).expect("parse test code");
    let lines: Vec<&str> = synthetic_test_code.lines().collect();
    let mut scanner = TestAstScanner::new("contracts/synthetic/src/lib.rs".into(), &lines, &allowlist);
    scanner.visit_file(&parsed);

    let unapproved: Vec<_> = scanner.findings.into_iter().filter(|f| !f.is_approved).collect();
    assert!(
        unapproved.is_empty(),
        "Scanner must ignore unwrap inside #[cfg(test)] modules"
    );
}
