//! One function per command. Each takes the operands after the command name and the stdout writer
//! and returns the exit code of a command that ran (0, or 1 when it found a problem), or an error.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::ops::{self, LineOp, Located};
use crate::json;
use crate::project::{self, Project};
use crate::CliError;

type Outcome = Result<u8, CliError>;

/// `bim init <dir>`: creates `<dir>/doc.oplog`, empty, unless it exists. Prints nothing.
pub(crate) fn init(args: &[OsString], _out: &mut dyn Write) -> Outcome {
    let [dir] = operands::<1>(args, "init <dir>")?;
    project::init(Path::new(dir))?;
    Ok(0)
}

/// `bim apply <dir> <ops-file>`: appends the file's ops to the journal, all or none. Prints nothing.
///
/// `*.json` files are JSON ops (spec/cli.md), anything else the line format. The whole file is
/// parsed and checked first, then applied to a copy of the live document; only when every op has
/// applied is the journal rewritten. Any failure before that point leaves the journal untouched.
pub(crate) fn apply(args: &[OsString], _out: &mut dyn Write) -> Outcome {
    let [dir, file] = operands::<2>(args, "apply <dir> <ops-file>")?;
    let (dir, file) = (Path::new(dir), Path::new(file));
    let parsed = read_ops_file(file)?;
    let project = Project::open(dir)?;
    let mut scratch = project.doc.clone();
    for FileOp { at, op } in &parsed {
        op.apply_to(&mut scratch).map_err(|e| CliError::Input(format!("{at}: {}", op.describe_error(&e))))?;
    }
    let ops: Vec<LineOp> = parsed.into_iter().map(|file_op| file_op.op).collect();
    project.append(&ops)?;
    Ok(0)
}

/// `bim show <dir> <id> [--json]`: prints the live entity named `<id>` (a ULID or a seed) as
/// `{"id","kind","props"}` (spec/cli.md): `id` is its `_name`, `props` every prop in key order,
/// reserved ones included, with values in their JSON encoding (spec/schema.md) and Refs naming
/// their target. The output is that JSON document with or without `--json`. An id that is not a
/// live entity (unknown or deleted) is an input error (exit 2).
pub(crate) fn show(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let operands_only: Vec<OsString> = args.iter().filter(|a| a.as_os_str() != "--json").cloned().collect();
    let [dir, name] = operands::<2>(&operands_only, "show <dir> <id> --json")?;
    let project = Project::open(Path::new(dir))?;
    let name = name.to_str().ok_or_else(|| CliError::Input(format!("id `{}` is not valid UTF-8", name.to_string_lossy())))?;
    let id = ops::resolve_id(name);
    let entity = project.doc.get(id).ok_or_else(|| CliError::Input(format!("entity `{name}` not found")))?;
    let mut text = String::from("{\"id\":");
    json::write_str(&mut text, &project.name(id));
    text.push_str(",\"kind\":");
    json::write_str(&mut text, &entity.kind);
    text.push_str(",\"props\":{");
    for (i, (key, value)) in entity.props.iter().enumerate() {
        if i > 0 {
            text.push(',');
        }
        json::write_str(&mut text, key);
        text.push(':');
        project.value_json(value, &mut text);
    }
    text.push_str("}}");
    emit(out, &text)?;
    Ok(0)
}

/// `bim hash <dir>`: prints the document's content hash as 16 lowercase hex digits.
pub(crate) fn hash(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let [dir] = operands::<1>(args, "hash <dir>")?;
    let project = Project::open(Path::new(dir))?;
    emit(out, &format!("{:016x}", project.doc.content_hash()))?;
    Ok(0)
}

/// `bim validate <dir>`: prints `dangling <source> <key> <target>` for every dangling ref, naming
/// entities by the seed they were written with. Exit 1 if there is any, else 0 with no output.
pub(crate) fn validate(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let [dir] = operands::<1>(args, "validate <dir>")?;
    let project = Project::open(Path::new(dir))?;
    let dangling = project.dangling();
    for d in &dangling {
        emit(out, &format!("dangling {} {} {}", project.name(d.source), d.key, project.name(d.target)))?;
    }
    Ok(u8::from(!dangling.is_empty()))
}

/// `bim diff <dirA> <dirB>`: prints `same` (exit 0) when the documents' content hashes match, else
/// `differ` (exit 1). Both projects are opened before anything is printed.
pub(crate) fn diff(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let [a, b] = operands::<2>(args, "diff <dirA> <dirB>")?;
    let a = Project::open(Path::new(a))?;
    let b = Project::open(Path::new(b))?;
    if a.doc.content_hash() == b.doc.content_hash() {
        emit(out, "same")?;
        Ok(0)
    } else {
        emit(out, "differ")?;
        Ok(1)
    }
}

/// Exactly `N` operands, else a usage error quoting the command's form.
fn operands<'a, const N: usize>(args: &'a [OsString], form: &str) -> Result<&'a [OsString; N], CliError> {
    <&[OsString; N]>::try_from(args)
        .map_err(|_| CliError::Usage(format!("`bim {form}` takes {N} operand(s), got {}", args.len())))
}

/// Writes one line of result to stdout.
fn emit(out: &mut dyn Write, line: &str) -> Result<(), CliError> {
    writeln!(out, "{line}").map_err(|e| CliError::Input(format!("cannot write to standard output: {e}")))
}

/// An op read from an ops file, with where it came from (`<file>:<line>` or `<file>: ops[<i>]`)
/// for error messages.
#[derive(Debug)]
struct FileOp {
    at: String,
    op: LineOp,
}

