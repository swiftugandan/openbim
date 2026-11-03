//! One function per command. Each takes the operands after the command name and the stdout writer
//! and returns the exit code of a command that ran (0, or 1 when it found a problem), or an error.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;

use bim_doc::Document;
use bim_schema::{KindRegistry, SchemaError};
use bim_store::{store_path, Store, STORE_FILE};
use bim_sync::SyncDb;

use crate::ops::{self, LineOp, Located};
use crate::json;
use crate::project::{self, Project};
use crate::store::{existing_store, store_error};
use crate::sync;
use crate::CliError;

type Outcome = Result<u8, CliError>;

/// `bim init <dir>`: creates `<dir>/doc.oplog`, empty, unless it exists, and `<dir>/bim.sqlite`,
/// an empty store, unless it exists. The store also records the project's CRDT peer id, a random
/// 32-bit number drawn once (a copy of the directory shares it). Records no revision. Prints
/// nothing. Running it again on a set-up project writes nothing.
pub(crate) fn init(args: &[OsString], _out: &mut dyn Write) -> Outcome {
    let [dir] = operands::<1>(args, "init <dir>")?;
    let dir = Path::new(dir);
    // Refuse before creating anything if something that is not a file stands where the store goes.
    let store = dir.join(STORE_FILE);
    if store.exists() && !store.is_file() {
        return Err(CliError::Input(format!("{}: exists and is not a file", store.display())));
    }
    project::init(dir)?;
    Store::open_or_create(dir).map_err(|e| store_error(dir, e))?;
    let unusable = |e| CliError::Input(format!("{}: {e}", store.display()));
    SyncDb::open_or_create(&store_path(dir)).map_err(unusable)?.ensure_peer().map_err(unusable)?;
    Ok(0)
}

/// `bim sync serve <dir> --port <p>` serves the project over WebSocket (it prints `listening
/// <port>` first and runs until killed); `bim sync <dir> <ws-url>` exchanges state with such a
/// server until both sides hold the same edits, prints nothing and exits 0. If the server cannot
/// be reached, or anything else goes wrong, the command exits 2 and `<dir>` is as it was (see
/// [`crate::sync`]).
///
/// The serve form is the one that has `--port`: `serve`, `<dir>` and `--port <p>` with `<p>` a port
/// number from 0 to 65535 (0 picks a free one), the flag before or after the directory.
pub(crate) fn sync(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let usage = |what: &str| CliError::Usage(format!("`bim sync serve <dir> --port <p>` or `bim sync <dir> <ws-url>`: {what}"));
    if args.iter().any(|arg| arg == "--port") {
        let (mut dir, mut port, mut serve) = (None, None, false);
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            if arg == "--port" {
                let value = rest.next().ok_or_else(|| usage("`--port` needs a port number"))?;
                let value = value.to_str().and_then(|text| text.parse::<u16>().ok());
                let value = value.ok_or_else(|| usage("the port must be a number from 0 to 65535"))?;
                if port.replace(value).is_some() {
                    return Err(usage("`--port` given twice"));
                }
            } else if arg == "serve" && !serve {
                serve = true;
            } else if dir.replace(arg).is_some() {
                return Err(usage("takes one directory"));
            }
        }
        return match (serve, dir, port) {
            (true, Some(dir), Some(port)) => sync::serve(Path::new(dir), port, out),
            _ => Err(usage("`--port` goes with `serve <dir>`")),
        };
    }
    let [dir, url] = operands::<2>(args, "sync <dir> <ws-url>")?;
    let url = url.to_str().ok_or_else(|| CliError::Usage(format!("url `{}` is not valid UTF-8", url.to_string_lossy())))?;
    bim_sync::parse_ws_url(url).map_err(|e| CliError::Usage(e.to_string()))?;
    sync::client(Path::new(dir), url)
}

