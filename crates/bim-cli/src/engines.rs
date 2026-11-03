//! The trades and standards of `bim`, resolved only through a [`bim_plugin::Registry`]:
//!
//! * [`registry`] registers the shipped engines (the `arch` trade, [`bim_arch::ArchEngine`], and
//!   the `struct` trade, [`bim_struct::StructEngine`], and the `mep` trade, [`bim_mep::MepEngine`])
//!   and the shipped declarative code packs (`ibc`, `eurocode`, `accessibility`:
//!   [`bim_codes::shipped`]); `bim-plugin` itself has no built-ins.
//! * `bim solve <dir> --trade <trade> [--method newton|hardy-cross] [--json]` looks the trade up
//!   with [`Registry::engine`] and prints its result: with `--json` as one JSON document, without
//!   it as text, one line per element of each list in the result (`wall W1 volume 2.4 side_area
//!   12`, see [`result_text`]). An unknown trade is a usage error (exit 2). `--method` picks the
//!   network solver of the `mep` trade (default `newton`); an unknown method, or `--method` with
//!   any other trade, is a usage error (exit 2).
//! * `bim validate <dir> [--standard <pack>] [--json]` reports the dangling refs, the materialized
//!   `ConflictDiagnostic` entities and, with `--standard`, the violations of that rule set; exit 1
//!   if there is any. `<pack>` is a registered rule-set id (`ibc`, `eurocode`, `accessibility`:
//!   declarative packs, re-read from their files on every run), else the path of an existing file:
//!   a declarative JSON pack ([`PackRuleSet`], spec/plugins.md; a file with the extension `.json`
//!   or whose first non-blank character is `{`, read afresh on every run), else a `rule-set`
//!   WebAssembly component ([`WasmRuleSet`]). Either is registered under its `id()` (a pack's
//!   `pack`) and then looked up by that id. Anything else, a malformed pack, a pack with an unknown
//!   `check` or an invalid rule, and a file that is not a valid `rule-set` component, exits 2,
//!   before anything is printed. A trap, fuel or time exhaustion or memory failure of a wasm
//!   component exits 3 with a message. Plugins and packs never change the document.
//!
//! `--json` prints `{"violations":[{"rule","entity","message","measured","limit"}],
//! "dangling":[{"source","key","target"}],"conflicts":[{"id","entity","key","target","reason"}]}`
//! (spec/cli.md); violations in the rule set's order, unchanged. Without `--json`, `validate`
//! prints one line per finding: `dangling <source> <key> <target>` (as before `--standard`
//! existed), `violation <rule> <entity> <measured> <limit> <message>`, `conflict <id>`.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;

use bim_arch::ArchEngine;
use bim_codes::PackRuleSet;
use bim_mep::{Method, MepEngine};
use bim_plugin::{EngineError, Registry, StandardRuleSet, Violation, WasmRuleSet};

use crate::commands::emit;
use crate::geometry::parse;
use crate::json;
use crate::project::Project;
use crate::CliError;

type Outcome = Result<u8, CliError>;

/// The registry of the shipped trades and standards.
pub(crate) fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register_engine(Box::new(ArchEngine::new()));
    registry.register_engine(Box::new(bim_struct::StructEngine::new()));
    registry.register_engine(Box::new(MepEngine::new()));
    for rule_set in bim_codes::shipped() {
        registry.register_rule_set(rule_set); // lazy: a pack file is read when the rule set runs
    }
    registry
}

/// `bim solve <dir> --trade <trade> [--method newton|hardy-cross] [--json]`.
pub(crate) fn solve(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let form = "solve <dir> --trade <trade> [--method newton|hardy-cross] --json";
    let p = parse(args, &["--trade", "--method"], form)?;
    let trade = p.value("--trade").ok_or_else(|| CliError::Usage(format!("`bim {form}`: `--trade` is required")))?;
    let mut registry = registry();
    if let Some(method) = p.value("--method") {
        if trade != "mep" {
            return Err(CliError::Usage(format!("`bim {form}`: `--method` applies to the `mep` trade only")));
        }
        let method = method.to_str().and_then(Method::parse).ok_or_else(|| {
            CliError::Usage(format!("unknown method `{}` (known: newton, hardy-cross)", method.to_string_lossy()))
        })?;
        registry.register_engine(Box::new(MepEngine::with_method(method)));
    }
    let engine = trade
        .to_str()
        .and_then(|name| registry.engine(name))
        .ok_or_else(|| CliError::Usage(format!("unknown trade `{}`", trade.to_string_lossy())))?;
    let project = Project::open(Path::new(p.positional[0]))?;
    let ctx = project.engine_context();
    let result = engine.solve(&ctx).map_err(CliError::Engine)?;
    if p.json {
        let mut text = String::new();
        write_json(&mut text, &result);
        emit(out, &text)?;
    } else {
        write!(out, "{}", result_text(&result)).map_err(|e| CliError::Input(format!("cannot write to standard output: {e}")))?;
    }
    Ok(0)
}

