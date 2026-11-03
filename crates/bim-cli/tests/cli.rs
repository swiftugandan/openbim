//! Black-box tests: the real `bim` binary against the contract in TASKS.md (T1.5) and spec/cli.md.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use bim_cli::json::{self, Json};
use bim_doc::{Document, EntityId, Op, Value};
use bim_store::{op_id, Store};

const BIM: &str = env!("CARGO_BIN_EXE_bim");

/// A scratch directory under cargo's target tmp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, content).unwrap();
        path
    }

    /// An initialised project `name` with `ops` applied.
    fn project(&self, name: &str, ops: &str) -> PathBuf {
        let dir = self.path(name);
        bim(&[&"init", &dir]).expect(0);
        if !ops.is_empty() {
            let file = self.write(&format!("{name}.ops"), ops);
            bim(&[&"apply", &dir, &file]).expect(0);
        }
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Run {
    code: i32,
    out: String,
    err: String,
}

impl Run {
    /// Asserts the exit code and returns the run for further checks.
    fn expect(self, code: i32) -> Run {
        assert_eq!(self.code, code, "stdout: {:?}\nstderr: {:?}", self.out, self.err);
        self
    }

    /// Asserts the exit code is 2, that nothing went to stdout, and that stderr starts with `bim: `.
    fn rejected(self) -> Run {
        let run = self.expect(2);
        assert_eq!(run.out, "", "an error must not print results");
        assert!(run.err.starts_with("bim: "), "stderr: {:?}", run.err);
        run
    }
}

fn bim(args: &[&dyn AsRef<OsStr>]) -> Run {
    let output = Command::new(BIM)
        .args(args.iter().map(|arg| arg.as_ref()))
        .output()
        .expect("spawn bim");
    Run {
        code: output.status.code().expect("bim was killed by a signal"),
        out: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn journal(dir: &Path) -> Vec<u8> {
    fs::read(dir.join("doc.oplog")).unwrap()
}

fn hash(dir: &Path) -> String {
    let out = bim(&[&"hash", &dir]).expect(0).out;
    assert!(out.ends_with('\n'), "{out:?}");
    out.trim_end().to_string()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// Applies what `bim apply` makes of `create <seed> <kind>`: the create, then the reserved props
/// `_name` (the seed as written) and `_schema` (1) that spec/schema.md has bim set at create.
fn create(doc: &mut Document, seed: &str, kind: &str) {
    let id = EntityId::parse(seed);
    doc.apply(Op::Create { id, kind: kind.into() }).unwrap();
    doc.apply(Op::Set { id, key: "_name".into(), value: Value::Text(seed.into()) }).unwrap();
    doc.apply(Op::Set { id, key: "_schema".into(), value: Value::Num(1.0) }).unwrap();
}

/// The judge's two builds of the same model, in different op orders.
const A_OPS: &str = "create wall Wall\ncreate lvl Level\nset wall height num 3.5\nset wall host ref lvl\nset wall name text north\n";
const B_OPS: &str = "create lvl Level\ncreate wall Wall\nset wall name text north\nset wall height num 9\nset wall height num 3.5\nset wall host ref lvl\n";

// ---- init -----------------------------------------------------------------------------------

#[test]
fn init_creates_an_empty_journal_and_prints_nothing() {
    let s = Scratch::new();
    let dir = s.path("p");
    let run = bim(&[&"init", &dir]).expect(0);
    assert_eq!((run.out.as_str(), run.err.as_str()), ("", ""));
    assert_eq!(journal(&dir), b"");
    assert_eq!(files_in(&dir), ["bim.sqlite", "doc.oplog"]);
    // A freshly initialised project is a valid, empty document.
    bim(&[&"validate", &dir]).expect(0);
    assert_eq!(hash(&dir), hash(&s.project("q", "")));
}

#[test]
fn init_creates_missing_parent_directories() {
    let s = Scratch::new();
    let dir = s.path("a").join("b").join("c");
    bim(&[&"init", &dir]).expect(0);
    assert!(dir.join("doc.oplog").is_file());
}

#[test]
fn init_is_idempotent_and_never_touches_an_existing_journal() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = (journal(&dir), hash(&dir));
    bim(&[&"init", &dir]).expect(0);
    assert_eq!((journal(&dir), hash(&dir)), before);
    // An existing empty directory is fine too.
    let empty = s.path("empty");
    fs::create_dir(&empty).unwrap();
    bim(&[&"init", &empty]).expect(0);
    assert!(empty.join("doc.oplog").is_file());
}

#[test]
fn init_fails_with_2_when_the_path_cannot_be_a_project() {
    let s = Scratch::new();
    let file = s.write("file", "x");
    let run = bim(&[&"init", &file]).rejected();
    assert!(run.err.contains("file"), "{}", run.err);
    assert_eq!(fs::read(&file).unwrap(), b"x");
    let dir = s.path("d");
    fs::create_dir_all(dir.join("doc.oplog")).unwrap();
    bim(&[&"init", &dir]).rejected();
    assert!(dir.join("doc.oplog").is_dir());
}

// ---- hash / diff --------------------------------------------------------------------------

#[test]
fn hash_prints_exactly_sixteen_lowercase_hex_digits() {
    let s = Scratch::new();
    for ops in ["", A_OPS] {
        let dir = s.project("p", ops);
        let run = bim(&[&"hash", &dir]).expect(0);
        assert_eq!(run.err, "");
        let digits = run.out.strip_suffix('\n').expect("one line");
        assert_eq!(digits.len(), 16, "{digits}");
        assert!(digits.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{digits}");
        assert_eq!(hash(&dir), digits, "deterministic");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn hash_is_zero_padded_to_sixteen_digits() {
    // About one document in sixteen hashes to a value below 2^60; find such a model by search.
    let hash_of = |n: usize| {
        let mut doc = Document::new();
        create(&mut doc, &format!("e{n}"), "Wall");
        doc.content_hash()
    };
    let n = (0..10_000).find(|&n| hash_of(n) < 1 << 60).expect("a leading-zero hash among 10000 models");
    let s = Scratch::new();
    let dir = s.project("p", &format!("create e{n} Wall\n"));
    let digits = hash(&dir);
    assert_eq!(digits, format!("{:016x}", hash_of(n)));
    assert_eq!(digits.len(), 16);
    assert!(digits.starts_with('0'), "{digits}");
}

#[test]
fn hash_matches_the_bim_doc_content_hash_of_the_same_ops() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let id = EntityId::derive;
    let mut doc = Document::new();
    create(&mut doc, "wall", "Wall");
    create(&mut doc, "lvl", "Level");
    for op in [
        Op::Set { id: id("wall"), key: "height".into(), value: Value::Num(3.5) },
        Op::Set { id: id("wall"), key: "host".into(), value: Value::Ref(id("lvl")) },
        Op::Set { id: id("wall"), key: "name".into(), value: Value::Text("north".into()) },
    ] {
        doc.apply(op).unwrap();
    }
    assert_eq!(hash(&dir), format!("{:016x}", doc.content_hash()));
}

#[test]
fn the_same_model_built_in_another_order_hashes_alike_and_diffs_same() {
    let s = Scratch::new();
    let (a, b) = (s.project("A", A_OPS), s.project("B", B_OPS));
    assert_eq!(hash(&a), hash(&b));
    let run = bim(&[&"diff", &a, &b]).expect(0);
    assert_eq!((run.out.as_str(), run.err.as_str()), ("same\n", ""));
    // A project is the same as itself, and an empty project differs from a built one.
    bim(&[&"diff", &a, &a]).expect(0);
    let empty = s.project("E", "");
    assert_eq!(bim(&[&"diff", &a, &empty]).expect(1).out, "differ\n");
}

#[test]
fn diff_prints_differ_with_exit_1_when_the_documents_differ() {
    let s = Scratch::new();
    let (a, b) = (s.project("A", A_OPS), s.project("B", B_OPS));
    let del = s.write("del.ops", "delete lvl\n");
    bim(&[&"apply", &a, &del]).expect(0);
    for run in [bim(&[&"diff", &a, &b]), bim(&[&"diff", &b, &a])] {
        let run = run.expect(1);
        assert_eq!((run.out.as_str(), run.err.as_str()), ("differ\n", ""));
    }
    // One changed number is enough.
    let c = s.project("C", &format!("{A_OPS}set wall height num 3.6\n"));
    let d = s.project("D", A_OPS);
    assert_eq!(bim(&[&"diff", &c, &d]).expect(1).out, "differ\n");
}

#[test]
fn diff_needs_two_valid_projects() {
    let s = Scratch::new();
    let a = s.project("A", A_OPS);
    let missing = s.path("missing");
    bim(&[&"diff", &a, &missing]).rejected();
    bim(&[&"diff", &missing, &a]).rejected();
    let bare = s.path("bare");
    fs::create_dir(&bare).unwrap();
    bim(&[&"diff", &a, &bare]).rejected();
}

// ---- validate -------------------------------------------------------------------------------

#[test]
fn validate_is_silent_and_exits_0_without_dangling_refs() {
    let s = Scratch::new();
    for ops in ["", A_OPS, "create a Wall\nset a self ref a\nset a t text ghost\n"] {
        let dir = s.project("p", ops);
        let run = bim(&[&"validate", &dir]).expect(0);
        assert_eq!((run.out.as_str(), run.err.as_str()), ("", ""), "{ops}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn validate_prints_one_line_per_dangling_ref_and_exits_1() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    bim(&[&"apply", &dir, &s.write("del.ops", "delete lvl\n")]).expect(0);
    let run = bim(&[&"validate", &dir]).expect(1);
    assert_eq!((run.out.as_str(), run.err.as_str()), ("dangling wall host lvl\n", ""));

    // Several dangling refs: one line each, named by seed, including a ref to a seed never created.
    let more = s.write("more.ops", "create door Opening\nset door host ref wall\nset door product ref nothing\nset wall level ref lvl\n");
    bim(&[&"apply", &dir, &more]).expect(0);
    let run = bim(&[&"validate", &dir]).expect(1);
    let mut lines: Vec<&str> = run.out.lines().collect();
    lines.sort();
    assert_eq!(
        lines,
        ["dangling door product nothing", "dangling wall host lvl", "dangling wall level lvl"]
    );
    assert_eq!(run.err, "");

    // Recreating the target resolves the refs that pointed at it.
    bim(&[&"apply", &dir, &s.write("re.ops", "create lvl Level\n")]).expect(0);
    let run = bim(&[&"validate", &dir]).expect(1);
    assert_eq!(run.out, "dangling door product nothing\n");
    bim(&[&"apply", &dir, &s.write("re2.ops", "create nothing Product\n")]).expect(0);
    bim(&[&"validate", &dir]).expect(0);
}

// ---- apply: errors leave the log unchanged -----------------------------------------------

#[test]
fn a_file_with_one_bad_line_is_rejected_whole() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = (journal(&dir), hash(&dir));
    let bad = s.write("bad.ops", "set wall height num 4\nbogus op here\n");
    let run = bim(&[&"apply", &dir, &bad]).rejected();
    assert!(run.err.contains("bad.ops:2:"), "{}", run.err);
    assert!(run.err.contains("unknown op `bogus`"), "{}", run.err);
    assert_eq!((journal(&dir), hash(&dir)), before);
    assert_eq!(files_in(&dir), ["bim.sqlite", "doc.oplog"], "no temporary file is left behind");
}

#[test]
fn an_op_the_document_refuses_rejects_the_whole_file() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = (journal(&dir), hash(&dir));
    let cases = [
        ("set on a missing entity", "set ghost height num 1\n", ":1: entity `ghost` not found"),
        ("delete of a missing entity", "delete ghost\n", ":1: entity `ghost` not found"),
        ("create of an existing entity", "create wall Slab\n", ":1: entity `wall` already exists"),
        (
            "valid ops before the refused one",
            "create door Opening\nset door width num 0.9\nset wall height num 4\nset ghost x num 1\n",
            ":4: entity `ghost` not found",
        ),
        ("a duplicate create inside one file", "create fresh Wall\ncreate fresh Wall\n", ":2: entity `fresh` already exists"),
        ("an op on an entity the file deleted", "create fresh Wall\ndelete fresh\nset fresh k num 1\n", ":3: entity `fresh` not found"),
    ];
    for (what, text, message) in cases {
        let file = s.write("refused.ops", text);
        let run = bim(&[&"apply", &dir, &file]).rejected();
        assert!(run.err.contains(message), "{what}: {}", run.err);
        assert!(run.err.contains("refused.ops"), "{what}: {}", run.err);
        assert_eq!((journal(&dir), hash(&dir)), before, "{what}");
    }
    assert_eq!(files_in(&dir), ["bim.sqlite", "doc.oplog"]);
}

#[test]
fn syntax_errors_exit_2_and_name_the_line() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = journal(&dir);
    let cases = [
        ("unknown op", "frob a b\n", "unknown op `frob`"),
        ("op keyword is case sensitive", "CREATE a Wall\n", "unknown op `CREATE`"),
        ("create with 2 words", "create a\n", "word(s)"),
        ("create with 4 words", "create a Wall extra\n", "word(s)"),
        ("set with 4 words", "set wall height num\n", "word(s)"),
        ("set with 6 words", "set wall name text north south\n", "word(s)"),
        ("delete with 3 words", "delete wall lvl\n", "word(s)"),
        ("unknown value type", "set wall flag bool true\n", "unknown value type `bool`"),
        ("value type is case sensitive", "set wall height NUM 3\n", "unknown value type `NUM`"),
        ("bad number", "set wall height num tall\n", "is not a number"),
        ("hex number", "set wall height num 0x10\n", "is not a number"),
        ("comment syntax does not exist", "# note\n", "unknown op `#`"),
    ];
    for (what, text, message) in cases {
        let file = s.write("syntax.ops", format!("\ncreate ok Wall\n{text}"));
        let run = bim(&[&"apply", &dir, &file]).rejected();
        assert!(run.err.contains("syntax.ops:3:"), "{what}: {}", run.err);
        assert!(run.err.contains(message), "{what}: {}", run.err);
        assert_eq!(journal(&dir), before, "{what}");
    }
}

#[test]
fn non_finite_numbers_are_rejected() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = journal(&dir);
    for word in ["nan", "NaN", "inf", "-inf", "infinity", "1e400", "-1e999"] {
        let file = s.write("nf.ops", format!("set wall height num {word}\n"));
        let run = bim(&[&"apply", &dir, &file]).rejected();
        assert!(run.err.contains("is not finite"), "{word}: {}", run.err);
        assert_eq!(journal(&dir), before, "{word}");
    }
    // Text that merely looks like one is just text.
    bim(&[&"apply", &dir, &s.write("ok.ops", "set wall label text nan\n")]).expect(0);
}

#[test]
fn reserved_keys_are_rejected_in_op_files() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = journal(&dir);
    for key in ["_name", "_schema", "_created_by", "_stamp", "_mine"] {
        let file = s.write("res.ops", format!("create ok Wall\nset wall {key} num 1\n"));
        let run = bim(&[&"apply", &dir, &file]).rejected();
        assert!(run.err.contains("res.ops:2:") && run.err.contains("reserved"), "{key}: {}", run.err);
        assert_eq!(journal(&dir), before, "{key}");
    }
}

// ---- JSON ops and show -------------------------------------------------------------------

const ULID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn show(dir: &Path, id: &str) -> String {
    let run = bim(&[&"show", &dir, &id, &"--json"]).expect(0);
    assert_eq!(run.err, "");
    run.out.strip_suffix('\n').expect("one line").to_string()
}

#[test]
fn json_ops_build_entities_that_show_reports_with_reserved_props() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    let file = s.write(
        "a.json",
        format!(
            r#"{{"schema": 1, "ops": [
              {{"op": "create", "id": "L1", "kind": "Level", "props": {{"elevation": 0}}}},
              {{"op": "create", "id": "{ULID}", "kind": "Wall", "props": {{
                 "level": {{"ref": "L1"}}, "x2": 5.5, "y2": -1e-3, "big": 1e300, "third": 0.30000000000000004,
                 "name": "north wall, façade \"q\"", "on": true, "off": false,
                 "tags": [{{"ref": "L1"}}, "a", [1, false], []]}}}},
              {{"op": "set", "id": "L1", "key": "elevation", "value": 3.25}},
              {{"op": "create", "id": "tmp", "kind": "Level"}},
              {{"op": "delete", "id": "tmp"}}
            ]}}"#
        ),
    );
    bim(&[&"apply", &dir, &file]).expect(0);
    assert_eq!(
        show(&dir, "L1"),
        r#"{"id":"L1","kind":"Level","props":{"_name":"L1","_schema":1.0,"elevation":3.25}}"#
    );
    assert_eq!(
        show(&dir, ULID),
        format!(
            r#"{{"id":"{ULID}","kind":"Wall","props":{{"_name":"{ULID}","_schema":1.0,"big":1e300,"level":{{"ref":"L1"}},"name":"north wall, façade \"q\"","off":false,"on":true,"tags":[{{"ref":"L1"}},"a",[1.0,false],[]],"third":0.30000000000000004,"x2":5.5,"y2":-0.001}}}}"#
        )
    );
    // `--json` may come anywhere; unknown and deleted ids exit 2 with nothing on stdout.
    assert_eq!(bim(&[&"show", &"--json", &dir, &"L1"]).expect(0).out, format!("{}\n", show(&dir, "L1")));
    for id in ["tmp", "nobody", "l1"] {
        let run = bim(&[&"show", &dir, &id, &"--json"]).rejected();
        assert!(run.err.contains(&format!("entity `{id}` not found")), "{}", run.err);
    }
    bim(&[&"show", &dir, &"--json"]).rejected();
    bim(&[&"show", &dir, &"L1", &"extra", &"--json"]).rejected();
    // Line-format creates carry the reserved props too, and the journal replays all of it.
    bim(&[&"apply", &dir, &s.write("b.ops", "create w Wall\nset w host ref L1\n")]).expect(0);
    assert_eq!(show(&dir, "w"), r#"{"id":"w","kind":"Wall","props":{"_name":"w","_schema":1.0,"host":{"ref":"L1"}}}"#);
    let copy = s.project("q", "");
    bim(&[&"apply", &copy, &dir.join("doc.oplog")]).expect(0);
    assert_eq!(hash(&copy), hash(&dir), "the journal is itself a valid ops file");
    assert_eq!(show(&copy, ULID), show(&dir, ULID));
}

