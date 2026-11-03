//! The trades and standards of `bim`, resolved only through a [`bim_plugin::Registry`]:
//!
//! * [`registry`] registers the shipped engines (today the `arch` trade, [`ArchEngine`]);
//!   `bim-plugin` itself has no built-ins.
//! * `bim solve <dir> --trade <trade> [--json]` looks the trade up with [`Registry::engine`] and
//!   prints its result as one JSON document (with or without `--json`). An unknown trade is a
//!   usage error (exit 2).
//! * `bim validate <dir> [--standard <pack>] [--json]` reports the dangling refs, the materialized
//!   `ConflictDiagnostic` entities and, with `--standard`, the violations of that rule set; exit 1
//!   if there is any. `<pack>` is a registered rule-set id, else the path of an existing file,
//!   which is loaded as a `rule-set` WebAssembly component ([`WasmRuleSet`]), registered under its
//!   `id()` and then looked up by that id; anything else, and a file that is not a valid
//!   `rule-set` component, exits 2. A trap, fuel or time exhaustion or memory failure of the
//!   component exits 3 with a message. Plugins never change the document.
//!
//! `--json` prints `{"violations":[{"rule","entity","message","measured","limit"}],
//! "dangling":[{"source","key","target"}],"conflicts":[{"id","entity","key","target","reason"}]}`
//! (spec/cli.md); violations in the rule set's order, unchanged. Without `--json`, `validate`
//! prints one line per finding: `dangling <source> <key> <target>` (as before `--standard`
//! existed), `violation <rule> <entity> <measured> <limit> <message>`, `conflict <id>`.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use bim_plugin::{EngineContext, EngineError, Registry, StandardRuleSet, TradeEngine, Violation, WasmRuleSet};

use crate::commands::emit;
use crate::geometry::{arch_json, parse};
use crate::json;
use crate::project::Project;
use crate::CliError;

type Outcome = Result<u8, CliError>;

/// The registry of the shipped trades and standards.
pub(crate) fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register_engine(Box::new(ArchEngine));
    registry
}

/// The `arch` trade: the architectural quantities of the compiled geometry ([`arch_json`]).
pub struct ArchEngine;

impl TradeEngine for ArchEngine {
    fn name(&self) -> &str {
        "arch"
    }

    fn solve(&self, ctx: &EngineContext<'_>) -> Result<serde_json::Value, EngineError> {
        serde_json::from_str(&arch_json(ctx.model())).map_err(|e| EngineError::Plugin(format!("arch: invalid result: {e}")))
    }
}

/// `bim solve <dir> --trade <trade> [--json]`.
pub(crate) fn solve(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let form = "solve <dir> --trade <trade> --json";
    let p = parse(args, &["--trade"], form)?;
    let trade = p.value("--trade").ok_or_else(|| CliError::Usage(format!("`bim {form}`: `--trade` is required")))?;
    let registry = registry();
    let engine = trade
        .to_str()
        .and_then(|name| registry.engine(name))
        .ok_or_else(|| CliError::Usage(format!("unknown trade `{}`", trade.to_string_lossy())))?;
    let project = Project::open(Path::new(p.positional[0]))?;
    let ctx = project.engine_context();
    let result = engine.solve(&ctx).map_err(CliError::Engine)?;
    let mut text = String::new();
    write_json(&mut text, &result);
    emit(out, &text)?;
    Ok(0)
}

/// `bim validate <dir> [--standard <pack>] [--json]`.
pub(crate) fn validate(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let form = "validate <dir> [--standard <pack>] --json";
    let p = parse(args, &["--standard"], form)?;
    let project = Project::open(Path::new(p.positional[0]))?;
    let ctx = project.engine_context();
    let violations = match p.value("--standard") {
        None => Vec::new(),
        Some(pack) => {
            let mut registry = registry();
            let id = resolve_standard(&mut registry, pack)?;
            let rule_set = registry.rule_set(&id).ok_or_else(|| CliError::Internal(format!("rule set `{id}` vanished")))?;
            rule_set.try_check(&ctx).map_err(CliError::Engine)?
        }
    };
    let dangling = project.dangling();
    let conflicts: Vec<_> = ctx.entities().into_iter().filter(|(_, e)| e.kind == "ConflictDiagnostic").collect();

    if p.json {
        let mut s = String::from("{\"violations\":[");
        for (i, v) in violations.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            write_violation(&mut s, v);
        }
        s.push_str("],\"dangling\":[");
        for (i, d) in dangling.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"source\":");
            json::write_str(&mut s, &project.name(d.source));
            s.push_str(",\"key\":");
            json::write_str(&mut s, &d.key);
            s.push_str(",\"target\":");
            json::write_str(&mut s, &project.name(d.target));
            s.push('}');
        }
        s.push_str("],\"conflicts\":[");
        for (i, (name, e)) in conflicts.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let mut c = serde_json::Map::new();
            c.insert("id".to_string(), serde_json::Value::String(name.clone()));
            for key in ["entity", "key", "target", "reason"] {
                let value = e.props.get(key).map_or(serde_json::Value::Null, |v| ctx.value_json(v));
                c.insert(key.to_string(), value);
            }
            write_json(&mut s, &serde_json::Value::Object(c));
        }
        s.push_str("]}");
        emit(out, &s)?;
    } else {
        for d in &dangling {
            emit(out, &format!("dangling {} {} {}", project.name(d.source), d.key, project.name(d.target)))?;
        }
        for v in &violations {
            emit(out, &format!("violation {} {} {} {} {}", v.rule, v.entity, v.measured, v.limit, v.message))?;
        }
        for (name, _) in &conflicts {
            emit(out, &format!("conflict {name}"))?;
        }
    }
    Ok(u8::from(!violations.is_empty() || !dangling.is_empty() || !conflicts.is_empty()))
}

