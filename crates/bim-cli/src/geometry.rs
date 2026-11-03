//! The commands over the compiled building geometry ([`bim_model`]):
//!
//! * `bim solve <dir> --trade arch [--json]` prints the architectural quantities (spec/cli.md):
//!   `{"rooms":[],"walls":[{"id","volume","side_area","layers":[{"material","volume"}]}],
//!   "slabs":[{"id","volume","area"}],"stairs":[],"curtain_walls":[]}` (rooms, stairs and curtain
//!   walls come with later engines). Without `--json` it prints one line per wall and slab. Any
//!   other trade is a usage error (exit 2).
//! * `bim export <dir> --format gltf --out <file>` writes the glTF 2.0 document of
//!   [`bim_model::gltf`]: one mesh node per element with `extras.id`. Any other format is a usage
//!   error (exit 2). The file is written to a temporary file next to it and renamed into place.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;

use bim_model::Model;

use crate::json;
use crate::project::Project;
use crate::CliError;

type Outcome = Result<u8, CliError>;

/// Operands and `--flag value` / `--json` options, in any order.
struct Parsed<'a> {
    positional: Vec<&'a OsString>,
    values: Vec<(&'static str, &'a OsString)>,
    json: bool,
}

fn parse<'a>(args: &'a [OsString], valued: &[&'static str], form: &str) -> Result<Parsed<'a>, CliError> {
    let usage = |what: String| CliError::Usage(format!("`bim {form}`: {what}"));
    let mut p = Parsed { positional: Vec::new(), values: Vec::new(), json: false };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--json" {
            if p.json {
                return Err(usage("`--json` given twice".into()));
            }
            p.json = true;
        } else if let Some(flag) = valued.iter().find(|f| arg.to_str() == Some(f)) {
            let value = it.next().ok_or_else(|| usage(format!("`{flag}` needs a value")))?;
            if p.values.iter().any(|(f, _)| f == flag) {
                return Err(usage(format!("`{flag}` given twice")));
            }
            p.values.push((flag, value));
        } else if arg.to_str().is_some_and(|s| s.starts_with("--")) {
            return Err(usage(format!("unknown option `{}`", arg.to_string_lossy())));
        } else {
            p.positional.push(arg);
        }
    }
    if p.positional.len() != 1 {
        return Err(usage(format!("takes one directory, got {} operand(s)", p.positional.len())));
    }
    Ok(p)
}

impl Parsed<'_> {
    fn value(&self, flag: &str) -> Option<&OsString> {
        self.values.iter().find(|(f, _)| *f == flag).map(|(_, v)| *v)
    }
}

/// `bim solve <dir> --trade arch [--json]`.
pub(crate) fn solve(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let form = "solve <dir> --trade <trade> --json";
    let p = parse(args, &["--trade"], form)?;
    let trade = p.value("--trade").ok_or_else(|| CliError::Usage(format!("`bim {form}`: `--trade` is required")))?;
    if trade != "arch" {
        return Err(CliError::Usage(format!("unknown trade `{}` (known: arch)", trade.to_string_lossy())));
    }
    let project = Project::open(Path::new(p.positional[0]))?;
    let model = Model::compile(&project.doc);
    let text = if p.json { arch_json(&model) } else { arch_text(&model) };
    write!(out, "{text}").map_err(|e| CliError::Input(format!("cannot write to standard output: {e}")))?;
    Ok(0)
}

fn arch_json(model: &Model) -> String {
    let mut s = String::from("{\"rooms\":[],\"walls\":[");
    for (i, w) in model.walls.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"id\":");
        json::write_str(&mut s, &w.name);
        s.push_str(",\"volume\":");
        json::write_num(&mut s, w.volume);
        s.push_str(",\"side_area\":");
        json::write_num(&mut s, w.side_area);
        s.push_str(",\"layers\":[");
        for (k, l) in w.layers.iter().enumerate() {
            if k > 0 {
                s.push(',');
            }
            s.push_str("{\"material\":");
            json::write_str(&mut s, &l.material);
            s.push_str(",\"volume\":");
            json::write_num(&mut s, l.volume);
            s.push('}');
        }
        s.push_str("]}");
    }
    s.push_str("],\"slabs\":[");
    for (i, sl) in model.slabs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"id\":");
        json::write_str(&mut s, &sl.name);
        s.push_str(",\"volume\":");
        json::write_num(&mut s, sl.volume);
        s.push_str(",\"area\":");
        json::write_num(&mut s, sl.area);
        s.push('}');
    }
    s.push_str("],\"stairs\":[],\"curtain_walls\":[]}\n");
    s
}

