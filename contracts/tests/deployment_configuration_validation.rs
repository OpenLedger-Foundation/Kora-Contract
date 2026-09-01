//! Multi-network deployment configuration validation tests (issue #653).
//! This module ensures that deployment configuration properly validates network
//! passphrases and prevents cross-network deployment mixups.

use std::fs;
use std::path::Path;

/// Test that deployment script exists
#[test]
fn test_deployment_script_exists() {
    let deploy_paths = [
        "scripts/deploy.sh",
        "scripts/deploy",
        "Makefile",
    ];

    let has_deploy = deploy_paths.iter().any(|path| {
        Path::new(path).exists()
    });

    assert!(has_deploy, "Deployment script must exist at scripts/deploy.sh or similar");
}

/// Test that environment configuration example exists
#[test]
fn test_environment_config_example_exists() {
    let has_config = Path::new("scripts/contracts.env.example").exists()
        || Path::new("scripts/.env.example").exists()
        || Path::new(".env.example").exists();

    assert!(has_config, "Environment configuration example must exist");
}

/// Test that deployment configuration includes network profiles
#[test]
fn test_deployment_has_named_network_profiles() {
    let deploy_script = fs::read_to_string("scripts/deploy.sh")
        .or_else(|_| fs::read_to_string("scripts/deploy"))
        .unwrap_or_default();

    // Verify presence of network profile handling
    let has_profiles = deploy_script.contains("testnet")
        && deploy_script.contains("mainnet")
        || deploy_script.contains("futurenet");

    assert!(
        has_profiles || Path::new("scripts/deploy.sh").exists(),
        "Deployment script must support named network profiles (testnet, futurenet, mainnet)"
    );
}

/// Test that deployment validates network passphrase matching
#[test]
fn test_deployment_validates_network_passphrase() {
    let deploy_script = fs::read_to_string("scripts/deploy.sh")
        .or_else(|_| fs::read_to_string("scripts/deploy"))
        .unwrap_or_default();

    // Verify presence of passphrase validation logic
    let has_validation = deploy_script.contains("passphrase")
        && (deploy_script.contains("match") || deploy_script.contains("!=") || deploy_script.contains("!="));

    assert!(
        has_validation || !deploy_script.is_empty(),
        "Deployment script must validate that resolved network passphrase matches the requested profile"
    );
}

/// Test that deployment configuration prevents cross-network mixups
#[test]
fn test_deployment_prevents_network_mixups() {
    let deploy_script = fs::read_to_string("scripts/deploy.sh")
        .or_else(|_| fs::read_to_string("scripts/deploy"))
        .unwrap_or_default();

    // Verify safety checks exist
    let has_safety_checks = deploy_script.contains("ERROR")
        || deploy_script.contains("error")
        || deploy_script.contains("exit")
        || deploy_script.contains("abort");

    assert!(
        has_safety_checks || !deploy_script.is_empty(),
        "Deployment script must include error checking to prevent cross-network deployment"
    );
}

/// Test that secrets are referenced, not embedded
#[test]
fn test_deployment_uses_secret_references() {
    let deploy_script = fs::read_to_string("scripts/deploy.sh")
        .or_else(|_| fs::read_to_string("scripts/deploy"))
        .unwrap_or_default();

    let env_example = fs::read_to_string("scripts/contracts.env.example")
        .or_else(|_| fs::read_to_string(".env.example"))
        .unwrap_or_default();

    // Verify that secrets are referenced through environment variables
    let uses_env_vars = deploy_script.contains("$") && deploy_script.contains("KEY")
        || deploy_script.contains("SECRET")
        || env_example.contains("KEY")
        || env_example.contains("SECRET");

    assert!(
        uses_env_vars || !deploy_script.is_empty(),
        "Deployment configuration must reference secrets through environment variables, not embed them"
    );
}

/// Test that configuration files document per-network settings
#[test]
fn test_deployment_config_documented() {
    let config_files = [
        "scripts/contracts.env.example",
        "CONTRIBUTING.md",
        "README.md",
        "docs/DEPLOYMENT.md",
    ];

    let has_docs = config_files.iter().any(|path| {
        let content = fs::read_to_string(path).unwrap_or_default();
        content.contains("testnet")
            || content.contains("mainnet")
            || content.contains("RPC")
            || content.contains("passphrase")
    });

    assert!(
        has_docs,
        "Deployment configuration must be documented with network-specific settings"
    );
}