/// `bim apply <dir> <ops-file>`: appends the file's ops to the journal and the store, all or none.
/// Prints nothing.
///
/// `*.json` files are JSON ops (spec/cli.md), anything else the line format. The whole file is
/// parsed and checked first, then applied to a copy of the live document; only when every op has
/// applied are the journal and the store written, in one SQLite transaction
/// ([`Project::apply_ops`]). Any failure before that point leaves both untouched, and a file with
/// no ops does not even open the store.
///
/// Each op is also checked against the project's kind registry (core kinds plus the trade packages
/// in `<dir>/plugins`; spec/schema.md, Extension kinds), see [`check_kinds`].
pub(crate) fn apply(args: &[OsString], _out: &mut dyn Write) -> Outcome {
    let [dir, file] = operands::<2>(args, "apply <dir> <ops-file>")?;
    let (dir, file) = (Path::new(dir), Path::new(file));
    let parsed = read_ops_file(file)?;
    let project = Project::open(dir)?;
    let mut scratch = project.doc.clone();
    for FileOp { at, op } in &parsed {
        check_kinds(&project.kinds, &scratch, op).map_err(|e| CliError::Input(format!("{at}: {e}")))?;
        op.apply_to(&mut scratch).map_err(|e| CliError::Input(format!("{at}: {}", op.describe_error(&e))))?;
    }
    let ops: Vec<LineOp> = parsed.into_iter().map(|file_op| file_op.op).collect();
    project.apply_ops(&ops, &scratch)?;
    Ok(0)
}

/// `bim sketch <dir> <file.json>`: applies the direct-modelling commands of the file
/// (spec/sketch.md) through `bim_sketch::apply`, which turns the whole file into document ops or
/// rejects it (exit 2, nothing changed): a malformed file, an unknown or unimplemented op, a
/// command naming a face, edge or vertex the Shape does not have, a field constraint broken. The
/// ops are journalled like those of `bim apply`, naming each Shape by the name the file gave it.
pub(crate) fn sketch(args: &[OsString], _out: &mut dyn Write) -> Outcome {
    let [dir, file] = operands::<2>(args, "sketch <dir> <file.json>")?;
    let (dir, file) = (Path::new(dir), Path::new(file));
    let shown = file.display();
    let bytes = fs::read(file).map_err(|e| CliError::Input(format!("{shown}: {e}")))?;
    let text = String::from_utf8(bytes).map_err(|_| CliError::Input(format!("{shown}: not valid UTF-8")))?;
    let project = Project::open(dir)?;
    let outcome = bim_sketch::apply(&project.doc, &text).map_err(|e| CliError::Input(format!("{shown}: {e}")))?;
    let name = |id: bim_doc::EntityId| -> String {
        outcome
            .names
            .get(&id)
            .cloned()
            .or_else(|| project.written_name(id).map(str::to_string))
            .unwrap_or_else(|| id.to_ulid_string())
    };
    let ops = LineOp::from_doc_ops(&outcome.ops, &name);
    let mut scratch = project.doc.clone();
    for op in &ops {
        op.apply_to(&mut scratch).map_err(|e| CliError::Input(format!("{shown}: {}", op.describe_error(&e))))?;
    }
    project.apply_ops(&ops, &scratch)?;
    Ok(0)
}

