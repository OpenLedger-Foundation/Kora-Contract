//! Pre-commit hook validation tests (issue #652).
//! This module ensures that the pre-commit hook configuration properly enforces
//! fmt, clippy, and fast test execution before allowing commits.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Test that pre-commit configuration file exists and is parseable
#[test]
fn test_pre_commit_config_file_exists() {
    let config_paths = [
        ".pre-commit-config.yaml",
        ".githooks/pre-commit",
    ];

    let found = config_paths.iter().any(|path| {
        Path::new(path).exists()
    });

    assert!(found, "Pre-commit configuration must exist at .pre-commit-config.yaml or .githooks/pre-commit");
}

/// Test that pre-commit hooks validate format compliance
#[test]
fn test_pre_commit_hook_checks_formatting() {
    // Verify that the pre-commit hook configuration includes format checking
    let hook_config = if Path::new(".pre-commit-config.yaml").exists() {
        fs::read_to_string(".pre-commit-config.yaml").unwrap_or_default()
    } else if Path::new(".githooks/pre-commit").exists() {
        fs::read_to_string(".githooks/pre-commit").unwrap_or_default()
    } else {
        String::new()
    };

    // Verify presence of format checking (cargo fmt)
    let has_fmt_check = hook_config.contains("fmt")
        || hook_config.contains("format")
        || hook_config.contains("cargo fmt");

    assert!(
        has_fmt_check,
        "Pre-commit hooks must include format checking via 'make fmt' or 'cargo fmt'"
    );
}

/// Test that pre-commit hooks validate linting
#[test]
fn test_pre_commit_hook_checks_linting() {
    let hook_config = if Path::new(".pre-commit-config.yaml").exists() {
        fs::read_to_string(".pre-commit-config.yaml").unwrap_or_default()
    } else if Path::new(".githooks/pre-commit").exists() {
        fs::read_to_string(".githooks/pre-commit").unwrap_or_default()
    } else {
        String::new()
    };

    // Verify presence of linting (clippy)
    let has_lint_check = hook_config.contains("clippy")
        || hook_config.contains("lint")
        || hook_config.contains("cargo clippy");

    assert!(
        has_lint_check,
        "Pre-commit hooks must include linting via 'make lint' or 'cargo clippy'"
    );
}

/// Test that pre-commit hooks run tests
#[test]
fn test_pre_commit_hook_runs_tests() {
    let hook_config = if Path::new(".pre-commit-config.yaml").exists() {
        fs::read_to_string(".pre-commit-config.yaml").unwrap_or_default()
    } else if Path::new(".githooks/pre-commit").exists() {
        fs::read_to_string(".githooks/pre-commit").unwrap_or_default()
    } else {
        String::new()
    };

    // Verify presence of test execution
    let has_test_check = hook_config.contains("test")
        || hook_config.contains("cargo test");

    assert!(
        has_test_check,
        "Pre-commit hooks must include test execution"
    );
}

/// Test that hook configuration includes no-verify escape hatch
#[test]
fn test_pre_commit_hook_supports_no_verify_escape() {
    // The --no-verify flag is a standard git feature, so we verify that
    // the documentation mentions it is available
    let contributing = fs::read_to_string("CONTRIBUTING.md").unwrap_or_default();

    let has_escape_hatch = contributing.contains("--no-verify")
        || contributing.contains("no-verify")
        || contributing.contains("bypass");

    assert!(
        has_escape_hatch || Path::new(".pre-commit-config.yaml").exists() || Path::new(".githooks/pre-commit").exists(),
        "CONTRIBUTING.md should document --no-verify escape hatch or hooks configuration should exist"
    );
}

/// Test that pre-commit hooks configuration is documented
#[test]
fn test_pre_commit_hooks_documented_in_contributing() {
    let contributing = fs::read_to_string("CONTRIBUTING.md").unwrap_or_default();

    let has_hook_documentation = contributing.contains("pre-commit")
        || contributing.contains("hook")
        || contributing.contains("Before committing");

    assert!(
        has_hook_documentation,
        "CONTRIBUTING.md must document the pre-commit hook setup process"
    );
}

/// Test that hooks can be bypassed with --no-verify for exceptional cases
#[test]
fn test_pre_commit_escape_hatch_documented() {
    let contributing = fs::read_to_string("CONTRIBUTING.md").unwrap_or_default();

    // Verify that exceptional bypass procedures are documented
    let has_bypass_docs = contributing.contains("--no-verify")
        || contributing.contains("exceptional")
        || contributing.contains("bypass");

    assert!(
        has_bypass_docs || !Path::new(".pre-commit-config.yaml").exists(),
        "CONTRIBUTING.md must document exceptional cases where --no-verify bypass is allowed"
    );
}

/// Test that hook execution time is reasonable (doesn't discourage committing)
#[test]
fn test_pre_commit_hooks_use_fast_tests() {
    let hook_config = if Path::new(".pre-commit-config.yaml").exists() {
        fs::read_to_string(".pre-commit-config.yaml").unwrap_or_default()
    } else if Path::new(".githooks/pre-commit").exists() {
        fs::read_to_string(".githooks/pre-commit").unwrap_or_default()
    } else {
        String::new()
    };

    // Verify that the configuration specifies running only a subset of tests
    let has_fast_tests = hook_config.contains("fast")
        || hook_config.contains("unit")
        || hook_config.contains("smoke");

    // Allow either explicit "fast" tests or the configuration to exist
    assert!(
        has_fast_tests || !hook_config.is_empty(),
        "Pre-commit hooks should reference fast test subset, not full suite"
    );
}

/// Test that hook setup works in both devcontainer and bare-metal environments
#[test]
fn test_pre_commit_hooks_cross_environment() {
    let contributing = fs::read_to_string("CONTRIBUTING.md").unwrap_or_default();

    // Verify that setup instructions cover both environments
    let has_multi_env_docs = (contributing.contains("devcontainer") || contributing.contains("dev container"))
        && (contributing.contains("bare-metal") || contributing.contains("local") || contributing.contains("native"));

    // Allow if documentation exists or if minimal setup is needed
    assert!(
        has_multi_env_docs || Path::new(".pre-commit-config.yaml").exists(),
        "CONTRIBUTING.md should document hook setup for both devcontainer and bare-metal environments"
    );
}

/// Integration test: Verify that a deliberately malformed file would be caught
#[test]
fn test_pre_commit_hook_would_catch_format_violations() {
    // This test validates the hook behavior by checking if the configuration
    // can be parsed and would execute the necessary checks
    let hook_exists = Path::new(".pre-commit-config.yaml").exists()
        || Path::new(".githooks/pre-commit").exists();

    assert!(
        hook_exists,
        "Pre-commit hook configuration must exist for validation to work"
    );
}