#[test]
fn json_and_line_ops_for_the_same_model_hash_alike() {
    let s = Scratch::new();
    let line = s.project("line", "create L1 Level\nset L1 elevation num 3.25\ncreate w Wall\nset w host ref L1\n");
    let json = s.project("json", "");
    let file = s.write(
        "c.json",
        r#"{"schema": 1, "ops": [{"op": "create", "id": "w", "kind": "Wall"},
            {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 1}},
            {"op": "set", "id": "w", "key": "host", "value": {"ref": "L1"}},
            {"op": "set", "id": "L1", "key": "elevation", "value": 3.25}]}"#,
    );
    bim(&[&"apply", &json, &file]).expect(0);
    assert_eq!(hash(&line), hash(&json));
    bim(&[&"diff", &line, &json]).expect(0);
}

#[test]
fn invalid_json_ops_files_change_nothing_and_exit_2() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = (journal(&dir), hash(&dir));
    let first = r#"{"op": "create", "id": "fresh", "kind": "Level"}"#;
    let cases = [
        ("set on a missing id", format!(r#"{{"schema": 1, "ops": [{first}, {{"op": "set", "id": "ghost", "key": "x", "value": 1}}]}}"#), "ops[1]: entity `ghost` not found"),
        ("create of an existing id", format!(r#"{{"schema": 1, "ops": [{first}, {{"op": "create", "id": "wall", "kind": "Wall"}}]}}"#), "ops[1]: entity `wall` already exists"),
        ("unknown op", format!(r#"{{"schema": 1, "ops": [{first}, {{"op": "rename", "id": "wall"}}]}}"#), "unknown op `rename`"),
        ("null value", format!(r#"{{"schema": 1, "ops": [{first}, {{"op": "set", "id": "wall", "key": "x", "value": null}}]}}"#), "null"),
        ("reserved set", format!(r#"{{"schema": 1, "ops": [{first}, {{"op": "set", "id": "wall", "key": "_name", "value": "evil"}}]}}"#), "reserved"),
        ("reserved prop at create", r#"{"schema": 1, "ops": [{"op": "create", "id": "fresh", "kind": "Wall", "props": {"_schema": 2}}]}"#.to_string(), "reserved"),
        ("schema 2", format!(r#"{{"schema": 2, "ops": [{first}]}}"#), "schema version"),
        ("malformed", format!(r#"{{"schema": 1, "ops": [{first},"#), "invalid JSON"),
    ];
    let latin1 = (
        "not UTF-8",
        b"{\"schema\": 1, \"ops\": [{\"op\": \"create\", \"id\": \"fresh\", \"kind\": \"caf\xe9\"}]}".to_vec(),
        "UTF-8",
    );
    let cases = cases.into_iter().map(|(what, text, message)| (what, text.into_bytes(), message)).chain([latin1]);
    for (what, bytes, message) in cases {
        let run = bim(&[&"apply", &dir, &s.write("bad.json", bytes)]).rejected();
        assert!(run.err.contains(message) && run.err.contains("bad.json"), "{what}: {}", run.err);
        assert_eq!((journal(&dir), hash(&dir)), before, "{what}");
        bim(&[&"show", &dir, &"fresh", &"--json"]).rejected();
    }
}

#[test]
fn values_without_a_word_form_reach_the_journal_as_json_lines() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    let file = s.write(
        "v.json",
        r#"{"schema": 1, "ops": [{"op": "create", "id": "a b", "kind": "Wall", "props": {"name": "two words", "on": true, "l": [{"ref": "a b"}]}}, {"op": "set", "id": "a b", "key": "h", "value": 3}]}"#,
    );
    bim(&[&"apply", &dir, &file]).expect(0);
    assert_eq!(
        String::from_utf8(journal(&dir)).unwrap(),
        concat!(
            r#"{"op":"create","id":"a b","kind":"Wall"}"#, "\n",
            r#"{"op":"set","id":"a b","key":"name","value":"two words"}"#, "\n",
            r#"{"op":"set","id":"a b","key":"on","value":true}"#, "\n",
            r#"{"op":"set","id":"a b","key":"l","value":[{"ref":"a b"}]}"#, "\n",
            r#"{"op":"set","id":"a b","key":"h","value":3.0}"#, "\n",
        )
    );
    assert_eq!(
        show(&dir, "a b"),
        r#"{"id":"a b","kind":"Wall","props":{"_name":"a b","_schema":1.0,"h":3.0,"l":[{"ref":"a b"}],"name":"two words","on":true}}"#
    );
    // A dangling ref inside a List is reported by validate.
    bim(&[&"apply", &dir, &s.write("d.json", r#"{"schema": 1, "ops": [{"op": "set", "id": "a b", "key": "l", "value": [[{"ref": "gone"}]]}]}"#)]).expect(0);
    assert_eq!(bim(&[&"validate", &dir]).expect(1).out, "dangling a b l gone\n");
}

#[test]
fn unreadable_ops_files_exit_2() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = journal(&dir);
    bim(&[&"apply", &dir, &s.path("missing.ops")]).rejected();
    let directory = s.path("a-directory");
    fs::create_dir(&directory).unwrap();
    bim(&[&"apply", &dir, &directory]).rejected();
    let run = bim(&[&"apply", &dir, &s.write("latin1.ops", b"create caf\xe9 Wall\n")]).rejected();
    assert!(run.err.contains("UTF-8"), "{}", run.err);
    assert_eq!(journal(&dir), before);
}

#[test]
fn apply_needs_an_initialised_project() {
    let s = Scratch::new();
    let ops = s.write("a.ops", A_OPS);
    let missing = s.path("missing");
    let run = bim(&[&"apply", &missing, &ops]).rejected();
    assert!(run.err.contains("no such directory"), "{}", run.err);
    assert!(!missing.exists(), "apply must not create the project");
    let bare = s.path("bare");
    fs::create_dir(&bare).unwrap();
    let run = bim(&[&"apply", &bare, &ops]).rejected();
    assert!(run.err.contains("bim init"), "{}", run.err);
    assert_eq!(files_in(&bare), Vec::<String>::new(), "apply must not create the journal");
    let file = s.write("file", "x");
    bim(&[&"apply", &file, &ops]).rejected();
    for command in ["hash", "validate"] {
        bim(&[&command, &missing]).rejected();
        bim(&[&command, &bare]).rejected();
        bim(&[&command, &file]).rejected();
    }
}

// ---- apply: success and the journal -------------------------------------------------------

#[test]
fn blank_lines_are_ignored_and_a_blank_only_file_changes_nothing() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let before = (journal(&dir), hash(&dir));
    let modified = fs::metadata(dir.join("doc.oplog")).unwrap().modified().unwrap();
    for (name, content) in [("empty", ""), ("newlines", "\n\n\n"), ("spaces", "  \n\t\n \r\n")] {
        let run = bim(&[&"apply", &dir, &s.write(name, content)]).expect(0);
        assert_eq!((run.out.as_str(), run.err.as_str()), ("", ""));
        assert_eq!((journal(&dir), hash(&dir)), before, "{name}");
    }
    assert_eq!(fs::metadata(dir.join("doc.oplog")).unwrap().modified().unwrap(), modified, "not even rewritten");
    // Blank lines between ops are skipped and do not reach the journal.
    let spaced = s.write("spaced.ops", "\ncreate a Wall\n\n   \nset a h num 1\n\n");
    let fresh = s.project("q", "");
    bim(&[&"apply", &fresh, &spaced]).expect(0);
    assert_eq!(journal(&fresh), b"create a Wall\nset a h num 1.0\n");
}

#[test]
fn an_ops_file_may_start_with_a_byte_order_mark() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    bim(&[&"apply", &dir, &s.write("bom.ops", "\u{feff}create a Wall\nset a h num 1\n")]).expect(0);
    assert_eq!(journal(&dir), b"create a Wall\nset a h num 1.0\n", "the mark is not copied into the journal");
}

#[test]
fn the_journal_holds_canonical_op_lines_with_seeds_as_written() {
    let s = Scratch::new();
    let ops = "create  wall\tWall\r\n\r\n set wall  height num 9\r\nset wall host ref Lvl-1\nset wall name text north\nset wall x num +3.50\nset wall y num -0\ndelete wall\n";
    let dir = s.project("p", ops);
    assert_eq!(
        String::from_utf8(journal(&dir)).unwrap(),
        "create wall Wall\nset wall height num 9.0\nset wall host ref Lvl-1\nset wall name text north\nset wall x num 3.5\nset wall y num -0.0\ndelete wall\n"
    );
    // The journal is itself a valid ops file: applying it to a fresh project rebuilds the same document.
    let copy = s.project("q", "");
    bim(&[&"apply", &copy, &dir.join("doc.oplog")]).expect(0);
    assert_eq!(journal(&copy), journal(&dir));
    assert_eq!(hash(&copy), hash(&dir));
    // Non-ASCII seeds, kinds and words survive.
    let unicode = s.project("u", "create Wänd-1 Wall\nset Wänd-1 name text nörd\nset Wänd-1 host ref Ebene-€\n");
    assert_eq!(
        String::from_utf8(journal(&unicode)).unwrap(),
        "create Wänd-1 Wall\nset Wänd-1 name text nörd\nset Wänd-1 host ref Ebene-€\n"
    );
    assert_eq!(bim(&[&"validate", &unicode]).expect(1).out, "dangling Wänd-1 host Ebene-€\n");
}

#[test]
fn apply_appends_and_keeps_the_existing_journal_bytes() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    // A hand-edited journal: odd spacing, CRLF, no final newline.
    fs::write(dir.join("doc.oplog"), "create  a   Wall\r\n\r\nset a h num 2").unwrap();
    bim(&[&"apply", &dir, &s.write("more.ops", "set a h num 3\ncreate b Wall\n")]).expect(0);
    assert_eq!(
        String::from_utf8(journal(&dir)).unwrap(),
        "create  a   Wall\r\n\r\nset a h num 2\nset a h num 3.0\ncreate b Wall\n"
    );
    // Two applies equal one apply of the concatenation.
    let (two, one) = (s.project("two", ""), s.project("one", ""));
    bim(&[&"apply", &two, &s.write("1.ops", "create a Wall\nset a h num 1\n")]).expect(0);
    bim(&[&"apply", &two, &s.write("2.ops", "set a h num 2\ncreate b Wall\nset b p ref a\n")]).expect(0);
    bim(&[&"apply", &one, &s.write("3.ops", "create a Wall\nset a h num 1\nset a h num 2\ncreate b Wall\nset b p ref a\n")]).expect(0);
    assert_eq!(hash(&two), hash(&one));
    assert_eq!(bim(&[&"diff", &two, &one]).expect(0).out, "same\n");
    // Later files see the earlier ones' entities.
    bim(&[&"apply", &two, &s.write("4.ops", "delete a\n")]).expect(0);
    assert_eq!(bim(&[&"validate", &two]).expect(1).out, "dangling b p a\n");
}

#[test]
fn ops_in_one_file_see_each_other() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    let file = s.write("chain.ops", "create a Wall\nset a h num 1\ndelete a\ncreate a Slab\nset a t num 2\n");
    bim(&[&"apply", &dir, &file]).expect(0);
    let mut doc = Document::new();
    let a = EntityId::derive("a");
    create(&mut doc, "a", "Wall");
    doc.apply(Op::Set { id: a, key: "h".into(), value: Value::Num(1.0) }).unwrap();
    doc.apply(Op::Delete { id: a }).unwrap();
    create(&mut doc, "a", "Slab");
    doc.apply(Op::Set { id: a, key: "t".into(), value: Value::Num(2.0) }).unwrap();
    assert_eq!(hash(&dir), format!("{:016x}", doc.content_hash()));
}

#[test]
fn numbers_survive_the_journal_bit_for_bit() {
    let s = Scratch::new();
    let words = ["0.30000000000000004", "1e300", "5e-324", "-0", "9", "3.5", "1.7976931348623157e308", "2.2250738585072014e-308", "1e21", "1e-7", "123456789012345680"];
    let ops: String = std::iter::once("create e Wall\n".to_string())
        .chain(words.iter().enumerate().map(|(i, w)| format!("set e k{i} num {w}\n")))
        .collect();
    let dir = s.project("p", &ops);
    let mut doc = Document::new();
    let e = EntityId::derive("e");
    create(&mut doc, "e", "Wall");
    for (i, w) in words.iter().enumerate() {
        let x: f64 = w.parse().unwrap();
        doc.apply(Op::Set { id: e, key: format!("k{i}"), value: Value::Num(x) }).unwrap();
    }
    // `hash` replays the journal, so this is the journal's round trip checked against the source numbers.
    assert_eq!(hash(&dir), format!("{:016x}", doc.content_hash()));
    // And replaying the replay changes nothing.
    let again = s.project("q", "");
    bim(&[&"apply", &again, &dir.join("doc.oplog")]).expect(0);
    assert_eq!(journal(&again), journal(&dir));
}

#[test]
fn a_read_only_journal_is_not_replaced() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let path = dir.join("doc.oplog");
    let original = fs::metadata(&path).unwrap().permissions();
    let mut read_only = original.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&path, read_only).unwrap();
    let before = journal(&dir);
    let run = bim(&[&"apply", &dir, &s.write("more.ops", "set wall height num 4\n")]);
    fs::set_permissions(&path, original).unwrap();
    let run = run.rejected();
    assert!(run.err.contains("read-only"), "{}", run.err);
    assert_eq!(journal(&dir), before);
    assert_eq!(files_in(&dir), ["bim.sqlite", "doc.oplog"]);
}

// ---- corrupt journals ---------------------------------------------------------------------

#[test]
fn a_corrupt_journal_exits_2_names_its_line_and_is_never_modified() {
    let s = Scratch::new();
    let cases = [
        ("garbage", "create a Wall\ngarbage\n", ":2: corrupt journal: unknown op `garbage`"),
        ("a refused op", "create a Wall\ncreate a Wall\n", ":2: corrupt journal: entity `a` already exists"),
        ("a set on nothing", "set ghost h num 1\n", ":1: corrupt journal: entity `ghost` not found"),
        ("a non-finite number", "create a W\nset a h num nan\n", ":2: corrupt journal"),
    ];
    for (what, text, message) in cases {
        let dir = s.project("p", "");
        fs::write(dir.join("doc.oplog"), text).unwrap();
        let ops = s.write("fine.ops", "create zzz Wall\n");
        for args in [
            vec![&"hash" as &dyn AsRef<OsStr>, &dir],
            vec![&"validate", &dir],
            vec![&"diff", &dir, &dir],
            vec![&"apply", &dir, &ops],
        ] {
            let run = bim(&args).rejected();
            assert!(run.err.contains(message), "{what}: {}", run.err);
            assert!(run.err.contains("doc.oplog"), "{what}: {}", run.err);
        }
        assert_eq!(journal(&dir), text.as_bytes(), "{what}");
        fs::remove_dir_all(&dir).unwrap();
    }
    // A journal that is not UTF-8.
    let dir = s.project("bin", "");
    fs::write(dir.join("doc.oplog"), b"create caf\xe9 Wall\n").unwrap();
    assert!(bim(&[&"hash", &dir]).rejected().err.contains("UTF-8"));
}

// ---- kind registry and trade packages -----------------------------------------------------

/// spec/schema.md's kinds table, sorted: what `bim plugin list` reports as `core_kinds`.
const CORE_KINDS: [&str; 34] = [
    "AreaLoad", "Beam", "Circuit", "Column", "ComponentDefinition", "ComponentInstance", "ConflictDiagnostic",
    "Constraint", "CurtainWall", "Dimension", "Furniture", "Guide", "Issue", "Level", "LoadCombination", "Material",
    "MepSystem", "Opening", "Point", "PointLoad", "Port", "Product", "Ramp", "Roof", "Scene", "Segment", "Shape", "Slab",
    "SlabOpening", "Space", "Stair", "Support", "Wall", "WallType",
];

const FIRE: &str = r#"{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0", "kinds": {
    "fire:Sprinkler": {"props": {"level": "ref", "x": "num", "y": "num", "z": "num", "coverage_radius": "num"},
                       "geometry": {"box": {"centre": ["x", "y", "z"], "size": [0.1, 0.1, 0.1]}}},
    "fire:Zone": {"props": {"name": "text", "active": "bool", "rooms": "list"}}},
    "engine": "fire.wasm"}"#;

const ACOUSTIC: &str = r#"{"package": "acoustic", "version": "2.1.0", "abi": "openbim:trade@0.1.0",
    "kinds": {"acoustic:Panel": {"props": {"nrc": "num"}}, "acoustic:Baffle": {}}}"#;

/// Installs `text` as `<dir>/plugins/<package>/trade.json`.
fn install(dir: &Path, package: &str, text: &str) {
    let package_dir = dir.join("plugins").join(package);
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(package_dir.join("trade.json"), text).unwrap();
}

fn core_kinds_json() -> String {
    format!("[\"{}\"]", CORE_KINDS.join("\",\""))
}

#[test]
fn every_core_kind_can_be_created_with_any_props_but_other_kinds_cannot() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    let ops: String = CORE_KINDS.iter().enumerate().map(|(i, kind)| format!("create k{i} {kind}\nset k{i} anything text goes\n")).collect();
    bim(&[&"apply", &dir, &s.write("core.ops", ops)]).expect(0);
    assert_eq!(show(&dir, "k0"), r#"{"id":"k0","kind":"AreaLoad","props":{"_name":"k0","_schema":1.0,"anything":"goes"}}"#);
    let before = (journal(&dir), hash(&dir));
    let cases = [
        ("line format", "bad.ops", "create ok Level\ncreate b Bogus\n", "bad.ops:2: unknown kind `Bogus`"),
        ("a lower-case core name", "bad.ops", "create b wall\n", "bad.ops:1: unknown kind `wall`"),
        ("a trailing space is part of the name", "bad.ops", "{\"op\":\"create\",\"id\":\"b\",\"kind\":\"Wall \"}\n", "unknown kind `Wall `"),
        ("an extension kind whose package is not installed", "bad.ops", "create b fire:Sprinkler\n", "unknown kind `fire:Sprinkler`"),
        (
            "JSON ops, with props",
            "bad.json",
            r#"{"schema": 1, "ops": [{"op": "create", "id": "ok", "kind": "Level"}, {"op": "create", "id": "b", "kind": "Bogus", "props": {"x": 1}}]}"#,
            "bad.json: ops[1]: unknown kind `Bogus`",
        ),
    ];
    for (what, name, text, message) in cases {
        let run = bim(&[&"apply", &dir, &s.write(name, text)]).rejected();
        assert!(run.err.contains(message), "{what}: {}", run.err);
        assert_eq!((journal(&dir), hash(&dir)), before, "{what}");
        bim(&[&"show", &dir, &"b", &"--json"]).rejected();
        bim(&[&"show", &dir, &"ok", &"--json"]).rejected();
    }
}

#[test]
fn extension_kinds_and_props_are_checked_by_apply() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let h_core = hash(&dir);
    install(&dir, "fire", FIRE);
    install(&dir, "acoustic", ACOUSTIC);
    assert_eq!(hash(&dir), h_core, "installing a package changes no entity");

    let ok = s.write(
        "ok.json",
        r#"{"schema": 1, "ops": [
            {"op": "create", "id": "S1", "kind": "fire:Sprinkler", "props": {"level": {"ref": "lvl"}, "x": 1.5, "coverage_radius": 2.5}},
            {"op": "create", "id": "Z1", "kind": "fire:Zone", "props": {"name": "north", "active": true, "rooms": [{"ref": "lvl"}, 1, []]}},
            {"op": "set", "id": "Z1", "key": "rooms", "value": []},
            {"op": "create", "id": "AP", "kind": "acoustic:Panel", "props": {"nrc": 0.85}},
            {"op": "create", "id": "AB", "kind": "acoustic:Baffle"},
            {"op": "delete", "id": "AB"}]}"#,
    );
    bim(&[&"apply", &dir, &ok]).expect(0);
    bim(&[&"apply", &dir, &s.write("ok.ops", "create S2 fire:Sprinkler\nset S2 x num 3\nset S2 level ref lvl\nset Z1 name text south\n")]).expect(0);
    assert_eq!(
        show(&dir, "S1"),
        r#"{"id":"S1","kind":"fire:Sprinkler","props":{"_name":"S1","_schema":1.0,"coverage_radius":2.5,"level":{"ref":"lvl"},"x":1.5}}"#
    );
    bim(&[&"show", &dir, &"AB", &"--json"]).rejected();
    // Core kinds stay unchecked next to packages.
    bim(&[&"apply", &dir, &s.write("core.ops", "set wall height text tall\nset wall host num 1\ncreate k Shape\nset k colour text red\n")]).expect(0);
    bim(&[&"validate", &dir]).expect(0);

    let before = (journal(&dir), hash(&dir));
    let bad = [
        ("a kind the package does not declare", "create H fire:Hose\n", "bad.ops:1: unknown kind `fire:Hose`"),
        ("a kind of another package's namespace", "create H acoustic:Sprinkler\n", "unknown kind `acoustic:Sprinkler`"),
        ("a num given as text", "create H fire:Sprinkler\nset H x text 1.5\n", "bad.ops:2: prop `x` of `fire:Sprinkler` must be num, got text"),
        ("a num given as a ref", "set S1 z ref lvl\n", "bad.ops:1: prop `z` of `fire:Sprinkler` must be num, got ref"),
        ("a ref given as text", "set S1 level text lvl\n", "must be ref, got text"),
        ("a text given as a num", "set Z1 name num 4\n", "prop `name` of `fire:Zone` must be text, got num"),
        ("an undeclared prop", "set S1 colour text red\n", "bad.ops:1: kind `fire:Sprinkler` declares no prop `colour`"),
        ("a prop of another kind", "create H fire:Zone\nset H x num 1\n", "kind `fire:Zone` declares no prop `x`"),
        ("a valid create then an invalid set", "create H fire:Sprinkler\nset H x num 1\nset H y text a\n", "bad.ops:3: prop `y`"),
        ("a valid set then an invalid set", "set S1 x num 9\nset S1 y text north\n", "bad.ops:2:"),
        ("a reserved key on an extension entity", "set S1 _name text evil\n", "reserved"),
        ("an unknown entity is still reported as such", "set ghost x text 1\n", "bad.ops:1: entity `ghost` not found"),
    ];
    for (what, text, message) in bad {
        let run = bim(&[&"apply", &dir, &s.write("bad.ops", text)]).rejected();
        assert!(run.err.contains(message), "{what}: {}", run.err);
        assert_eq!((journal(&dir), hash(&dir)), before, "{what}");
    }
    let wrong = |what: &str, ops: &str, message: &str| {
        let run = bim(&[&"apply", &dir, &s.write("bad.json", format!(r#"{{"schema": 1, "ops": [{ops}]}}"#))]).rejected();
        assert!(run.err.contains(message), "{what}: {}", run.err);
        assert_eq!((journal(&dir), hash(&dir)), before, "{what}");
    };
    wrong("a num prop given as text", r#"{"op": "create", "id": "H", "kind": "fire:Sprinkler", "props": {"x": "1.5"}}"#, "bad.json: ops[0]: prop `x` of `fire:Sprinkler` must be num, got text");
    wrong("a num prop given as a bool", r#"{"op": "create", "id": "H", "kind": "fire:Sprinkler", "props": {"z": true}}"#, "must be num, got bool");
    wrong("a num prop given as a list", r#"{"op": "create", "id": "H", "kind": "fire:Sprinkler", "props": {"coverage_radius": [2.5]}}"#, "must be num, got list");
    wrong("a text prop given as a number", r#"{"op": "create", "id": "H", "kind": "fire:Zone", "props": {"name": 3}}"#, "prop `name` of `fire:Zone` must be text, got num");
    wrong("a bool prop given as text", r#"{"op": "create", "id": "H", "kind": "fire:Zone", "props": {"active": "true"}}"#, "must be bool, got text");
    wrong("a list prop given as a ref", r#"{"op": "set", "id": "Z1", "key": "rooms", "value": {"ref": "lvl"}}"#, "must be list, got ref");
    wrong("an undeclared prop at create", r#"{"op": "create", "id": "H", "kind": "fire:Sprinkler", "props": {"x": 1, "colour": "red"}}"#, "declares no prop `colour`");
    wrong(
        "a valid create and set before an invalid set",
        r#"{"op": "create", "id": "H", "kind": "fire:Sprinkler", "props": {"x": 1}}, {"op": "set", "id": "S1", "key": "x", "value": 9}, {"op": "set", "id": "S1", "key": "y", "value": "north"}"#,
        "bad.json: ops[2]: prop `y` of `fire:Sprinkler` must be num, got text",
    );
    wrong("a reserved prop at create", r#"{"op": "create", "id": "H", "kind": "fire:Zone", "props": {"_schema": 2}}"#, "reserved");
    bim(&[&"show", &dir, &"H", &"--json"]).rejected();
    assert!(show(&dir, "S1").contains(r#""x":1.5"#), "a rejected file changed nothing");
}

#[test]
fn an_entity_is_checked_against_the_kind_it_has_when_the_op_is_reached() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    install(&dir, "fire", FIRE);
    // The id is reused: a core entity, deleted, comes back as an extension entity, and the sets that
    // follow are checked against the new kind.
    let ok = "create a Wall\nset a x text north\ndelete a\ncreate a fire:Sprinkler\nset a x num 1\n";
    bim(&[&"apply", &dir, &s.write("ok.ops", ok)]).expect(0);
    assert!(show(&dir, "a").contains(r#""kind":"fire:Sprinkler""#));
    let run = bim(&[&"apply", &dir, &s.write("bad.ops", "delete a\ncreate a fire:Sprinkler\nset a x text north\n")]).rejected();
    assert!(run.err.contains("bad.ops:3: prop `x` of `fire:Sprinkler` must be num, got text"), "{}", run.err);
    // And the other way round: once it is a core entity again, nothing is checked.
    let run = bim(&[&"apply", &dir, &s.write("again.ops", "delete a\ncreate a Wall\nset a x text north\nset a y text fine\n")]).expect(0);
    assert_eq!(run.out, "");
    assert!(show(&dir, "a").contains(r#""kind":"Wall""#));
}

#[test]
fn removing_a_package_leaves_the_project_readable() {
    let s = Scratch::new();
    let dir = s.project("p", "create L1 Level\n");
    install(&dir, "fire", FIRE);
    bim(&[&"apply", &dir, &s.write("e.ops", "create S1 fire:Sprinkler\nset S1 x num 1\nset S1 level ref L1\n")]).expect(0);
    let h = hash(&dir);
    fs::remove_dir_all(dir.join("plugins")).unwrap();
    // The journal is replayed without kind checks: nothing is lost with the package.
    assert_eq!(hash(&dir), h);
    bim(&[&"validate", &dir]).expect(0);
    assert!(show(&dir, "S1").contains(r#""kind":"fire:Sprinkler""#));
    // New creates need a declared kind; sets on the orphaned entity have nothing to be checked against.
    bim(&[&"apply", &dir, &s.write("new.ops", "create S2 fire:Sprinkler\n")]).rejected();
    bim(&[&"apply", &dir, &s.write("more.ops", "set S1 x text anything\nset S1 colour text red\n")]).expect(0);
    // A journal that was never written through `bim apply` is replayed as it is.
    let copy = s.project("q", "");
    fs::write(copy.join("doc.oplog"), "create a Bogus\nset a h num 1\n").unwrap();
    assert_eq!(hash(&copy).len(), 16);
    bim(&[&"validate", &copy]).expect(0);
    assert!(show(&copy, "a").contains(r#""kind":"Bogus""#));
}

#[test]
fn a_malformed_or_inconsistent_package_makes_every_command_exit_2_naming_its_file() {
    let s = Scratch::new();
    let good = s.project("good", A_OPS);
    let mep = FIRE.replace("fire", "mep");
    // (defect, package directory, its trade.json; `None`: the directory has none)
    let cases = [
        ("malformed JSON", "fire", Some("{\"package\": \"fire\", \"kinds\": {".to_string())),
        ("a directory that is not the package name", "fire2", Some(FIRE.to_string())),
        ("a built-in trade", "mep", Some(mep)),
        ("a wrong abi", "fire", Some(FIRE.replace("trade@0.1.0", "trade@0.2.0"))),
        ("an unknown prop type", "fire", Some(FIRE.replace(r#""coverage_radius": "num""#, r#""coverage_radius": "float""#))),
        ("an unknown geometry template", "fire", Some(FIRE.replace(r#""box""#, r#""sphere""#))),
        ("a geometry entry that names no prop", "fire", Some(FIRE.replace(r#"["x", "y", "z"]"#, r#"["x", "y", "w"]"#))),
        ("a kind outside the namespace", "fire", Some(FIRE.replace(r#""fire:Zone""#, r#""acoustic:Zone""#))),
        ("a kind without a namespace", "fire", Some(FIRE.replace(r#""fire:Zone""#, r#""Zone""#))),
        ("a package directory without a trade.json", "fire", None),
    ];
    for (what, package, text) in cases {
        let dir = s.project("broken", A_OPS);
        install(&dir, "acoustic", ACOUSTIC); // a valid package does not hide the broken one
        match text {
            Some(text) => install(&dir, package, &text),
            None => fs::create_dir_all(dir.join("plugins").join(package)).unwrap(),
        }
        let file = format!("{package}/trade.json");
        let ops = s.write("fine.ops", "create extra Wall\n");
        let before = journal(&dir);
        let commands: [Vec<&dyn AsRef<OsStr>>; 7] = [
            vec![&"hash", &dir],
            vec![&"apply", &dir, &ops],
            vec![&"show", &dir, &"wall", &"--json"],
            vec![&"validate", &dir],
            vec![&"diff", &dir, &good],
            vec![&"diff", &good, &dir],
            vec![&"plugin", &"list", &dir, &"--json"],
        ];
        for args in commands {
            let run = bim(&args).rejected();
            assert!(run.err.contains(&file), "{what}: stderr does not name {file}: {}", run.err);
        }
        assert_eq!(journal(&dir), before, "{what}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn stray_files_and_dot_directories_in_plugins_are_not_packages() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let plugins = dir.join("plugins");
    fs::create_dir_all(plugins.join(".git")).unwrap();
    fs::write(plugins.join("README.md"), "notes").unwrap();
    let run = bim(&[&"plugin", &"list", &dir, &"--json"]).expect(0);
    assert_eq!(run.out, format!("{{\"packages\":[],\"core_kinds\":{}}}\n", core_kinds_json()));
    bim(&[&"hash", &dir]).expect(0);
}

#[test]
fn plugins_that_is_not_a_directory_exits_2() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    fs::write(dir.join("plugins"), "x").unwrap();
    let run = bim(&[&"hash", &dir]).rejected();
    assert!(run.err.contains("plugins: not a directory"), "{}", run.err);
}

#[test]
fn plugin_list_reports_the_packages_and_the_core_kinds() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let empty = format!("{{\"packages\":[],\"core_kinds\":{}}}\n", core_kinds_json());
    let run = bim(&[&"plugin", &"list", &dir, &"--json"]).expect(0);
    assert_eq!((run.out.as_str(), run.err.as_str()), (empty.as_str(), ""));

    install(&dir, "fire", FIRE);
    install(&dir, "acoustic", ACOUSTIC);
    let want = format!(
        concat!(
            r#"{{"packages":[{{"name":"acoustic","version":"2.1.0","abi":"openbim:trade@0.1.0","kinds":["acoustic:Baffle","acoustic:Panel"],"engine":null}},"#,
            r#"{{"name":"fire","version":"0.1.0","abi":"openbim:trade@0.1.0","kinds":["fire:Sprinkler","fire:Zone"],"engine":"plugins/fire/fire.wasm"}}],"#,
            r#""core_kinds":{}}}"#,
            "\n"
        ),
        core_kinds_json()
    );
    for args in [
        vec![&"plugin" as &dyn AsRef<OsStr>, &"list", &dir, &"--json"],
        vec![&"plugin", &"--json", &"list", &dir],
        vec![&"plugin", &"list", &dir],
    ] {
        let run = bim(&args).expect(0);
        assert_eq!((run.out.as_str(), run.err.as_str()), (want.as_str(), ""));
    }
    // `plugin list` reads only the directory and its plugins: no journal is needed.
    let bare = s.path("bare");
    fs::create_dir(&bare).unwrap();
    install(&bare, "acoustic", ACOUSTIC);
    assert!(bim(&[&"plugin", &"list", &bare, &"--json"]).expect(0).out.contains(r#""name":"acoustic""#));
    bim(&[&"plugin", &"list", &s.path("missing"), &"--json"]).rejected();
    let file = s.write("file", "x");
    bim(&[&"plugin", &"list", &file, &"--json"]).rejected();
}

// ---- command line -------------------------------------------------------------------------

#[test]
fn usage_errors_exit_2() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    let ops = s.write("a.ops", A_OPS);
    let cases: Vec<Vec<&dyn AsRef<OsStr>>> = vec![
        vec![],
        vec![&"frobnicate"],
        vec![&"--help"],
        vec![&"init"],
        vec![&"init", &dir, &dir],
        vec![&"apply"],
        vec![&"apply", &dir],
        vec![&"apply", &dir, &ops, &ops],
        vec![&"hash"],
        vec![&"hash", &dir, &dir],
        vec![&"hash", &dir, &"--json"],
        vec![&"validate"],
        vec![&"validate", &dir, &dir],
        vec![&"diff"],
        vec![&"diff", &dir],
        vec![&"diff", &dir, &dir, &dir],
        vec![&"plugin"],
        vec![&"plugin", &"list"],
        vec![&"plugin", &"list", &dir, &dir],
        vec![&"plugin", &"remove", &dir],
        vec![&"plugin", &"--json"],
        vec![&"diff", &dir, &dir, &"--json"],
        vec![&"diff", &dir, &"0123456789abcdef", &"ABCDEF0123456789", &"--json"],
        vec![&"diff", &dir, &"0123456789abcdef", &"0123456789abcde", &"--json"],
        vec![&"diff", &dir, &"0123456789abcdef", &"0123456789abcdeg"],
        vec![&"commit"],
        vec![&"commit", &dir],
        vec![&"commit", &"-m", &"msg"],
        vec![&"commit", &dir, &"-m"],
        vec![&"commit", &dir, &dir, &"-m", &"msg"],
        vec![&"commit", &dir, &"-m", &"a", &"-m", &"b"],
        vec![&"commit", &dir, &"-m", &"msg", &"--json"],
        vec![&"log"],
        vec![&"log", &dir, &dir],
        vec![&"log", &"--json"],
        vec![&"sync"],
        vec![&"sync", &dir],
        vec![&"sync", &dir, &"ws://127.0.0.1:1", &"extra"],
        vec![&"sync", &dir, &"http://127.0.0.1:1"],
        vec![&"sync", &dir, &"wss://127.0.0.1:1"],
        vec![&"sync", &"serve"],
        vec![&"sync", &"serve", &dir],
        vec![&"sync", &"serve", &dir, &"--port"],
        vec![&"sync", &"serve", &dir, &"--port", &"x"],
        vec![&"sync", &"serve", &dir, &"--port", &"-1"],
        vec![&"sync", &"serve", &dir, &"--port", &"65536"],
        vec![&"sync", &"serve", &"--port", &"1"],
        vec![&"sync", &"serve", &dir, &dir, &"--port", &"1"],
        vec![&"sync", &"serve", &dir, &"--port", &"1", &"--port", &"2"],
        vec![&"sync", &dir, &"--port", &"1"],
    ];
    for args in cases {
        let run = bim(&args).rejected();
        assert!(run.err.contains("usage:"), "{}", run.err);
    }
    assert!(journal(&dir).is_empty());
}

#[cfg(unix)]
#[test]
fn paths_that_are_not_utf8_do_not_panic() {
    use std::os::unix::ffi::OsStrExt;
    let s = Scratch::new();
    let dir = s.0.join(OsStr::from_bytes(b"p\xff\xfe"));
    let ops = s.0.join(OsStr::from_bytes(b"ops\xff.txt"));
    fs::write(&ops, A_OPS).unwrap();
    bim(&[&"init", &dir]).expect(0);
    bim(&[&"apply", &dir, &ops]).expect(0);
    assert_eq!(hash(&dir).len(), 16);
    bim(&[&"validate", &dir]).expect(0);
    bim(&[&"diff", &dir, &dir]).expect(0);
    // A non-UTF-8 command name is a usage error, not a crash.
    let run = bim(&[&OsStr::from_bytes(b"\xff\xfe")]).rejected();
    assert!(run.err.contains("unknown command"), "{}", run.err);
    // A missing non-UTF-8 directory is an ordinary exit 2.
    bim(&[&"hash", &s.0.join(OsStr::from_bytes(b"gone\xff"))]).rejected();
}

#[cfg(target_os = "linux")]
#[test]
fn a_stdout_that_cannot_be_written_is_an_error_not_a_panic() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let Ok(full) = fs::OpenOptions::new().write(true).open("/dev/full") else { return };
    let output = Command::new(BIM).args(["hash".as_ref(), dir.as_os_str()]).stdout(full).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot write to standard output"));
}

// ---- store, commit, log, revision diff (T3.2) -----------------------------------------------

/// One entry of `bim log --json`.
#[derive(Debug, PartialEq)]
struct Rev {
    rev: String,
    message: String,
    ops: Vec<String>,
}

fn strings(value: &Json) -> Vec<String> {
    let Json::Array(items) = value else { panic!("not an array: {value:?}") };
    items.iter().map(|item| if let Json::Str(s) = item { s.clone() } else { panic!("not a string: {item:?}") }).collect()
}

fn member<'a>(object: &'a Json, name: &str) -> &'a Json {
    let Json::Object(members) = object else { panic!("not an object: {object:?}") };
    &members.iter().find(|(key, _)| key == name).unwrap_or_else(|| panic!("no member {name}")).1
}

fn log(dir: &Path) -> Vec<Rev> {
    let run = bim(&[&"log", &dir, &"--json"]).expect(0);
    assert_eq!(run.err, "");
    let text = run.out.strip_suffix('\n').expect("one line");
    assert!(!text.contains('\n'));
    let Json::Array(entries) = json::parse(text).expect("log prints JSON") else { panic!("log is not an array") };
    entries
        .iter()
        .map(|entry| {
            let Json::Str(rev) = member(entry, "rev") else { panic!("rev") };
            let Json::Str(message) = member(entry, "message") else { panic!("message") };
            Rev { rev: rev.clone(), message: message.clone(), ops: strings(member(entry, "ops")) }
        })
        .collect()
}

fn commit(dir: &Path, message: &str) -> String {
    let run = bim(&[&"commit", &dir, &"-m", &message]).expect(0);
    assert_eq!(run.err, "");
    let rev = run.out.strip_suffix('\n').expect("one line").to_string();
    assert_eq!(rev, hash(dir), "a revision id is the content hash");
    rev
}

fn store_bytes(dir: &Path) -> Vec<u8> {
    fs::read(dir.join("bim.sqlite")).unwrap()
}

fn ids(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| op_id(line)).collect()
}

/// `bim diff <dir> <a> <b> --json` with its exit code, parsed as (added, removed, changed).
fn rev_diff(dir: &Path, a: &str, b: &str, code: i32) -> (Vec<String>, Vec<String>, Vec<(String, Vec<String>)>) {
    let run = bim(&[&"diff", &dir, &a, &b, &"--json"]).expect(code);
    assert_eq!(run.err, "");
    let doc = json::parse(run.out.strip_suffix('\n').expect("one line")).expect("JSON");
    let Json::Array(changed) = member(&doc, "changed") else { panic!("changed") };
    let changed = changed.iter().map(|c| {
        let Json::Str(id) = member(c, "id") else { panic!("id") };
        (id.clone(), strings(member(c, "keys")))
    });
    (strings(member(&doc, "added")), strings(member(&doc, "removed")), changed.collect())
}

#[test]
fn init_creates_a_sqlite_store_next_to_the_journal() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    assert!(store_bytes(&dir).starts_with(b"SQLite format 3\0"));
    assert_eq!(log(&dir), []);
    // init is idempotent: the store is not rewritten ...
    let before = store_bytes(&dir);
    bim(&[&"init", &dir]).expect(0);
    assert_eq!(store_bytes(&dir), before);
    // ... and a project that predates the store gets one, with the journal left as it was.
    let old = s.path("old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("doc.oplog"), "create a Wall\n").unwrap();
    bim(&[&"init", &old]).expect(0);
    assert!(store_bytes(&old).starts_with(b"SQLite format 3\0"));
    assert_eq!(journal(&old), b"create a Wall\n");
    // The store is where `init` records no revision.
    assert_eq!(log(&old), []);
    // init still refuses a path that cannot be a project, and then creates no store.
    let blocked = s.path("blocked");
    fs::create_dir_all(blocked.join("doc.oplog")).unwrap();
    bim(&[&"init", &blocked]).rejected();
    assert!(!blocked.join("bim.sqlite").exists());
    // Likewise when something that is not a file stands where the store goes: nothing is created.
    let obstructed = s.path("obstructed");
    fs::create_dir_all(obstructed.join("bim.sqlite")).unwrap();
    let run = bim(&[&"init", &obstructed]).rejected();
    assert!(run.err.contains("bim.sqlite") && run.err.contains("not a file"), "{}", run.err);
    assert_eq!(files_in(&obstructed), ["bim.sqlite"]);
}

#[test]
fn commit_prints_the_content_hash_and_log_lists_revisions_oldest_first() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    let empty = commit(&dir, "empty");
    bim(&[&"apply", &dir, &s.write("1.ops", A_OPS)]).expect(0);
    let first = commit(&dir, "first");
    bim(&[&"apply", &dir, &s.write("2.ops", "set wall height num 4\ncreate door Opening\n")]).expect(0);
    let message = "second: \"quoted\", back\\slash, ünïcode, tab\tand spaces";
    let second = commit(&dir, message);
    assert_eq!(empty, hash(&s.project("q", "")), "an empty document's revision");
    let all = log(&dir);
    assert_eq!(all.iter().map(|r| r.rev.as_str()).collect::<Vec<_>>(), [empty.as_str(), &first, &second]);
    assert_eq!(all.iter().map(|r| r.message.as_str()).collect::<Vec<_>>(), ["empty", "first", message]);
    assert_eq!(all[0].ops, Vec::<String>::new());
    assert_eq!(
        all[1].ops,
        ids(&[
            "create wall Wall",
            "create lvl Level",
            "set wall height num 3.5",
            "set wall host ref lvl",
            "set wall name text north"
        ])
    );
    assert_eq!(all[2].ops, ids(&["set wall height num 4.0", "create door Opening"]));
    // Everything is one line of JSON with 16-hex ids, whatever the message holds.
    for rev in all.iter().flat_map(|r| std::iter::once(&r.rev).chain(&r.ops)) {
        assert!(rev.len() == 16 && rev.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{rev}");
    }
    // `--json` is optional for log, and may stand anywhere.
    let plain = bim(&[&"log", &dir]).expect(0).out;
    assert_eq!(plain, bim(&[&"log", &dir, &"--json"]).expect(0).out);
    assert_eq!(plain, bim(&[&"log", &"--json", &dir]).expect(0).out);
    // The message may come before the directory too, and be empty.
    let third = bim(&[&"commit", &"-m", &"", &dir]).expect(0).out;
    assert_eq!(third.trim_end(), second);
    assert_eq!(log(&dir)[3].message, "");
}

#[test]
fn op_ids_are_content_addressed_by_the_canonical_op_text() {
    let s = Scratch::new();
    let (a, b) = (s.project("a", ""), s.project("b", ""));
    // The same ops, written differently, in two projects: the same ids.
    bim(&[&"apply", &a, &s.write("a.ops", "create  w\tWall\r\n\r\n set w  h num 3\n")]).expect(0);
    bim(&[&"apply", &b, &s.write("b.json", r#"{"schema":1,"ops":[{"op":"create","id":"w","kind":"Wall","props":{"h":3}}]}"#)]).expect(0);
    commit(&a, "a");
    commit(&b, "b");
    let want = ids(&["create w Wall", "set w h num 3.0"]);
    assert_eq!(log(&a)[0].ops, want);
    assert_eq!(log(&b)[0].ops, want);
    // An op with no word form is hashed in its JSON form.
    bim(&[&"apply", &a, &s.write("c.json", r#"{"schema":1,"ops":[{"op":"set","id":"w","key":"label","value":"two words"}]}"#)]).expect(0);
    commit(&a, "c");
    assert_eq!(log(&a)[1].ops, ids(&[r#"{"op":"set","id":"w","key":"label","value":"two words"}"#]));
    // An op applied twice has one id both times: the log lists it twice, the store keeps it once.
    bim(&[&"apply", &a, &s.write("d.ops", "set w h num 4\nset w h num 3\nset w h num 4\n")]).expect(0);
    commit(&a, "d");
    let four = op_id("set w h num 4.0");
    assert_eq!(log(&a)[2].ops, [four.clone(), op_id("set w h num 3.0"), four]);
}

#[test]
fn committing_without_changes_still_records_a_revision() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let first = commit(&dir, "one");
    let again = commit(&dir, "two");
    assert_eq!(first, again);
    let all = log(&dir);
    assert_eq!((all.len(), all[1].message.as_str(), all[1].ops.len()), (2, "two", 0));
    assert_eq!(all[0].ops.len(), 5);
    // Going back to an earlier document reproduces its revision id.
    bim(&[&"apply", &dir, &s.write("x.ops", "set wall height num 9\n")]).expect(0);
    let changed = commit(&dir, "changed");
    assert_ne!(changed, first);
    bim(&[&"apply", &dir, &s.write("y.ops", "set wall height num 3.5\n")]).expect(0);
    assert_eq!(commit(&dir, "reverted"), first);
    assert_eq!(log(&dir).len(), 4);
    rev_diff(&dir, &first, &first, 0);
    assert_eq!(rev_diff(&dir, &changed, &first, 1).2, [("wall".to_string(), vec!["height".to_string()])]);
}

#[test]
fn a_rejected_apply_changes_neither_the_log_nor_the_store() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    commit(&dir, "first");
    let before = (store_bytes(&dir), journal(&dir), bim(&[&"log", &dir]).expect(0).out);
    let cases = [
        ("a bad line", "set wall height num 4\nbogus op here\n"),
        ("a refused op", "set wall height num 4\nset ghost x num 1\n"),
        ("a duplicate create", "create fresh W\ncreate fresh W\n"),
        ("a reserved key", "set wall _name text evil\n"),
    ];
    for (what, text) in cases {
        bim(&[&"apply", &dir, &s.write("bad.ops", text)]).rejected();
        let after = (store_bytes(&dir), journal(&dir), bim(&[&"log", &dir]).expect(0).out);
        assert_eq!(after, before, "{what}");
    }
    // A file with no ops does not touch the store either.
    bim(&[&"apply", &dir, &s.write("blank.ops", "\n  \n")]).expect(0);
    assert_eq!((store_bytes(&dir), journal(&dir)), (before.0.clone(), before.1.clone()));
}

#[test]
fn revision_diffs_name_entities_and_the_keys_that_changed() {
    let s = Scratch::new();
    let dir = s.project("p", "create L1 Level\nset L1 elevation num 0\ncreate W1 Wall\nset W1 height num 3\ncreate W3 Wall\n");
    let one = commit(&dir, "one");
    bim(&[&"apply", &dir, &s.write("2.ops", "set W1 height num 3.5\nset W1 name text north\ncreate W2 Wall\ndelete W3\nset L1 elevation num -0\n")]).expect(0);
    let two = commit(&dir, "two");
    // The exact text: one line, ids sorted, keys sorted.
    let run = bim(&[&"diff", &dir, &one, &two, &"--json"]).expect(1);
    assert_eq!(run.out, "{\"added\":[\"W2\"],\"removed\":[\"W3\"],\"changed\":[{\"id\":\"W1\",\"keys\":[\"height\",\"name\"]}]}\n");
    // `-0` and `0` are one number: L1 did not change. The other way round swaps added and removed.
    let (added, removed, changed) = rev_diff(&dir, &two, &one, 1);
    assert_eq!((added, removed), (vec!["W3".to_string()], vec!["W2".to_string()]));
    assert_eq!(changed, [("W1".to_string(), vec!["height".to_string(), "name".to_string()])]);
    // `--json` may stand anywhere, and the output is the same without it.
    assert_eq!(bim(&[&"diff", &"--json", &dir, &one, &two]).expect(1).out, run.out);
    assert_eq!(bim(&[&"diff", &dir, &one, &two]).expect(1).out, run.out);
    // A revision never differs from itself: exit 0 and an empty document.
    let same = bim(&[&"diff", &dir, &two, &two, &"--json"]).expect(0);
    assert_eq!(same.out, "{\"added\":[],\"removed\":[],\"changed\":[]}\n");
    // Names with characters JSON must escape.
    bim(&[&"apply", &dir, &s.write("3.json", r#"{"schema":1,"ops":[{"op":"create","id":"q \"1\"","kind":"Wall","props":{"a b":1}}]}"#)]).expect(0);
    let three = commit(&dir, "three");
    assert_eq!(rev_diff(&dir, &two, &three, 1).0, ["q \"1\""]);
    // A kind that changed shows as `_kind`.
    bim(&[&"apply", &dir, &s.write("4.ops", "delete W1\ncreate W1 Slab\nset W1 height num 3.5\nset W1 name text north\n")]).expect(0);
    let four = commit(&dir, "four");
    assert_eq!(rev_diff(&dir, &three, &four, 1).2, [("W1".to_string(), vec!["_kind".to_string()])]);
}

#[test]
fn revision_diff_errors_exit_2_and_say_why() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let rev = commit(&dir, "one");
    let unknown = "0123456789abcdef";
    for (a, b) in [(rev.as_str(), unknown), (unknown, rev.as_str()), (unknown, unknown)] {
        let run = bim(&[&"diff", &dir, &a, &b, &"--json"]).rejected();
        assert!(run.err.contains(&format!("unknown revision `{unknown}`")), "{}", run.err);
        assert!(!run.err.contains("usage:"), "{}", run.err);
    }
    // Not a revision id at all: a usage error, and nothing is looked up.
    for bad in ["x", "0123456789ABCDEF", "0123456789abcde", "0123456789abcdef0", "", "wall"] {
        let run = bim(&[&"diff", &dir, &bad, &rev, &"--json"]).rejected();
        assert!(run.err.contains("usage:") && run.err.contains("not a revision id"), "{bad}: {}", run.err);
    }
    // A directory that is not a project, and a project that never had a store.
    bim(&[&"diff", &s.path("missing"), &rev, &rev, &"--json"]).rejected();
    let bare = s.path("bare");
    fs::create_dir(&bare).unwrap();
    bim(&[&"diff", &bare, &rev, &rev, &"--json"]).rejected();
    let old = s.path("old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("doc.oplog"), "").unwrap();
    let run = bim(&[&"diff", &old, &rev, &rev, &"--json"]).rejected();
    assert!(run.err.contains("unknown revision"), "{}", run.err);
    assert!(!old.join("bim.sqlite").exists(), "reading revisions never creates the store");
    // The two-directory form is still there, and takes no --json.
    bim(&[&"diff", &dir, &dir]).expect(0);
    bim(&[&"diff", &dir, &dir, &"--json"]).rejected();
}

#[test]
fn log_and_the_read_commands_never_create_or_change_the_store() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    commit(&dir, "one");
    let (store, journal_before) = (store_bytes(&dir), journal(&dir));
    let modified = fs::metadata(dir.join("bim.sqlite")).unwrap().modified().unwrap();
    for args in [
        vec![&"hash" as &dyn AsRef<OsStr>, &dir],
        vec![&"validate", &dir],
        vec![&"show", &dir, &"wall", &"--json"],
        vec![&"diff", &dir, &dir],
        vec![&"log", &dir, &"--json"],
    ] {
        bim(&args).expect(0);
    }
    assert_eq!(store_bytes(&dir), store);
    assert_eq!(journal(&dir), journal_before);
    assert_eq!(fs::metadata(dir.join("bim.sqlite")).unwrap().modified().unwrap(), modified);
    assert_eq!(files_in(&dir), ["bim.sqlite", "doc.oplog"], "no journal file, no temporary file");
    // A project without a store: reading creates none, `log` is empty.
    let old = s.path("old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("doc.oplog"), A_OPS).unwrap();
    for args in [
        vec![&"hash" as &dyn AsRef<OsStr>, &old],
        vec![&"validate", &old],
        vec![&"show", &old, &"wall", &"--json"],
        vec![&"diff", &old, &dir],
        vec![&"log", &old, &"--json"],
    ] {
        bim(&args).expect(0);
    }
    assert_eq!(files_in(&old), ["doc.oplog"]);
    assert_eq!(bim(&[&"log", &old]).expect(0).out, "[]\n");
}

#[test]
fn commit_rebuilds_a_missing_store_from_the_journal() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    commit(&dir, "before");
    fs::remove_file(dir.join("bim.sqlite")).unwrap();
    assert_eq!(bim(&[&"log", &dir]).expect(0).out, "[]\n", "the revisions went with the file");
    let rev = commit(&dir, "after");
    assert_eq!(rev, hash(&dir));
    let all = log(&dir);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].ops, ids(&[
        "create wall Wall", "create lvl Level", "set wall height num 3.5", "set wall host ref lvl", "set wall name text north",
    ]));
    // apply works on a project whose store was deleted, too, and keeps both in step.
    fs::remove_file(dir.join("bim.sqlite")).unwrap();
    bim(&[&"apply", &dir, &s.write("more.ops", "set wall height num 4\n")]).expect(0);
    commit(&dir, "again");
    assert_eq!(log(&dir)[0].ops.len(), 6, "the journal's five ops and the new one");
    assert_state_matches_journal(&dir);
}

/// The store's state equals the document the journal replays to.
fn assert_state_matches_journal(dir: &Path) {
    let project = bim_cli::project::Project::open(dir).unwrap();
    let state = Store::open_existing(dir).unwrap().expect("a store").state().unwrap();
    assert_eq!(state.len(), project.doc.len());
    for entity in project.doc.entities() {
        let row = &state[&bim_store::entity_key(entity.id)];
        assert_eq!(row.kind, entity.kind);
        assert_eq!(row.name, project.name(entity.id));
        assert_eq!(row.props.len(), entity.props.len());
        for (key, value) in &entity.props {
            let mut text = String::new();
            bim_store::encode_value(value, &mut text);
            assert_eq!(row.props[key], text, "{key}");
        }
    }
}

#[test]
fn a_journal_ahead_of_the_store_is_caught_up_by_the_next_write() {
    // What a process killed between the journal rename and the SQLite commit leaves behind.
    let s = Scratch::new();
    let dir = s.project("p", "create a Wall\nset a h num 1\n");
    let behind = store_bytes(&dir);
    fs::write(dir.join("doc.oplog"), "create a Wall\nset a h num 1.0\nset a h num 2.0\ncreate b Wall\n").unwrap();
    assert_eq!(store_bytes(&dir), behind);
    bim(&[&"apply", &dir, &s.write("more.ops", "set b h num 3\n")]).expect(0);
    assert_state_matches_journal(&dir);
    let rev = commit(&dir, "caught up");
    assert_eq!(rev, hash(&dir));
    assert_eq!(
        log(&dir)[0].ops,
        ids(&["create a Wall", "set a h num 1.0", "set a h num 2.0", "create b Wall", "set b h num 3.0"])
    );
    // And a commit alone catches up as well.
    fs::write(dir.join("doc.oplog"), "create a Wall\nset a h num 1.0\nset a h num 2.0\ncreate b Wall\nset b h num 3.0\ndelete a\n").unwrap();
    commit(&dir, "after a delete");
    assert_state_matches_journal(&dir);
    assert_eq!(log(&dir)[1].ops, ids(&["delete a"]));
}

#[test]
fn a_hand_rewritten_journal_is_followed_by_the_store() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    let old = commit(&dir, "old");
    fs::write(dir.join("doc.oplog"), "create other Slab\nset other t num 0.2\n").unwrap();
    bim(&[&"apply", &dir, &s.write("more.ops", "set other t num 0.3\n")]).expect(0);
    assert_state_matches_journal(&dir);
    let new = commit(&dir, "new");
    assert_ne!(old, new);
    let all = log(&dir);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].ops.len(), 5, "the old revision keeps its ops");
    // The diff between them uses the snapshots, not a replay of any journal.
    let (added, removed, _) = rev_diff(&dir, &old, &new, 1);
    assert_eq!((added, removed), (vec!["other".to_string()], vec!["lvl".to_string(), "wall".to_string()]));
}

#[test]
fn a_broken_store_stops_the_store_commands_and_nothing_else() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    fs::write(dir.join("bim.sqlite"), "this is not a database, only text, but long enough to be read as one").unwrap();
    let journal_before = journal(&dir);
    for args in [
        vec![&"apply" as &dyn AsRef<OsStr>, &dir, &s.write("m.ops", "set wall height num 4\n")],
        vec![&"commit", &dir, &"-m", &"x"],
        vec![&"log", &dir, &"--json"],
        vec![&"diff", &dir, &"0123456789abcdef", &"0123456789abcdef", &"--json"],
    ] {
        let run = bim(&args).rejected();
        assert!(run.err.contains("bim.sqlite"), "{}", run.err);
    }
    assert_eq!(journal(&dir), journal_before, "a failed store write leaves the journal alone");
    // The journal is the document: the read commands do not care about the store.
    bim(&[&"hash", &dir]).expect(0);
    bim(&[&"validate", &dir]).expect(0);
    bim(&[&"diff", &dir, &dir]).expect(0);
    // A directory where the store should be is just as unusable.
    fs::remove_file(dir.join("bim.sqlite")).unwrap();
    fs::create_dir(dir.join("bim.sqlite")).unwrap();
    bim(&[&"apply", &dir, &s.write("m2.ops", "set wall height num 4\n")]).rejected();
    bim(&[&"log", &dir]).rejected();
    assert_eq!(journal(&dir), journal_before);
}

#[test]
fn a_corrupt_journal_stops_commit_and_log_too() {
    let s = Scratch::new();
    let dir = s.project("p", A_OPS);
    commit(&dir, "ok");
    fs::write(dir.join("doc.oplog"), "create a Wall\ngarbage\n").unwrap();
    let rev = "0123456789abcdef";
    for args in [
        vec![&"commit" as &dyn AsRef<OsStr>, &dir, &"-m", &"x"],
        vec![&"log", &dir],
        vec![&"diff", &dir, &rev, &rev, &"--json"],
    ] {
        let run = bim(&args).rejected();
        assert!(run.err.contains(":2: corrupt journal"), "{}", run.err);
    }
    assert_eq!(journal(&dir), b"create a Wall\ngarbage\n");
}

#[test]
fn a_project_directory_named_like_a_sqlite_uri_is_just_a_directory() {
    // SQLite reads a file name that starts with `file:` as a URI: `file:p?mode=memory/bim.sqlite`
    // would be a malformed request for the file `p`. The store must open it as a plain path.
    let s = Scratch::new();
    let in_scratch = |args: &[&str]| {
        let output = Command::new(BIM).args(args).current_dir(&s.0).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    };
    let dir = "file:p?mode=memory";
    in_scratch(&["init", dir]);
    fs::write(s.path("ops.txt"), "create a Wall\n").unwrap();
    in_scratch(&["apply", dir, "ops.txt"]);
    let rev = in_scratch(&["commit", dir, "-m", "uri"]);
    assert_eq!(files_in(&s.path(dir)), ["bim.sqlite", "doc.oplog"]);
    assert_eq!(files_in(&s.0), [dir, "ops.txt"], "nothing else was created next to it");
    assert!(in_scratch(&["log", dir]).contains(rev.trim_end()));
}

#[test]
fn concurrent_applies_never_lose_or_tear_anything() {
    let s = Scratch::new();
    let dir = s.project("p", "create a Wall\n");
    let files: Vec<PathBuf> =
        (0..8).map(|i| s.write(&format!("w{i}.ops"), format!("create w{i} Wall\nset w{i} h num {i}\n"))).collect();
    let runs: Vec<Run> = std::thread::scope(|scope| {
        let handles: Vec<_> = files.iter().map(|file| scope.spawn(|| bim(&[&"apply", &dir, file]))).collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    });
    let journal_text = String::from_utf8(journal(&dir)).unwrap();
    let mut won = 0;
    for (i, run) in runs.iter().enumerate() {
        let landed = journal_text.contains(&format!("create w{i} Wall\nset w{i} h num {i}.0\n"));
        match run.code {
            0 => {
                won += 1;
                assert!(landed, "w{i} exited 0 but its ops are not in the journal");
            }
            // A writer that lost the race changes nothing and says so (exit 2, never 3).
            2 => {
                assert!(!landed, "w{i} exited 2 but its ops are in the journal");
                assert!(run.err.contains("changed while") || run.err.contains("locked"), "{}", run.err);
            }
            other => panic!("w{i} exited {other}: {}", run.err),
        }
    }
    assert!(won >= 1, "some writer must win");
    assert_eq!(journal_text.lines().count(), 1 + 2 * won, "no op lost, none twice");
    assert_state_matches_journal(&dir);
    commit(&dir, "all");
    assert_eq!(log(&dir)[0].ops.len(), 1 + 2 * won);
    assert_eq!(files_in(&dir), ["bim.sqlite", "doc.oplog"]);
}

// ---- sync (T3.3) -------------------------------------------------------------------------------

/// A `bim sync serve` process on a project, killed when dropped.
struct Server {
    child: std::process::Child,
    port: u16,
    url: String,
}

impl Server {
    /// Starts the server on `dir` and waits for its `listening <port>` line.
    fn start(dir: &Path) -> Server {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        let mut child = Command::new(BIM)
            .arg("sync")
            .arg("serve")
            .arg(dir)
            .args(["--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn bim sync serve");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let (announce, announced) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = stdout.read_line(&mut line);
            let _ = announce.send(line);
            // Keep the pipe drained, so a chatty server could never block on it.
            let mut sink = String::new();
            while stdout.read_line(&mut sink).map(|n| n > 0).unwrap_or(false) {
                sink.clear();
            }
        });
        // A server that never announces itself must not hang the test run.
        let line = announced.recv_timeout(std::time::Duration::from_secs(60)).unwrap_or_else(|_| {
            let _ = child.kill();
            panic!("the server did not announce its port within 60 s")
        });
        let port: u16 = line
            .strip_prefix("listening ")
            .and_then(|rest| rest.trim_end().parse().ok())
            .unwrap_or_else(|| panic!("the first stdout line was {line:?}, not `listening <port>`"));
        assert_ne!(port, 0, "the real port is announced, never 0");
        Server { child, port, url: format!("ws://127.0.0.1:{port}") }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.kill();
    }
}

fn sync(dir: &Path, server: &Server) -> Run {
    bim(&[&"sync", &dir, &server.url.as_str()])
}

/// `bim validate` output, sorted, with the exit code.
fn validation(dir: &Path) -> (i32, Vec<String>) {
    let run = bim(&[&"validate", &dir]);
    assert!(run.code == 0 || run.code == 1, "validate exited {}: {}", run.code, run.err);
    let mut lines: Vec<String> = run.out.lines().map(str::to_string).collect();
    lines.sort();
    (run.code, lines)
}

const SYNC_BASE: &str = "create L1 Level\nset L1 elevation num 0\ncreate L2 Level\nset L2 elevation num 3\n\
                         create W1 Wall\nset W1 level ref L1\nset W1 height num 3\n";

#[test]
fn init_gives_each_project_a_persisted_peer_id_that_copies_share() {
    let s = Scratch::new();
    let (a, b) = (s.project("a", ""), s.project("b", ""));
    let peer = |dir: &Path| bim_sync::SyncDb::open_existing(&dir.join("bim.sqlite")).unwrap().unwrap().peer().unwrap();
    let (pa, pb) = (peer(&a).expect("a peer id"), peer(&b).expect("a peer id"));
    assert_ne!(pa, pb, "independent random ids");
    assert_ne!(pa, 0);
    // init again: the id stays, and the store is not even rewritten.
    let before = store_bytes(&a);
    bim(&[&"init", &a]).expect(0);
    assert_eq!(peer(&a), Some(pa));
    assert_eq!(store_bytes(&a), before);
    // Applying ops keeps it. A byte copy of the directory shares it.
    bim(&[&"apply", &a, &s.write("x.ops", "create x Wall\n")]).expect(0);
    assert_eq!(peer(&a), Some(pa));
    let copy = s.path("copy");
    fs::create_dir(&copy).unwrap();
    for file in ["doc.oplog", "bim.sqlite"] {
        fs::copy(a.join(file), copy.join(file)).unwrap();
    }
    assert_eq!(peer(&copy), Some(pa));
    // Many projects, many ids.
    let ids: std::collections::BTreeSet<u32> = (0..20).map(|i| peer(&s.project(&format!("p{i}"), "")).unwrap()).collect();
    assert_eq!(ids.len(), 20);
}

#[test]
fn two_projects_sync_through_a_server_and_end_equal() {
    let s = Scratch::new();
    let (srv, a, b) = (s.project("srv", ""), s.project("a", SYNC_BASE), s.project("b", ""));
    let server = Server::start(&srv);
    let run = sync(&a, &server).expect(0);
    assert_eq!((run.out.as_str(), run.err.as_str()), ("", ""), "a successful sync prints nothing");
    sync(&b, &server).expect(0);
    assert_eq!(hash(&a), hash(&b));
    assert_eq!(hash(&srv), hash(&a), "the server directory holds the merged state");
    assert_eq!(bim(&[&"diff", &a, &b]).expect(0).out, "same\n");
    // Edit on both sides, sync both ways.
    bim(&[&"apply", &a, &s.write("a.ops", "set W1 height num 4\ncreate extra Wall\n")]).expect(0);
    bim(&[&"apply", &b, &s.write("b.ops", "delete L2\nset W1 name text north\n")]).expect(0);
    assert_ne!(hash(&a), hash(&b));
    for dir in [&a, &b, &a] {
        sync(dir, &server).expect(0);
    }
    assert_eq!(hash(&a), hash(&b));
    assert_eq!(hash(&srv), hash(&a));
    assert_eq!(validation(&a), validation(&b));
    let shown = show(&a, "W1");
    assert_eq!(shown, show(&b, "W1"));
    assert!(shown.contains("\"name\":\"north\"") && shown.contains("\"extra\"") == false, "{shown}");
    assert!(shown.contains("\"height\":4"), "{shown}");
    bim(&[&"show", &b, &"extra", &"--json"]).expect(0);
    bim(&[&"show", &a, &"L2", &"--json"]).rejected();
    // A converged sync touches nothing: not a byte, not a modification time.
    let before: Vec<_> = [&srv, &a, &b].iter().map(|d| (journal(d), store_bytes(d))).collect();
    let modified: Vec<_> = [&a, &b].iter().map(|d| fs::metadata(d.join("doc.oplog")).unwrap().modified().unwrap()).collect();
    for dir in [&a, &b, &a, &b] {
        sync(dir, &server).expect(0);
    }
    let after: Vec<_> = [&srv, &a, &b].iter().map(|d| (journal(d), store_bytes(d))).collect();
    assert_eq!(before, after);
    let modified_after: Vec<_> = [&a, &b].iter().map(|d| fs::metadata(d.join("doc.oplog")).unwrap().modified().unwrap()).collect();
    assert_eq!(modified, modified_after);
    for dir in [&srv, &a, &b] {
        assert_eq!(files_in(dir), ["bim.sqlite", "doc.oplog"], "no temporary file is left behind");
    }
    // The store follows the journal, and revisions work on top of a synced project.
    assert_state_matches_journal(&b);
    let rev = commit(&b, "after sync");
    assert_eq!(rev, hash(&b));
    assert!(!log(&b)[0].ops.is_empty());
}

#[test]
fn an_unreachable_server_exits_2_and_leaves_the_directory_byte_identical() {
    let s = Scratch::new();
    let a = s.project("a", SYNC_BASE);
    bim(&[&"apply", &a, &s.write("more.ops", "set W1 height num 9\n")]).expect(0);
    // A port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let snapshot = |dir: &Path| (journal(dir), store_bytes(dir), files_in(dir));
    let before = snapshot(&a);
    let modified = (fs::metadata(a.join("doc.oplog")).unwrap().modified().unwrap(), fs::metadata(a.join("bim.sqlite")).unwrap().modified().unwrap());
    let run = bim(&[&"sync", &a, &format!("ws://127.0.0.1:{port}")]).rejected();
    assert!(run.err.contains("127.0.0.1"), "{}", run.err);
    assert_eq!(snapshot(&a), before);
    assert_eq!(
        (fs::metadata(a.join("doc.oplog")).unwrap().modified().unwrap(), fs::metadata(a.join("bim.sqlite")).unwrap().modified().unwrap()),
        modified
    );
    // A project that never had a store gets none from a failed sync.
    let old = s.path("old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("doc.oplog"), SYNC_BASE).unwrap();
    bim(&[&"sync", &old, &format!("ws://127.0.0.1:{port}")]).rejected();
    assert_eq!(files_in(&old), ["doc.oplog"]);
    assert_eq!(journal(&old), SYNC_BASE.as_bytes());
    // Something that listens but is not a WebSocket server, and one that hangs up at once.
    let junk = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let junk_port = junk.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for (n, stream) in junk.incoming().enumerate().take(2) {
            let mut stream = stream.unwrap();
            if n == 0 {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        }
    });
    bim(&[&"sync", &a, &format!("ws://127.0.0.1:{junk_port}")]).rejected();
    bim(&[&"sync", &a, &format!("ws://127.0.0.1:{junk_port}")]).rejected();
    assert_eq!(snapshot(&a), before);
}

#[test]
fn sync_refuses_what_is_not_a_project_or_not_a_ws_url_without_creating_anything() {
    let s = Scratch::new();
    let nothing = s.path("nothing");
    bim(&[&"sync", &nothing, &"ws://127.0.0.1:1"]).rejected();
    assert!(!nothing.exists());
    let empty = s.path("empty");
    fs::create_dir(&empty).unwrap();
    bim(&[&"sync", &empty, &"ws://127.0.0.1:1"]).rejected();
    assert_eq!(files_in(&empty), Vec::<String>::new());
    let a = s.project("a", "");
    for url in ["http://127.0.0.1:1", "wss://127.0.0.1:1", "127.0.0.1:1", "ws://", "ws://x:99999"] {
        let run = bim(&[&"sync", &a, &url]).rejected();
        assert!(run.err.contains("usage:"), "{url}: {}", run.err);
    }
    assert_eq!(files_in(&a), ["bim.sqlite", "doc.oplog"]);
    // The server refuses a directory that is not a project before it announces anything.
    let run = bim(&[&"sync", &"serve", &nothing, &"--port", &"0"]).rejected();
    assert!(run.err.contains("nothing"), "{}", run.err);
    assert!(!nothing.exists());
    bim(&[&"sync", &"serve", &empty, &"--port", &"0"]).rejected();
    assert_eq!(files_in(&empty), Vec::<String>::new());
}

#[test]
fn a_port_in_use_is_an_input_error_and_the_flag_may_come_first() {
    let s = Scratch::new();
    let srv = s.project("srv", "");
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port().to_string();
    let run = bim(&[&"sync", &"serve", &srv, &"--port", &port.as_str()]).rejected();
    assert!(run.err.contains("cannot listen"), "{}", run.err);
    let run = bim(&[&"sync", &"serve", &"--port", &port.as_str(), &srv]).rejected();
    assert!(run.err.contains("cannot listen"), "the flag may precede the directory: {}", run.err);
}

#[test]
fn a_killed_server_loses_nothing_a_finished_sync_delivered() {
    let s = Scratch::new();
    let (srv, a, b, c) = (s.project("srv", ""), s.project("a", SYNC_BASE), s.project("b", ""), s.project("c", ""));
    let mut server = Server::start(&srv);
    sync(&a, &server).expect(0);
    bim(&[&"apply", &b, &s.write("b0.ops", "create bee Wall\nset bee h num 2\n")]).expect(0);
    sync(&b, &server).expect(0);
    server.kill(); // SIGKILL: no chance to flush anything
    assert_eq!(files_in(&srv), ["bim.sqlite", "doc.oplog"], "no hot journal is left");
    let server = Server::start(&srv);
    sync(&c, &server).expect(0);
    assert_eq!(hash(&c), hash(&srv));
    for dir in [&a, &b, &a] {
        sync(dir, &server).expect(0);
    }
    assert_eq!(hash(&a), hash(&b));
    assert_eq!(hash(&a), hash(&c));
    assert_eq!(hash(&a), hash(&srv));
    show(&c, "bee");
    show(&c, "W1");
}

#[test]
fn a_server_survives_garbage_from_the_network() {
    use std::io::{Read, Write};
    let s = Scratch::new();
    let (srv, a, b) = (s.project("srv", ""), s.project("a", SYNC_BASE), s.project("b", ""));
    let server = Server::start(&srv);
    let garbage: [&[u8]; 5] = [
        b"",
        b"\xff\xfe\x00\x01 not http",
        b"GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n\x82\xff\xff\xff\xff\xff\xff\xff\xff",
        b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nbody",
        &[b'G'; 100_000],
    ];
    for bytes in garbage {
        let mut socket = std::net::TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        let _ = socket.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let _ = socket.write_all(bytes);
        let _ = socket.shutdown(std::net::Shutdown::Write);
        let mut sink = Vec::new();
        let _ = socket.read_to_end(&mut sink);
    }
    sync(&a, &server).expect(0);
    sync(&b, &server).expect(0);
    assert_eq!(hash(&a), hash(&b));
}

#[test]
fn concurrent_syncs_through_one_server_converge() {
    let s = Scratch::new();
    let srv = s.project("srv", "");
    let dirs: Vec<PathBuf> = (0..6).map(|i| s.project(&format!("p{i}"), if i == 0 { "create shared Wall\n" } else { "" })).collect();
    let server = Server::start(&srv);
    for dir in &dirs {
        sync(dir, &server).expect(0);
    }
    // Everyone now edits the shared entity and adds one of their own, offline.
    for (i, dir) in dirs.iter().enumerate() {
        bim(&[&"apply", dir, &s.write(&format!("e{i}.ops"), format!("set shared k num {i}\ncreate e{i} Wall\nset e{i} h num {i}\n"))]).expect(0);
    }
    for _ in 0..2 {
        let runs: Vec<Run> = std::thread::scope(|scope| {
            let handles: Vec<_> = dirs.iter().map(|d| scope.spawn(|| sync(d, &server))).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for run in runs {
            run.expect(0);
        }
    }
    for dir in &dirs {
        sync(dir, &server).expect(0);
    }
    let reference = hash(&srv);
    for dir in &dirs {
        assert_eq!(hash(dir), reference, "{}", dir.display());
    }
    assert!(show(&dirs[3], "e5").contains("\"h\":5"));
    // The contested key has exactly one winner, the same everywhere.
    let winner = show(&srv, "shared");
    for dir in &dirs {
        assert_eq!(show(dir, "shared"), winner);
    }
}

// ------------------------------------------------------------------ bim sketch (spec/sketch.md)

#[test]
fn sketch_applies_the_spec_example_atomically_and_exports_it() {
    let s = Scratch::new();
    let dir = s.project("sk", "");
    let file = s.write(
        "box.json",
        r#"{"schema": 1, "commands": [
          {"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [4, 0, 0], "v": [0, 3, 0]},
          {"op": "pushpull", "shape": "box", "face": 1, "distance": 2.5}]}"#,
    );
    let run = bim(&[&"sketch", &dir, &file]).expect(0);
    assert_eq!((run.out.as_str(), run.err.as_str()), ("", ""));
    let shown = show(&dir, "box");
    assert!(shown.starts_with(r#"{"id":"box","kind":"Shape","props":{"#), "{shown}");
    assert!(shown.contains(r#""face_ids":[1.0,2.0,3.0,4.0,5.0,6.0]"#), "{shown}");
    assert!(shown.contains(r#""edges":[]"#), "{shown}");
    let Json::Object(doc) = json::parse(&shown).unwrap() else { panic!() };
    let Json::Object(props) = &doc.iter().find(|(k, _)| k == "props").unwrap().1 else { panic!() };
    let Json::Array(vertices) = &props.iter().find(|(k, _)| k == "vertices").unwrap().1 else { panic!() };
    assert_eq!(vertices.len(), 24, "8 vertices");
    // The journal replays to the same document, and a second command file edits the Shape in place.
    let h = hash(&dir);
    assert_eq!(hash(&dir), h);
    let more = s.write("more.json", r#"{"schema": 1, "commands": [{"op": "pushpull", "shape": "box", "face": 1, "distance": -1}]}"#);
    bim(&[&"sketch", &dir, &more]).expect(0);
    assert_ne!(hash(&dir), h);
    // Invalid files change nothing and exit 2.
    let h = hash(&dir);
    for text in [
        r#"{"schema": 1, "commands": [{"op": "pushpull", "shape": "box", "face": 1, "distance": 1}, {"op": "pushpull", "shape": "box", "face": 99, "distance": 1}]}"#,
        r#"{"schema": 1, "commands": [{"op": "move", "shape": "box", "faces": [99], "vector": [1, 0, 0]}]}"#,
        r#"{"schema": 1, "commands": [{"op": "rect", "shape": "box", "corner": [0, 0, 0], "u": [1, 0, 0], "v": [1, 1, 0]}]}"#,
        r#"{"schema": 1"#,
        r#"{"schema": 1, "ops": []}"#,
    ] {
        let bad = s.write("bad.json", text);
        bim(&[&"sketch", &dir, &bad]).rejected();
        assert_eq!(hash(&dir), h, "{text}");
    }
    bim(&[&"sketch", &dir, &s.path("missing.json")]).rejected();
    bim(&[&"sketch", &s.path("nope"), &file]).rejected();
    // The Shape is exported as one closed glTF mesh node.
    let out = s.path("box.gltf");
    bim(&[&"export", &dir, &"--format", &"gltf", &"--out", &out]).expect(0);
    let text = fs::read_to_string(&out).unwrap();
    assert!(text.contains(r#""extras":{"id":"box","kind":"Shape"}"#), "{text}");
}

// ---- MEP circuits, segment solids and `bim clash` (T4.7) ---------------------------------

/// A wall, a pipe through it, a thinner pipe 4 cm off its face, and a circuit.
const MEP_SCENE: &str = r#"{"schema": 1, "ops": [
  {"op": "create", "id": "L0", "kind": "Level", "props": {"elevation": 0}},
  {"op": "create", "id": "WT", "kind": "WallType", "props": {"layer_thickness": [0.1, 0.1], "layer_material": ["a", "b"]}},
  {"op": "create", "id": "W", "kind": "Wall", "props": {"level": {"ref": "L0"}, "type": {"ref": "WT"}, "x1": 0, "y1": 0, "x2": 6, "y2": 0, "height": 3}},
  {"op": "create", "id": "WS", "kind": "MepSystem", "props": {"name": "w", "medium": "water", "density": 998.2, "viscosity": 0.001002}},
  {"op": "create", "id": "pa", "kind": "Port", "props": {"system": {"ref": "WS"}, "x": 2, "y": -1, "z": 1.5, "kind": "source", "pressure": 200000}},
  {"op": "create", "id": "pb", "kind": "Port", "props": {"system": {"ref": "WS"}, "x": 2, "y": 1, "z": 1.5, "kind": "sink", "flow": 0.001}},
  {"op": "create", "id": "P", "kind": "Segment", "props": {"from": {"ref": "pa"}, "to": {"ref": "pb"}, "shape": "round", "diameter": 0.1, "roughness": 0.0001}},
  {"op": "create", "id": "qa", "kind": "Port", "props": {"system": {"ref": "WS"}, "x": 3.5, "y": 0.18, "z": 1.5, "kind": "source", "pressure": 200000}},
  {"op": "create", "id": "qb", "kind": "Port", "props": {"system": {"ref": "WS"}, "x": 5.5, "y": 0.18, "z": 1.5, "kind": "sink", "flow": 0.001}},
  {"op": "create", "id": "Q", "kind": "Segment", "props": {"from": {"ref": "qa"}, "to": {"ref": "qb"}, "shape": "round", "diameter": 0.08, "roughness": 0.0001}},
  {"op": "create", "id": "e0", "kind": "Port", "props": {"system": {"ref": "WS"}, "kind": "junction"}},
  {"op": "create", "id": "e1", "kind": "Port", "props": {"system": {"ref": "WS"}, "kind": "junction"}},
  {"op": "create", "id": "C1", "kind": "Circuit", "props": {"panel": {"ref": "e0"}, "voltage": 230, "loads": [{"ref": "e1"}], "load_watts": [2300],
    "conductor_area": 0.0000025, "resistivity": 0.0000000172, "length": 30}}
]}"#;

fn field<'a>(object: &'a Json, key: &str) -> &'a Json {
    let Json::Object(members) = object else { panic!("{object:?} is not an object") };
    &members.iter().find(|(k, _)| k == key).unwrap_or_else(|| panic!("no `{key}` in {object:?}")).1
}

fn number(value: &Json) -> f64 {
    let Json::Num(x) = value else { panic!("{value:?} is not a number") };
    *x
}

fn array(value: &Json) -> &[Json] {
    let Json::Array(items) = value else { panic!("{value:?} is not an array") };
    items
}

fn text(value: &Json) -> &str {
    let Json::Str(s) = value else { panic!("{value:?} is not a string") };
    s
}

#[test]
fn clash_and_circuits_through_the_binary() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    let ops = s.write("scene.json", MEP_SCENE);
    bim(&[&"apply", &dir, &ops]).expect(0);
    let before = hash(&dir);

    // The pipe through the wall is a hard clash of the pipe's 16-gon section times the wall's thickness.
    let run = bim(&[&"clash", &dir, &"--json"]).expect(1);
    assert_eq!(run.err, "");
    assert_eq!(run.out.matches('\n').count(), 1, "one document");
    let doc = json::parse(&run.out).unwrap();
    let hard = array(field(&doc, "hard"));
    assert_eq!(hard.len(), 1, "{}", run.out);
    assert_eq!((text(field(&hard[0], "a")), text(field(&hard[0], "b"))), ("P", "W"));
    let a16 = 8.0 * (std::f64::consts::PI / 8.0).sin() * 0.05 * 0.05;
    assert!((number(field(&hard[0], "volume")) - a16 * 0.2).abs() < 1e-12 * a16);
    assert!(array(field(&doc, "soft")).is_empty(), "no clearance, no soft clashes");
    assert!(run.out.ends_with("\"matrix\":{\"Segment\":{\"Wall\":1}}}\n"), "{}", run.out);

    // With a clearance the pipe Q is soft: its centre is 0.18 off the centreline, its nearest vertex
    // 0.18 − 0.04 = 0.14 from it, 0.04 from the wall's face at 0.1.
    let run = bim(&[&"clash", &dir, &"--clearance", &"0.05", &"--json"]).expect(1);
    let doc = json::parse(&run.out).unwrap();
    let soft = array(field(&doc, "soft"));
    assert_eq!(soft.len(), 1, "{}", run.out);
    assert_eq!((text(field(&soft[0], "a")), text(field(&soft[0], "b"))), ("Q", "W"));
    assert!((number(field(&soft[0], "distance")) - 0.04).abs() < 1e-12);
    let run = bim(&[&"clash", &dir, &"--clearance", &"0.035", &"--json"]).expect(1);
    assert!(array(field(&json::parse(&run.out).unwrap(), "soft")).is_empty());

    // A bad clearance is a usage error, found before anything is read.
    for bad in ["x", "-0.5", "nan"] {
        let run = bim(&[&"clash", &dir, &"--clearance", &bad, &"--json"]).rejected();
        assert!(run.err.contains("usage:") && run.err.contains("--clearance"), "{}", run.err);
    }
    bim(&[&"clash", &s.path("nope"), &"--json"]).rejected();
    bim(&[&"clash", &dir, &"--json", &"--json"]).rejected();

    // The circuit: 10 A through 2 · 30 m of copper at 2.5 mm² drops 8 V, 3.4783 % of 230 V.
    let run = bim(&[&"solve", &dir, &"--trade", &"mep", &"--json"]).expect(0);
    let doc = json::parse(&run.out).unwrap();
    let circuits = array(field(&doc, "circuits"));
    assert_eq!(circuits.len(), 1);
    assert_eq!(text(field(&circuits[0], "id")), "C1");
    assert_eq!(number(field(&circuits[0], "current")), 10.0);
    assert!((number(field(&circuits[0], "voltage_drop")) - 10.0 * 2.0 * 1.72e-8 * 30.0 / 2.5e-6).abs() < 1e-12);
    assert!((number(field(&circuits[0], "voltage_drop_pct")) - 100.0 * 4.128 / 230.0).abs() < 1e-9);
    // The circuit's ports are no hydraulic nodes; the segments are sized.
    let ports: Vec<&str> = array(field(&doc, "ports")).iter().map(|p| text(field(p, "id"))).collect();
    assert_eq!(ports, ["pa", "pb", "qa", "qb"], "e0 and e1 are only the circuit's");
    assert!(array(field(&doc, "segments")).iter().all(|seg| text(field(seg, "size")).starts_with("DN")));

    // The segments are solids in the glTF export, with the entity name as `extras.id`.
    let out = s.path("mep.gltf");
    bim(&[&"export", &dir, &"--format", &"gltf", &"--out", &out]).expect(0);
    let gltf = fs::read_to_string(&out).unwrap();
    for id in ["P", "Q", "W"] {
        assert!(gltf.contains(&format!("\"extras\":{{\"id\":\"{id}\"")), "{id}: {gltf}");
    }
    assert!(gltf.contains("\"extras\":{\"id\":\"P\",\"kind\":\"Segment\"}"), "{gltf}");
    // None of this changed the project.
    assert_eq!(hash(&dir), before);
}
