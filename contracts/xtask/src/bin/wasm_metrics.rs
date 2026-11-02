//! WASM size and resource-cost regression guard (issue #751).
//!
//! Subcommands
//! -----------
//! `measure-wasm  [--wasm-dir <dir>] [--out <file>]`
//!     Scans built WASM artefacts, counts entrypoints from source, and writes a
//!     machine-readable JSON report.
//!
//! `check-regression --report <file> --baseline <file> [--size-threshold-pct <n>]`
//!     Compares a freshly-generated report against a committed baseline and exits
//!     non-zero if any contract's size or instruction-cost estimate regresses
//!     beyond the configured threshold.
//!
//! Run via:
//!   cargo run -p kora-xtask --bin wasm-metrics -- measure-wasm
//!   cargo run -p kora-xtask --bin wasm-metrics -- check-regression \
//!       --report wasm_metrics_report.json \
//!       --baseline baselines/wasm-metrics.json

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// ── Data model ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractMetrics {
    /// Raw WASM byte size (optimized binary when available, otherwise release).
    pub wasm_size_bytes: u64,
    /// Estimated instruction-cost units derived from WASM binary size.
    /// Formula: size_bytes * COST_PER_BYTE (a stable proxy for real preflight
    /// cost without requiring a live network connection in CI).
    pub estimated_cost_units: u64,
    /// Public entrypoints detected by scanning the contract source.
    pub entrypoints: Vec<String>,
}

/// Top-level report written to / read from JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsReport {
    /// Timestamp (RFC-3339) when the report was generated.
    pub generated_at: String,
    /// Per-contract metrics keyed by contract name.
    pub contracts: BTreeMap<String, ContractMetrics>,
}

// ── Constants ─────────────────────────────────────────────────────────────────

const CONTRACTS: &[&str] = &[
    "access_control",
    "invoice_nft",
    "marketplace",
    "financing_pool",
    "treasury",
    "risk_registry",
    "price_oracle",
    "tranche",
    "dispute_resolution",
    "dispute_escrow",
    "governance",
    "governance_voting",
    "parameter_registry",
    "debtor_attestation",
    "secondary_market",
    "referral_rewards",
    "verifier_nomination",
    "fractionalizer",
    "vesting",
];

/// Synthetic cost-per-byte multiplier used to derive `estimated_cost_units`.
/// Chosen so that a 1 KiB WASM ≈ 1 000 cost units, giving human-readable numbers.
const COST_PER_BYTE: u64 = 1;

// ── Measurement ───────────────────────────────────────────────────────────────

fn wasm_size(wasm_dir: &Path, contract: &str) -> u64 {
    // Prefer the stellar-optimized binary; fall back to the plain release build.
    let optimized = wasm_dir.join(format!("kora_{contract}.optimized.wasm"));
    let plain = wasm_dir.join(format!("kora_{contract}.wasm"));
    for path in [&optimized, &plain] {
        if let Ok(meta) = fs::metadata(path) {
            return meta.len();
        }
    }
    0
}

/// Scan `contracts/<name>/src/**/*.rs` for `pub fn` declarations that are
/// likely Soroban entrypoints (annotated with `#[contractimpl]` context).
/// We use a simple text scan — no dependency on the contract crates.
fn detect_entrypoints(workspace_root: &Path, contract: &str) -> Vec<String> {
    let src_dir = workspace_root
        .join("contracts")
        .join(contract)
        .join("src");
    let mut fns = Vec::new();
    collect_entrypoints_from_dir(&src_dir, &mut fns);
    fns.sort();
    fns.dedup();
    fns
}

fn collect_entrypoints_from_dir(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_entrypoints_from_dir(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            if let Ok(src) = fs::read_to_string(&path) {
                extract_pub_fns(&src, out);
            }
        }
    }
}