/// Reads and parses an ops file, and checks the rules that belong to `bim apply` rather than to the
/// syntax.
fn read_ops_file(file: &Path) -> Result<Vec<FileOp>, CliError> {
    let shown = file.display();
    let bytes = fs::read(file).map_err(|e| CliError::Input(format!("{shown}: {e}")))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| CliError::Input(format!("{shown}: not valid UTF-8")))?;
    // spec/cli.md: `*.json` is the JSON ops format; anything else is the line format.
    let parsed: Vec<FileOp> = if file.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")) {
        ops::parse_json_ops(&text)
            .map_err(|reason| CliError::Input(format!("{shown}: {reason}")))?
            .into_iter()
            .map(|(index, op)| FileOp { at: format!("{shown}: ops[{index}]"), op })
            .collect()
    } else {
        ops::parse_text(&text)
            .map_err(|e| CliError::Input(format!("{shown}:{}: {}", e.line, e.reason)))?
            .into_iter()
            .map(|Located { line, op }| FileOp { at: format!("{shown}:{line}"), op })
            .collect()
    };
    // spec/schema.md: op files reject creates/sets of reserved keys (a leading `_`); bim owns those.
    // A JSON create's props arrive here as sets.
    for FileOp { at, op } in &parsed {
        if let Some(key) = op.key().filter(|key| key.starts_with('_')) {
            return Err(CliError::Input(format!(
                "{at}: key `{key}` is reserved (keys starting with `_` are set by bim itself)"
            )));
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    fn input_error(outcome: Outcome) -> String {
        match outcome {
            Err(CliError::Input(message)) => message,
            other => panic!("wanted an Input error, got {other:?}"),
        }
    }

    #[test]
    fn operands_demands_exact_arity() {
        let args = [os("a"), os("b")];
        assert!(operands::<2>(&args, "x <a> <b>").is_ok());
        for n in [0, 1, 3] {
            let Err(CliError::Usage(message)) = operands::<3>(&args[..n.min(2)], "x <a> <b> <c>") else {
                panic!("arity {n}");
            };
            assert!(message.contains("`bim x <a> <b> <c>` takes 3 operand(s)"), "{message}");
        }
    }

    #[test]
    fn ops_files_with_reserved_keys_are_rejected() {
        let tmp = TempDir::new();
        let file = tmp.path().join("r.ops");
        for key in ["_name", "_schema", "_created_by", "_stamp", "_anything", "_"] {
            fs::write(&file, format!("create a Wall\nset a {key} num 1\n")).unwrap();
            let message = input_error(read_ops_file(&file).map(|_| 0));
            assert!(message.contains(":2: key `") && message.contains("is reserved"), "{message}");
        }
        fs::write(&file, "create a Wall\nset a name_ num 1\nset a n_ame num 1\n").unwrap();
        assert_eq!(read_ops_file(&file).unwrap().len(), 3, "only a leading underscore is reserved");
    }

    #[test]
    fn json_ops_files_are_read_by_extension() {
        let tmp = TempDir::new();
        for name in ["ops.json", "OPS.JSON"] {
            let file = tmp.path().join(name);
            fs::write(&file, r#"{"schema": 1, "ops": [{"op": "create", "id": "a", "kind": "W", "props": {"h": 1}}]}"#)
                .unwrap();
            assert_eq!(read_ops_file(&file).unwrap().len(), 2, "create + one prop set");
        }
        // The same text under another extension is the line format.
        let file = tmp.path().join("ops.txt");
        fs::write(&file, r#"{"schema": 1, "ops": []}"#).unwrap();
        assert!(input_error(read_ops_file(&file).map(|_| 0)).contains(":1: missing member `op`"));
    }

    #[test]
    fn json_ops_files_with_reserved_keys_are_rejected() {
        let tmp = TempDir::new();
        let file = tmp.path().join("r.json");
        for op in [
            r#"{"op": "create", "id": "b", "kind": "W", "props": {"ok": 1, "_name": "x"}}"#,
            r#"{"op": "set", "id": "a", "key": "_schema", "value": 2}"#,
        ] {
            fs::write(&file, format!(r#"{{"schema": 1, "ops": [{{"op": "create", "id": "a", "kind": "W"}}, {op}]}}"#)).unwrap();
            let message = input_error(read_ops_file(&file).map(|_| 0));
            assert!(message.contains(": ops[1]: key `_") && message.contains("is reserved"), "{message}");
        }
    }

    #[test]
    fn json_errors_name_the_file_and_op() {
        let tmp = TempDir::new();
        let file = tmp.path().join("bad.json");
        fs::write(&file, r#"{"schema": 1, "ops": [{"op": "delete", "id": "a"}, {"op": "set", "id": "a", "key": "k", "value": null}]}"#).unwrap();
        let message = input_error(read_ops_file(&file).map(|_| 0));
        assert!(message.starts_with(&format!("{}: ops[1]: `value`: null", file.display())), "{message}");
    }

    #[test]
    fn ops_file_errors_name_the_file_and_line() {
        let tmp = TempDir::new();
        let file = tmp.path().join("bad.ops");
        fs::write(&file, "create a Wall\n\nbogus op here\n").unwrap();
        let message = input_error(read_ops_file(&file).map(|_| 0));
        assert_eq!(
            message,
            format!("{}:3: unknown op `bogus` (expected create, set or delete)", file.display())
        );
        let missing = tmp.path().join("missing.ops");
        assert!(input_error(read_ops_file(&missing).map(|_| 0)).starts_with(&missing.display().to_string()));
        fs::write(&file, [b'c', 0xff, 0xfe]).unwrap();
        assert!(input_error(read_ops_file(&file).map(|_| 0)).ends_with("not valid UTF-8"));
    }
}