/// `bim import <dir> <file.ifc|file.json>`: adds the building elements of an IFC4 or IFC2X3 file
/// (`bim_ifc::import`), or the entities of an ifcJSON file (`*.json`, `*.ifcjson`;
/// `bim_ifc::ifcjson`), as ops, all or none. Each imported entity is created with its id's ULID (or
/// the import's seed for derived entities) as its name; an entity the project already has (an
/// earlier import of the same objects) is updated with a `set` per prop whose value differs. A
/// file that does not parse (truncated, malformed), a JSON file that is not ifcJSON, a FILE_SCHEMA
/// other than IFC4 / IFC2X3 (e.g. IFC4X3), a reference to an undefined instance under a product's
/// placement or representation, or an id the project holds as another kind exit 2 with nothing
/// changed: the file is read completely, and the ops checked against a copy of the document, before
/// anything is written. What the import skips or approximates is reported on stderr as
/// `bim: warning: …` lines.
pub(crate) fn import(args: &[OsString], _out: &mut dyn Write) -> Outcome {
    let [dir, file] = operands::<2>(args, "import <dir> <file.ifc|file.json>")?;
    let (dir, file) = (Path::new(dir), Path::new(file));
    let shown = file.display();
    let bytes = fs::read(file).map_err(|e| CliError::Input(format!("{shown}: {e}")))?;
    let imported = if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("json") || e.eq_ignore_ascii_case("ifcjson")) {
        let text = String::from_utf8(bytes).map_err(|_| CliError::Input(format!("{shown}: not valid UTF-8")))?;
        bim_ifc::ifcjson::import_ifcjson(&text).map_err(|e| CliError::Input(format!("{shown}: {e}")))?
    } else {
        let step = bim_ifc::StepFile::read(&bytes[..]).map_err(|e| CliError::Input(format!("{shown}: {e}")))?;
        bim_ifc::import::import_ifc(&step).map_err(|e| CliError::Input(format!("{shown}: {e}")))?
    };
    let project = Project::open(dir)?;
    let seeds: std::collections::HashMap<bim_doc::EntityId, &str> =
        imported.entities.iter().map(|e| (e.id, e.seed.as_str())).collect();
    let name = |id: bim_doc::EntityId| -> String {
        seeds
            .get(&id)
            .map(|s| s.to_string())
            .or_else(|| project.written_name(id).map(str::to_string))
            .unwrap_or_else(|| id.to_ulid_string())
    };
    let mut scratch = project.doc.clone();
    let mut ops_out: Vec<LineOp> = Vec::new();
    for ent in &imported.entities {
        let mut ops = Vec::new();
        let existing = match scratch.get(ent.id) {
            Some(e) if e.kind != ent.kind => {
                return Err(CliError::Input(format!(
                    "{shown}: {} cannot be imported as a {}: the project holds it as a {}",
                    ent.seed, ent.kind, e.kind
                )))
            }
            Some(e) => Some(e.props.clone()),
            None => {
                ops.push(LineOp::Create { seed: ent.seed.clone(), kind: ent.kind.clone() });
                None
            }
        };
        for (key, value) in &ent.props {
            if existing.as_ref().and_then(|p| p.get(key)) == Some(value) {
                continue;
            }
            ops.push(LineOp::Set { seed: ent.seed.clone(), key: key.clone(), value: ops::LineValue::from_value(value, &name) });
        }
        for op in ops {
            check_kinds(&project.kinds, &scratch, &op).map_err(|e| CliError::Input(format!("{shown}: {e}")))?;
            op.apply_to(&mut scratch).map_err(|e| CliError::Input(format!("{shown}: {}", op.describe_error(&e))))?;
            ops_out.push(op);
        }
    }
    project.apply_ops(&ops_out, &scratch)?;
    for w in &imported.warnings {
        eprintln!("bim: warning: {shown}: {w}");
    }
    Ok(0)
}

/// The kind rules of spec/schema.md (Extension kinds) for one op, checked against `doc` as it
/// stands when the op is reached (so an entity created earlier in the same file is known):
///
/// * a `create` needs a kind that is a core kind or declared by a loaded package;
/// * a `set` on an entity of an extension kind needs a declared prop of that type. (A JSON create's
///   `props` arrive here as one `set` each, so the create and each of its props are checked in order.)
///
/// A `delete` is not checked. A `set` on an entity that does not exist is left for the document to
/// refuse (``entity `x` not found``), and so is a `set` on an entity whose kind the registry no
/// longer knows (its package was removed): there is nothing to check it against.
fn check_kinds(kinds: &KindRegistry, doc: &Document, op: &LineOp) -> Result<(), SchemaError> {
    match op {
        LineOp::Create { kind, .. } => kinds.check_create(kind, &[]),
        LineOp::Set { seed, key, value } => match doc.get(ops::resolve_id(seed)) {
            Some(entity) if kinds.kind(&entity.kind).is_some() => kinds.check_set(&entity.kind, key, &value.to_value()),
            _ => Ok(()),
        },
        LineOp::Delete { .. } => Ok(()),
    }
}