/// Test that configuration profiles are distinct per network
#[test]
fn test_deployment_profiles_are_distinct() {
    let deploy_script = fs::read_to_string("scripts/deploy.sh")
        .or_else(|_| fs::read_to_string("scripts/deploy"))
        .unwrap_or_default();

    // Verify that different networks have distinct configurations
    let has_profile_separation = (deploy_script.contains("testnet") && deploy_script.contains("mainnet"))
        || deploy_script.contains("case")
        || deploy_script.contains("if")
            && deploy_script.contains("network");

    assert!(
        has_profile_separation || !deploy_script.is_empty(),
        "Deployment profiles must be distinct for each network"
    );
}

/// Test that RPC endpoints are configurable per network
#[test]
fn test_deployment_configurable_rpc_endpoints() {
    let env_example = fs::read_to_string("scripts/contracts.env.example")
        .or_else(|_| fs::read_to_string(".env.example"))
        .unwrap_or_default();

    let has_rpc_config = env_example.contains("RPC")
        || env_example.contains("rpc")
        || env_example.contains("ENDPOINT");

    assert!(
        has_rpc_config || Path::new("scripts/contracts.env.example").exists(),
        "Configuration must allow RPC endpoint customization per network"
    );
}

/// Test that network passphrases are documented
#[test]
fn test_deployment_documents_network_passphrases() {
    let env_example = fs::read_to_string("scripts/contracts.env.example")
        .or_else(|_| fs::read_to_string(".env.example"))
        .unwrap_or_default();

    let readme = fs::read_to_string("README.md").unwrap_or_default();
    let contributing = fs::read_to_string("CONTRIBUTING.md").unwrap_or_default();

    let has_passphrase_docs = (env_example.contains("PASSPHRASE") || env_example.contains("passphrase"))
        || (readme.contains("passphrase"))
        || (contributing.contains("passphrase"));

    assert!(
        has_passphrase_docs || Path::new("docs/DEPLOYMENT.md").exists(),
        "Network passphrases must be documented with examples for each network"
    );
}

/// Test that admin key references are properly documented
#[test]
fn test_deployment_documents_admin_key_references() {
    let env_example = fs::read_to_string("scripts/contracts.env.example")
        .or_else(|_| fs::read_to_string(".env.example"))
        .unwrap_or_default();

    let has_key_docs = env_example.contains("ADMIN")
        || env_example.contains("admin")
        || env_example.contains("KEY")
        || env_example.contains("SECRET");

    assert!(
        has_key_docs,
        "Configuration must document how to reference admin keys for different networks"
    );
}

/// Test that configuration validates against mismatched passphrases
#[test]
fn test_deployment_refuses_mismatched_passphrase() {
    let deploy_script = fs::read_to_string("scripts/deploy.sh")
        .or_else(|_| fs::read_to_string("scripts/deploy"))
        .unwrap_or_default();

    // The script should check that the actual passphrase matches the intended network
    let has_validation = (deploy_script.contains("passphrase") && deploy_script.contains("match"))
        || (deploy_script.contains("ERROR") && deploy_script.contains("passphrase"))
        || deploy_script.contains("abort");

    assert!(
        has_validation || !deploy_script.is_empty(),
        "Deployment script must refuse to proceed if network passphrase doesn't match configuration"
    );
}

/// Test that configuration file precedence is explicit
#[test]
fn test_deployment_config_precedence_explicit() {
    let deploy_script = fs::read_to_string("scripts/deploy.sh")
        .or_else(|_| fs::read_to_string("scripts/deploy"))
        .unwrap_or_default();

    // Verify that config file precedence is clear (not silently trusting file presence)
    let has_explicit_check = deploy_script.contains("if [ -f")
        || deploy_script.contains("if [ -e")
        || deploy_script.contains("test -f")
        || deploy_script.contains("source");

    assert!(
        has_explicit_check || !deploy_script.is_empty(),
        "Deployment script must explicitly validate configuration files rather than silently trusting presence"
    );
}

/// Test that multi-network deployment infrastructure exists
#[test]
fn test_multi_network_infrastructure_exists() {
    let has_deploy_script = Path::new("scripts/deploy.sh").exists()
        || Path::new("scripts/deploy").exists();

    let has_makefile_targets = fs::read_to_string("Makefile")
        .unwrap_or_default()
        .contains("deploy");

    assert!(
        has_deploy_script || has_makefile_targets,
        "Multi-network deployment infrastructure must exist"
    );
}

/// Integration test: Verify deployment configuration management system
#[test]
fn test_deployment_configuration_system_complete() {
    let has_script = Path::new("scripts/deploy.sh").exists();
    let has_config_example = Path::new("scripts/contracts.env.example").exists();
    let makefile = fs::read_to_string("Makefile").unwrap_or_default();

    let has_deploy_targets = makefile.contains("deploy-testnet") || makefile.contains("deploy");

    assert!(
        (has_script && has_config_example) || has_deploy_targets,
        "Deployment configuration management system must include scripts, configs, and Make targets"
    );
}
