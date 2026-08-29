//! Upgrade Coordinator (issue #574)
//!
//! A standalone CLI tool that orchestrates contract upgrades across the Kora
//! protocol.  It discovers compiled WASM artefacts, computes their SHA-256
//! hashes, and maintains a JSON staging file that records which contracts are
//! scheduled for upgrade along with the timelock timestamps.
//!
//! Usage:
//!   cargo run -p kora-xtask --bin upgrade-coordinator -- <COMMAND> [ARGS]
//!
//! Commands:
//!   stage   <contract> <wasm_path>   Stage an upgrade proposal (24 h timelock)
//!   list                               List all staged proposals
//!   commit <contract>                  Commit a staged proposal (after timelock)
//!   cancel <contract>                  Cancel a staged proposal
//!
//! The staging file is written to `.kilo/upgrade-proposals.json` by default.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

const TIMELOCK_DELAY_SECS: u64 = 86_400; // 24 hours
const CONTRACTS_DIR: &str = "contracts";
const STAGING_FILE: &str = ".kilo/upgrade-proposals.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Proposal {
    contract: String,
    wasm_path: PathBuf,
    wasm_hash: String,
    proposed_at: u64,
    committed: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct State {
    proposals: BTreeMap<String, Proposal>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_secs()
}

fn sha256_file(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = fs::File::open(path).expect("failed to open WASM file");
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).expect("failed to read WASM file");
    let mut hasher = Sha256::new();
    hasher.update(&buf);
    let result = hasher.finalize();
    format!("{:x}", result)
}

fn discover_wasm(contract_name: &str) -> Option<PathBuf> {
    let contracts_root = Path::new(CONTRACTS_DIR);
    let pattern = format!("{contract_name}-*/target/soroban-*/release/{contract_name}.wasm");
    let crate_dir = contracts_root.join(contract_name);
    if !crate_dir.is_dir() {
        return None;
    }
    for entry in walkdir::WalkDir::new(&crate_dir) {
        if let Ok(entry) = entry {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "wasm") {
                let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if name == contract_name || name.contains(&format!("{contract_name}.")) {
                    return Some(path.to_path_buf());
                }
            }
        }
    }
    None
}

fn load_state() -> State {
    let path = Path::new(STAGING_FILE);
    if path.exists() {
        let content = fs::read_to_string(path).expect("failed to read staging file");
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        State::default()
    }
}

fn save_state(state: &State) {
    let path = Path::new(STAGING_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("failed to create staging directory");
    }
    let content = serde_json::to_string_pretty(state).expect("failed to serialize state");
    fs::write(path, content).expect("failed to write staging file");
}

fn usage() {
    eprintln!("Upgrade Coordinator — orchestrates Kora contract upgrades");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  stage   <contract> [wasm_path]   Stage an upgrade (auto-discovers WASM if path omitted)");
    eprintln!("  list                                    List all staged proposals");
    eprintln!("  commit <contract>                       Commit a staged proposal (after timelock)");
    eprintln!("  cancel <contract>                       Cancel a staged proposal");
    eprintln!();
    eprintln!("The timelock is {} seconds (24 h).", TIMELOCK_DELAY_SECS);
}

fn cmd_stage(args: &[String]) {
    if args.is_empty() {
        eprintln!("error: missing <contract>");
        usage();
        std::process::exit(1);
    }

    let contract = &args[0];
    let wasm_path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else if let Some(found) = discover_wasm(contract) {
        found
    } else {
        eprintln!("error: could not auto-discover WASM for '{}'", contract);
        eprintln!("hint: build the contract first, then re-run without the wasm_path argument");
        eprintln!("      or pass the path explicitly: stage {} <wasm_path>", contract);
        std::process::exit(1);
    };

    if !wasm_path.exists() {
        eprintln!("error: WASM file not found: {}", wasm_path.display());
        std::process::exit(1);
    }

    let hash = sha256_file(&wasm_path);
    let now = now_secs();

    let mut state = load_state();
    let existing = state.proposals.get(contract);
    let proposed_at = existing.map(|p| p.proposed_at).unwrap_or(now);

    let proposal = Proposal {
        contract: contract.to_string(),
        wasm_path: wasm_path.clone(),
        wasm_hash: hash.clone(),
        proposed_at,
        committed: false,
    };

    state.proposals.insert(contract.to_string(), proposal);
    save_state(&state);

    println!("Staged upgrade for '{}' → {}", contract, hash);
    println!("  WASM:      {}", wasm_path.display());
    println!("  Hash:      {}", hash);
    let remaining = proposed_at.saturating_add(TIMELOCK_DELAY_SECS).saturating_sub(now);
    if remaining > 0 {
        println!("  Timelock:  {} seconds remaining", remaining);
    } else {
        println!("  Timelock:  elapsed — ready to commit");
    }
}