/// `bim plugin list <dir> [--json]`: prints the project's trade packages and the core kinds as one
/// JSON document (spec/cli.md): `{"packages":[{"name","version","abi","kinds","engine"}],
/// "core_kinds":[…]}`. Packages are sorted by name and so are their kinds; `engine` is the component
/// path relative to `<dir>` (`plugins/<package>/<engine>`), or `null` for a package without one (the
/// file is not looked at). The output is the same with or without `--json`. A malformed or
/// inconsistent package is an input error (exit 2) naming its `trade.json`. The project need not
/// have a journal: only `<dir>` and its `plugins` are read.
pub(crate) fn plugin(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let operands_only: Vec<OsString> = args.iter().filter(|a| a.as_os_str() != "--json").cloned().collect();
    let [subcommand, dir] = operands::<2>(&operands_only, "plugin list <dir> --json")?;
    if subcommand != "list" {
        return Err(CliError::Usage(format!(
            "unknown plugin command `{}` (expected `list`)",
            subcommand.to_string_lossy()
        )));
    }
    let dir = Path::new(dir);
    project::check_dir(dir)?;
    let kinds = project::load_kinds(dir)?;
    let mut text = String::from("{\"packages\":[");
    for (i, package) in kinds.packages().iter().enumerate() {
        if i > 0 {
            text.push(',');
        }
        text.push_str("{\"name\":");
        json::write_str(&mut text, &package.name);
        text.push_str(",\"version\":");
        json::write_str(&mut text, &package.version);
        text.push_str(",\"abi\":");
        json::write_str(&mut text, &package.abi);
        text.push_str(",\"kinds\":");
        write_strings(&mut text, package.kinds.iter().map(String::as_str));
        text.push_str(",\"engine\":");
        match &package.engine {
            Some(engine) => json::write_str(&mut text, &format!("plugins/{}/{engine}", package.name)),
            None => text.push_str("null"),
        }
        text.push('}');
    }
    text.push_str("],\"core_kinds\":");
    write_strings(&mut text, bim_schema::CORE_KINDS);
    text.push('}');
    emit(out, &text)?;
    Ok(0)
}

/// Appends a JSON array of strings.
fn write_strings<'a>(out: &mut String, items: impl IntoIterator<Item = &'a str>) {
    out.push('[');
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json::write_str(out, item);
    }
    out.push(']');
}

