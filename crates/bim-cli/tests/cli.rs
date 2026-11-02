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
        doc.apply(Op::Create { id: EntityId::derive(&format!("e{n}")), kind: "W".into() }).unwrap();
        doc.content_hash()
    };
    let n = (0..10_000).find(|&n| hash_of(n) < 1 << 60).expect("a leading-zero hash among 10000 models");
    let s = Scratch::new();
    let dir = s.project("p", &format!("create e{n} W\n"));
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
    for op in [
        Op::Create { id: id("wall"), kind: "Wall".into() },
        Op::Create { id: id("lvl"), kind: "Level".into() },
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
    for ops in ["", A_OPS, "create a W\nset a self ref a\nset a t text ghost\n"] {
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
        ("a duplicate create inside one file", "create fresh W\ncreate fresh W\n", ":2: entity `fresh` already exists"),
        ("an op on an entity the file deleted", "create fresh W\ndelete fresh\nset fresh k num 1\n", ":3: entity `fresh` not found"),
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
        let file = s.write("syntax.ops", format!("\ncreate ok W\n{text}"));
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
        let file = s.write("res.ops", format!("create ok W\nset wall {key} num 1\n"));
        let run = bim(&[&"apply", &dir, &file]).rejected();
        assert!(run.err.contains("res.ops:2:") && run.err.contains("reserved"), "{key}: {}", run.err);
        assert_eq!(journal(&dir), before, "{key}");
    }
}

#[test]
fn json_ops_files_are_not_supported_yet_and_exit_2() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    let file = s.write("ops.json", r#"{"schema": 1, "ops": [{"op": "create", "id": "L1", "kind": "Level"}]}"#);
    let run = bim(&[&"apply", &dir, &file]).rejected();
    assert!(run.err.contains("JSON"), "{}", run.err);
    assert_eq!(journal(&dir), b"");
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
    let spaced = s.write("spaced.ops", "\ncreate a W\n\n   \nset a h num 1\n\n");
    let fresh = s.project("q", "");
    bim(&[&"apply", &fresh, &spaced]).expect(0);
    assert_eq!(journal(&fresh), b"create a W\nset a h num 1.0\n");
}

#[test]
fn an_ops_file_may_start_with_a_byte_order_mark() {
    let s = Scratch::new();
    let dir = s.project("p", "");
    bim(&[&"apply", &dir, &s.write("bom.ops", "\u{feff}create a W\nset a h num 1\n")]).expect(0);
    assert_eq!(journal(&dir), b"create a W\nset a h num 1.0\n", "the mark is not copied into the journal");
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
    bim(&[&"apply", &two, &s.write("1.ops", "create a W\nset a h num 1\n")]).expect(0);
    bim(&[&"apply", &two, &s.write("2.ops", "set a h num 2\ncreate b W\nset b p ref a\n")]).expect(0);
    bim(&[&"apply", &one, &s.write("3.ops", "create a W\nset a h num 1\nset a h num 2\ncreate b W\nset b p ref a\n")]).expect(0);
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
    let file = s.write("chain.ops", "create a W\nset a h num 1\ndelete a\ncreate a Slab\nset a t num 2\n");
    bim(&[&"apply", &dir, &file]).expect(0);
    let mut doc = Document::new();
    let a = EntityId::derive("a");
    for op in [
        Op::Create { id: a, kind: "W".into() },
        Op::Set { id: a, key: "h".into(), value: Value::Num(1.0) },
        Op::Delete { id: a },
        Op::Create { id: a, kind: "Slab".into() },
        Op::Set { id: a, key: "t".into(), value: Value::Num(2.0) },
    ] {
        doc.apply(op).unwrap();
    }
    assert_eq!(hash(&dir), format!("{:016x}", doc.content_hash()));
}

#[test]
fn numbers_survive_the_journal_bit_for_bit() {
    let s = Scratch::new();
    let words = ["0.30000000000000004", "1e300", "5e-324", "-0", "9", "3.5", "1.7976931348623157e308", "2.2250738585072014e-308", "1e21", "1e-7", "123456789012345680"];
    let ops: String = std::iter::once("create e W\n".to_string())
        .chain(words.iter().enumerate().map(|(i, w)| format!("set e k{i} num {w}\n")))
        .collect();
    let dir = s.project("p", &ops);
    let mut doc = Document::new();
    let e = EntityId::derive("e");
    doc.apply(Op::Create { id: e, kind: "W".into() }).unwrap();
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