/// Resolves `--standard <pack>` to the id of a rule set in `registry`: a registered id as is, else
/// an existing file, loaded as a `rule-set` component and registered under its id.
fn resolve_standard(registry: &mut Registry, pack: &OsString) -> Result<String, CliError> {
    if let Some(id) = pack.to_str().filter(|id| registry.rule_set(id).is_some()) {
        return Ok(id.to_string());
    }
    let path = Path::new(pack);
    if !path.is_file() {
        return Err(CliError::Usage(format!(
            "unknown standard `{}`: neither a pack id nor an existing file",
            pack.to_string_lossy()
        )));
    }
    let rule_set = WasmRuleSet::load(path).map_err(CliError::Engine)?;
    let id = rule_set.id().to_string();
    registry.register_rule_set(Box::new(rule_set));
    Ok(id)
}

fn write_violation(s: &mut String, v: &Violation) {
    s.push_str("{\"rule\":");
    json::write_str(s, &v.rule);
    s.push_str(",\"entity\":");
    json::write_str(s, &v.entity);
    s.push_str(",\"message\":");
    json::write_str(s, &v.message);
    s.push_str(",\"measured\":");
    json::write_num(s, v.measured);
    s.push_str(",\"limit\":");
    json::write_num(s, v.limit);
    s.push('}');
}

/// Appends `value` as compact JSON, numbers and strings written as everywhere else in `bim`
/// ([`json::write_num`], [`json::write_str`]), object keys in their order.
fn write_json(s: &mut String, value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => s.push_str("null"),
        serde_json::Value::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => s.push_str(&i.to_string()),
            (_, Some(u), _) => s.push_str(&u.to_string()),
            (_, _, Some(x)) => json::write_num(s, x),
            _ => s.push_str(&n.to_string()),
        },
        serde_json::Value::String(text) => json::write_str(s, text),
        serde_json::Value::Array(items) => {
            s.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write_json(s, item);
            }
            s.push(']');
        }
        serde_json::Value::Object(map) => {
            s.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                json::write_str(s, key);
                s.push(':');
                write_json(s, item);
            }
            s.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::fs;

    fn run(args: &[&str]) -> (u8, String, String) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = crate::run(&args, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn the_registry_has_arch_and_no_rule_sets() {
        let registry = registry();
        assert_eq!(registry.engine("arch").map(|e| e.name()), Some("arch"));
        assert!(registry.engine("nope").is_none());
        assert!(registry.rule_set("arch").is_none());
    }

    #[test]
    fn write_json_keeps_key_order_and_number_forms() {
        let mut s = String::new();
        write_json(&mut s, &serde_json::from_str(r#"{"z":[1,2.5,-3,1e-7,null,true],"a":"x\"y"}"#).unwrap());
        assert_eq!(s, r#"{"z":[1,2.5,-3,1e-7,null,true],"a":"x\"y"}"#);
    }

    #[test]
    fn validate_json_and_standard_resolution() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("p");
        let d = dir.to_str().unwrap();
        assert_eq!(run(&["init", d]).0, 0);
        let (code, out, _) = run(&["validate", d, "--json"]);
        assert_eq!((code, out.as_str()), (0, "{\"violations\":[],\"dangling\":[],\"conflicts\":[]}\n"));
        let ops = tmp.path().join("m.json");
        fs::write(&ops, r#"{"schema":1,"ops":[{"op":"create","id":"a","kind":"Wall","props":{"host":{"ref":"gone"}}}]}"#).unwrap();
        assert_eq!(run(&["apply", d, ops.to_str().unwrap()]).0, 0);
        let (code, out, _) = run(&["validate", "--json", d]);
        assert_eq!(code, 1);
        assert_eq!(out, "{\"violations\":[],\"dangling\":[{\"source\":\"a\",\"key\":\"host\",\"target\":\"gone\"}],\"conflicts\":[]}\n");
        // Unknown pack ids, missing files and files that are not components all exit 2.
        let junk = tmp.path().join("junk.wasm");
        fs::write(&junk, "not a component").unwrap();
        for pack in ["no-such-pack", "/nonexistent/x.wasm", junk.to_str().unwrap()] {
            let (code, out, err) = run(&["validate", d, "--standard", pack, "--json"]);
            assert_eq!((code, out.as_str()), (2, ""), "{pack}: {err}");
        }
        assert_eq!(run(&["validate", d, "--standard"]).0, 2);
        assert_eq!(run(&["validate", d, "--standard", "a", "--standard", "b"]).0, 2);
    }
}
