//! Pause-matrix documentation check (issue #1087).
//!
//! Statically finds every pause-guarded contract entrypoint and verifies that
//! `docs/SECURITY.md` → *Pause-Semantics Enforcement Matrix* documents it.
//!
//! An entrypoint is a `pub fn` inside a `#[contractimpl]` impl block. It is
//! pause-guarded when its body calls `require_not_paused` (or checks `is_paused`
//! and returns a `*Paused` error), directly or through a private `Self::` helper
//! that does. `#[cfg(test)]` modules are ignored.
//!
//! The check fails when:
//!   - a pause-guarded entrypoint has no `✓ YES` row,
//!   - a `✓ YES` row names an entrypoint that is not pause-guarded,
//!   - a `✗ NO` row names an entrypoint that is pause-guarded or does not exist,
//!   - an `N/A` row is used for a contract that has pause-guarded entrypoints,
//!   - a workspace contract has no row at all,
//!   - a pause-guarded entrypoint has no row in `docs/ACCESS_MATRIX.md`
//!     under a `### <n>. <contract>` section (issue #1183: undocumented
//!     pause bypasses — a pausable entrypoint missing from the access matrix
//!     is an integrator/incident doc hole).
//!
//! Usage:
//!   cargo run -p kora-xtask --bin check_pause_matrix
//!   cargo run -p kora-xtask --bin check_pause_matrix -- --list   # print the inventory

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use syn::{ImplItem, Item, Visibility};

/// Crates under `contracts/` that are not deployable contracts.
const NON_CONTRACT_CRATES: &[&str] = &["shared", "tests", "xtask", "fuzz"];
/// Names never treated as propagating helpers (cross-contract client calls).
const NON_HELPERS: &[&str] = &["is_paused"];

#[derive(Debug, Default)]
struct ContractInventory {
    entrypoints: BTreeSet<String>,
    pausable: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Yes,
    No,
    NotApplicable,
}

#[derive(Debug)]
struct Row {
    contract: String,
    entrypoints: Vec<String>,
    status: Status,
    line: usize,
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Identifier-boundary-aware substring search.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let s = start + pos;
        let e = s + needle.len();
        let before_ok = s == 0 || !is_ident_char(bytes[s - 1]);
        let after_ok = e == bytes.len() || !is_ident_char(bytes[e]);
        if before_ok && after_ok {
            return true;
        }
        start = s + 1;
    }
    false
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && quote::quote!(#a)
                .to_string()
                .replace(' ', "")
                .contains("cfg(test)")
    })
}

fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs
        .iter()
        .any(|a| a.path().segments.last().is_some_and(|s| s.ident == name))
}

struct FnInfo {
    name: String,
    text: String,
    /// `pub fn` in a `#[contractimpl]` block.
    entrypoint: bool,
    /// Free function (callable without `Self::`).
    free: bool,
}

