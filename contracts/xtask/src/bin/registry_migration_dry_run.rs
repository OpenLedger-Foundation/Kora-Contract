//! Offline dry-run of a parameter_registry storage layout v2 migration (issue #1180).
//!
//! Usage: registry-migration-dry-run --snapshot <v1.json> --target <v2.json>
//! The snapshot is the output of `bootstrap-registry`. Never writes or uses the network.
//! Exit codes: 0 safe, 1 argument/input error, 2 migration has blocking problems.

use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;

#[derive(Deserialize)]
struct Snapshot {
    params: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    schema_version: u32,
    params: BTreeMap<String, TargetParam>,
    /// v1 keys intentionally removed by v2.
    #[serde(default)]
    dropped: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetParam {
    min: Value,
    max: Value,
    default: Option<Value>,
    renamed_from: Option<String>,
}

/// i128 values may be JSON integers or, beyond 64 bits, decimal strings.
fn int(v: &Value) -> Result<i128, String> {
    let text = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
    text.parse().map_err(|_| format!("not an i128 integer: {v}"))
}

/// Returns (planned operations, blocking problems).
fn plan(snapshot: &str, target: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let snap: Snapshot = serde_json::from_str(snapshot).map_err(|e| format!("snapshot: {e}"))?;
    let target: Target = serde_json::from_str(target).map_err(|e| format!("target: {e}"))?;
    if target.schema_version != 2 {
        return Err(format!("unsupported schema_version {}", target.schema_version));
    }
    if snap.params.is_empty() || target.params.is_empty() {
        return Err("snapshot and target params must not be empty".into());
    }
    let (mut ops, mut problems, mut used) = (Vec::new(), Vec::new(), BTreeSet::new());
    for (key, spec) in &target.params {
        let (min, max) = (int(&spec.min)?, int(&spec.max)?);
        if min > max {
            problems.push(format!("{key}: min {min} > max {max}"));
            continue;
        }
        // Prefer an already-migrated key so re-running the plan is idempotent.
        let source = if snap.params.contains_key(key) {
            key
        } else {
            spec.renamed_from.as_ref().unwrap_or(key)
        };
        let value = match (snap.params.get(source), &spec.default) {
            (Some(v), _) => {
                if !used.insert(source.clone()) {
                    problems.push(format!("{key}: source {source} is already mapped"));
                }
                let op = if source == key { "KEEP" } else { "RENAME" };
                let value = int(v)?;
                ops.push(format!("{op} {source} -> {key} = {value}"));
                value
            }
            (None, Some(v)) => {
                let value = int(v)?;
                ops.push(format!("ADD {key} = {value}"));
                value
            }
            (None, None) => {
                problems.push(format!("{key}: missing in snapshot and has no default"));
                continue;
            }
        };
        if value < min || value > max {
            problems.push(format!("{key}: {value} outside [{min}, {max}]"));
        }
    }
    for key in snap.params.keys().filter(|k| !used.contains(*k)) {
        if target.dropped.contains(key) {
            ops.push(format!("DROP {key}"));
        } else {
            problems.push(format!("{key}: would be silently lost; map it or list it in dropped"));
        }
    }
    Ok((ops, problems))
}

fn run(args: &[String]) -> Result<bool, String> {
    let (snapshot, target) = match args {
        [a, s, b, t] if a == "--snapshot" && b == "--target" => (s, t),
        _ => return Err("usage: --snapshot <v1.json> --target <v2.json>".into()),
    };
    let read = |p: &String| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"));
    let (ops, problems) = plan(&read(snapshot)?, &read(target)?)?;
    for op in ops {
        println!("{op}");
    }
    for problem in &problems {
        eprintln!("BLOCKED {problem}");
    }
    Ok(problems.is_empty())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = r#"{"schema_version":2,"params":{
        "Fee":{"min":0,"max":100},
        "New":{"min":0,"max":10,"default":5},
        "Renamed":{"min":0,"max":"85070591730234615865843651857942052863","renamed_from":"Old"}},
        "dropped":["Gone"]}"#;

    #[test]
    fn plans_all_operations_and_rerun_is_idempotent() {
        let (ops, bad) = plan(r#"{"params":{"Fee":50,"Old":7,"Gone":1}}"#, TARGET).unwrap();
        assert!(bad.is_empty());
        assert_eq!(ops, ["KEEP Fee -> Fee = 50", "ADD New = 5", "RENAME Old -> Renamed = 7", "DROP Gone"]);
        let (ops, bad) = plan(r#"{"params":{"Fee":50,"New":5,"Renamed":7}}"#, TARGET).unwrap();
        assert!(bad.is_empty() && ops.iter().all(|o| o.starts_with("KEEP")));
    }

    #[test]
    fn blocks_loss_missing_and_out_of_bounds() {
        let (_, bad) = plan(r#"{"params":{"Fee":101,"Lost":1}}"#, TARGET).unwrap();
        assert_eq!(bad.len(), 3);
    }

    #[test]
    fn malformed_input_fails_closed() {
        for snap in ["", "{}", r#"{"params":{}}"#, r#"{"params":{"Fee":1.5}}"#] {
            assert!(plan(snap, TARGET).is_err());
        }
        assert!(plan(r#"{"params":{"Fee":1}}"#, r#"{"schema_version":3,"params":{}}"#).is_err());
        assert!(run(&[]).is_err());
    }
}