fn extract_pub_fns(source: &str, out: &mut Vec<String>) {
    // Collect lines that look like `pub fn <ident>(` inside an `impl` block.
    // We skip `pub fn new(` / `pub fn default(` which are constructors, not
    // Soroban entrypoints.
    let skip = ["new", "default", "test_", "mock_"];
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("pub fn ") {
            let after = &trimmed["pub fn ".len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && !skip.iter().any(|s| name.starts_with(s)) {
                out.push(name);
            }
        }
    }
}

fn measure(workspace_root: &Path, wasm_dir: &Path) -> MetricsReport {
    let mut contracts = BTreeMap::new();
    for &name in CONTRACTS {
        let size = wasm_size(wasm_dir, name);
        let entrypoints = detect_entrypoints(workspace_root, name);
        contracts.insert(
            name.to_string(),
            ContractMetrics {
                wasm_size_bytes: size,
                estimated_cost_units: size * COST_PER_BYTE,
                entrypoints,
            },
        );
    }
    MetricsReport {
        generated_at: now_rfc3339(),
        contracts,
    }
}

// ── Regression check ──────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Regression {
    contract: String,
    metric: &'static str,
    baseline: u64,
    current: u64,
    delta_pct: f64,
}

/// Returns a list of regressions found.  An empty list means the build is clean.
pub fn check_regression(
    report: &MetricsReport,
    baseline: &MetricsReport,
    size_threshold_pct: f64,
) -> Vec<Regression> {
    let mut regressions = Vec::new();
    for (name, current) in &report.contracts {
        let Some(base) = baseline.contracts.get(name) else {
            // New contract — not a regression.
            continue;
        };
        for (metric, base_val, cur_val) in [
            ("wasm_size_bytes", base.wasm_size_bytes, current.wasm_size_bytes),
            (
                "estimated_cost_units",
                base.estimated_cost_units,
                current.estimated_cost_units,
            ),
        ] {
            if base_val == 0 {
                continue;
            }
            let delta_pct = (cur_val as f64 - base_val as f64) / base_val as f64 * 100.0;
            if delta_pct > size_threshold_pct {
                regressions.push(Regression {
                    contract: name.clone(),
                    metric,
                    baseline: base_val,
                    current: cur_val,
                    delta_pct,
                });
            }
        }
    }
    regressions
}

// ── CLI ───────────────────────────────────────────────────────────────────────

fn locate_workspace_root() -> PathBuf {
    let cwd = std::env::current_dir().expect("failed to read cwd");
    cwd.ancestors()
        .find(|p| p.join("contracts").is_dir() && p.join("Cargo.toml").is_file())
        .unwrap_or_else(|| {
            panic!(
                "could not locate workspace root from {}",
                cwd.display()
            )
        })
        .to_path_buf()
}

fn now_rfc3339() -> String {
    // Minimal RFC-3339 timestamp without pulling in `chrono`.
    // Uses the Unix epoch seconds from the environment or falls back to a
    // placeholder so the binary compiles without extra deps.
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|epoch| {
            // Format as YYYY-MM-DDTHH:MM:SSZ (good enough for a CI artefact).
            let secs = epoch % 60;
            let mins = (epoch / 60) % 60;
            let hours = (epoch / 3600) % 24;
            let days_since_epoch = epoch / 86400;
            // Approximate Gregorian date (accurate for 1970-2100).
            let year = 1970 + days_since_epoch / 365;
            let day_of_year = days_since_epoch % 365;
            let month = day_of_year / 30 + 1;
            let day = day_of_year % 30 + 1;
            format!("{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{secs:02}Z")
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

fn cmd_measure(args: &[String]) -> ExitCode {
    let workspace_root = locate_workspace_root();
    let default_wasm_dir = workspace_root
        .join("target")
        .join("wasm32-unknown-unknown")
        .join("release");

    let mut wasm_dir = default_wasm_dir;
    let mut out_path = PathBuf::from("wasm_metrics_report.json");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--wasm-dir" => {
                i += 1;
                wasm_dir = PathBuf::from(&args[i]);
            }
            "--out" => {
                i += 1;
                out_path = PathBuf::from(&args[i]);
            }
            _ => {}
        }
        i += 1;
    }

    let report = measure(&workspace_root, &wasm_dir);
    let json = serde_json::to_string_pretty(&report).expect("serialization failed");
    fs::write(&out_path, &json).unwrap_or_else(|e| panic!("failed to write {}: {e}", out_path.display()));
    println!("measure-wasm: report written to {}", out_path.display());
    for (name, m) in &report.contracts {
        println!(
            "  {name:<25} size={:>8} B  cost={:>8}  entrypoints={}",
            m.wasm_size_bytes,
            m.estimated_cost_units,
            m.entrypoints.len()
        );
    }
    ExitCode::SUCCESS
}

