//! Standalone consistency check (issue #421): every `KoraError::<Variant>` reference
//! in a dependent crate must resolve to a variant actually declared on `KoraError` in
//! `contracts/shared/src/errors.rs`. Run via `cargo run -p kora-xtask --bin
//! check-error-variants` (wired into CI in `.github/workflows/ci.yml`).
//!
//! This tool intentionally has no dependency on the contract crates themselves — it
//! parses their source as plain text, so it keeps working even when the workspace
//! itself fails to compile.

mod spec;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Contract crates (directory names under `contracts/`) that may reference `KoraError`.
const WORKSPACE_CRATES: &[&str] = &[
    "shared",
    "invoice_nft",
    "marketplace",
    "financing_pool",
    "treasury",
    "risk_registry",
    "access_control",
    "price_oracle",
    "secondary_market",
];

fn strip_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.find("//").map_or(line, |idx| &line[..idx]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract the top-level variant identifiers of `enum {enum_name}` declared in `source`.
fn extract_enum_variants(source: &str, enum_name: &str) -> Option<Vec<String>> {
    let stripped = strip_line_comments(source);
    let needle = format!("enum {enum_name}");
    let start = stripped.find(&needle)?;
    let brace_start = stripped[start..].find('{')? + start;

    let mut depth = 0i32;
    let mut end = None;
    for (i, c) in stripped[brace_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(brace_start + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &stripped[brace_start + 1..end?];

    // Split the body on top-level commas (depth 0), skipping over tuple/struct
    // variant payloads like `Invoice(u64)` or `Proposal(u64)`.
    let mut raw_variants = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in body.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                raw_variants.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        raw_variants.push(current);
    }

    Some(
        raw_variants
            .iter()
            .filter_map(|v| {
                let trimmed = v.trim();
                let ident: String = trimmed
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                (!ident.is_empty()).then_some(ident)
            })
            .collect(),
    )
}

/// Find every `{type_name}::Ident` usage in `source`, returning the identifiers referenced.
fn find_variant_usages(source: &str, type_name: &str) -> Vec<String> {
    let needle = format!("{type_name}::");
    let mut usages = Vec::new();
    let mut rest = source;
    while let Some(idx) = rest.find(&needle) {
        let after = &rest[idx + needle.len()..];
        let ident: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !ident.is_empty() {
            usages.push(ident.clone());
        }
        rest = &after[ident.len()..];
    }
    usages
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rs_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Scan every crate under `workspace_root/contracts/*/src` for `KoraError::<Ident>`
/// usages that don't match a variant declared in `contracts/shared/src/errors.rs`.
/// Returns `(file, undefined_variant)` pairs.
fn check_kora_error(workspace_root: &Path) -> Vec<(PathBuf, String)> {
    let errors_path = workspace_root.join("contracts/shared/src/errors.rs");
    let errors_source = fs::read_to_string(&errors_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", errors_path.display()));
    let declared: BTreeSet<String> = extract_enum_variants(&errors_source, "KoraError")
        .unwrap_or_else(|| panic!("failed to parse `enum KoraError` in {}", errors_path.display()))
        .into_iter()
        .collect();

    let mut undefined = Vec::new();
    for crate_name in WORKSPACE_CRATES {
        let src_dir = workspace_root.join("contracts").join(crate_name).join("src");
        if !src_dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_rs_files(&src_dir, &mut files).expect("failed to walk src dir");
        for file in files {
            let source = fs::read_to_string(&file).unwrap_or_default();
            for variant in find_variant_usages(&source, "KoraError") {
                if !declared.contains(&variant) {
                    undefined.push((file.clone(), variant));
                }
            }
        }
    }
    undefined
}

fn locate_workspace_root() -> PathBuf {
    let cwd = std::env::current_dir().expect("failed to read current directory");
    cwd.ancestors()
        .find(|p| p.join("contracts").is_dir() && p.join("Cargo.toml").is_file())
        .unwrap_or_else(|| panic!(
            "could not locate workspace root (no ancestor of {} contains both contracts/ and Cargo.toml)",
            cwd.display()
        ))
        .to_path_buf()
}

fn main() -> ExitCode {
    let workspace_root = locate_workspace_root();
    let undefined = check_kora_error(&workspace_root);

    if undefined.is_empty() {
        println!("check-error-variants: OK — every KoraError::<Variant> reference resolves to a declared variant.");
        ExitCode::SUCCESS
    } else {
        eprintln!("check-error-variants: found references to undefined KoraError variants:\n");
        for (file, variant) in &undefined {
            eprintln!(
                "  {} references KoraError::{variant}, which is not declared in contracts/shared/src/errors.rs",
                file.display()
            );
        }
        eprintln!(
            "\nFix: add the missing variant(s) to `enum KoraError` in contracts/shared/src/errors.rs, \
             or correct the typo at the call site above."
        );
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_unit_variants() {
        let source = r#"
            pub enum KoraError {
                Unauthorized = 1,
                NotAdmin = 2, // trailing comment
                InvalidAmount = 14,
            }
        "#;
        let variants = extract_enum_variants(source, "KoraError").unwrap();
        assert_eq!(variants, vec!["Unauthorized", "NotAdmin", "InvalidAmount"]);
    }

    #[test]
    fn extracts_tuple_variants_alongside_unit_variants() {
        let source = r#"
            pub enum DataKey {
                Admin,
                Invoice(u64),
                Proposal(u64),
                CurrencyAllowlist(Symbol),
            }
        "#;
        let variants = extract_enum_variants(source, "DataKey").unwrap();
        assert_eq!(
            variants,
            vec!["Admin", "Invoice", "Proposal", "CurrencyAllowlist"]
        );
    }

    #[test]
    fn finds_all_usages_of_a_type() {
        let source = r#"
            fn f() -> Result<(), KoraError> {
                if true {
                    return Err(KoraError::NotAdmin);
                }
                Err(KoraError::InvalidAmount)
            }
        "#;
        let usages = find_variant_usages(source, "KoraError");
        assert_eq!(usages, vec!["NotAdmin", "InvalidAmount"]);
    }

    /// Regression fixture (issue #421 acceptance criteria): a deliberately-introduced
    /// reference to an undefined variant must be flagged.
    #[test]
    fn flags_a_deliberately_undefined_variant_fixture() {
        let errors_source = r#"
            pub enum KoraError {
                Foo = 1,
                Bar = 2,
            }
        "#;
        let declared: BTreeSet<String> = extract_enum_variants(errors_source, "KoraError")
            .unwrap()
            .into_iter()
            .collect();

        let usage_source = r#"
            fn f() -> Result<(), KoraError> {
                Err(KoraError::Baz)
            }
        "#;
        let undefined: Vec<_> = find_variant_usages(usage_source, "KoraError")
            .into_iter()
            .filter(|v| !declared.contains(v))
            .collect();

        assert_eq!(undefined, vec!["Baz".to_string()]);
    }

    #[test]
    fn does_not_flag_defined_variants() {
        let errors_source = r#"
            pub enum KoraError {
                Foo = 1,
                Bar = 2,
            }
        "#;
        let declared: BTreeSet<String> = extract_enum_variants(errors_source, "KoraError")
            .unwrap()
            .into_iter()
            .collect();

        let usage_source = r#"
            fn f() -> Result<(), KoraError> {
                Err(KoraError::Bar)
            }
        "#;
        let undefined: Vec<_> = find_variant_usages(usage_source, "KoraError")
            .into_iter()
            .filter(|v| !declared.contains(v))
            .collect();

        assert!(undefined.is_empty());
    }
}
