//! Automated Access Control Matrix Verification Tool
//!
//! This tool statically analyzes all contract source files and verifies that
//! every public entrypoint's authorization implementation matches the documented
//! `docs/ACCESS_MATRIX.md` specification.
//!
//! The markdown file is the single source of truth: the expected matrix is
//! parsed from its per-contract tables on every run (no hardcoded copy), so any
//! drift between docs and implementation fails the check.
//!
//! Exits with code 1 if any drift is detected (documented ≠ implemented) or if
//! any public entrypoint is missing from the matrix.
//!
//! Usage:
//!   cargo run -p kora-xtask --bin check_access_matrix
//!   cargo run -p kora-xtask --bin check_access_matrix -- --contract marketplace
//!   cargo run -p kora-xtask --bin check_access_matrix -- --verbose

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use syn::{File, Item, ImplItem, ImplItemFn};

type ContractName = String;
type FunctionName = String;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Authorization {
    Admin,
    AdminWithMultisig,
    Verifier,
    Feeder,
    SelfOwned, // Caller owns the resource (SME on their invoice, Investor on their position)
    ContractCaller(String), // Specific contract address (e.g., "marketplace")
    Governance,
    Permissionless,
    Conditional(Vec<Authorization>), // Multiple checks (e.g., Admin + NotPaused)
    Undocumented, // Not in ACCESS_MATRIX.md
}

#[derive(Debug)]
struct Drift {
    contract: ContractName,
    function: FunctionName,
    expected: Authorization,
    actual: Authorization,
}

/// One row parsed from an `ACCESS_MATRIX.md` per-contract table.
#[derive(Debug, Clone)]
struct MatrixEntry {
    contract: ContractName,
    function: FunctionName,
    auth: Authorization,
    /// 1-based markdown source line, used in parser error messages.
    line: usize,
}

// ── Markdown matrix parser ────────────────────────────────────────────────────

/// Contracts whose per-contract tables are parsed: (canonical snake_case name,
/// compact spaceless form). Role cells are matched in compact form because
/// markdown writes `**InvoiceNft Contract**`, which lowercases to
/// `invoicenft contract` (no underscores). Canonical order is used when
/// joining multi-contract `ContractCaller` roles (e.g. "financing_pool_or_marketplace").
const KNOWN_CONTRACTS: [(&str, &str); 11] = [
    ("financing_pool", "financingpool"),
    ("marketplace", "marketplace"),
    ("invoice_nft", "invoicenft"),
    ("treasury", "treasury"),
    ("access_control", "accesscontrol"),
    ("price_oracle", "priceoracle"),
    ("risk_registry", "riskregistry"),
    // Extended in #1037 to cover invoice_nft transition callers and new crates
    ("governance", "governance"),
    ("debtor_attestation", "debtorattestation"),
    ("dispute_escrow", "disputeescrow"),
    ("parameter_registry", "parameterregistry"),
];