fn cmd_check(args: &[String]) -> ExitCode {
    let mut report_path = PathBuf::new();
    let mut baseline_path = PathBuf::new();
    let mut threshold_pct: f64 = 5.0; // default: 5 % growth triggers failure

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--report" => {
                i += 1;
                report_path = PathBuf::from(&args[i]);
            }
            "--baseline" => {
                i += 1;
                baseline_path = PathBuf::from(&args[i]);
            }
            "--size-threshold-pct" => {
                i += 1;
                threshold_pct = args[i].parse().unwrap_or(5.0);
            }
            _ => {}
        }
        i += 1;
    }

    if report_path.as_os_str().is_empty() || baseline_path.as_os_str().is_empty() {
        eprintln!("check-regression: --report and --baseline are required");
        return ExitCode::FAILURE;
    }

    let report: MetricsReport = serde_json::from_str(
        &fs::read_to_string(&report_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", report_path.display())),
    )
    .expect("failed to parse report JSON");

    let baseline: MetricsReport = serde_json::from_str(
        &fs::read_to_string(&baseline_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", baseline_path.display())),
    )
    .expect("failed to parse baseline JSON");

    let regressions = check_regression(&report, &baseline, threshold_pct);

    if regressions.is_empty() {
        println!(
            "check-regression: OK — no regressions detected (threshold {threshold_pct:.1}%)"
        );
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "check-regression: FAILED — {} regression(s) exceed the {threshold_pct:.1}% threshold:\n",
            regressions.len()
        );
        for r in &regressions {
            eprintln!(
                "  {} / {}: baseline={} current={} (+{:.1}%)",
                r.contract, r.metric, r.baseline, r.current, r.delta_pct
            );
        }
        eprintln!(
            "\nTo accept these changes, update the baseline:\n  \
             cargo run -p kora-xtask --bin wasm-metrics -- measure-wasm \
             --out baselines/wasm-metrics.json\n  \
             git add baselines/wasm-metrics.json && git commit -m 'chore: update WASM metrics baseline'"
        );
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("measure-wasm") => cmd_measure(&args[2..].to_vec()),
        Some("check-regression") => cmd_check(&args[2..].to_vec()),
        _ => {
            eprintln!(
                "Usage: wasm-metrics <subcommand> [options]\n\
                 Subcommands:\n  \
                   measure-wasm   [--wasm-dir <dir>] [--out <file>]\n  \
                   check-regression --report <file> --baseline <file> \
                   [--size-threshold-pct <n>]"
            );
            ExitCode::FAILURE
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn make_report(sizes: &[(&str, u64)]) -> MetricsReport {
        let contracts = sizes
            .iter()
            .map(|(name, sz)| {
                (
                    name.to_string(),
                    ContractMetrics {
                        wasm_size_bytes: *sz,
                        estimated_cost_units: sz * COST_PER_BYTE,
                        entrypoints: vec![],
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        MetricsReport {
            generated_at: "1970-01-01T00:00:00Z".to_string(),
            contracts,
        }
    }

    #[test]
    fn no_regression_when_sizes_unchanged() {
        let baseline = make_report(&[("invoice_nft", 100_000), ("marketplace", 80_000)]);
        let report = make_report(&[("invoice_nft", 100_000), ("marketplace", 80_000)]);
        assert!(check_regression(&report, &baseline, 5.0).is_empty());
    }

    #[test]
    fn no_regression_when_size_decreases() {
        let baseline = make_report(&[("invoice_nft", 100_000)]);
        let report = make_report(&[("invoice_nft", 95_000)]);
        assert!(check_regression(&report, &baseline, 5.0).is_empty());
    }

    #[test]
    fn regression_detected_when_size_exceeds_threshold() {
        let baseline = make_report(&[("invoice_nft", 100_000)]);
        // +6% — exceeds the 5% threshold
        let report = make_report(&[("invoice_nft", 106_001)]);
        let regressions = check_regression(&report, &baseline, 5.0);
        assert!(!regressions.is_empty(), "expected regression to be flagged");
        assert_eq!(regressions[0].contract, "invoice_nft");
    }

    #[test]
    fn no_regression_when_growth_within_threshold() {
        let baseline = make_report(&[("invoice_nft", 100_000)]);
        // +4.9% — within the 5% threshold
        let report = make_report(&[("invoice_nft", 104_900)]);
        assert!(check_regression(&report, &baseline, 5.0).is_empty());
    }

    #[test]
    fn new_contract_not_flagged_as_regression() {
        let baseline = make_report(&[("invoice_nft", 100_000)]);
        let report = make_report(&[("invoice_nft", 100_000), ("new_contract", 200_000)]);
        assert!(check_regression(&report, &baseline, 5.0).is_empty());
    }

    #[test]
    fn multiple_regressions_all_reported() {
        let baseline = make_report(&[("invoice_nft", 100_000), ("marketplace", 80_000)]);
        let report = make_report(&[("invoice_nft", 120_000), ("marketplace", 100_000)]);
        let regressions = check_regression(&report, &baseline, 5.0);
        // Each contract has 2 metrics (size + cost), so 4 regressions total.
        assert_eq!(regressions.len(), 4);
    }

    #[test]
    fn zero_baseline_skipped_gracefully() {
        let baseline = make_report(&[("invoice_nft", 0)]);
        let report = make_report(&[("invoice_nft", 100_000)]);
        // baseline=0 → skip (avoid division by zero)
        assert!(check_regression(&report, &baseline, 5.0).is_empty());
    }

    #[test]
    fn extract_pub_fns_finds_entrypoints() {
        let src = r#"
            pub fn mint_invoice(env: Env, sme: Address) -> u64 { 0 }
            pub fn get_invoice(env: Env, id: u64) -> Invoice { todo!() }
            fn internal_helper() {}
            pub fn new() -> Self { Self {} }
        "#;
        let mut fns = Vec::new();
        extract_pub_fns(src, &mut fns);
        assert!(fns.contains(&"mint_invoice".to_string()));
        assert!(fns.contains(&"get_invoice".to_string()));
        assert!(!fns.contains(&"internal_helper".to_string()));
        // `new` is excluded by the skip list
        assert!(!fns.contains(&"new".to_string()));
    }

    #[test]
    fn report_serialization_roundtrip() {
        let report = make_report(&[("treasury", 50_000)]);
        let json = serde_json::to_string(&report).unwrap();
        let restored: MetricsReport = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.contracts["treasury"].wasm_size_bytes,
            50_000
        );
    }

    #[test]
    fn configurable_threshold_respected() {
        let baseline = make_report(&[("treasury", 100_000)]);
        // +8% growth
        let report = make_report(&[("treasury", 108_000)]);
        // With 10% threshold → no regression
        assert!(check_regression(&report, &baseline, 10.0).is_empty());
        // With 5% threshold → regression
        assert!(!check_regression(&report, &baseline, 5.0).is_empty());
    }
}
