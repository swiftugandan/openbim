//! The on-disk project: a directory holding `doc.oplog`, the text journal of every op applied to
//! the document.
//!
//! **The journal is the source of truth.** It is the line format of [`crate::ops`], one canonical op
//! per line, UTF-8, each line ended by `\n`, with no header; `bim init` creates it empty.
//! [`Project::open`] replays it into a [`Document`]. Entities are stored under the seed they were
//! written with (never the hashed id), so a replay reproduces the document exactly and the names
//! the user chose stay recoverable.
//!
//! Appending never edits in place: the new journal (the old bytes, verbatim, plus the new lines) is
//! written to a temporary file in the same directory, synced, and renamed over `doc.oplog`. A
//! failure or a crash therefore leaves either the old journal or the new one, never a torn line.
//! The project has a single writer: two `bim apply` runs racing on one project can lose one run's ops.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bim_doc::{Document, EntityId, Value};

use crate::ops::{self, resolve_id, LineOp, Located};
use crate::CliError;

/// File name of the journal inside a project directory.
pub const OPLOG: &str = "doc.oplog";

/// A project directory opened for use: its journal replayed into a document.
pub struct Project {
    /// The project directory.
    pub dir: PathBuf,
    /// The live document: the result of replaying the journal.
    pub doc: Document,
    /// The seed each entity was named by in the journal (created, referenced or both).
    names: HashMap<EntityId, String>,
    /// The journal text as read.
    journal: String,
}

/// A prop holding a ref to an entity that is not in the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dangling {
    /// The entity that holds the ref.
    pub source: EntityId,
    /// The prop that holds it.
    pub key: String,
    /// The missing entity. `None` only if `bim-doc` reports a dangling prop that is not a plain
    /// `Ref` (it cannot with the value types of line ops).
    pub target: Option<EntityId>,
}

/// Creates the project directory (and missing parents) and an empty journal in it. Idempotent: an
/// existing journal is left exactly as it is.
pub fn init(dir: &Path) -> Result<(), CliError> {
    fs::create_dir_all(dir).map_err(|e| io_error(dir, &e))?;
    let path = dir.join(OPLOG);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if path.is_file() {
                Ok(())
            } else {
                Err(CliError::Input(format!("{}: exists and is not a file", path.display())))
            }
        }
        Err(e) => Err(io_error(&path, &e)),
    }
}

impl Project {
    /// Reads `<dir>/doc.oplog` and replays it. A missing directory, a directory without a journal,
    /// an unreadable journal and a corrupt one (an invalid line, or an op the document refuses)
    /// are all [`CliError::Input`]; the corrupt-line message names the line.
    pub fn open(dir: &Path) -> Result<Project, CliError> {
        match fs::metadata(dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(CliError::Input(format!("{}: not a directory", dir.display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(CliError::Input(format!("{}: no such directory", dir.display())));
            }
            Err(e) => return Err(io_error(dir, &e)),
        }
        let path = dir.join(OPLOG);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(CliError::Input(format!(
                    "{}: not a bim project ({OPLOG} not found; run `bim init`)",
                    dir.display()
                )));
            }
            Err(e) => return Err(io_error(&path, &e)),
        };
        let journal = String::from_utf8(bytes).map_err(|_| {
            CliError::Input(format!("{}: corrupt journal: not valid UTF-8", path.display()))
        })?;
        Project::from_journal(dir, journal)
    }

    /// Replays journal text that was read from `<dir>/doc.oplog`.
    fn from_journal(dir: &Path, journal: String) -> Result<Project, CliError> {
        let corrupt = |line: usize, reason: &str| {
            CliError::Input(format!("{}:{line}: corrupt journal: {reason}", dir.join(OPLOG).display()))
        };
        let parsed = ops::parse_text(&journal).map_err(|e| corrupt(e.line, &e.reason))?;
        let mut doc = Document::new();
        let mut names = HashMap::new();
        for Located { line, op } in &parsed {
            doc.apply(op.to_op()).map_err(|e| corrupt(*line, &op.describe_error(&e)))?;
            for name in op.names() {
                names.entry(resolve_id(name)).or_insert_with(|| name.to_string());
            }
        }
        Ok(Project { dir: dir.to_path_buf(), doc, names, journal })
    }

