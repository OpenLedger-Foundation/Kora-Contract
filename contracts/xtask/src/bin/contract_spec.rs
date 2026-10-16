//! Standalone contract spec / interface extractor (issue #937).
//!
//! Extracts contract specifications from compiled Soroban WASM custom sections
//! (`contractspecv0`) or JSON specs into structured JSON or line-by-line interface txt.
//!
//! Run via:
//!     cargo run -p kora-xtask --bin contract-spec -- <PATH_TO_WASM> [--format json|interface] [--out <FILE>]

use kora_xtask::spec::extract_contract_spec;
use std::path::PathBuf;
use std::process::ExitCode;

fn print_help() {
    println!(
        r#"kora-xtask contract-spec: Contract specification & interface extractor

USAGE:
    cargo run -p kora-xtask --bin contract-spec -- <PATH_TO_WASM_OR_JSON> [OPTIONS]

ARGUMENTS:
    <PATH_TO_WASM_OR_JSON>    Path to the compiled contract WASM or spec JSON file.

OPTIONS:
    --format <json|interface>
        Output format:
          'json' (default): Machine-readable JSON AST for SDK code generation.
          'interface': Line-by-line function signatures for check-interface-compat.sh.
    --out <FILE_PATH>
        Write output to file instead of stdout.
    -h, --help
        Print this help message.

EXAMPLES:
    # Print JSON spec to stdout
    cargo run -p kora-xtask --bin contract-spec -- target/wasm32-unknown-unknown/release/invoice_nft.wasm

    # Write interface signatures to a text file for CI compat check
    cargo run -p kora-xtask --bin contract-spec -- target/wasm32-unknown-unknown/release/invoice_nft.wasm --format interface --out specs/pr/invoice_nft.txt
"#
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }

    let mut input_path: Option<PathBuf> = None;
    let mut format = "json";
    let mut output_path: Option<PathBuf> = None;

    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--format" => {
                idx += 1;
                if idx < args.len() {
                    format = &args[idx];
                } else {
                    eprintln!("Error: --format requires an argument (json or interface)");
                    return ExitCode::FAILURE;
                }
            }
            "--out" => {
                idx += 1;
                if idx < args.len() {
                    output_path = Some(PathBuf::from(&args[idx]));
                } else {
                    eprintln!("Error: --out requires a file path argument");
                    return ExitCode::FAILURE;
                }
            }
            other => {
                if !other.starts_with('-') && input_path.is_none() {
                    input_path = Some(PathBuf::from(other));
                } else {
                    eprintln!("Unknown argument: {other}");
                    print_help();
                    return ExitCode::FAILURE;
                }
            }
        }
        idx += 1;
    }

    let input = match input_path {
        Some(p) => p,
        None => {
            eprintln!("Error: Missing input path to contract WASM or spec file.");
            print_help();
            return ExitCode::FAILURE;
        }
    };

    if !input.exists() {
        eprintln!("Error: Input file does not exist: {}", input.display());
        return ExitCode::FAILURE;
    }

    let spec = match extract_contract_spec(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "Error extracting contract spec from {}: {e}",
                input.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let output_text = match format {
        "interface" | "txt" => spec.to_interface_txt(),
        "json" => match spec.to_json() {
            Ok(j) => j,
            Err(e) => {
                eprintln!("Error serializing spec to JSON: {e}");
                return ExitCode::FAILURE;
            }
        },
        other => {
            eprintln!("Error: Unsupported format '{other}'. Choose 'json' or 'interface'.");
            return ExitCode::FAILURE;
        }
    };

    if let Some(out_p) = output_path {
        if let Some(parent) = out_p.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        if let Err(e) = std::fs::write(&out_p, output_text) {
            eprintln!("Error writing output to {}: {e}", out_p.display());
            return ExitCode::FAILURE;
        }
        println!("Successfully wrote spec ({format}) to {}", out_p.display());
    } else {
        println!("{output_text}");
    }

    ExitCode::SUCCESS
}