fn cmd_list(_args: &[String]) {
    let state = load_state();
    if state.proposals.is_empty() {
        println!("No staged upgrade proposals.");
        return;
    }

    println!("Staged upgrade proposals:");
    println!("{:<20} {:<66} {:<12} {:<12}", "CONTRACT", "WASM_HASH", "STATUS", "TIMELock");
    println!("{:-<20} {:-<66} {:-<12} {:-<12}", "", "", "", "");

    for (name, proposal) in &state.proposals {
        let now = now_secs();
        let ready = now >= proposal.proposed_at + TIMELOCK_DELAY_SECS;
        let status = if proposal.committed {
            "committed".to_string()
        } else if ready {
            "ready".to_string()
        } else {
            format!("pending ({}s)", proposal.proposed_at + TIMELOCK_DELAY_SECS - now)
        };

        println!(
            "{:<20} {:<66} {:<12} {:<12}",
            name,
            &proposal.wasm_hash,
            status,
            proposal.proposed_at,
        );
    }
}

fn cmd_commit(args: &[String]) {
    if args.is_empty() {
        eprintln!("error: missing <contract>");
        usage();
        std::process::exit(1);
    }

    let contract = &args[0];
    let mut state = load_state();

    let proposal = match state.proposals.get(contract) {
        Some(p) if !p.committed => p.clone(),
        Some(_) => {
            eprintln!("error: '{}' is already committed", contract);
            std::process::exit(1);
        }
        None => {
            eprintln!("error: no staged proposal for '{}'", contract);
            std::process::exit(1);
        }
    };

    let now = now_secs();
    if now < proposal.proposed_at + TIMELOCK_DELAY_SECS {
        let remaining = proposal.proposed_at + TIMELOCK_DELAY_SECS - now;
        eprintln!(
            "error: timelock not elapsed — {} seconds remaining for '{}'",
            remaining, contract
        );
        eprintln!("hint: wait for the {}-second timelock to expire, then re-run.", TIMELOCK_DELAY_SECS);
        std::process::exit(1);
    }

    let p = state.proposals.get_mut(contract).unwrap();
    p.committed = true;
    save_state(&state);

    println!("Committed upgrade for '{}'", contract);
    println!("  WASM hash: {}", proposal.wasm_hash);
    println!("  WASM path: {}", proposal.wasm_path.display());
    println!();
    println!("Next steps:");
    println!("  1. Upload the WASM to the network: soroban wasm upload --wasm {}", proposal.wasm_path.display());
    println!("  2. Call propose_upgrade on the contract with the returned hash");
    println!("  3. Wait {} seconds for the on-chain timelock", TIMELOCK_DELAY_SECS);
    println!("  4. Call execute_upgrade on the contract");
}

fn cmd_cancel(args: &[String]) {
    if args.is_empty() {
        eprintln!("error: missing <contract>");
        usage();
        std::process::exit(1);
    }

    let contract = &args[0];
    let mut state = load_state();

    if let Some(removed) = state.proposals.remove(contract) {
        save_state(&state);
        println!("Cancelled staged upgrade for '{}'", contract);
        println!("  WASM hash: {}", removed.wasm_hash);
    } else {
        eprintln!("error: no staged proposal for '{}'", contract);
        std::process::exit(1);
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        usage();
        return ExitCode::FAILURE;
    }

    let command = &args[0];
    let rest = &args[1..];

    match command.as_str() {
        "stage" => {
            cmd_stage(rest);
            ExitCode::SUCCESS
        }
        "list" => {
            cmd_list(rest);
            ExitCode::SUCCESS
        }
        "commit" => {
            cmd_commit(rest);
            ExitCode::SUCCESS
        }
        "cancel" => {
            cmd_cancel(rest);
            ExitCode::SUCCESS
        }
        "-h" | "--help" | "help" => {
            usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("error: unknown command '{}'", other);
            usage();
            ExitCode::FAILURE
        }
    }
}