    /// The name to show for `id`: the seed it was written with, else its 32 hex digits.
    pub fn name(&self, id: EntityId) -> String {
        self.names.get(&id).cloned().unwrap_or_else(|| id.to_string())
    }

    /// Every dangling ref, in `bim-doc`'s order (by source id, then key).
    pub fn dangling(&self) -> Vec<Dangling> {
        self.doc
            .dangling_refs()
            .into_iter()
            .map(|(source, key)| {
                let target = match self.doc.get(source).and_then(|e| e.props.get(&key)) {
                    Some(Value::Ref(target)) => Some(*target),
                    _ => None,
                };
                Dangling { source, key, target }
            })
            .collect()
    }

    /// The journal text after appending `ops`: the current text verbatim, a line break if it did
    /// not end in one, then one canonical line per op.
    pub fn appended(&self, ops: &[LineOp]) -> String {
        let mut text = self.journal.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for op in ops {
            text.push_str(&op.to_string());
            text.push('\n');
        }
        text
    }

    /// Appends `ops` to the journal on disk, atomically (see the module docs). Does not check that
    /// the ops apply; the caller has done that. An empty `ops` writes nothing.
    pub fn append(&self, ops: &[LineOp]) -> Result<(), CliError> {
        if ops.is_empty() {
            return Ok(());
        }
        let path = self.dir.join(OPLOG);
        // Replacing the file would bypass a read-only journal, which an in-place append would respect.
        if fs::metadata(&path).is_ok_and(|meta| meta.permissions().readonly()) {
            return Err(CliError::Input(format!("{}: journal is read-only", path.display())));
        }
        replace_journal(&self.dir, self.appended(ops).as_bytes()).map_err(|e| io_error(&path, &e))
    }
}