fn arch_text(model: &Model) -> String {
    let mut s = String::new();
    for w in &model.walls {
        s.push_str(&format!("wall {} volume {} side_area {}\n", w.name, w.volume, w.side_area));
    }
    for sl in &model.slabs {
        s.push_str(&format!("slab {} volume {} area {}\n", sl.name, sl.volume, sl.area));
    }
    s
}

/// `bim export <dir> --format gltf --out <file>`.
pub(crate) fn export(args: &[OsString], _out: &mut dyn Write) -> Outcome {
    let form = "export <dir> --format <format> --out <file>";
    let p = parse(args, &["--format", "--out"], form)?;
    if p.json {
        return Err(CliError::Usage(format!("`bim {form}`: `--json` is not an option of export")));
    }
    let format = p.value("--format").ok_or_else(|| CliError::Usage(format!("`bim {form}`: `--format` is required")))?;
    let file = p.value("--out").ok_or_else(|| CliError::Usage(format!("`bim {form}`: `--out` is required")))?;
    if format != "gltf" {
        return Err(CliError::Usage(format!("unknown export format `{}` (known: gltf)", format.to_string_lossy())));
    }
    let project = Project::open(Path::new(p.positional[0]))?;
    let model = Model::compile(&project.doc);
    let text = bim_model::gltf::to_gltf(&model.elements);
    write_atomically(Path::new(file), text.as_bytes())?;
    Ok(0)
}

/// Writes `bytes` to a temporary file next to `path` and renames it over `path`.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let fail = |e: std::io::Error| CliError::Input(format!("{}: {e}", path.display()));
    let name = path.file_name().ok_or_else(|| CliError::Input(format!("{}: not a file path", path.display())))?;
    let mut tmp_name = OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(fail(e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn run(args: &[&str]) -> (u8, String, String) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = crate::run(&args, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn solve_and_export_a_small_model() {
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
        let (code, out, _) = run(&["solve", d, "--trade", "arch", "--json"]);
        assert_eq!(code, 0);
        let doc = json::parse(&out).unwrap();
        assert!(out.contains("\"id\":\"W\",\"volume\":2.4"), "{out}");
        assert!(out.contains("{\"material\":\"a\",\"volume\":1.2"), "{out}");
        assert!(out.ends_with("\"curtain_walls\":[]}\n") && out.matches('\n').count() == 1, "{out}");
        assert!(matches!(doc, json::Json::Object(_)));
        let gl = tmp.path().join("m.gltf");
        let (code, out, err) = run(&["export", d, "--format", "gltf", "--out", gl.to_str().unwrap()]);
        assert_eq!((code, out.as_str()), (0, ""), "{err}");
        let text = fs::read_to_string(&gl).unwrap();
        assert!(json::parse(&text).is_ok());
        assert!(text.contains("\"extras\":{\"id\":\"W\""));
    }

    #[test]
    fn usage_errors() {
        for args in [
            &["solve"][..],
            &["solve", "d"],
            &["solve", "d", "--trade"],
            &["solve", "d", "--trade", "nope", "--json"],
            &["solve", "d", "e", "--trade", "arch"],
            &["solve", "d", "--trade", "arch", "--bogus"],
            &["export", "d", "--format", "gltf"],
            &["export", "d", "--out", "f"],
            &["export", "d", "--format", "obj", "--out", "f"],
        ] {
            let (code, out, err) = run(args);
            assert_eq!(code, 2, "{args:?}");
            assert_eq!(out, "");
            assert!(err.contains("usage:"), "{args:?}: {err}");
        }
        let (code, _, _) = run(&["solve", "/nonexistent/bim-dir", "--trade", "arch", "--json"]);
        assert_eq!(code, 2);
    }
}
