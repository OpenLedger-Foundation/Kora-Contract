//! Generates the event table in `docs/EVENTS.md` from the shared event helpers.
//!
//! Usage:
//!   cargo run -p kora-xtask --bin gen_events            # rewrite the table
//!   cargo run -p kora-xtask --bin gen_events -- --check # fail on drift

use quote::ToTokens;
use std::path::Path;
use std::process::ExitCode;
use std::{env, fs};
use syn::visit::{self, Visit};
use syn::{Expr, FnArg, Item, Lit, Visibility};

const BEGIN: &str = "<!-- BEGIN GENERATED EVENTS -->";
const END: &str = "<!-- END GENERATED EVENTS -->";

#[derive(Default)]
struct Emits(Vec<Result<(String, Vec<String>), String>>);

fn topic(expr: &Expr) -> Option<String> {
    let lit = match expr {
        Expr::Macro(m) if m.mac.path.segments.last()?.ident == "symbol_short" => {
            m.mac.parse_body::<Lit>().ok()?
        }
        Expr::Call(c) if compact(&c.func).ends_with("Symbol::new") => match c.args.last()? {
            Expr::Lit(l) => l.lit.clone(),
            _ => return None,
        },
        _ => return None,
    };
    match lit {
        Lit::Str(s) => Some(s.value()),
        _ => None,
    }
}

fn compact(tokens: &impl ToTokens) -> String {
    tokens.to_token_stream().to_string().replace(' ', "")
}

fn field(expr: &Expr) -> String {
    match compact(expr).as_str() {
        "env.ledger().timestamp()" => "timestamp".into(),
        s => s
            .trim_start_matches('&')
            .trim_end_matches(".clone()")
            .trim_end_matches(".cloned()")
            .into(),
    }
}

impl<'ast> Visit<'ast> for Emits {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if compact(&call.func) == "emit" {
            let found = call.args.iter().nth(1).and_then(topic);
            self.0.push(match (found, call.args.iter().nth(2)) {
                (Some(t), Some(Expr::Tuple(p))) => Ok((t, p.elems.iter().map(field).collect())),
                (Some(t), Some(p)) => Ok((t, vec![field(p)])),
                _ => Err("emit call has no literal topic and payload".into()),
            });
        }
        visit::visit_expr_call(self, call);
    }
}

/// Render one row per emitted event, in source order.
fn render(source: &str) -> Result<String, String> {
    let file = syn::parse_file(source).map_err(|e| format!("cannot parse events: {e}"))?;
    let mut out = String::from("| Topic | Function | Parameters | Payload |\n|---|---|---|---|\n");
    let mut count = 0;
    for item in &file.items {
        let Item::Fn(f) = item else { continue };
        if !matches!(f.vis, Visibility::Public(_)) {
            continue;
        }
        let mut emits = Emits::default();
        emits.visit_block(&f.block);
        let name = &f.sig.ident;
        let params: Vec<String> = f
            .sig
            .inputs
            .iter()
            .skip(1)
            .filter_map(|a| match a {
                FnArg::Typed(t) => Some(format!("{}: {}", compact(&t.pat), compact(&t.ty))),
                _ => None,
            })
            .collect();
        for emitted in emits.0 {
            let (topic, payload) = emitted.map_err(|e| format!("{name}: {e}"))?;
            let cell = |v: &[String]| v.join(", ").replace('|', "\\|");
            out.push_str(&format!(
                "| `{topic}` | `{name}` | `{}` | `({})` |\n",
                cell(&params),
                cell(&payload)
            ));
            count += 1;
        }
    }
    if count == 0 {
        return Err("no event definitions found".into());
    }
    Ok(out)
}

/// Replace the marked region; markers must each occur once, in order.
fn splice(doc: &str, table: &str) -> Result<String, String> {
    if doc.matches(BEGIN).count() != 1 || doc.matches(END).count() != 1 {
        return Err("expected exactly one pair of generated-events markers".into());
    }
    let (head, rest) = doc.split_once(BEGIN).ok_or("missing begin marker")?;
    let (_, tail) = rest.split_once(END).ok_or("end marker precedes begin")?;
    Ok(format!("{head}{BEGIN}\n{table}{END}{tail}"))
}

fn run(check: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let doc_path = root.join("docs/EVENTS.md");
    let doc = read(&doc_path)?;
    let table = render(&read(&root.join("contracts/shared/src/events.rs"))?)?;
    let updated = splice(&doc, &table)?;
    if check && updated != doc {
        return Err(
            "docs/EVENTS.md is stale; run `cargo run -p kora-xtask --bin gen_events`".into(),
        );
    }
    if !check && updated != doc {
        fs::write(&doc_path, updated).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => run(false),
        ["--check"] => run(true),
        _ => Err("usage: gen_events [--check]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_both_topic_forms() {
        let t = render(
            r#"pub fn a(env: &Env, id: u64) { emit(env, symbol_short!("A"), (id, env.ledger().timestamp())); }
               pub fn b(env: &Env, w: &Address) { emit(env, Symbol::new(env, "LONG_TOPIC"), (w.clone(),)); }"#,
        )
        .unwrap();
        assert!(t.contains("| `A` | `a` | `id: u64` | `(id, timestamp)` |"));
        assert!(t.contains("| `LONG_TOPIC` | `b` | `w: &Address` | `(w)` |"));
    }

    #[test]
    fn rejects_dynamic_topics_and_empty_or_invalid_source() {
        assert!(render("pub fn a(env: &Env, t: Symbol) { emit(env, t, (1,)); }").is_err());
        assert!(render("").is_err());
        assert!(render("not rust").is_err());
    }

    #[test]
    fn splice_is_idempotent_and_requires_markers() {
        let doc = format!("x\n{BEGIN}\nold\n{END}\ny");
        let once = splice(&doc, "new\n").unwrap();
        assert_eq!(once, splice(&once, "new\n").unwrap());
        assert!(!once.contains("old"));
        assert!(splice("none", "t").is_err());
        assert!(splice(&format!("{END}{BEGIN}"), "t").is_err());
    }

    #[test]
    fn committed_doc_is_current() {
        run(true).unwrap();
    }
}