/// The text form of an engine result (`bim solve` without `--json`): for every list in the result
/// object, one line per element that is an object with an `id`: the list's name without its
/// plural `s`, the id, then every numeric field with its value (`wall W1 volume 2.4 side_area
/// 12`); a top-level number is one `<key> <value>` line. Nested lists and objects, strings and
/// booleans are left out; numbers print as `f64` does.
pub(crate) fn result_text(result: &serde_json::Value) -> String {
    let mut s = String::new();
    let Some(object) = result.as_object() else {
        return s;
    };
    for (key, value) in object {
        match value {
            serde_json::Value::Array(items) => {
                let singular = key.strip_suffix('s').unwrap_or(key);
                for item in items {
                    let Some(fields) = item.as_object() else { continue };
                    let Some(id) = fields.get("id").and_then(serde_json::Value::as_str) else { continue };
                    s.push_str(singular);
                    s.push(' ');
                    s.push_str(id);
                    for (k, v) in fields {
                        if let Some(x) = v.as_f64().filter(|_| k != "id") {
                            s.push_str(&format!(" {k} {x}"));
                        }
                    }
                    s.push('\n');
                }
            }
            serde_json::Value::Number(n) => {
                if let Some(x) = n.as_f64() {
                    s.push_str(&format!("{key} {x}\n"));
                }
            }
            _ => {}
        }
    }
    s
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
/// an existing file, read once and loaded as a declarative pack ([`is_declarative_pack`]) or as a
/// `rule-set` component, and registered under its id.
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
    let origin = path.display().to_string();
    let bytes = fs::read(path).map_err(|e| CliError::Engine(EngineError::Input(format!("{origin}: {e}"))))?;
    let rule_set: Box<dyn StandardRuleSet> = if is_declarative_pack(path, &bytes) {
        Box::new(PackRuleSet::from_bytes(&bytes, &origin).map_err(CliError::Engine)?)
    } else {
        Box::new(WasmRuleSet::from_bytes(&bytes, &origin).map_err(CliError::Engine)?)
    };
    let id = rule_set.id().to_string();
    registry.register_rule_set(rule_set);
    Ok(id)
}

