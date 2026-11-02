//! Black-box tests: the real `bim` binary against the contract in TASKS.md (T1.5) and spec/cli.md.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use bim_doc::{Document, EntityId, Op, Value};

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
    assert_eq!(files_in(&dir), ["doc.oplog"]);
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
    assert_eq!(files_in(&dir), ["doc.oplog"], "no temporary file is left behind");
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
    assert_eq!(files_in(&dir), ["doc.oplog"]);
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
    assert_eq!(files_in(&dir), ["doc.oplog"]);
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