/// Writes `bytes` to a temporary file next to the journal, syncs it, and renames it over the
/// journal. The temporary file is removed if anything fails. Its name is unique per call, so
/// concurrent writers (other processes, or other threads of this one) never share a temporary file.
fn replace_journal(dir: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{OPLOG}.{}.{unique}.tmp", std::process::id()));
    let result = write_synced(&tmp, bytes).and_then(|()| fs::rename(&tmp, dir.join(OPLOG)));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn io_error(path: &Path, error: &io::Error) -> CliError {
    CliError::Input(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::parse_line;
    use crate::testutil::TempDir;

    fn id(seed: &str) -> EntityId {
        EntityId::derive(seed)
    }

    fn project(journal: &str) -> Project {
        Project::from_journal(Path::new("proj"), journal.to_string()).unwrap()
    }

    fn line(text: &str) -> LineOp {
        parse_line(text).unwrap().unwrap()
    }

    fn open_err(journal: &str) -> String {
        match Project::from_journal(Path::new("proj"), journal.to_string()) {
            Err(CliError::Input(message)) => message,
            Err(other) => panic!("wanted an Input error, got {other:?}"),
            Ok(_) => panic!("journal should be rejected: {journal:?}"),
        }
    }

    const MODEL: &str = "create wall Wall\ncreate lvl Level\nset wall height num 3.5\nset wall host ref lvl\n";

    #[test]
    fn replay_matches_a_document_built_from_the_same_ops() {
        let p = project(MODEL);
        let mut doc = Document::new();
        for op in bim_doc_ops() {
            doc.apply(op).unwrap();
        }
        assert_eq!(p.doc, doc);
        assert_eq!(p.doc.content_hash(), doc.content_hash());
    }

    fn bim_doc_ops() -> Vec<bim_doc::Op> {
        use bim_doc::Op;
        vec![
            Op::Create { id: id("wall"), kind: "Wall".into() },
            Op::Create { id: id("lvl"), kind: "Level".into() },
            Op::Set { id: id("wall"), key: "height".into(), value: Value::Num(3.5) },
            Op::Set { id: id("wall"), key: "host".into(), value: Value::Ref(id("lvl")) },
        ]
    }

    #[test]
    fn an_empty_or_blank_journal_is_an_empty_document() {
        for journal in ["", "\n", "  \n\t\n"] {
            let p = project(journal);
            assert!(p.doc.is_empty(), "{journal:?}");
            assert_eq!(p.doc.content_hash(), Document::new().content_hash());
        }
    }

    #[test]
    fn replay_accepts_crlf_blank_lines_and_a_missing_final_newline() {
        let p = project("create a Wall\r\n\r\n  set a h num 2\r\ndelete a");
        assert!(p.doc.is_empty());
        assert_eq!(p.doc.log().len(), 3);
    }

    #[test]
    fn names_cover_created_and_referenced_seeds() {
        let p = project("create wall Wall\nset wall host ref lvl\nset wall n text north\n");
        assert_eq!(p.name(id("wall")), "wall");
        assert_eq!(p.name(id("lvl")), "lvl", "a ref target that was never created is still named");
        let unknown = id("never-mentioned");
        assert_eq!(p.name(unknown), format!("{:032x}", unknown.0));
    }

    #[test]
    fn dangling_reports_source_key_and_target() {
        let p = project(&format!("{MODEL}delete lvl\n"));
        assert_eq!(
            p.dangling(),
            vec![Dangling { source: id("wall"), key: "host".into(), target: Some(id("lvl")) }]
        );
        assert!(project(MODEL).dangling().is_empty());
        assert!(project("create a W\nset a self ref a\n").dangling().is_empty(), "a self ref is not dangling");
        // A ref to an entity that never existed dangles too; text that merely looks like a seed does not.
        let p = project("create a W\nset a r ref ghost\nset a t text ghost\n");
        assert_eq!(p.dangling().len(), 1);
        assert_eq!(p.dangling()[0].target, Some(id("ghost")));
    }

    #[test]
    fn corrupt_journals_name_the_line() {
        let message = open_err("create a Wall\n\nbogus op here\n");
        assert_eq!(message, "proj/doc.oplog:3: corrupt journal: unknown op `bogus` (expected create, set or delete)");
        let message = open_err("create a Wall\ncreate a Wall\n");
        assert_eq!(message, "proj/doc.oplog:2: corrupt journal: entity `a` already exists");
        let message = open_err("set ghost h num 1\n");
        assert_eq!(message, "proj/doc.oplog:1: corrupt journal: entity `ghost` not found");
    }

    #[test]
    fn appended_keeps_the_old_text_verbatim() {
        let old = "create  a   Wall\r\n\r\nset a h num 2";
        let p = project(old);
        let new = p.appended(&[line("create b  Wall"), line("set b h num 3")]);
        assert_eq!(new, format!("{old}\ncreate b Wall\nset b h num 3.0\n"));
        let p = project("create a Wall\n");
        assert_eq!(p.appended(&[line("delete a")]), "create a Wall\ndelete a\n");
        assert_eq!(project("").appended(&[line("create a Wall")]), "create a Wall\n");
        assert_eq!(p.appended(&[]), "create a Wall\n");
    }

    #[test]
    fn the_appended_journal_replays_to_the_same_document_as_applying_the_ops() {
        let p = project(MODEL);
        let ops = [line("create door Opening"), line("set door host ref wall"), line("delete lvl")];
        let replayed = project(&p.appended(&ops));
        let mut doc = p.doc.clone();
        for op in &ops {
            doc.apply(op.to_op()).unwrap();
        }
        assert_eq!(replayed.doc, doc);
    }

    #[test]
    fn init_creates_the_directory_and_an_empty_journal() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("deep").join("project");
        init(&dir).unwrap();
        assert_eq!(fs::read(dir.join(OPLOG)).unwrap(), b"");
        assert!(Project::open(&dir).unwrap().doc.is_empty());
    }

    #[test]
    fn init_never_truncates() {
        let tmp = TempDir::new();
        init(tmp.path()).unwrap();
        fs::write(tmp.path().join(OPLOG), "create a Wall\n").unwrap();
        init(tmp.path()).unwrap();
        assert_eq!(fs::read_to_string(tmp.path().join(OPLOG)).unwrap(), "create a Wall\n");
    }

    #[test]
    fn init_refuses_a_file_or_a_directory_in_the_way() {
        let tmp = TempDir::new();
        let file = tmp.path().join("file");
        fs::write(&file, "x").unwrap();
        assert!(matches!(init(&file), Err(CliError::Input(_))));
        let dir = tmp.path().join("dir");
        fs::create_dir_all(dir.join(OPLOG)).unwrap();
        let Err(CliError::Input(message)) = init(&dir) else { panic!("init over a directory journal") };
        assert!(message.contains("exists and is not a file"), "{message}");
    }

    #[test]
    fn open_distinguishes_missing_dir_non_dir_and_missing_journal() {
        let tmp = TempDir::new();
        let message = |dir: &Path| match Project::open(dir) {
            Err(CliError::Input(m)) => m,
            _ => panic!("{} should not open", dir.display()),
        };
        assert!(message(&tmp.path().join("nope")).ends_with("no such directory"));
        let file = tmp.path().join("file");
        fs::write(&file, "x").unwrap();
        assert!(message(&file).ends_with("not a directory"));
        assert!(message(tmp.path()).contains("not a bim project"));
        fs::write(tmp.path().join(OPLOG), [0xff, 0xfe, b'\n']).unwrap();
        assert!(message(tmp.path()).contains("not valid UTF-8"));
    }

    #[test]
    fn append_writes_atomically_and_leaves_no_temporary_file() {
        let tmp = TempDir::new();
        init(tmp.path()).unwrap();
        let p = Project::open(tmp.path()).unwrap();
        p.append(&[line("create a Wall"), line("set a h num 2")]).unwrap();
        assert_eq!(fs::read_to_string(tmp.path().join(OPLOG)).unwrap(), "create a Wall\nset a h num 2.0\n");
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, [std::ffi::OsString::from(OPLOG)]);
        // Appending nothing does not even touch the file.
        let before = fs::metadata(tmp.path().join(OPLOG)).unwrap().modified().unwrap();
        Project::open(tmp.path()).unwrap().append(&[]).unwrap();
        assert_eq!(fs::metadata(tmp.path().join(OPLOG)).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn a_failed_replace_removes_the_temporary_file_and_keeps_the_journal() {
        let tmp = TempDir::new();
        // A directory where the journal should be makes the final rename fail.
        fs::create_dir(tmp.path().join(OPLOG)).unwrap();
        assert!(replace_journal(tmp.path(), b"create a Wall\n").is_err());
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, [std::ffi::OsString::from(OPLOG)]);
    }

    #[test]
    fn a_read_only_journal_is_respected() {
        let tmp = TempDir::new();
        init(tmp.path()).unwrap();
        let path = tmp.path().join(OPLOG);
        let p = Project::open(tmp.path()).unwrap();
        let original = fs::metadata(&path).unwrap().permissions();
        let mut read_only = original.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&path, read_only).unwrap();
        let result = p.append(&[line("create a Wall")]);
        fs::set_permissions(&path, original).unwrap();
        let Err(CliError::Input(message)) = result else { panic!("append to a read-only journal") };
        assert!(message.contains("read-only"), "{message}");
        assert_eq!(fs::read(&path).unwrap(), b"");
    }
}
