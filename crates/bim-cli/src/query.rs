//! `bim query <dir> <program-file> <atom> [--json] [--naive]`: evaluates the Datalog program in
//! `<program-file>` (spec/mcp.md "Query language (Datalog)") over the built-in relations of the
//! project's document ([`bim_query::DocumentEdb`]) and prints the rows answering `<atom>`, sorted
//! and deduplicated exactly as the MCP `query` tool's: with `--json` as `{"rows": [[…], …]}` (one
//! value per atom argument in order; entities as their `_name`, strings as JSON strings, numbers
//! as JSON numbers, booleans as JSON booleans), without it one line per row, values separated by a
//! tab. `--naive` evaluates by naive bottom-up iteration (the reference mode) instead of
//! semi-naive; the rows are the same. A program or atom that does not parse, breaks a static rule
//! (arity, safety, a rule for a built-in) or cannot be stratified exits 2 with the reason on
//! stderr; so does an unreadable program file. An engine failure while deriving a built-in exits
//! with the engine error's code.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;

use bim_query::{DocumentEdb, Mode, QueryError, Val};

use crate::commands::emit;
use crate::json;
use crate::project::Project;
use crate::CliError;

/// `bim query <dir> <program-file> <atom> [--json] [--naive]`.
pub(crate) fn query(args: &[OsString], out: &mut dyn Write) -> Result<u8, CliError> {
    let form = "query <dir> <program-file> <atom> [--json] [--naive]";
    let usage = |what: String| CliError::Usage(format!("`bim {form}`: {what}"));
    let (mut as_json, mut naive) = (false, false);
    let mut positional = Vec::new();
    for arg in args {
        let flag = match arg.to_str() {
            Some("--json") => &mut as_json,
            Some("--naive") => &mut naive,
            Some(s) if s.starts_with("--") => return Err(usage(format!("unknown option `{s}`"))),
            _ => {
                positional.push(arg);
                continue;
            }
        };
        if *flag {
            return Err(usage(format!("`{}` given twice", arg.to_string_lossy())));
        }
        *flag = true;
    }
    let [dir, file, atom] = positional[..] else {
        return Err(usage(format!("takes 3 operands, got {}", positional.len())));
    };
    let atom = atom.to_str().ok_or_else(|| CliError::Input("the query atom is not valid UTF-8".into()))?;
    let file = Path::new(file);
    let bytes = fs::read(file).map_err(|e| CliError::Input(format!("{}: {e}", file.display())))?;
    let program = String::from_utf8(bytes).map_err(|_| CliError::Input(format!("{}: not valid UTF-8", file.display())))?;

    let program = bim_query::parse_program(&program).map_err(query_error)?;
    let atom = bim_query::parse_atom(atom).map_err(query_error)?;
    let checked = bim_query::check(&program, &atom).map_err(query_error)?;
    let project = Project::open(Path::new(dir))?;
    let ctx = project.engine_context();
    let edb = DocumentEdb::new(&ctx);
    let mode = if naive { Mode::Naive } else { Mode::SemiNaive };
    let rows = bim_query::run(&checked, &edb, mode).map_err(query_error)?;

    if as_json {
        let mut s = String::from("{\"rows\":[");
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('[');
            for (j, v) in row.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                match v {
                    Val::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
                    Val::Num(x) => json::write_num(&mut s, *x),
                    Val::Str(t) => json::write_str(&mut s, t),
                }
            }
            s.push(']');
        }
        s.push_str("]}");
        emit(out, &s)?;
    } else {
        for row in &rows {
            let line: Vec<String> = row.iter().map(Val::to_string).collect();
            emit(out, &line.join("\t"))?;
        }
    }
    Ok(0)
}

fn query_error(e: QueryError) -> CliError {
    match e {
        QueryError::Program(m) => CliError::Input(m),
        QueryError::Engine(e) => CliError::Engine(e),
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil::TempDir;

    fn run(args: &[&str]) -> (u8, String, String) {
        let args: Vec<std::ffi::OsString> = args.iter().map(std::ffi::OsString::from).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = crate::run(&args, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn rows_flags_and_errors() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("p");
        let dir = dir.to_str().unwrap();
        assert_eq!(run(&["init", dir]).0, 0);
        let ops = tmp.path().join("ops.json");
        std::fs::write(
            &ops,
            r#"{"schema":1,"ops":[{"op":"create","id":"L1","kind":"Level","props":{"elevation":3}},
                {"op":"create","id":"I1","kind":"Issue","props":{"title":"t","status":"open","entities":[{"ref":"L1"}]}}]}"#,
        )
        .unwrap();
        assert_eq!(run(&["apply", dir, ops.to_str().unwrap()]).0, 0);
        let prog = tmp.path().join("p.dl");
        std::fs::write(&prog, "r(X, Y) :- ref(X, \"entities\", Y).\ne(X, V) :- prop(X, \"elevation\", V).\n").unwrap();
        let prog = prog.to_str().unwrap();

        let (code, out, _) = run(&["query", dir, prog, "r(X, Y)", "--json"]);
        assert_eq!((code, out.as_str()), (0, "{\"rows\":[[\"I1\",\"L1\"]]}\n"));
        let (code, out, _) = run(&["query", "--naive", dir, prog, "e(X, 3)", "--json"]);
        assert_eq!((code, out.as_str()), (0, "{\"rows\":[[\"L1\",3.0]]}\n"));
        let (code, out, _) = run(&["query", dir, prog, "entity(X, \"Issue\")"]);
        assert_eq!((code, out.as_str()), (0, "I1\tIssue\n"));

        let bad = tmp.path().join("bad.dl");
        for text in ["p(X) :- q(X.\n", "q(1). p(X) :- q(X), not p(X).\n"] {
            std::fs::write(&bad, text).unwrap();
            let (code, out, err) = run(&["query", dir, bad.to_str().unwrap(), "p(X)", "--json"]);
            assert_eq!((code, out.as_str()), (2, ""), "{text}");
            assert!(err.starts_with("bim: program"), "{err}");
        }
        for args in [&["query", dir, prog][..], &["query", dir, prog, "p(X)", "--json", "--json"], &["query", dir, prog, "p(X)", "--fast"]] {
            assert_eq!(run(args).0, 2, "{args:?}");
        }
        assert_eq!(run(&["query", dir, prog, "p(X"]).0, 2);
        assert_eq!(run(&["query", dir, "/nonexistent/prog.dl", "p(X)"]).0, 2);
    }
}