/// `bim show <dir> <id> [--json]`: prints the live entity named `<id>` (a ULID or a seed) as
/// `{"id","kind","props"}` (spec/cli.md): `id` is its `_name`, `props` every prop in key order,
/// reserved ones included, with values in their JSON encoding (spec/schema.md) and Refs naming
/// their target. The output is that JSON document with or without `--json`. An id that is not a
/// live entity (unknown or deleted) is an input error (exit 2).
pub(crate) fn show(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let (operands_only, _) = split_json_flag(args);
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

/// `bim diff <dirA> <dirB>` prints `same` (exit 0) when the documents' content hashes match, else
/// `differ` (exit 1); both projects are opened before anything is printed.
///
/// `bim diff <dir> <revA> <revB> [--json]` compares two recorded revisions of one project and
/// prints `{"added":[id],"removed":[id],"changed":[{"id","keys":[…]}]}` (spec/cli.md): ids are the
/// entities' names, sorted; `keys` are the props that differ, sorted (a changed kind shows as
/// `_kind`). Exit 0 if the revisions hold the same document, else 1; a revision the project does not
/// have is an input error (exit 2). The operands tell the forms apart: two directories, or a
/// directory and two revision ids (16 lowercase hex digits).
pub(crate) fn diff(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let (positional, json) = split_json_flag(args);
    match (positional.as_slice(), json) {
        ([a, b], false) => {
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
        ([dir, rev_a, rev_b], _) => diff_revisions(Path::new(dir), rev_a, rev_b, out),
        _ => Err(CliError::Usage(format!(
            "`bim diff <dirA> <dirB>` takes 2 operands and `bim diff <dir> <revA> <revB> --json` takes 3, got {}",
            positional.len()
        ))),
    }
}

fn diff_revisions(dir: &Path, rev_a: &OsString, rev_b: &OsString, out: &mut dyn Write) -> Outcome {
    let rev = |arg: &OsString| match arg.to_str() {
        Some(text) if is_rev(text) => Ok(text.to_string()),
        _ => Err(CliError::Usage(format!(
            "`bim diff <dir> <revA> <revB>`: `{}` is not a revision id (16 lowercase hex digits)",
            arg.to_string_lossy()
        ))),
    };
    let (rev_a, rev_b) = (rev(rev_a)?, rev(rev_b)?);
    Project::open(dir)?;
    let store = existing_store(dir)?.ok_or_else(|| CliError::Input(format!("unknown revision `{rev_a}`")))?;
    let diff = store.diff(&rev_a, &rev_b).map_err(|e| store_error(dir, e))?;
    let mut text = String::from("{\"added\":");
    write_strings(&mut text, diff.added.iter().map(String::as_str));
    text.push_str(",\"removed\":");
    write_strings(&mut text, diff.removed.iter().map(String::as_str));
    text.push_str(",\"changed\":[");
    for (i, (id, keys)) in diff.changed.iter().enumerate() {
        if i > 0 {
            text.push(',');
        }
        text.push_str("{\"id\":");
        json::write_str(&mut text, id);
        text.push_str(",\"keys\":");
        write_strings(&mut text, keys.iter().map(String::as_str));
        text.push('}');
    }
    text.push_str("]}");
    emit(out, &text)?;
    Ok(u8::from(!diff.is_empty()))
}

/// `bim commit <dir> -m <msg>`: records a revision of the document as it is now and prints its id,
/// the content hash as 16 hex digits (what `bim hash` prints). The revision lists the ops applied
/// since the previous revision. Committing twice without changes records two revisions with one id.
pub(crate) fn commit(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let usage = |what: &str| CliError::Usage(format!("`bim commit <dir> -m <msg>`: {what}"));
    let (mut dir, mut message) = (None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "-m" {
            let value = rest.next().ok_or_else(|| usage("`-m` needs a message"))?;
            if message.replace(value).is_some() {
                return Err(usage("`-m` given twice"));
            }
        } else if dir.replace(arg).is_some() {
            return Err(usage("takes one directory"));
        }
    }
    let (Some(dir), Some(message)) = (dir, message) else {
        return Err(usage("needs a directory and a message"));
    };
    let message = message
        .to_str()
        .ok_or_else(|| CliError::Input(format!("message `{}` is not valid UTF-8", message.to_string_lossy())))?;
    let project = Project::open(Path::new(dir))?;
    let rev = project.commit_revision(message)?;
    emit(out, &rev)?;
    Ok(0)
}

/// `bim log <dir> [--json]`: prints `[{"rev","message","ops"}]` on one line, oldest revision first;
/// `ops` are the ids (16 hex digits) of the ops recorded since the previous revision, in apply
/// order. `[]` for a project without revisions. Only reads: it never creates `bim.sqlite`.
pub(crate) fn log(args: &[OsString], out: &mut dyn Write) -> Outcome {
    let (positional, _) = split_json_flag(args);
    let [dir] = operands::<1>(&positional, "log <dir> --json")?;
    let dir = Path::new(dir);
    Project::open(dir)?;
    let revisions = match existing_store(dir)? {
        Some(store) => store.revisions().map_err(|e| store_error(dir, e))?,
        None => Vec::new(),
    };
    let mut text = String::from("[");
    for (i, revision) in revisions.iter().enumerate() {
        if i > 0 {
            text.push(',');
        }
        text.push_str("{\"rev\":");
        json::write_str(&mut text, &revision.rev);
        text.push_str(",\"message\":");
        json::write_str(&mut text, &revision.message);
        text.push_str(",\"ops\":");
        write_strings(&mut text, revision.ops.iter().map(String::as_str));
        text.push('}');
    }
    text.push(']');
    emit(out, &text)?;
    Ok(0)
}

/// A revision id: 16 lowercase hex digits.
fn is_rev(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}


/// The operands other than `--json`, and whether `--json` was given.
fn split_json_flag(args: &[OsString]) -> (Vec<OsString>, bool) {
    let positional: Vec<OsString> = args.iter().filter(|a| a.as_os_str() != "--json").cloned().collect();
    let json = positional.len() != args.len();
    (positional, json)
}

/// Exactly `N` operands, else a usage error quoting the command's form.
fn operands<'a, const N: usize>(args: &'a [OsString], form: &str) -> Result<&'a [OsString; N], CliError> {
    <&[OsString; N]>::try_from(args)
        .map_err(|_| CliError::Usage(format!("`bim {form}` takes {N} operand(s), got {}", args.len())))
}