/// Map a `Required Role` cell (plus its `Conditional Authorization` cell) to an
/// [`Authorization`] variant. `line` is the 1-based markdown source line used in
/// error messages for unrecognized roles.
fn map_role_cell(role_cell: &str, conditional_cell: &str, line: usize) -> Result<Authorization, String> {
    let role = role_cell.replace('*', "").replace('`', "");
    let role = role.trim().to_lowercase();
    let conditional = conditional_cell.to_lowercase();

    // The pending-admin handover (`accept_admin`) is authenticated by the
    // *proposed* address, not the stored admin — check before the admin rule.
    if role.contains("pending") {
        return Ok(Authorization::SelfOwned);
    }
    // Admin first: cells like "**Admin** (Marketplace)" still mean Admin, and a
    // multisig/quorum conditional promotes to AdminWithMultisig.
    if role.contains("admin") {
        if conditional.contains("multisig") || conditional.contains("quorum") {
            return Ok(Authorization::AdminWithMultisig);
        }
        return Ok(Authorization::Admin);
    }
    if role.contains("verifier") {
        return Ok(Authorization::Verifier);
    }
    if role.contains("feeder") {
        return Ok(Authorization::Feeder);
    }
    if role.contains("governance") {
        return Ok(Authorization::Governance);
    }
    // Contract-restricted callers, e.g. `**Marketplace Contract**` or
    // `**FinancingPool or Marketplace**` (joined in canonical order).
    // Matched on the compact (spaceless) role text so `InvoiceNft` also matches
    // `invoice_nft`, and only when the cell actually names a contract role.
    let compact: String = role.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let names_contract =
        role.contains("contract") || role.contains(" or ") || role.contains("caller");
    if names_contract {
        let mentioned: Vec<&str> = KNOWN_CONTRACTS
            .iter()
            .filter(|(_, compact_name)| compact.contains(compact_name))
            .map(|(canonical, _)| *canonical)
            .collect();
        if !mentioned.is_empty() {
            return Ok(Authorization::ContractCaller(mentioned.join("_or_")));
        }
    }
    if role.contains("permissionless") || role.contains("none (one-time)") || role == "none" {
        return Ok(Authorization::Permissionless);
    }
    // Caller-owned resources: SME / Investor / Challenger / Buyer / Seller /
    // Approver / Guardian / multisig proposer / voter / stakeholder / etc.
    const SELF_WORDS: [&str; 18] = [
        "self", "sme", "investor", "challenger", "buyer", "seller", "approver", "anyone",
        "pending", "payer", "proposer", "executor", "canceller", "voter", "submitter",
        "objector", "initiator", "stakeholder",
    ];
    if SELF_WORDS.iter().any(|w| role.contains(w)) {
        return Ok(Authorization::SelfOwned);
    }
    Err(format!(
        "line {line}: unrecognized Required Role `{role_cell}` — use one of Admin, \
         Verifier, Feeder, Governance, a Contract name, Self/SME/Investor/Caller role, \
         Permissionless, or None (one-time)"
    ))
}

/// Returns true for the `|---|---|` separator rows of markdown tables.
fn is_separator_row(cells: &[&str]) -> bool {
    !cells.is_empty()
        && cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':'))
}

/// Parse the access matrix from `ACCESS_MATRIX.md` markdown source.
///
/// Section headings look like `### 9. secondary_market`; every
/// `| Entrypoint | Required Role | Conditional Authorization | Notes |` row beneath
/// one becomes an entry. Malformed rows and unrecognized roles produce
/// line-numbered errors.
fn parse_access_matrix(markdown: &str) -> Result<Vec<MatrixEntry>, Vec<String>> {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut current_contract: Option<ContractName> = None;

    for (idx, raw_line) in markdown.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();

        // Section heading: `### 9. secondary_market`
        if line.starts_with("###") {
            let title = line.trim_start_matches('#').trim();
            // Split off the leading `N.` numbering when present.
            let name = title
                .split_once('.')
                .map(|(_, rest)| rest.trim())
                .unwrap_or(title)
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c| c == '`' || c == '*' || c == '_')
                .to_lowercase();
            if !name.is_empty() && name != "appendix" && name != "appendix:" {
                // Severity appendix subsections ("### Critical ...", etc.) carry
                // no entrypoint tables; keep the previous contract scope.
                if !["critical", "high", "medium", "low"].contains(&name.as_str()) {
                    current_contract = Some(name);
                }
            }
            continue;
        }

        if !line.starts_with('|') {
            continue;
        }

        let cells: Vec<&str> = line
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() < 4 || is_separator_row(&cells) {
            continue;
        }
        // Header row of each table.
        if cells[0].trim_matches(|c| c == '*' || c == '`' || c == '_').eq_ignore_ascii_case("entrypoint") {
            continue;
        }

        let Some(contract) = current_contract.clone() else {
            errors.push(format!("line {line_no}: table row outside any contract section"));
            continue;
        };

        let function = cells[0].replace('`', "").trim().to_string();
        if function.is_empty() || function.contains(char::is_whitespace) {
            errors.push(format!("line {line_no}: malformed entrypoint cell `{}`", cells[0]));
            continue;
        }
        match map_role_cell(cells[1], cells[2], line_no) {
            Ok(auth) => entries.push(MatrixEntry {
                contract,
                function,
                auth,
                line: line_no,
            }),
            Err(e) => errors.push(e),
        }
    }

    if errors.is_empty() {
        Ok(entries)
    } else {
        Err(errors)
    }
}

