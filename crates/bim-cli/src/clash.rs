//! `bim clash <dir> [--clearance <m>] [--json]`: the hard and soft clashes between the elements of
//! the compiled model ([`bim_model::clash`], where the rules are: hard = the solids share volume,
//! soft = not hard and closer than the clearance, joined walls excluded).
//!
//! `--clearance` is a length in metres, finite and not negative (default
//! [`bim_model::clash::DEFAULT_CLEARANCE`], no soft clashes); anything else is a usage error
//! (exit 2) found before the project is opened. `--json` prints one document,
//! `{"hard":[{"a","b","volume"}],"soft":[{"a","b","distance"}],"matrix":{"<KindA>":{"<KindB>":count}}}`
//! (spec/cli.md): elements by name with `a` before `b`, sorted by `(a, b)`, `matrix` counting the
//! hard clashes by unordered kind pair (each pair written once, the kind that sorts first as the
//! outer key). Without `--json` there is one line per clash, `hard <a> <b> <volume>` and
//! `soft <a> <b> <distance>`. Exit 1 if there is any hard clash, else 0 (soft clashes alone do not
//! fail the command).

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use bim_model::clash::{clash, ClashReport, DEFAULT_CLEARANCE};
use bim_model::Model;

use crate::commands::emit;
use crate::geometry::parse;
use crate::json;
use crate::project::Project;
use crate::CliError;

type Outcome = Result<u8, CliError>;

/// `bim clash <dir> [--clearance <m>] [--json]`.
pub(crate) fn clash_command(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let form = "clash <dir> [--clearance <m>] --json";
    let p = parse(args, &["--clearance"], form)?;
    let clearance = match p.value("--clearance") {
        None => DEFAULT_CLEARANCE,
        Some(value) => value.to_str().and_then(|s| s.parse::<f64>().ok()).filter(|x| x.is_finite() && *x >= 0.0).ok_or_else(|| {
            CliError::Usage(format!(
                "`bim {form}`: `--clearance` must be a non-negative number of metres, got `{}`",
                value.to_string_lossy()
            ))
        })?,
    };
    let project = Project::open(Path::new(p.positional[0]))?;
    let model = Model::compile(&project.doc);
    let report = clash(&project.doc, &model, clearance);
    if p.json {
        emit(out, &report_json(&report))?;
    } else {
        for c in &report.hard {
            emit(out, &format!("hard {} {} {}", c.a, c.b, c.volume))?;
        }
        for c in &report.soft {
            emit(out, &format!("soft {} {} {}", c.a, c.b, c.distance))?;
        }
    }
    Ok(u8::from(!report.hard.is_empty()))
}