/// Writes one line of result to stdout.
pub(crate) fn emit(out: &mut dyn Write, line: &str) -> Result<(), CliError> {
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

    fn run(args: &[&str]) -> (u8, String) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = crate::run(&args, &mut out, &mut err);
        (code, String::from_utf8(err).unwrap())
    }

    #[test]
    fn import_is_atomic_and_idempotent() {
        let tmp = TempDir::new();
        let p = |name: &str| tmp.path().join(name).to_str().unwrap().to_string();
        let (src, dst, ifc) = (p("src"), p("dst"), p("m.ifc"));
        assert_eq!(run(&["init", &src]).0, 0);
        let ops = p("m.json");
        fs::write(
            &ops,
            r#"{"schema":1,"ops":[
              {"op":"create","id":"L0","kind":"Level","props":{"elevation":0}},
              {"op":"create","id":"L1","kind":"Level","props":{"elevation":3}},
              {"op":"create","id":"WT","kind":"WallType","props":{"layer_thickness":[0.2],"layer_material":["C"]}},
              {"op":"create","id":"W1","kind":"Wall","props":{"level":{"ref":"L0"},"type":{"ref":"WT"},"x1":0,"y1":0,"x2":5,"y2":0,"height":3}},
              {"op":"create","id":"D1","kind":"Opening","props":{"host":{"ref":"W1"},"kind":"door","offset":1,"sill":0,"width":0.9,"height":2.1}},
              {"op":"create","id":"S1","kind":"Slab","props":{"level":{"ref":"L1"},"boundary":[0,0,5,0,5,4,2,4,2,2,0,2],"thickness":0.2}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(run(&["apply", &src, &ops]).0, 0);
        assert_eq!(run(&["export", &src, "--format", "ifc", "--out", &ifc]).0, 0);
        assert_eq!(run(&["init", &dst]).0, 0);
        let journal = || fs::read_to_string(Path::new(&dst).join("doc.oplog")).unwrap();
        // A truncated file, a JSON file that is not ifcJSON (an ops file) and a missing file change nothing.
        let text = fs::read_to_string(&ifc).unwrap();
        let cut = p("cut.ifc");
        fs::write(&cut, &text[..text.len() / 2]).unwrap();
        for f in [cut.as_str(), ops.as_str(), p("missing.ifc").as_str()] {
            assert_eq!(run(&["import", &dst, f]).0, 2, "{f}");
        }
        assert_eq!(journal(), "");
        // The import keeps the ids (GlobalIds) and the kinds; a second import adds nothing.
        let (code, err) = run(&["import", &dst, &ifc]);
        assert_eq!(code, 0, "{err}");
        let after = journal();
        for seed in ["W1", "D1", "S1", "L1"] {
            let ulid = bim_doc::EntityId::derive(seed).to_ulid_string();
            assert!(after.contains(&format!("create {ulid} ")), "{seed} missing:\n{after}");
        }
        let src_doc = Project::open(Path::new(&src)).unwrap().doc;
        let dst_doc = Project::open(Path::new(&dst)).unwrap().doc;
        let (a, b) = (bim_model::Model::compile(&src_doc), bim_model::Model::compile(&dst_doc));
        assert_eq!(a.walls.len(), 1);
        assert!((a.walls[0].volume - b.walls[0].volume).abs() < 1e-9);
        assert_eq!(dst_doc.get(bim_doc::EntityId::derive("S1")).unwrap().kind, "Slab");
        assert!((a.slabs[0].volume - b.slabs[0].volume).abs() < 1e-9);
        assert_eq!(run(&["import", &dst, &ifc]).0, 0);
        assert_eq!(journal(), after);
        // An id the project holds as another kind is refused.
        let clash = p("clash.json");
        fs::write(&clash, format!(r#"{{"schema":1,"ops":[{{"op":"create","id":"{}","kind":"Level","props":{{}}}}]}}"#, bim_doc::EntityId::derive("W1").to_ulid_string())).unwrap();
        let other = p("other");
        assert_eq!(run(&["init", &other]).0, 0);
        assert_eq!(run(&["apply", &other, &clash]).0, 0);
        let (code, err) = run(&["import", &other, &ifc]);
        assert_eq!(code, 2);
        assert!(err.contains("holds it as a Level"), "{err}");
        // Another schema (IFC4X3) and a reference to an undefined instance under a product are
        // invalid input: exit 2, nothing applied.
        let fresh = p("fresh");
        assert_eq!(run(&["init", &fresh]).0, 0);
        let fresh_journal = || fs::read_to_string(Path::new(&fresh).join("doc.oplog")).unwrap();
        let x3 = p("x3.ifc");
        fs::write(&x3, text.replace("FILE_SCHEMA(('IFC4'))", "FILE_SCHEMA(('IFC4X3'))")).unwrap();
        let dangling = p("dangling.ifc");
        // Point the wall's placement (its 6th attribute) at an instance the file does not define.
        let wall = text.lines().find(|l| l.contains("=IFCWALL(")).unwrap();
        let mut fields: Vec<String> = wall.splitn(7, ',').map(str::to_string).collect();
        assert!(fields[5].starts_with('#'), "{wall}");
        fields[5] = "#999999".into();
        fs::write(&dangling, text.replace(wall, &fields.join(","))).unwrap();
        assert_ne!(fs::read_to_string(&dangling).unwrap(), text);
        for (f, why) in [(&x3, "unsupported FILE_SCHEMA"), (&dangling, "does not define")] {
            let (code, err) = run(&["import", &fresh, f]);
            assert_eq!(code, 2, "{f}: {err}");
            assert!(err.contains(why), "{err}");
        }
        assert_eq!(fresh_journal(), "");
    }

    #[test]
    fn ifcjson_round_trips_the_document_and_invalid_ifcjson_changes_nothing() {
        let tmp = TempDir::new();
        let p = |name: &str| tmp.path().join(name).to_str().unwrap().to_string();
        let ulid = bim_doc::EntityId::new_ulid(1_700_000_000_000, 0x2a2a).to_ulid_string();
        let ops = p("ops.json");
        fs::write(
            &ops,
            format!(
                r#"{{"schema":1,"ops":[
              {{"op":"create","id":"L0","kind":"Level","props":{{"elevation":0}}}},
              {{"op":"create","id":"L1","kind":"Level","props":{{"elevation":3.1}}}},
              {{"op":"create","id":"WT","kind":"WallType","props":{{"layer_thickness":[0.1,0.15],"layer_material":["Brick","Mineral wool"],"layer_conductivity":[0.77,0.04]}}}},
              {{"op":"create","id":"M","kind":"Material","props":{{"name":"C30/37","E":3.3e10,"density":2500}}}},
              {{"op":"create","id":"W1","kind":"Wall","props":{{"level":{{"ref":"L0"}},"type":{{"ref":"WT"}},"x1":0,"y1":0,"x2":7,"y2":0.1,"height":3}}}},
              {{"op":"create","id":"{ulid}","kind":"Wall","props":{{"level":{{"ref":"L1"}},"type":{{"ref":"WT"}},"x1":0,"y1":9,"x2":4,"y2":9,"height":2.7,"base_offset":0.05}}}},
              {{"op":"create","id":"D1","kind":"Opening","props":{{"host":{{"ref":"W1"}},"kind":"door","offset":2,"sill":0,"width":1,"height":2.2}}}},
              {{"op":"create","id":"S1","kind":"Slab","props":{{"level":{{"ref":"L1"}},"boundary":[0,0,7,0,7,5,0,5],"thickness":0.2}}}},
              {{"op":"create","id":"C1","kind":"Column","props":{{"x":3.5,"y":2.5,"base":{{"ref":"L0"}},"top":{{"ref":"L1"}},"section_b":0.3,"section_h":0.3,"material":{{"ref":"M"}}}}}},
              {{"op":"create","id":"SP","kind":"Space","props":{{"level":{{"ref":"L0"}},"x":3.5,"y":2,"name":"Hall"}}}}
            ]}}"#
            ),
        )
        .unwrap();
        let (a, b, c) = (p("a"), p("b"), p("c"));
        assert_eq!(run(&["init", &a]).0, 0);
        assert_eq!(run(&["apply", &a, &ops]).0, 0);
        let (first, second) = (p("a.json"), p("b.json"));
        let (code, err) = run(&["export", &a, "--format", "ifcjson", "--out", &first]);
        assert_eq!(code, 0, "{err}");
        let hash = |dir: &str| Project::open(Path::new(dir)).unwrap().doc.content_hash();
        let journal = |dir: &str| fs::read_to_string(Path::new(dir).join("doc.oplog")).unwrap();

        // Input that is not valid ifcJSON changes nothing: a truncated file, an ops file, a file
        // that is not UTF-8, an ifcJSON file whose members are malformed, one that is missing.
        assert_eq!(run(&["init", &b]).0, 0);
        let text = fs::read_to_string(&first).unwrap();
        let (cut, bytes, member) = (p("cut.json"), p("bytes.json"), p("member.json"));
        fs::write(&cut, &text[..text.len() / 2]).unwrap();
        fs::write(&bytes, [b'{', 0xff, 0xfe]).unwrap();
        fs::write(&member, text.replacen("\"kind\": \"Level\"", "\"kind\": 7", 1)).unwrap();
        for (f, why) in [
            (cut.as_str(), "not valid JSON"),
            (ops.as_str(), "not an ifcJSON file"),
            (bytes.as_str(), "not valid UTF-8"),
            (member.as_str(), "needs a \"kind\""),
            (p("missing.json").as_str(), "missing.json"),
        ] {
            let (code, err) = run(&["import", &b, f]);
            assert_eq!(code, 2, "{f}: {err}");
            assert!(err.contains(why) && err.contains(f), "{f}: {err}");
        }
        assert_eq!(journal(&b), "");

        // doc -> ifcJSON -> doc keeps the content hash, and so does a second trip.
        let (code, err) = run(&["import", &b, &first]);
        assert_eq!(code, 0, "{err}");
        assert_eq!(hash(&b), hash(&a));
        let after = journal(&b);
        assert!(after.contains(&format!("create {ulid} Wall")) && after.contains("create W1 Wall"), "{after}");
        assert_eq!(run(&["export", &b, "--format", "ifcjson", "--out", &second]).0, 0);
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        assert_eq!(run(&["init", &c]).0, 0);
        assert_eq!(run(&["import", &c, &second]).0, 0);
        assert_eq!(hash(&c), hash(&a));
        // Importing the same file again adds nothing; `.IFCJSON` is read like `.json`.
        assert_eq!(run(&["import", &b, &first]).0, 0);
        assert_eq!(journal(&b), after);
        let upper = p("copy.IFCJSON");
        fs::copy(&first, &upper).unwrap();
        let fresh = p("fresh");
        assert_eq!(run(&["init", &fresh]).0, 0);
        assert_eq!(run(&["import", &fresh, &upper]).0, 0);
        assert_eq!(hash(&fresh), hash(&a));

        // An id the project holds as another kind is refused, nothing changes.
        let other = p("other");
        assert_eq!(run(&["init", &other]).0, 0);
        let clash = p("clash.json");
        fs::write(&clash, r#"{"schema":1,"ops":[{"op":"create","id":"W1","kind":"Level","props":{}}]}"#).unwrap();
        assert_eq!(run(&["apply", &other, &clash]).0, 0);
        let before = journal(&other);
        let (code, err) = run(&["import", &other, &first]);
        assert_eq!(code, 2);
        assert!(err.contains("holds it as a Level"), "{err}");
        assert_eq!(journal(&other), before);
    }
}