/// Locate the workspace root (the directory containing `docs/ACCESS_MATRIX.md`)
/// by walking up from the current working directory.
fn find_matrix_path() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("docs").join("ACCESS_MATRIX.md");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Load the expected matrix by parsing `docs/ACCESS_MATRIX.md`. This is the
/// single source of truth — no hardcoded copy exists anywhere in this tool.
fn load_access_matrix() -> Vec<MatrixEntry> {
    let path = find_matrix_path().unwrap_or_else(|| Path::new("docs").join("ACCESS_MATRIX.md"));
    let markdown = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("Failed to read access matrix at {}", path.display());
    });
    match parse_access_matrix(&markdown) {
        Ok(entries) => entries,
        Err(errors) => {
            eprintln!("❌ Failed to parse {}:", path.display());
            for e in &errors {
                eprintln!("  {e}");
            }
            std::process::exit(2);
        }
    }
}

// ── Authorization detection ──────────────────────────────────────────────────

/// ASCII identifier character (letters, digits, underscore).
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Identifier-boundary-aware substring search: `needle` must not be glued to
/// surrounding identifier characters. This keeps `new_admin.require_auth()`
/// from matching `admin.require_auth()`, and `require_admin_action` from
/// matching `require_admin`.
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

/// Render a function to text with `///` doc comments stripped, so prose like
/// "Requires `admin.require_auth()`" in docs never affects classification —
/// only the implemented checks count.
fn code_text(func: &ImplItemFn) -> String {
    let mut stripped = func.clone();
    stripped.attrs.retain(|attr| !attr.path().is_ident("doc"));
    quote::quote!(#stripped).to_string()
}

/// Collect the auth targets of every `X.require_auth()` call in `code`
/// (e.g. `["seller"]` for `seller.require_auth()`).
fn auth_targets(code: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = code;
    while let Some(pos) = rest.find(".require_auth()") {
        let before = &rest[..pos];
        let ident: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if !ident.is_empty() {
            targets.push(ident);
        }
        rest = &rest[pos + ".require_auth()".len()..];
    }
    targets
}

/// Parse a function's body to detect authorization patterns.
fn detect_authorization(func: &ImplItemFn) -> Authorization {
    let func_body = code_text(func);
    // Case-insensitive for the multisig family: code spells it
    // `load_multisig_config`, `MultisigConfig`, `multisig_threshold`, …
    let lowered = func_body.to_lowercase();

    // Admin-gated: `Self::require_admin(..)` or a direct `admin.require_auth()`
    // (word-boundary matched, so `new_admin`/`current_admin` don't count).
    if contains_word(&func_body, "require_admin") || contains_word(&func_body, "admin.require_auth()") {
        // `require_no_quorum_required` is matched via the `quorum` substring on
        // purpose: it marks multisig-gated admin entrypoints.
        if lowered.contains("multisig") || func_body.contains("quorum") {
            return Authorization::AdminWithMultisig;
        }
        return Authorization::Admin;
    }

    // Verifier-gated: `require_verifier` (also `_flag` / `_primary` variants),
    // `resolve_verifier` (the underlying gate `require_verifier` delegates to),
    // or an `is_verifier` status check.
    if func_body.contains("require_verifier")
        || func_body.contains("resolve_verifier")
        || contains_word(&func_body, "is_verifier")
    {
        return Authorization::Verifier;
    }

    if contains_word(&func_body, "require_feeder") {
        return Authorization::Feeder;
    }

    if contains_word(&func_body, "require_auth") {
        // Cross-contract allowlist: `require_authorized_caller(&env, &caller,
        // &[DataKey::Marketplace])`.
        if contains_word(&func_body, "require_authorized_caller") {
            if func_body.contains("Marketplace") {
                return Authorization::ContractCaller("marketplace".into());
            }
            if func_body.contains("FinancingPool") {
                return Authorization::ContractCaller("financing_pool".into());
            }
            if func_body.contains("InvoiceNft") {
                return Authorization::ContractCaller("invoice_nft".into());
            }
            return Authorization::ContractCaller("unknown".into());
        }
        // The authenticated parameter IS a contract address
        // (e.g. `marketplace.require_auth()` in `release_funds`).
        for target in auth_targets(&func_body) {
            if ["marketplace", "financing_pool", "invoice_nft", "treasury"].contains(&target.as_str()) {
                return Authorization::ContractCaller(target);
            }
        }
        // Stored-address contract checks (e.g. `Self::require_invoice_nft`).
        if contains_word(&func_body, "require_marketplace") {
            return Authorization::ContractCaller("marketplace".into());
        }
        if contains_word(&func_body, "require_financing_pool") {
            return Authorization::ContractCaller("financing_pool".into());
        }
        if contains_word(&func_body, "require_invoice_nft") {
            return Authorization::ContractCaller("invoice_nft".into());
        }
        // Multisig-signer / governance flows (propose / approve / execute).
        // `governance` is a plain substring on purpose: the gate is spelled
        // `require_governance` (underscore-glued), and every lowercase
        // occurrence in contract bodies marks a governance-gated entrypoint.
        if lowered.contains("multisig")
            || contains_word(&func_body, "require_signer")
            || contains_word(&func_body, "load_signer_config")
            || func_body.contains("governance")
        {
            return Authorization::Governance;
        }
        // Self-owned if caller signs (SME, Investor, Challenger, etc.)
        return Authorization::SelfOwned;
    }

    // No authorization detected → likely permissionless view
    Authorization::Permissionless
}

/// True when a module carries `#[cfg(test)]` (mocks and unit tests must not be
/// treated as contract entrypoints).
fn is_test_module(item: &Item) -> bool {
    if let Item::Mod(module) = item {
        module.attrs.iter().any(|attr| {
            attr.path().is_ident("cfg")
                && attr
                    .meta
                    .require_list()
                    .ok()
                    .map(|list| {
                        list.tokens
                            .to_string()
                            .replace([' ', '"', '\''], "")
                            .contains("test")
                    })
                    .unwrap_or(false)
        })
    } else {
        false
    }
}

/// Extract all public functions from a contract's impl block, skipping
/// `#[cfg(test)]` modules (mocks and unit tests are not entrypoints).
fn extract_public_functions(file: &File) -> Vec<(FunctionName, Authorization)> {
    let mut functions = Vec::new();

    for item in &file.items {
        if is_test_module(item) {
            continue;
        }
        if let Item::Impl(impl_block) = item {
            // Only process #[contractimpl] blocks
            let has_contractimpl = impl_block.attrs.iter().any(|attr| {
                attr.path().is_ident("contractimpl")
            });

            if !has_contractimpl {
                continue;
            }

            for impl_item in &impl_block.items {
                if let ImplItem::Fn(func) = impl_item {
                    // Only process public functions
                    let is_public = matches!(func.vis, syn::Visibility::Public(_));
                    if !is_public {
                        continue;
                    }

                    let func_name = func.sig.ident.to_string();
                    let auth = detect_authorization(func);
                    functions.push((func_name, auth));
                }
            }
        }
    }

    functions
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let filter_contract = args.iter().position(|a| a == "--contract").and_then(|i| args.get(i + 1));
    let verbose = args.contains(&"--verbose".to_string());

    println!("🔍 Access Control Matrix Verification");
    println!("======================================\n");

    let contracts_dir = Path::new("contracts");
    let entries = load_access_matrix();
    let access_matrix: HashMap<(ContractName, FunctionName), Authorization> = entries
        .iter()
        .map(|e| ((e.contract.clone(), e.function.clone()), e.auth.clone()))
        .collect();

    if verbose {
        println!(
            "📖 Parsed {} documented entrypoints from docs/ACCESS_MATRIX.md",
            entries.len()
        );
        for e in &entries {
            if let Some(filter) = filter_contract {
                if filter != &e.contract {
                    continue;
                }
            }
            println!(
                "  ⎯ {}::{} → {:?} (docs line {})",
                e.contract, e.function, e.auth, e.line
            );
        }
        if let Some(filter) = filter_contract {
            println!("🔎 Filtering to contract: {filter}");
        }
        println!();
    }

    let mut total_functions = 0;
    let mut drift_found: Vec<Drift> = Vec::new();
    let mut undocumented: Vec<(ContractName, FunctionName)> = Vec::new();

    // Scan each contract directory.
    // invoice_nft transition entrypoints (set_listed, set_created, set_funded,
    // set_repaid, set_defaulted) and their authorised callers (marketplace,
    // financing_pool, admin) are now included via the full invoice_nft entry and
    // the matching ACCESS_MATRIX.md rows so drift fails CI (#1037).
    let contract_dirs = [
        "access_control",
        "risk_registry",
        "invoice_nft",
        "marketplace",
        "financing_pool",
        "price_oracle",
        "dispute_resolution",
        "treasury",
        "secondary_market",
        "vesting",
        // The contracts below were previously omitted; adding them ensures that
        // any new entrypoints they gain are caught by drift detection (#1037).
        "governance",
        "governance_voting",
        "referral_rewards",
        "debtor_attestation",
        "dispute_escrow",
        "parameter_registry",
    ];

    for contract_name in &contract_dirs {
        if let Some(filter) = filter_contract {
            if filter != contract_name {
                continue;
            }
        }

        let lib_path = contracts_dir.join(contract_name).join("src/lib.rs");
        if !lib_path.exists() {
            eprintln!("⚠️  Contract {} not found at {:?}", contract_name, lib_path);
            continue;
        }

        if verbose {
            println!("📄 Analyzing {}...", contract_name);
        }

        let source = fs::read_to_string(&lib_path).expect("Failed to read contract source");
        let file = syn::parse_file(&source).expect("Failed to parse Rust source");

        let functions = extract_public_functions(&file);
        total_functions += functions.len();

        for (func_name, actual_auth) in functions {
            let key = (contract_name.to_string(), func_name.clone());

            match access_matrix.get(&key) {
                Some(expected_auth) => {
                    if expected_auth != &actual_auth {
                        drift_found.push(Drift {
                            contract: contract_name.to_string(),
                            function: func_name.clone(),
                            expected: expected_auth.clone(),
                            actual: actual_auth,
                        });
                    } else if verbose {
                        println!("  ✅ {}::{} — {:?}", contract_name, func_name, expected_auth);
                    }
                }
                None => {
                    undocumented.push((contract_name.to_string(), func_name.clone()));
                }
            }
        }
    }

    println!("\n📊 Verification Summary");
    println!("=======================");
    println!("Total functions analyzed: {}", total_functions);
    println!("Authorization drift detected: {}", drift_found.len());
    println!("Undocumented entrypoints: {}", undocumented.len());

    if !drift_found.is_empty() {
        println!("\n❌ AUTHORIZATION DRIFT DETECTED:");
        for drift in &drift_found {
            println!("\n  Contract: {}", drift.contract);
            println!("  Function: {}", drift.function);
            println!("  Expected: {:?}", drift.expected);
            println!("  Actual:   {:?}", drift.actual);
            println!("  → Action: Update docs/ACCESS_MATRIX.md or fix code authorization");
        }
    }

    if !undocumented.is_empty() {
        println!("\n⚠️  UNDOCUMENTED ENTRYPOINTS:");
        for (contract, func) in &undocumented {
            println!("  {}::{}", contract, func);
        }
        println!("\n→ Action: Add these functions to docs/ACCESS_MATRIX.md with authorization requirements");
    }

    if drift_found.is_empty() && undocumented.is_empty() {
        println!("\n✅ All authorization checks match ACCESS_MATRIX.md specification!");
        std::process::exit(0);
    } else {
        println!("\n❌ Verification failed. Fix drift/documentation before committing.");
        std::process::exit(1);
    }
}

// ── Parser unit tests (fixture markdown snippets, issue #999) ────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
## Access Matrix by Contract

### 1. secondary_market

| Entrypoint | Required Role | Conditional Authorization | Notes |
|------------|---------------|---------------------------|-------|
| `initialize` | None (one-time) | Must not already be initialized | Sets initial admin |
| `pause` | **Admin** | Direct invoker must equal stored admin | No require_auth |
| `set_fee_bps` | **Admin** | Multisig quorum if configured | Config change |
| `list_position` | **Seller (Self)** | Position owner in financing_pool | |
| `release_funds` | **Marketplace Contract** | Caller must be marketplace | Cross-contract |
| `set_listed` | **InvoiceNft Contract** | Caller must be registered invoice_nft address | Cross-contract |
| `deposit` | **FinancingPool or Marketplace** | Whitelisted contract | Revenue ingress |
| `open_dispute` | **Challenger (Anyone)** | Within window | |
| `set_price` | **Feeder** | Not paused | |
| `resolve_dispute` | **Governance** | Dispute open | Multi-sig or DAO |
| `register_sme` | **Verifier** | Active verifier | |
"#;

    #[test]
    fn parses_all_fixture_rows() {
        let entries = parse_access_matrix(FIXTURE).expect("fixture must parse");
        assert_eq!(entries.len(), 11);
        assert!(entries.iter().all(|e| e.contract == "secondary_market"));
    }

    #[test]
    fn maps_admin_and_multisig_roles() {
        let entries = parse_access_matrix(FIXTURE).expect("fixture must parse");
        let by_fn: HashMap<_, _> = entries.into_iter().map(|e| (e.function, e.auth)).collect();
        assert_eq!(by_fn["pause"], Authorization::Admin);
        // Multisig/quorum conditional promotes Admin → AdminWithMultisig.
        assert_eq!(by_fn["set_fee_bps"], Authorization::AdminWithMultisig);
    }

    #[test]
    fn maps_contract_caller_roles_including_or_join() {
        let entries = parse_access_matrix(FIXTURE).expect("fixture must parse");
        let by_fn: HashMap<_, _> = entries.into_iter().map(|e| (e.function, e.auth)).collect();
        assert_eq!(
            by_fn["release_funds"],
            Authorization::ContractCaller("marketplace".into())
        );
        // Spaceless `InvoiceNft` still maps to the canonical snake_case name.
        assert_eq!(
            by_fn["set_listed"],
            Authorization::ContractCaller("invoice_nft".into())
        );
        assert_eq!(
            by_fn["deposit"],
            Authorization::ContractCaller("financing_pool_or_marketplace".into())
        );
    }

    #[test]
    fn maps_self_verifier_feeder_governance_permissionless_roles() {
        let entries = parse_access_matrix(FIXTURE).expect("fixture must parse");
        let by_fn: HashMap<_, _> = entries.into_iter().map(|e| (e.function, e.auth)).collect();
        assert_eq!(by_fn["initialize"], Authorization::Permissionless);
        assert_eq!(by_fn["list_position"], Authorization::SelfOwned);
        assert_eq!(by_fn["open_dispute"], Authorization::SelfOwned);
        assert_eq!(by_fn["set_price"], Authorization::Feeder);
        assert_eq!(by_fn["resolve_dispute"], Authorization::Governance);
        assert_eq!(by_fn["register_sme"], Authorization::Verifier);
    }

    #[test]
    fn reports_line_numbers_for_unknown_roles() {
        let bad = "### 1. vesting\n\n| Entrypoint | Required Role | Conditional Authorization | Notes |\n|---|---|---|---|\n| `release` | **MysteryRole** | | |\n";
        let err = parse_access_matrix(bad).expect_err("unknown role must fail");
        assert_eq!(err.len(), 1);
        assert!(err[0].contains("line 5"), "got: {}", err[0]);
        assert!(err[0].contains("MysteryRole"), "got: {}", err[0]);
    }

    #[test]
    fn skips_separator_and_header_rows() {
        let md = "### 2. vesting\n\n| Entrypoint | Required Role | Conditional Authorization | Notes |\n|------------|---------------|---------------------------|-------|\n| `release` | Permissionless | Anyone may trigger | |\n";
        let entries = parse_access_matrix(md).expect("must parse");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].function, "release");
        assert_eq!(entries[0].auth, Authorization::Permissionless);
    }

    fn parse_fn(source: &str) -> ImplItemFn {
        let file: File = syn::parse_str(source).expect("test fn must parse");
        for item in &file.items {
            if let Item::Impl(block) = item {
                for impl_item in &block.items {
                    if let ImplItem::Fn(f) = impl_item {
                        return f.clone();
                    }
                }
            }
        }
        panic!("no fn found");
    }

    #[test]
    fn detection_ignores_doc_comments() {
        // The doc mentions `admin.require_auth()` but the body has none.
        let func = parse_fn(
            "#[contractimpl] impl C { /// Requires `admin.require_auth()` per docs. pub fn release(env: Env) -> Result<(), E> { Ok(()) } }",
        );
        assert_eq!(detect_authorization(&func), Authorization::Permissionless);
    }

    #[test]
    fn detection_distinguishes_new_admin_auth() {
        // `new_admin.require_auth()` must NOT count as `admin.require_auth()`.
        let accept = parse_fn(
            "#[contractimpl] impl C { pub fn accept_admin(env: Env, new_admin: Address) { new_admin.require_auth(); } }",
        );
        assert_eq!(detect_authorization(&accept), Authorization::SelfOwned);
        let gated = parse_fn(
            "#[contractimpl] impl C { pub fn set_x(env: Env, admin: Address) { admin.require_auth(); Self::require_admin(&env, &admin)?; } }",
        );
        assert_eq!(detect_authorization(&gated), Authorization::Admin);
    }

    #[test]
    fn detection_classifies_verifier_helpers() {
        // `resolve_verifier` is the gate `require_verifier` delegates to.
        let register = parse_fn(
            "#[contractimpl] impl C { pub fn register_sme(env: Env, verifier: Address) { verifier.require_auth(); let primary = Self::resolve_verifier(&env, &verifier)?; } }",
        );
        assert_eq!(detect_authorization(&register), Authorization::Verifier);
        // `require_verifier_primary` is a verifier gate, not an admin gate.
        let sub = parse_fn(
            "#[contractimpl] impl C { pub fn add_sub_account(env: Env, primary: Address) { primary.require_auth(); Self::require_verifier_primary(&env, &primary)?; } }",
        );
        assert_eq!(detect_authorization(&sub), Authorization::Verifier);
    }

    #[test]
    fn detection_classifies_contract_callers_and_governance() {
        let funds = parse_fn(
            "#[contractimpl] impl C { pub fn release_funds(env: Env, marketplace: Address) { marketplace.require_auth(); } }",
        );
        assert_eq!(
            detect_authorization(&funds),
            Authorization::ContractCaller("marketplace".into())
        );
        let allowed = parse_fn(
            "#[contractimpl] impl C { pub fn set_funded(env: Env, caller: Address) { caller.require_auth(); Self::require_authorized_caller(&env, &caller, &[DataKey::FinancingPool])?; } }",
        );
        assert_eq!(
            detect_authorization(&allowed),
            Authorization::ContractCaller("financing_pool".into())
        );
        let multisig = parse_fn(
            "#[contractimpl] impl C { pub fn approve_action(env: Env, approver: Address) { approver.require_auth(); Self::load_signer_config(&env)?; } }",
        );
        assert_eq!(detect_authorization(&multisig), Authorization::Governance);
    }

    #[test]
    fn skips_cfg_test_modules() {
        let file: File = syn::parse_str(
            "#[contractimpl] impl Real { pub fn real_fn(env: Env) {} } #[cfg(test)] mod tests { #[contractimpl] impl Mock { pub fn mock_fn(env: Env) {} } }",
        )
        .expect("must parse");
        let fns = extract_public_functions(&file);
        assert_eq!(fns.len(), 1);
        assert_eq!(fns[0].0, "real_fn");
    }
}
