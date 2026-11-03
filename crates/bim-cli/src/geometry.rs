//! The compiled building geometry ([`bim_model`]) and the commands over it:
//!
//! * The `arch` trade's quantities (`bim solve --trade arch`) are computed by `bim_arch::ArchEngine`,
//!   registered by [`crate::engines`]; this module holds the shared option parser ([`parse`]).
//! * `bim export <dir> --format gltf --out <file>` writes the glTF 2.0 document of
//!   [`bim_model::gltf`]: one mesh node per element with `extras.id`.
//! * `bim export <dir> --format ifc --out <file>` writes the IFC4 file of [`bim_ifc::export`].
//! * `bim export <dir> --format svg --view <view> --out <file>` writes the drawing of
//!   [`bim_draw`] (plan, section or elevation, spec/cli.md); an unknown or unparseable view is a
//!   usage error (exit 2), as is `--view` with another format or svg without it.
//!   Any other format is a usage error (exit 2). The file is written to a temporary file next to it and renamed into place.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;

use bim_model::Model;

use crate::project::Project;
use crate::CliError;

type Outcome = Result<u8, CliError>;

/// Operands and `--flag value` / `--json` options, in any order.
pub(crate) struct Parsed<'a> {
    pub(crate) positional: Vec<&'a OsString>,
    values: Vec<(&'static str, &'a OsString)>,
    pub(crate) json: bool,
}

pub(crate) fn parse<'a>(args: &'a [OsString], valued: &[&'static str], form: &str) -> Result<Parsed<'a>, CliError> {
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
    pub(crate) fn value(&self, flag: &str) -> Option<&OsString> {
        self.values.iter().find(|(f, _)| *f == flag).map(|(_, v)| *v)
    }
}

/// `bim export <dir> --format gltf|ifc --out <file>` and
/// `bim export <dir> --format svg --view <view> --out <file>`.
pub(crate) fn export(args: &[OsString], _out: &mut dyn Write) -> Outcome {
    let form = "export <dir> --format <format> [--view <view>] --out <file>";
    let p = parse(args, &["--format", "--out", "--view"], form)?;
    if p.json {
        return Err(CliError::Usage(format!("`bim {form}`: `--json` is not an option of export")));
    }
    let format = p.value("--format").ok_or_else(|| CliError::Usage(format!("`bim {form}`: `--format` is required")))?;
    let file = p.value("--out").ok_or_else(|| CliError::Usage(format!("`bim {form}`: `--out` is required")))?;
    if format != "gltf" && format != "ifc" && format != "svg" {
        return Err(CliError::Usage(format!("unknown export format `{}` (known: gltf, ifc, svg)", format.to_string_lossy())));
    }
    let view = p.value("--view");
    if format == "svg" && view.is_none() {
        return Err(CliError::Usage(format!("`bim {form}`: `--view` is required for a drawing format")));
    }
    if format != "svg" && view.is_some() {
        return Err(CliError::Usage(format!("`bim {form}`: `--view` applies to drawing formats only")));
    }
    let project = Project::open(Path::new(p.positional[0]))?;
    let text = if format == "ifc" {
        let name = Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = bim_ifc::export::export_ifc4_string(&project.doc, &name);
        if text.is_empty() {
            return Err(CliError::Input(format!("{}: the model has values IFC cannot represent", Path::new(file).display())));
        }
        text
    } else if let Some(view) = view {
        let view = view.to_str().ok_or_else(|| CliError::Usage(format!("view `{}` is not valid UTF-8", view.to_string_lossy())))?;
        let view = bim_draw::View::parse(&project.doc, view).map_err(|e| CliError::Usage(e.to_string()))?;
        let drawing = bim_draw::draw(&project.doc, &Model::compile(&project.doc), &view);
        bim_draw::svg::to_svg(&drawing)
    } else {
        bim_model::gltf::to_gltf(&Model::compile(&project.doc).elements)
    };
    write_atomically(Path::new(file), text.as_bytes())?;
    Ok(0)
}

/// Writes `bytes` to a temporary file next to `path` and renames it over `path`.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
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
    use crate::json;
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
        let ifc = tmp.path().join("m.ifc");
        let (code, out, err) = run(&["export", d, "--format", "ifc", "--out", ifc.to_str().unwrap()]);
        assert_eq!((code, out.as_str()), (0, ""), "{err}");
        let text = fs::read_to_string(&ifc).unwrap();
        assert!(text.starts_with("ISO-10303-21;") && text.contains("FILE_SCHEMA(('IFC4'));"), "{text}");
        let wall = bim_ifc::export::entity_guid(bim_doc::EntityId::derive("W"));
        assert!(text.contains(&format!("IFCWALL('{wall}'")), "{text}");
        assert!(text.contains("IFCSLAB(") && text.contains("'Pset_WallCommon'"));
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
            &["export", "d", "--format", "svg", "--out", "f"],
            &["export", "d", "--format", "gltf", "--view", "elevation:s", "--out", "f"],
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