/// The JSON document of `report`.
fn report_json(report: &ClashReport) -> String {
    let mut s = String::from("{\"hard\":[");
    for (i, c) in report.hard.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"a\":");
        json::write_str(&mut s, &c.a);
        s.push_str(",\"b\":");
        json::write_str(&mut s, &c.b);
        s.push_str(",\"volume\":");
        json::write_num(&mut s, c.volume);
        s.push('}');
    }
    s.push_str("],\"soft\":[");
    for (i, c) in report.soft.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"a\":");
        json::write_str(&mut s, &c.a);
        s.push_str(",\"b\":");
        json::write_str(&mut s, &c.b);
        s.push_str(",\"distance\":");
        json::write_num(&mut s, c.distance);
        s.push('}');
    }
    s.push_str("],\"matrix\":{");
    for (i, (kind, row)) in report.matrix.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        json::write_str(&mut s, kind);
        s.push_str(":{");
        for (k, (other, count)) in row.iter().enumerate() {
            if k > 0 {
                s.push(',');
            }
            json::write_str(&mut s, other);
            s.push(':');
            s.push_str(&count.to_string());
        }
        s.push('}');
    }
    s.push_str("}}");
    s
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

    /// A project with levels and three columns: A and B overlap, C stands 0.04 from B.
    fn project(tmp: &TempDir) -> String {
        let dir = tmp.path().join("p");
        let d = dir.to_str().unwrap().to_string();
        assert_eq!(run(&["init", &d]).0, 0);
        let ops = tmp.path().join("m.json");
        let column = |id: &str, x: f64| {
            format!(
                r#"{{"op":"create","id":"{id}","kind":"Column","props":{{"x":{x},"y":0,"base":{{"ref":"L0"}},"top":{{"ref":"L1"}},"section_b":1,"section_h":1}}}}"#
            )
        };
        fs::write(
            &ops,
            format!(
                r#"{{"schema":1,"ops":[
                {{"op":"create","id":"L0","kind":"Level","props":{{"elevation":0}}}},
                {{"op":"create","id":"L1","kind":"Level","props":{{"elevation":3}}}},
                {},{},{}]}}"#,
                column("A", 0.0),
                column("B", 0.5),
                column("C", 1.54)
            ),
        )
        .unwrap();
        assert_eq!(run(&["apply", &d, ops.to_str().unwrap()]).0, 0);
        d
    }

    #[test]
    fn json_report_and_exit_code() {
        let tmp = TempDir::new();
        let d = project(&tmp);
        let (code, out, err) = run(&["clash", &d, "--json"]);
        assert_eq!((code, err.as_str()), (1, ""), "{out}");
        // A and B share 0.5 × 1 × 3 m³ (to rounding).
        let (head, rest) = out.split_once("\"volume\":").unwrap();
        assert_eq!(head, "{\"hard\":[{\"a\":\"A\",\"b\":\"B\",");
        let (volume, tail) = rest.split_once('}').unwrap();
        assert!((volume.parse::<f64>().unwrap() - 1.5).abs() < 1e-14, "{out}");
        assert_eq!(tail, "],\"soft\":[],\"matrix\":{\"Column\":{\"Column\":1}}}\n");
        // With a clearance: B and C are 0.04 apart (the volume is exactly 0.5 × 1 × 3).
        let (code, out, _) = run(&["clash", &d, "--json", "--clearance", "0.05"]);
        assert_eq!(code, 1);
        let doc = json::parse(&out).unwrap();
        let json::Json::Object(fields) = doc else { panic!("{out}") };
        assert_eq!(fields.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["hard", "soft", "matrix"]);
        assert!(out.contains("\"soft\":[{\"a\":\"B\",\"b\":\"C\",\"distance\":0.04"), "{out}");
        assert!(out.ends_with("\"matrix\":{\"Column\":{\"Column\":1}}}\n") && out.matches('\n').count() == 1, "{out}");
        // A tighter clearance drops it.
        let (_, out, _) = run(&["clash", &d, "--clearance", "0.035", "--json"]);
        assert!(out.contains("\"soft\":[]"), "{out}");
        // Soft clashes alone do not fail the command; neither does nothing at all.
        let ops = tmp.path().join("fix.json");
        fs::write(&ops, r#"{"schema":1,"ops":[{"op":"delete","id":"A"}]}"#).unwrap();
        assert_eq!(run(&["apply", &d, ops.to_str().unwrap()]).0, 0);
        let (code, out, _) = run(&["clash", &d, "--clearance", "0.05", "--json"]);
        assert_eq!(code, 0);
        assert!(out.starts_with("{\"hard\":[],\"soft\":[{\"a\":\"B\",\"b\":\"C\""), "{out}");
        assert!(out.ends_with("\"matrix\":{}}\n"), "{out}");
        let (code, out, _) = run(&["clash", &d, "--json"]);
        assert_eq!((code, out.as_str()), (0, "{\"hard\":[],\"soft\":[],\"matrix\":{}}\n"));
    }

    #[test]
    fn text_form_has_one_line_per_clash() {
        let tmp = TempDir::new();
        let d = project(&tmp);
        let (code, out, _) = run(&["clash", &d, "--clearance", "0.05"]);
        assert_eq!(code, 1);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out}");
        let (head, volume) = lines[0].rsplit_once(' ').unwrap();
        assert_eq!(head, "hard A B");
        assert!((volume.parse::<f64>().unwrap() - 1.5).abs() < 1e-14, "{out}");
        assert!(lines[1].starts_with("soft B C 0.04"), "{out}");
    }

    #[test]
    fn a_bad_clearance_is_a_usage_error_before_the_project_is_opened() {
        for bad in ["x", "-1", "-0.001", "nan", "inf", "-inf", "", "1,5", "0.05m"] {
            let (code, out, err) = run(&["clash", "/nonexistent/bim-dir", "--clearance", bad, "--json"]);
            assert_eq!((code, out.as_str()), (2, ""), "{bad:?}");
            assert!(err.contains("usage:") && err.contains("--clearance"), "{bad:?}: {err}");
        }
        for ok in ["0", "0.0", "1e-2", ".5", "+0.5"] {
            // A valid clearance gets as far as opening the project.
            let (code, out, err) = run(&["clash", "/nonexistent/bim-dir", "--clearance", ok]);
            assert_eq!((code, out.as_str()), (2, ""), "{ok}");
            assert!(err.contains("no such directory") && !err.contains("usage:"), "{ok}: {err}");
        }
        let (code, _, err) = run(&["clash"]);
        assert_eq!(code, 2);
        assert!(err.contains("usage:"));
    }
}