fn collect_items(items: &[Item], out: &mut Vec<FnInfo>) {
    for item in items {
        match item {
            Item::Mod(m) if !is_cfg_test(&m.attrs) => {
                if let Some((_, inner)) = &m.content {
                    collect_items(inner, out);
                }
            }
            Item::Fn(f) if !is_cfg_test(&f.attrs) => {
                let mut f = f.clone();
                f.attrs.retain(|a| !a.path().is_ident("doc"));
                out.push(FnInfo {
                    name: f.sig.ident.to_string(),
                    text: quote::quote!(#f).to_string(),
                    entrypoint: false,
                    free: true,
                });
            }
            Item::Impl(imp) if !is_cfg_test(&imp.attrs) => {
                let contract_impl = has_attr(&imp.attrs, "contractimpl");
                for ii in &imp.items {
                    if let ImplItem::Fn(f) = ii {
                        if is_cfg_test(&f.attrs) {
                            continue;
                        }
                        let mut f = f.clone();
                        f.attrs.retain(|a| !a.path().is_ident("doc"));
                        out.push(FnInfo {
                            name: f.sig.ident.to_string(),
                            text: quote::quote!(#f).to_string(),
                            entrypoint: contract_impl && matches!(f.vis, Visibility::Public(_)),
                            free: false,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn directly_guarded(text: &str) -> bool {
    // Skip the signature: only the body counts.
    let body = text.find('{').map_or(text, |i| &text[i..]);
    contains_word(body, "require_not_paused")
        || (contains_word(body, "is_paused") && body.contains("Paused"))
}

fn calls(text: &str, name: &str, free: bool) -> bool {
    let body = text.find('{').map_or(text, |i| &text[i..]);
    if body.contains(&format!("Self :: {name} (")) {
        return true;
    }
    free && contains_word(body, name) && body.contains(&format!("{name} ("))
}

/// Inventory one contract from its Rust sources.
fn inventory_sources(sources: &[String]) -> ContractInventory {
    let mut fns = Vec::new();
    for src in sources {
        if let Ok(file) = syn::parse_file(src) {
            collect_items(&file.items, &mut fns);
        }
    }

    let mut guarded: BTreeSet<String> = fns
        .iter()
        .filter(|f| directly_guarded(&f.text))
        .map(|f| f.name.clone())
        .collect();
    loop {
        let helpers: Vec<(String, bool)> = fns
            .iter()
            .filter(|f| guarded.contains(&f.name) && !NON_HELPERS.contains(&f.name.as_str()))
            .map(|f| (f.name.clone(), f.free))
            .collect();
        let newly: Vec<String> = fns
            .iter()
            .filter(|f| !guarded.contains(&f.name))
            .filter(|f| helpers.iter().any(|(h, free)| calls(&f.text, h, *free)))
            .map(|f| f.name.clone())
            .collect();
        if newly.is_empty() {
            break;
        }
        guarded.extend(newly);
    }

    let mut inv = ContractInventory::default();
    for f in fns.iter().filter(|f| f.entrypoint) {
        inv.entrypoints.insert(f.name.clone());
        if guarded.contains(&f.name) && !NON_HELPERS.contains(&f.name.as_str()) {
            inv.pausable.insert(f.name.clone());
        }
    }
    inv
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
}

fn inventory_workspace(root: &Path) -> BTreeMap<String, ContractInventory> {
    let mut result = BTreeMap::new();
    let mut dirs: Vec<_> = fs::read_dir(root.join("contracts"))
        .expect("read contracts/")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("src/lib.rs").is_file())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        if NON_CONTRACT_CRATES.contains(&name.as_str()) {
            continue;
        }
        let mut files = Vec::new();
        collect_rs_files(&dir.join("src"), &mut files);
        files.sort();
        let sources: Vec<String> = files
            .iter()
            .filter_map(|f| fs::read_to_string(f).ok())
            .collect();
        result.insert(name, inventory_sources(&sources));
    }
    result
}

/// Parse `| **contract** | `fn` / `fn2` | ✓ YES | notes |` rows.
fn parse_matrix(markdown: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for (i, line) in markdown.lines().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with("| **") {
            continue;
        }
        let cells: Vec<&str> = trimmed
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() < 3 {
            continue;
        }
        let contract = cells[0].trim_matches('*').trim().to_string();
        let mut entrypoints = Vec::new();
        let mut rest = cells[1];
        while let Some(start) = rest.find('`') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('`') else { break };
            entrypoints.push(after[..end].trim().to_string());
            rest = &after[end + 1..];
        }
        let status = if cells[2].contains('✓') {
            Status::Yes
        } else if cells[2].contains('✗') {
            Status::No
        } else if cells[2].contains("N/A") {
            Status::NotApplicable
        } else {
            continue;
        };
        rows.push(Row {
            contract,
            entrypoints,
            status,
            line: i + 1,
        });
    }
    rows
}

fn check(inventory: &BTreeMap<String, ContractInventory>, rows: &[Row]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut documented_yes: BTreeSet<(String, String)> = BTreeSet::new();
    let mut listed: BTreeSet<String> = BTreeSet::new();

    for row in rows {
        listed.insert(row.contract.clone());
        let Some(inv) = inventory.get(&row.contract) else {
            problems.push(format!(
                "SECURITY.md:{}: unknown contract `{}`",
                row.line, row.contract
            ));
            continue;
        };
        match row.status {
            Status::Yes => {
                for ep in &row.entrypoints {
                    if !inv.pausable.contains(ep) {
                        problems.push(format!(
                            "SECURITY.md:{}: `{}::{ep}` is marked ✓ YES but is not pause-guarded",
                            row.line, row.contract
                        ));
                    }
                    documented_yes.insert((row.contract.clone(), ep.clone()));
                }
            }
            Status::No => {
                for ep in &row.entrypoints {
                    if !inv.entrypoints.contains(ep) {
                        problems.push(format!(
                            "SECURITY.md:{}: `{}::{ep}` (✗ NO) is not a contract entrypoint",
                            row.line, row.contract
                        ));
                    } else if inv.pausable.contains(ep) {
                        problems.push(format!(
                            "SECURITY.md:{}: `{}::{ep}` is marked ✗ NO but is pause-guarded",
                            row.line, row.contract
                        ));
                    }
                }
            }
            Status::NotApplicable => {
                if !inv.pausable.is_empty() {
                    problems.push(format!(
                        "SECURITY.md:{}: `{}` is marked N/A but has {} pause-guarded entrypoint(s)",
                        row.line,
                        row.contract,
                        inv.pausable.len()
                    ));
                }
            }
        }
    }

    for (contract, inv) in inventory {
        if !listed.contains(contract) {
            problems.push(format!(
                "`{contract}` is not in the pause matrix (add ✓ rows, or an N/A row if it has no pause guard)"
            ));
        }
        for ep in &inv.pausable {
            if !documented_yes.contains(&(contract.clone(), ep.clone())) {
                problems.push(format!(
                    "`{contract}::{ep}` is pause-guarded but has no ✓ YES row"
                ));
            }
        }
    }
    problems
}

/// Collect entrypoint names documented in `docs/ACCESS_MATRIX.md`, keyed by
/// contract. A section belongs to a contract when its `### ` heading's first
/// word after the optional `<n>.` prefix is a known contract name; table rows
/// below it contribute every backticked name in their first cell (call
/// suffixes such as `fn(args)` are trimmed).
fn parse_access_matrix(
    markdown: &str,
    contracts: &BTreeSet<String>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            current = trimmed.strip_prefix("### ").and_then(|h| {
                let h = h.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
                let word: String = h
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                contracts.contains(&word).then_some(word)
            });
            continue;
        }
        let Some(contract) = &current else { continue };
        if !trimmed.starts_with('|') {
            continue;
        }
        let first = trimmed.trim_matches('|').split('|').next().unwrap_or("");
        let mut rest = first;
        while let Some(start) = rest.find('`') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('`') else { break };
            let name = after[..end].split('(').next().unwrap_or("").trim();
            if !name.is_empty() {
                out.entry(contract.clone()).or_default().insert(name.to_string());
            }
            rest = &after[end + 1..];
        }
    }
    out
}

/// Every pause-guarded entrypoint must have an ACCESS_MATRIX row (#1183).
fn check_access_matrix(
    inventory: &BTreeMap<String, ContractInventory>,
    access: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (contract, inv) in inventory {
        for ep in &inv.pausable {
            if !access.get(contract).is_some_and(|s| s.contains(ep)) {
                problems.push(format!(
                    "ACCESS_MATRIX.md: `{contract}::{ep}` is pause-guarded but has no access-matrix row"
                ));
            }
        }
    }
    problems
}

fn locate_workspace_root() -> PathBuf {
    let cwd = std::env::current_dir().expect("failed to read current directory");
    cwd.ancestors()
        .find(|p| p.join("contracts").is_dir() && p.join("Cargo.toml").is_file())
        .expect("could not locate workspace root")
        .to_path_buf()
}

fn main() -> ExitCode {
    let root = locate_workspace_root();
    let inventory = inventory_workspace(&root);

    if std::env::args().any(|a| a == "--list") {
        for (c, inv) in &inventory {
            let list: Vec<&str> = inv.pausable.iter().map(String::as_str).collect();
            println!("{c} ({}): {}", list.len(), list.join(", "));
        }
        return ExitCode::SUCCESS;
    }

    let doc_path = root.join("docs/SECURITY.md");
    let markdown = fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", doc_path.display()));
    let mut problems = check(&inventory, &parse_matrix(&markdown));

    let access_path = root.join("docs/ACCESS_MATRIX.md");
    let access_md = fs::read_to_string(&access_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", access_path.display()));
    let names: BTreeSet<String> = inventory.keys().cloned().collect();
    problems.extend(check_access_matrix(
        &inventory,
        &parse_access_matrix(&access_md, &names),
    ));

    let total: usize = inventory.values().map(|i| i.pausable.len()).sum();
    if problems.is_empty() {
        println!(
            "check_pause_matrix: OK — {total} pause-guarded entrypoints across {} contracts are documented in docs/SECURITY.md and docs/ACCESS_MATRIX.md.",
            inventory.len()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("check_pause_matrix: docs/SECURITY.md / docs/ACCESS_MATRIX.md are out of sync with the code:\n");
        for p in &problems {
            eprintln!("  - {p}");
        }
        eprintln!("\nFix: update the Pause-Semantics Enforcement Matrix (run with --list to see the inventory).");
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
        #[contract] pub struct C;
        #[contractimpl]
        impl C {
            pub fn mint(env: Env) -> Result<(), E> { Self::require_not_paused(&env)?; Ok(()) }
            pub fn fund(env: Env) -> Result<(), E> { Self::do_fund(&env) }
            pub fn repay(env: Env) -> Result<(), E> { Ok(()) }
            /// Docs mentioning require_not_paused must not count.
            pub fn view(env: Env) -> u32 { 1 }
        }
        impl C {
            fn do_fund(env: &Env) -> Result<(), E> { Self::require_not_paused(env)?; Ok(()) }
            fn require_not_paused(env: &Env) -> Result<(), E> {
                if client(env).is_paused() { return Err(E::ProtocolPaused); }
                Ok(())
            }
            pub fn not_an_entrypoint(env: Env) { Self::require_not_paused(&env); }
        }
        #[cfg(test)]
        mod tests { #[contractimpl] impl C { pub fn test_only(env: Env) { Self::require_not_paused(&env); } } }
    "#;

    fn inv() -> BTreeMap<String, ContractInventory> {
        let mut m = BTreeMap::new();
        m.insert("c".to_string(), inventory_sources(&[SRC.to_string()]));
        m.insert(
            "other".to_string(),
            inventory_sources(&["#[contractimpl] impl O { pub fn x(env: Env) {} }".to_string()]),
        );
        m
    }

    #[test]
    fn detects_direct_and_helper_guards_only_on_entrypoints() {
        let i = inv();
        let c = &i["c"];
        assert_eq!(
            c.pausable.iter().cloned().collect::<Vec<_>>(),
            vec!["fund", "mint"]
        );
        assert!(c.entrypoints.contains("repay") && !c.entrypoints.contains("not_an_entrypoint"));
        assert!(!c.entrypoints.contains("test_only"));
    }

    #[test]
    fn accepts_a_complete_matrix() {
        let md = "| **c** | `mint` / `fund` | ✓ YES | |\n| **c** | `repay` | ✗ NO | settlement |\n| **other** | — | N/A | |";
        assert!(check(&inv(), &parse_matrix(md)).is_empty());
    }

    #[test]
    fn reports_every_kind_of_drift() {
        let md = "| **c** | `mint` / `repay` | ✓ YES | |\n| **c** | `fund` | ✗ NO | |\n| **c** | `ghost` | ✗ NO | |";
        let problems = check(&inv(), &parse_matrix(md)).join("\n");
        assert!(problems.contains("`c::repay` is marked ✓ YES but is not pause-guarded"));
        assert!(problems.contains("`c::fund` is marked ✗ NO but is pause-guarded"));
        assert!(problems.contains("`c::ghost` (✗ NO) is not a contract entrypoint"));
        assert!(problems.contains("`c::fund` is pause-guarded but has no ✓ YES row"));
        assert!(problems.contains("`other` is not in the pause matrix"));
    }

    #[test]
    fn rejects_na_for_pausable_contract() {
        let md = "| **c** | — | N/A | |\n| **other** | — | N/A | |";
        let problems = check(&inv(), &parse_matrix(md)).join("\n");
        assert!(problems.contains("`c` is marked N/A"));
    }

    fn names() -> BTreeSet<String> {
        ["c".to_string(), "other".to_string()].into_iter().collect()
    }

    #[test]
    fn access_matrix_accepts_documented_pausable_entrypoints() {
        let md = "### 1. c\n| Entrypoint | Role |\n|---|---|\n| `mint` | Admin |\n| `fund(amount)` | Investor |\n";
        let access = parse_access_matrix(md, &names());
        assert!(check_access_matrix(&inv(), &access).is_empty());
    }

    #[test]
    fn access_matrix_flags_undocumented_pause_bypass() {
        // `fund` is only documented under another contract's section.
        let md = "### 1. c\n| `mint` | Admin |\n### 2. other\n| `fund` | x |\n";
        let problems = check_access_matrix(&inv(), &parse_access_matrix(md, &names())).join("\n");
        assert!(problems.contains("`c::fund` is pause-guarded but has no access-matrix row"));
        assert!(!problems.contains("`c::mint`"));
    }

    #[test]
    fn access_matrix_missing_section_fails_closed() {
        let problems = check_access_matrix(&inv(), &parse_access_matrix("", &names()));
        assert_eq!(problems.len(), 2);
    }

    #[test]
    fn access_matrix_ignores_unknown_and_non_table_lines() {
        let md = "### Quick reference\n| `mint` | x |\n### 1. c\nprose `fund` here\n";
        let access = parse_access_matrix(md, &names());
        assert!(access.get("c").is_none());
    }
}