/// Whether the file at `path` with contents `bytes` is meant as a declarative JSON pack rather than
/// a WebAssembly component: its extension is `.json` (in any case) or its first byte that is not
/// white space, after an optional UTF-8 byte-order mark, is `{`. A component starts with `\0asm`.
fn is_declarative_pack(path: &Path, bytes: &[u8]) -> bool {
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("json")) {
        return true;
    }
    let body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{')
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
    fn the_registry_has_arch_struct_mep_and_the_shipped_packs() {
        let registry = registry();
        assert_eq!(registry.engine("arch").map(|e| e.name()), Some("arch"));
        assert_eq!(registry.engine("struct").map(|e| e.name()), Some("struct"));
        assert_eq!(registry.engine("mep").map(|e| e.name()), Some("mep"));
        assert!(registry.engine("nope").is_none());
        for id in ["ibc", "eurocode", "accessibility"] {
            assert_eq!(registry.rule_set(id).map(|r| r.id()), Some(id));
            assert!(registry.engine(id).is_none(), "{id} is a pack, not a trade");
        }
        assert!(registry.rule_set("arch").is_none());
        assert!(registry.rule_set("nope").is_none());
    }

    #[test]
    fn result_text_is_one_line_per_listed_element_with_its_numbers() {
        let result = serde_json::from_str(
            r#"{"rooms":[],"walls":[{"id":"W","volume":2.4,"side_area":12.0,"layers":[{"material":"a","volume":1.2}]},
                {"id":"V","volume":1,"side_area":0.5,"layers":[]}],"slabs":[{"id":"S","volume":3.2,"area":16}],
                "stairs":[],"curtain_walls":[],"continuity_residual":1e-7,"note":"x"}"#,
        )
        .unwrap();
        assert_eq!(
            result_text(&result),
            "wall W volume 2.4 side_area 12\nwall V volume 1 side_area 0.5\nslab S volume 3.2 area 16\ncontinuity_residual 0.0000001\n"
        );
        assert_eq!(result_text(&serde_json::json!([1, 2])), "");
    }

    #[test]
    fn solve_without_json_prints_the_text_form() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("p");
        let d = dir.to_str().unwrap();
        assert_eq!(run(&["init", d]).0, 0);
        let ops = tmp.path().join("m.ops.json");
        fs::write(&ops, r#"{"schema":1,"ops":[
          {"op":"create","id":"L1","kind":"Level","props":{"elevation":0}},
          {"op":"create","id":"T","kind":"WallType","props":{"layer_thickness":[0.1,0.1],"layer_material":["a","b"]}},
          {"op":"create","id":"W","kind":"Wall","props":{"level":{"ref":"L1"},"type":{"ref":"T"},"x1":0,"y1":0,"x2":4,"y2":0,"height":3}},
          {"op":"create","id":"S","kind":"Slab","props":{"level":{"ref":"L1"},"boundary":[0,0,4,0,4,4,0,4],"thickness":0.2}}
        ]}"#).unwrap();
        assert_eq!(run(&["apply", d, ops.to_str().unwrap()]).0, 0);
        let (code, out, err) = run(&["solve", d, "--trade", "arch"]);
        assert_eq!(code, 0, "{err}");
        // The same lines `bim solve --trade arch` printed before the registry existed.
        assert_eq!(out.lines().count(), 2, "{out}");
        assert!(out.lines().next().unwrap().starts_with("wall W volume 2.4"), "{out}");
        assert!(out.lines().next().unwrap().contains(" side_area 12"), "{out}");
        assert_eq!(out.lines().nth(1).unwrap(), "slab S volume 3.2 area 16", "{out}");
    }

    #[test]
    fn solve_mep_takes_a_method_and_rejects_bad_ones() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("p");
        let d = dir.to_str().unwrap();
        assert_eq!(run(&["init", d]).0, 0);
        let ops = tmp.path().join("m.json");
        fs::write(&ops, r#"{"schema":1,"ops":[
          {"op":"create","id":"W","kind":"MepSystem","props":{"name":"W","medium":"water","density":998.2,"viscosity":0.001002}},
          {"op":"create","id":"s","kind":"Port","props":{"system":{"ref":"W"},"x":0,"y":0,"z":0,"kind":"source","pressure":300000}},
          {"op":"create","id":"k","kind":"Port","props":{"system":{"ref":"W"},"x":30,"y":0,"z":0,"kind":"sink","flow":0.003}},
          {"op":"create","id":"p","kind":"Segment","props":{"from":{"ref":"s"},"to":{"ref":"k"},"shape":"round","diameter":0.05,"roughness":0}}
        ]}"#).unwrap();
        assert_eq!(run(&["apply", d, ops.to_str().unwrap()]).0, 0);
        let (code, newton, err) = run(&["solve", d, "--trade", "mep", "--json"]);
        assert_eq!(code, 0, "{err}");
        let doc: serde_json::Value = serde_json::from_str(&newton).unwrap();
        assert_eq!(doc["segments"][0]["id"], "p");
        assert!((doc["segments"][0]["flow"].as_f64().unwrap() - 0.003).abs() < 1e-15, "{newton}");
        assert_eq!(doc["ports"][0]["id"], "k");
        assert_eq!(doc["ports"][1]["pressure"], 300000.0);
        assert_eq!(doc["circuits"], serde_json::json!([]));
        let (code, hc, _) = run(&["solve", d, "--trade", "mep", "--method", "hardy-cross", "--json"]);
        assert_eq!((code, hc.contains("\"continuity_residual\"")), (0, true));
        assert_eq!(run(&["solve", d, "--trade", "mep", "--method", "newton", "--json"]).0, 0);
        assert_eq!(run(&["solve", d, "--trade", "mep", "--method", "gauss", "--json"]).0, 2);
        assert_eq!(run(&["solve", d, "--trade", "arch", "--method", "newton", "--json"]).0, 2);
        assert_eq!(run(&["solve", d, "--trade", "mep", "--method"]).0, 2);
        // A sink with no source to feed it is invalid input.
        fs::write(&ops, r#"{"schema":1,"ops":[{"op":"set","id":"s","key":"kind","value":"junction"}]}"#).unwrap();
        assert_eq!(run(&["apply", d, ops.to_str().unwrap()]).0, 0);
        assert_eq!(run(&["solve", d, "--trade", "mep", "--json"]).0, 2);
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

    /// A project with one 5 m wall and a 0.8 m door in it; returns the project directory.
    fn project_with_a_door(tmp: &TempDir) -> String {
        let dir = tmp.path().join("door-project");
        let d = dir.to_str().unwrap().to_string();
        assert_eq!(run(&["init", &d]).0, 0);
        let ops = tmp.path().join("door.ops.json");
        fs::write(
            &ops,
            r#"{"schema":1,"ops":[
              {"op":"create","id":"L","kind":"Level","props":{"elevation":0}},
              {"op":"create","id":"T","kind":"WallType","props":{"layer_thickness":[0.2]}},
              {"op":"create","id":"W","kind":"Wall","props":{"level":{"ref":"L"},"type":{"ref":"T"},"x1":0,"y1":0,"x2":5,"y2":0,"height":3}},
              {"op":"create","id":"D","kind":"Opening","props":{"host":{"ref":"W"},"kind":"door","offset":1,"sill":0,"width":0.8,"height":2.1}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(run(&["apply", &d, ops.to_str().unwrap()]).0, 0);
        d
    }

    const CLEAN: &str = "{\"violations\":[],\"dangling\":[],\"conflicts\":[]}\n";

    fn door_pack(limit: &str) -> String {
        format!(
            r#"{{"pack":"mine","rules":[{{"id":"R-door","applies_to":"Opening","where":{{"kind":"door"}},"check":"min_width","limit":{limit}}}]}}"#
        )
    }

    #[test]
    fn the_shipped_packs_are_standards_by_id() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("empty");
        let d = dir.to_str().unwrap();
        assert_eq!(run(&["init", d]).0, 0);
        for id in ["ibc", "eurocode", "accessibility"] {
            let (code, out, err) = run(&["validate", d, "--standard", id, "--json"]);
            assert_eq!((code, out.as_str()), (0, CLEAN), "{id}: {err}");
        }
        let d = project_with_a_door(&tmp);
        let (code, out, err) = run(&["validate", &d, "--standard", "ibc", "--json"]);
        assert_eq!(code, 1, "{err}");
        assert_eq!(
            out,
            "{\"violations\":[{\"rule\":\"IBC-1010.1.1\",\"entity\":\"D\",\"message\":\"Opening D: min_width 0.8 < limit 0.813\",\"measured\":0.8,\"limit\":0.813}],\"dangling\":[],\"conflicts\":[]}\n"
        );
        let (code, out, _) = run(&["validate", &d, "--standard", "accessibility"]);
        assert_eq!((code, out.as_str()), (1, "violation ACC-door-clear-width D 0.8 0.85 Opening D: min_width 0.8 < limit 0.85\n"));
        assert_eq!(run(&["validate", &d, "--standard", "eurocode", "--json"]).1, CLEAN);
    }

    #[test]
    fn validate_with_a_json_pack_path_reads_it_afresh_every_run() {
        let tmp = TempDir::new();
        let d = project_with_a_door(&tmp);
        let pack = tmp.path().join("mine.json");
        let p = pack.to_str().unwrap();
        fs::write(&pack, door_pack("0.85")).unwrap();
        let (code, out, err) = run(&["validate", &d, "--standard", p, "--json"]);
        assert_eq!(code, 1, "{err}");
        assert_eq!(
            out,
            "{\"violations\":[{\"rule\":\"R-door\",\"entity\":\"D\",\"message\":\"Opening D: min_width 0.8 < limit 0.85\",\"measured\":0.8,\"limit\":0.85}],\"dangling\":[],\"conflicts\":[]}\n"
        );
        let (code, out, _) = run(&["validate", &d, "--standard", p]);
        assert_eq!((code, out.as_str()), (1, "violation R-door D 0.8 0.85 Opening D: min_width 0.8 < limit 0.85\n"));
        // Edited between two runs: the next run sees the new limit.
        fs::write(&pack, door_pack("0.75")).unwrap();
        assert_eq!(run(&["validate", &d, "--standard", p, "--json"]), (0, CLEAN.to_string(), String::new()));
        // A pack that is named like a shipped one stands in for it, for that run only.
        fs::write(&pack, r#"{"pack":"ibc","rules":[]}"#).unwrap();
        assert_eq!(run(&["validate", &d, "--standard", p, "--json"]).1, CLEAN);
        assert_eq!(run(&["validate", &d, "--standard", "ibc", "--json"]).0, 1);
    }

    #[test]
    fn a_pack_is_recognised_by_its_content_when_it_has_no_json_extension() {
        let tmp = TempDir::new();
        let d = project_with_a_door(&tmp);
        let pack = door_pack("0.85");
        for (name, content) in [
            ("rules.pack", pack.clone()),
            ("rules", format!("  \n\t{pack}")),
            ("bom.txt", format!("\u{feff}{pack}")),
            ("RULES.JSON", pack.clone()),
            ("rules.wasm", pack.clone()), // a wasm component starts with \0asm, never with `{`
        ] {
            let path = tmp.path().join(name);
            fs::write(&path, content).unwrap();
            let (code, out, err) = run(&["validate", &d, "--standard", path.to_str().unwrap()]);
            assert_eq!((code, out.as_str()), (1, "violation R-door D 0.8 0.85 Opening D: min_width 0.8 < limit 0.85\n"), "{name}: {err}");
        }
    }

    #[test]
    fn malformed_packs_and_unknown_checks_exit_2_before_any_output() {
        let tmp = TempDir::new();
        let d = project_with_a_door(&tmp);
        let pack = tmp.path().join("bad.json");
        let door = r#"{"id":"R","applies_to":"Opening","check":"min_width","limit":0.85}"#;
        for text in [
            format!(r#"{{"pack":"p","rules":[{door},{{"id":"B","applies_to":"Space","check":"max_banana","limit":1}}]}}"#),
            r#"{"pack": "broken", "rules": ["#.to_string(),
            r#"{"pack":"p","rules":[{"id":"R","applies_to":"Wall","check":"riser_max","limit":1}]}"#.to_string(),
            r#"{"pack":"p","rules":[{"id":"R","applies_to":"Stair","check":"blondel","limit":1}]}"#.to_string(),
            r#"{"pack":"p","rules":[{"id":"R","applies_to":"Stair","check":"riser_max","limit":"1"}]}"#.to_string(),
            format!(r#"{{"pack":"p","rules":[{door},{door}]}}"#),
            r#"{"rules":[]}"#.to_string(),
            "[]".to_string(),
            String::new(),
        ] {
            fs::write(&pack, &text).unwrap();
            for flags in [&["--json"][..], &[][..]] {
                let mut args = vec!["validate", d.as_str(), "--standard", pack.to_str().unwrap()];
                args.extend_from_slice(flags);
                let (code, out, err) = run(&args);
                assert_eq!((code, out.as_str()), (2, ""), "{text}: {err}");
                assert!(err.contains("bad.json"), "{err}");
            }
        }
        // Not UTF-8 text, and a file that is neither a pack nor a component.
        fs::write(&pack, b"{\"pack\":\"\xff\"}").unwrap();
        assert_eq!(run(&["validate", &d, "--standard", pack.to_str().unwrap()]).0, 2);
        let other = tmp.path().join("other.bin");
        fs::write(&other, b"\0asm\x01\0\0\0").unwrap();
        assert_eq!(run(&["validate", &d, "--standard", other.to_str().unwrap()]).0, 2);
        assert_eq!(run(&["validate", &d, "--standard", tmp.path().to_str().unwrap()]).0, 2, "a directory");
    }
}
