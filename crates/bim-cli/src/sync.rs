//! The journal side of `bim sync` (crate `bim-sync` does the replicating).
//!
//! **Two representations of one document.** The journal `doc.oplog` is the source of truth for
//! the document and stays a plain text file. The sync log in `bim.sqlite` is the same history as
//! stamped edits (`bim_sync::OpLog`). A journal line becomes stamped edits one to one, except a
//! `create`, which is three: the create and the two reserved `set`s (`_name`, `_schema`) it implies
//! on replay. Edits arriving from other peers are written back into the journal as ordinary lines,
//! so `bim hash`, `show`, `validate` and every other command see merged state without knowing sync
//! exists.
//!
//! **Stamping is lazy.** `bim apply` is unchanged; local lines get their Lamport stamps when
//! `bim sync` runs, from the journal's tail since the last sync (the journal *marks*, below). A
//! stamp depends only on the persisted log and that tail, and is assigned before anything received
//! is merged, so a retried sync stamps the same lines identically (the server may already hold
//! them; identical edits merge as no-ops).
//!
//! **The journal is never rewritten.** After a merge the journal becomes the old bytes plus lines
//! that turn the document the journal replays to into the merged document ([`diff_ops`]). So
//! `bim-store` reconciles on its cheap prefix path, and a sync that has nothing to do touches no
//! file at all.
//!
//! **Crash safety.** A sync commits in this order, and a kill at any point leaves a state the next
//! sync repairs:
//!
//! 1. one SQLite transaction stores the new edits and the marks `journal_prev` (the journal as it
//!    is now) and `journal_cur` (the journal as it is about to become). Killed before the commit:
//!    nothing changed.
//! 2. a second transaction takes the write lock (so a concurrent `bim apply` waits), checks the
//!    journal is still the one that was read, and renames the new journal into place. Killed
//!    before the rename: the journal still matches `journal_prev`, so its tail is empty and the next
//!    sync derives the same new journal again.
//! 3. `bim-store` is brought in step (best effort; every write reconciles it anyway).
//!
//! On the next run the tail of the journal is what follows the longest mark it extends: after
//! `journal_cur` (the normal case), after `journal_prev` (killed in step 2, or an apply raced in),
//! and when the journal extends neither (somebody rewrote it) the tail is the difference between
//! the document the sync log stands for and the journal's document, which is the honest reading of
//! "the user changed the document".

use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use bim_doc::{Document, Entity, EntityId, Value};
use bim_store::{bytes_hash, store_path, Store, STORE_FILE};
use bim_sync::server::Backend;
use bim_sync::{same_state, same_value, Materialized, OpLog, StampedOp, SyncDb, SyncError, SyncOp, SyncValue};

use crate::ops::{parse_text, LineOp, LineValue, NAME_PROP, SCHEMA_PROP};
use crate::project::{io_error, replace_journal, Project, OPLOG};
use crate::store::store_error;
use crate::CliError;

/// Meta key: the journal as the last sync left it.
const CUR: &str = "journal_cur";
/// Meta key: the journal as it was before the last sync wrote it.
const PREV: &str = "journal_prev";

/// A journal as remembered between syncs: its length and the hash of its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    len: u64,
    hash: u64,
}

impl Mark {
    fn of(bytes: &[u8]) -> Mark {
        Mark { len: bytes.len() as u64, hash: bytes_hash(bytes) }
    }

    fn encode(self) -> String {
        format!("{}:{:016x}", self.len, self.hash)
    }

    fn decode(text: &str) -> Option<Mark> {
        let (len, hash) = text.split_once(':')?;
        Some(Mark { len: len.parse().ok()?, hash: u64::from_str_radix(hash, 16).ok().filter(|_| hash.len() == 16)? })
    }
}

/// A sync failure as a command error: always an input error (exit 2), because the network and the
/// store fail like any file or peer the user pointed `bim` at.
fn sync_error(dir: &Path, error: SyncError) -> CliError {
    match error {
        SyncError::Sqlite(_) | SyncError::Corrupt(_) => CliError::Input(format!("{}: {error}", dir.join(STORE_FILE).display())),
        other => CliError::Input(format!("sync failed: {other}")),
    }
}

// ---- journal lines <-> stamped edits --------------------------------------------------------------

/// The edits a journal line stands for: a `create` is the create and the reserved sets it implies
/// on replay (`LineOp::to_ops`), anything else is one edit.
fn line_to_sync(op: &LineOp) -> Vec<SyncOp> {
    match op {
        LineOp::Create { seed, kind } => vec![
            SyncOp::Create { name: seed.clone(), kind: kind.clone() },
            SyncOp::Set { name: seed.clone(), key: NAME_PROP.to_string(), value: SyncValue::Text(seed.clone()) },
            SyncOp::Set {
                name: seed.clone(),
                key: SCHEMA_PROP.to_string(),
                value: SyncValue::Num(f64::from(Document::SCHEMA_VERSION)),
            },
        ],
        LineOp::Set { seed, key, value } => {
            vec![SyncOp::Set { name: seed.clone(), key: key.clone(), value: sync_value(value) }]
        }
        LineOp::Delete { seed } => vec![SyncOp::Delete { name: seed.clone() }],
    }
}

fn sync_value(value: &LineValue) -> SyncValue {
    match value {
        LineValue::Num(x) => SyncValue::Num(*x),
        LineValue::Text(s) => SyncValue::Text(s.clone()),
        LineValue::Ref(s) => SyncValue::Ref(s.clone()),
        LineValue::Bool(b) => SyncValue::Bool(*b),
        LineValue::List(items) => SyncValue::List(items.iter().map(sync_value).collect()),
    }
}

/// A document value as a journal value, naming Ref targets with `name`.
fn line_value(value: &Value, name: &dyn Fn(EntityId) -> String) -> LineValue {
    match value {
        Value::Num(x) => LineValue::Num(*x),
        Value::Text(s) => LineValue::Text(s.clone()),
        Value::Ref(id) => LineValue::Ref(name(*id)),
        Value::Bool(b) => LineValue::Bool(*b),
        Value::List(items) => LineValue::List(items.iter().map(|item| line_value(item, name)).collect()),
    }
}

/// Journal lines that turn `from` into `to` (entity by entity, ascending ids, so the same two
/// documents always give the same lines):
///
/// * an entity only `from` has: `delete`;
/// * an entity only `to` has, or one whose kind differs, or one that lost a prop (a journal cannot
///   unset): `delete` if it exists, then a `create` and a `set` for every prop the create does not
///   already give (`_name` is the seed, `_schema` is 1);
/// * otherwise a `set` for every prop that is new or differs (numbers by bit pattern, so `-0.0`
///   and `0.0` are different).
///
/// `name` gives the name to write for an id; it must parse back to that id.
fn diff_ops(from: &Document, to: &Document, name: &dyn Fn(EntityId) -> String) -> Vec<LineOp> {
    let mut out = Vec::new();
    let (mut old, mut new) = (from.entities().peekable(), to.entities().peekable());
    loop {
        match (old.peek(), new.peek()) {
            (None, None) => break,
            (Some(gone), None) => {
                out.push(LineOp::Delete { seed: name(gone.id) });
                old.next();
            }
            (None, Some(added)) => {
                create_ops(added, name, &mut out);
                new.next();
            }
            (Some(before), Some(after)) => match before.id.cmp(&after.id) {
                std::cmp::Ordering::Less => {
                    out.push(LineOp::Delete { seed: name(before.id) });
                    old.next();
                }
                std::cmp::Ordering::Greater => {
                    create_ops(after, name, &mut out);
                    new.next();
                }
                std::cmp::Ordering::Equal => {
                    update_ops(before, after, name, &mut out);
                    old.next();
                    new.next();
                }
            },
        }
    }
    out
}

fn create_ops(entity: &Entity, name: &dyn Fn(EntityId) -> String, out: &mut Vec<LineOp>) {
    let seed = match entity.props.get(NAME_PROP) {
        Some(Value::Text(text)) if EntityId::parse(text) == entity.id => text.clone(),
        _ => name(entity.id),
    };
    out.push(LineOp::Create { seed: seed.clone(), kind: entity.kind.clone() });
    for (key, value) in &entity.props {
        let implied = match key.as_str() {
            NAME_PROP => Some(Value::Text(seed.clone())),
            SCHEMA_PROP => Some(Value::Num(f64::from(Document::SCHEMA_VERSION))),
            _ => None,
        };
        if implied.is_none_or(|implied| !same_value(&implied, value)) {
            out.push(LineOp::Set { seed: seed.clone(), key: key.clone(), value: line_value(value, name) });
        }
    }
}

fn update_ops(before: &Entity, after: &Entity, name: &dyn Fn(EntityId) -> String, out: &mut Vec<LineOp>) {
    if before.kind != after.kind || before.props.keys().any(|key| !after.props.contains_key(key)) {
        out.push(LineOp::Delete { seed: name(before.id) });
        create_ops(after, name, out);
        return;
    }
    for (key, value) in &after.props {
        if before.props.get(key).is_none_or(|old| !same_value(old, value)) {
            out.push(LineOp::Set { seed: name(after.id), key: key.clone(), value: line_value(value, name) });
        }
    }
}

/// How an id is written: the smallest name any edit gave it, else the name the journal gave it,
/// else its ULID text (which parses back to it).
fn name_of<'a>(merged: &'a Materialized, project: &'a Project) -> impl Fn(EntityId) -> String + 'a {
    move |id| {
        merged
            .names
            .get(&id)
            .cloned()
            .or_else(|| project.written_name(id).map(str::to_string))
            .unwrap_or_else(|| id.to_ulid_string())
    }
}

// ---- reading the project ---------------------------------------------------------------------------

/// The lines that turn the document `log` stands for into the journal's document.
fn state_diff(log: &OpLog, project: &Project) -> Vec<LineOp> {
    let view = log.materialize();
    let name = name_of(&view, project);
    diff_ops(&view.doc, &project.doc, &name)
}

/// If the journal is the marked journal or the marked journal plus whole new lines, how many ops
/// the marked part holds (so `project.ops[n..]` are the new ones).
fn marked_ops(project: &Project, mark: Mark) -> Option<usize> {
    let bytes = project.journal.as_bytes();
    let len = usize::try_from(mark.len).ok().filter(|&len| len <= bytes.len())?;
    if bytes_hash(&bytes[..len]) != mark.hash {
        return None;
    }
    if len == bytes.len() {
        return Some(project.ops.len());
    }
    // A cut inside a line proves nothing: the line may have been edited.
    if len != 0 && bytes[len - 1] != b'\n' {
        return None;
    }
    parse_text(project.journal.get(..len)?).ok().map(|marked| marked.len())
}

/// A project opened for syncing: what the journal and the sync log hold, and the journal's new
/// lines stamped. Nothing has been written.
pub(crate) struct Local {
    project: Project,
    peer: u32,
    /// The project has no peer id yet: `peer` is a fresh one, stored by `persist`.
    peer_is_new: bool,
    /// The persisted edits plus `local`.
    log: OpLog,
    /// The journal's tail since the last sync, stamped.
    local: Vec<StampedOp>,
    /// The journal as the last sync left it.
    cur: Option<Mark>,
}

impl Local {
    /// Reads `dir`. Writes nothing, creates nothing, not even a missing `bim.sqlite`.
    pub(crate) fn load(dir: &Path) -> Result<Local, CliError> {
        let project = Project::open(dir)?;
        let fail = |e: SyncError| sync_error(dir, e);
        let (peer, mut log, cur, prev) = match SyncDb::open_existing(&store_path(dir)).map_err(fail)? {
            None => (None, OpLog::new(), None, None),
            Some(db) => {
                let mark = |key: &str| -> Result<Option<Mark>, CliError> {
                    Ok(db.meta(key).map_err(fail)?.as_deref().and_then(Mark::decode))
                };
                (db.peer().map_err(fail)?, db.load_log().map_err(fail)?, mark(CUR)?, mark(PREV)?)
            }
        };
        let (peer, peer_is_new) = match peer {
            Some(peer) => (peer, false),
            None => (bim_sync::random_peer(), true),
        };
        let marked = [cur, prev].into_iter().flatten().find_map(|mark| marked_ops(&project, mark).filter(|&n| n <= project.ops.len()));
        let tail: Vec<LineOp> = match marked {
            Some(marked) => project.ops[marked..].to_vec(),
            // Never synced: the whole journal is local.
            None if cur.is_none() && prev.is_none() && log.is_empty() => project.ops.clone(),
            // The journal is not one this log knows (rewritten by hand): the difference is the edit.
            None => state_diff(&log, &project),
        };
        let local = log.stamp_local(peer, tail.iter().flat_map(line_to_sync).collect()).map_err(fail)?;
        log.insert_all(local.clone()).map_err(fail)?;
        Ok(Local { project, peer, peer_is_new, log, local, cur })
    }

    /// Merges `incoming` and makes the result durable, in the order the module docs describe.
    /// Returns the merged log. Nothing is written when there is nothing to do.
    ///
    /// Everything that can fail on bad input fails before the first write: a conflicting or
    /// invalid edit, a merged state the journal cannot express, a journal that is read-only.
    pub(crate) fn persist(mut self, incoming: Vec<StampedOp>) -> Result<OpLog, CliError> {
        let dir = self.project.dir.clone();
        let fail = |e: SyncError| sync_error(&dir, e);
        let fresh = self.log.insert_new(incoming).map_err(fail)?;
        let merged = self.log.materialize();
        let lines = diff_ops(&self.project.doc, &merged.doc, &name_of(&merged, &self.project));
        let new_journal = (!lines.is_empty()).then(|| self.project.appended(&lines));
        if let Some(text) = &new_journal {
            // The journal that will be on disk must replay to exactly the merged document.
            let replayed = Project::from_journal(&dir, text.clone()).map_err(|e| {
                CliError::Input(format!("the merged document cannot be written to the journal ({e}); nothing was changed"))
            })?;
            if !same_state(&replayed.doc, &merged.doc) {
                return Err(CliError::Input(
                    "the merged document cannot be written to the journal (an entity lacks `_name` or `_schema`?); nothing was changed"
                        .to_string(),
                ));
            }
            self.project.ensure_journal_writable()?;
        }
        let current = Mark::of(self.project.journal.as_bytes());
        let next = new_journal.as_deref().map_or(current, |text| Mark::of(text.as_bytes()));
        if self.local.is_empty() && fresh.is_empty() && new_journal.is_none() && !self.peer_is_new && self.cur == Some(current) {
            return Ok(self.log); // converged: not a byte is written
        }

        // The store and the sync tables exist from here on.
        Store::open_or_create(&dir).map_err(|e| store_error(&dir, e))?;
        let mut db = SyncDb::open_or_create(&store_path(&dir)).map_err(fail)?;
        let journal_path = dir.join(OPLOG);
        let read_journal = || fs::read(&journal_path).map_err(|e| io_error(&journal_path, &e));
        let changed = || CliError::Input(format!("{}: changed while this command was running; run it again", journal_path.display()));

        // Step 1: the edits and the marks, in one transaction.
        {
            let txn = db.transaction().map_err(fail)?;
            if read_journal()? != self.project.journal.as_bytes() {
                return Err(changed());
            }
            txn.insert_ops(&self.local).map_err(fail)?;
            txn.insert_ops(&fresh).map_err(fail)?;
            if self.peer_is_new && txn.set_peer(self.peer).map_err(fail)? != self.peer {
                return Err(CliError::Input("another process gave this project its peer id meanwhile; run it again".to_string()));
            }
            txn.set_meta(PREV, &current.encode()).map_err(fail)?;
            txn.set_meta(CUR, &next.encode()).map_err(fail)?;
            txn.commit().map_err(fail)?;
        }

        // Step 2: the journal, under the write lock so a concurrent `bim apply` cannot interleave.
        if let Some(text) = new_journal {
            let txn = db.transaction().map_err(fail)?;
            if read_journal()? != self.project.journal.as_bytes() {
                // An apply got in between. The edits are stored; its lines follow `journal_prev`, so
                // the next sync merges both. This run did not converge, and says so.
                return Err(changed());
            }
            replace_journal(&dir, text.as_bytes()).map_err(|e| io_error(&journal_path, &e))?;
            txn.commit().map_err(fail)?;
            drop(db);
            // Step 3: bim-store, with no sync transaction open (two write locks would deadlock).
            if let Err(e) = Project::open(&dir).and_then(|project| project.sync_store()) {
                let _ = writeln!(std::io::stderr(), "bim: warning: {e}; the store catches up at the next write");
            }
        }
        Ok(self.log)
    }
}

// ---- the two commands ------------------------------------------------------------------------------

/// `bim sync <dir> <ws-url>`: one full exchange with the server, then the merged state in `<dir>`.
/// Fails, changing nothing, if the server cannot be reached or the exchange fails.
pub(crate) fn client(dir: &Path, url: &str) -> Result<u8, CliError> {
    let local = Local::load(dir)?;
    let received = bim_sync::client::exchange(url, local.peer, &local.log).map_err(|e| sync_error(dir, e))?;
    local.persist(received)?;
    Ok(0)
}

/// The project directory behind a server. All state is read from the directory at every call, so
/// a restarted server, or edits made to the directory by other commands, lose nothing.
struct DirBackend {
    dir: PathBuf,
}

impl Backend for DirBackend {
    fn snapshot(&mut self) -> Result<OpLog, String> {
        Local::load(&self.dir).and_then(|local| local.persist(Vec::new())).map_err(|e| e.to_string())
    }

    fn merge(&mut self, ops: Vec<StampedOp>) -> Result<OpLog, String> {
        Local::load(&self.dir).and_then(|local| local.persist(ops)).map_err(|e| e.to_string())
    }
}

/// `bim sync serve <dir> --port <p>`: serves the project until killed. Prints `listening <port>`
/// (the real port, also for `--port 0`) once it accepts connections.
pub(crate) fn serve(dir: &Path, port: u16, out: &mut dyn Write) -> Result<u8, CliError> {
    let mut backend = DirBackend { dir: dir.to_path_buf() };
    // Fail now, before announcing anything, if `dir` is not a usable project; and catch up any
    // edits made to it while no server ran.
    backend.snapshot().map_err(CliError::Input)?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| CliError::Input(format!("cannot listen on 127.0.0.1:{port}: {e}")))?;
    let port = listener.local_addr().map_err(|e| CliError::Input(format!("cannot read the listening port: {e}")))?.port();
    writeln!(out, "listening {port}")
        .and_then(|()| out.flush())
        .map_err(|e| CliError::Input(format!("cannot write to standard output: {e}")))?;
    let error = bim_sync::server::serve(listener, backend);
    Err(CliError::Input(format!("the sync server stopped: {error}")))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use bim_doc::Op;
    use bim_sync::{vv_max, Stamp};

    use super::*;
    use crate::commands;
    use crate::ops::parse_line;
    use crate::testutil::TempDir;

    fn os(path: &Path) -> OsString {
        path.as_os_str().to_owned()
    }

    fn run(f: fn(&[OsString], &mut dyn Write) -> Result<u8, CliError>, args: &[OsString]) {
        f(args, &mut Vec::new()).unwrap();
    }

    /// An initialised project directory `name` under `tmp`.
    fn project(tmp: &TempDir, name: &str) -> PathBuf {
        let dir = tmp.path().join(name);
        run(commands::init, &[os(&dir)]);
        dir
    }

    fn apply(tmp: &TempDir, dir: &Path, text: &str) {
        let file = tmp.path().join(format!("ops-{}.txt", fs::read_dir(tmp.path()).unwrap().count()));
        fs::write(&file, text).unwrap();
        run(commands::apply, &[os(dir), os(&file)]);
    }

    fn hash(dir: &Path) -> u64 {
        Project::open(dir).unwrap().doc.content_hash()
    }

    fn journal(dir: &Path) -> Vec<u8> {
        fs::read(dir.join(OPLOG)).unwrap()
    }

    fn store(dir: &Path) -> Vec<u8> {
        fs::read(dir.join(STORE_FILE)).unwrap()
    }

    /// `bim sync <dir>` against a server whose state is `server`'s directory, without sockets: the
    /// same four steps as the protocol, in the same order.
    fn sync(dir: &Path, server: &mut DirBackend) -> Result<(), CliError> {
        let local = Local::load(dir)?;
        let snapshot = server.snapshot().map_err(CliError::Input)?;
        let for_client = snapshot.missing_for(&local.log.vv());
        let push = local.log.missing_for(&snapshot.vv());
        let mut known = vv_max(&local.log.vv(), &snapshot.vv());
        for op in &push {
            let held = known.entry(op.stamp.peer).or_insert(0);
            *held = (*held).max(op.stamp.counter);
        }
        let merged = server.merge(push).map_err(CliError::Input)?;
        let mut received = for_client;
        received.extend(merged.missing_for(&known));
        local.persist(received).map(|_| ())
    }

    const BASE: &str = "create L1 Level\nset L1 elevation num 0\ncreate L2 Level\nset L2 elevation num 3\n\
                        create W1 Wall\nset W1 level ref L1\nset W1 height num 3\n";

    fn assert_same(dirs: &[&Path]) {
        let reference = hash(dirs[0]);
        for dir in dirs {
            assert_eq!(hash(dir), reference, "{}", dir.display());
        }
    }

    #[test]
    fn a_create_is_three_edits_and_the_others_one() {
        for text in ["create w Wall", "create w x Wall", "set w h num 3", "delete w", "set w r ref w"] {
            let Ok(Some(line)) = parse_line(text) else { continue };
            let from_line: Vec<Op> = line.to_ops();
            let from_edits: Vec<Op> = line_to_sync(&line).iter().map(SyncOp::to_doc_op).collect();
            assert_eq!(from_line, from_edits, "{text}");
        }
        let line = parse_line(r#"{"op":"set","id":"w","key":"k","value":[1,true,"a b",{"ref":"z"}]}"#).unwrap().unwrap();
        let edits = line_to_sync(&line);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].to_doc_op(), line.to_ops()[0]);
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % n
        }
    }

    fn random_doc(rng: &mut Rng) -> Document {
        let mut doc = Document::new();
        for name in ["a", "b", "c", "d", "e"] {
            if rng.below(3) == 0 {
                continue;
            }
            let line = LineOp::Create { seed: name.into(), kind: ["Wall", "Slab"][rng.below(2)].into() };
            line.apply_to(&mut doc).unwrap();
            for key in ["h", "r", "t", "_name", "x"] {
                let value = match rng.below(7) {
                    0 => continue,
                    1 => LineValue::Num(-0.0),
                    2 => LineValue::Num(rng.below(3) as f64),
                    3 => LineValue::Text(["", "north", "two words", "ünï\n\"q\""][rng.below(4)].into()),
                    4 => LineValue::Ref(["a", "b", "ghost"][rng.below(3)].into()),
                    5 => LineValue::Bool(rng.below(2) == 0),
                    _ => LineValue::List(vec![LineValue::Num(1.5), LineValue::Ref("c".into()), LineValue::List(vec![])]),
                };
                // `_name` is set to a name that parses back to the entity half of the time.
                let value = if key == "_name" && rng.below(2) == 0 { LineValue::Text(name.into()) } else { value };
                LineOp::Set { seed: name.into(), key: key.into(), value }.apply_to(&mut doc).unwrap();
            }
        }
        doc
    }

    #[test]
    fn diff_ops_turns_any_document_into_any_other_through_journal_text() {
        let mut rng = Rng(42);
        let mut cases = 0;
        for _ in 0..400 {
            let (from, to) = (random_doc(&mut rng), random_doc(&mut rng));
            let name = |id: EntityId| ["a", "b", "c", "d", "e", "ghost"].iter().find(|n| EntityId::derive(n) == id).map_or_else(|| id.to_ulid_string(), |n| n.to_string());
            let lines = diff_ops(&from, &to, &name);
            // Through the journal text, as sync writes it: the journal of `from`, then the diff.
            let mut journal: String = diff_ops(&Document::new(), &from, &name).iter().map(|op| format!("{op}\n")).collect();
            journal.extend(lines.iter().map(|op| format!("{op}\n")));
            let replayed = Project::from_journal(Path::new("t"), journal).unwrap();
            // A document whose entity lacks `_name` or `_schema` cannot be written as a journal at
            // all (every create sets both); `persist` refuses such a merge, see below.
            let expressible = |doc: &Document| doc.entities().all(|e| e.props.contains_key(NAME_PROP) && e.props.contains_key(SCHEMA_PROP));
            if !expressible(&from) || !expressible(&to) {
                continue;
            }
            cases += 1;
            assert!(same_state(&replayed.doc, &to), "from {from:?}\nto {to:?}\nlines {lines:?}");
        }
        assert!(cases > 100, "only {cases} cases were checkable");
    }

    #[test]
    fn diff_ops_emits_nothing_for_equal_documents_and_the_minimum_otherwise() {
        let mut doc = Document::new();
        for text in ["create a Wall", "set a h num 1", "create b Wall", "set b h num 2"] {
            parse_line(text).unwrap().unwrap().apply_to(&mut doc).unwrap();
        }
        let name = |id: EntityId| if id == EntityId::derive("a") { "a".to_string() } else { "b".to_string() };
        assert!(diff_ops(&doc, &doc, &name).is_empty());
        let mut changed = doc.clone();
        parse_line("set a h num 5").unwrap().unwrap().apply_to(&mut changed).unwrap();
        parse_line("set a h2 num 6").unwrap().unwrap().apply_to(&mut changed).unwrap();
        let lines: Vec<String> = diff_ops(&doc, &changed, &name).iter().map(ToString::to_string).collect();
        assert_eq!(lines, ["set a h num 5.0", "set a h2 num 6.0"]);
        // A prop that disappeared cannot be unset: the entity is deleted and created again.
        let lines: Vec<String> = diff_ops(&changed, &doc, &name).iter().map(ToString::to_string).collect();
        assert_eq!(lines, ["delete a", "create a Wall", "set a h num 1.0"]);
        // -0.0 and 0.0 are different edits.
        let mut zero = doc.clone();
        parse_line("set a h num 0").unwrap().unwrap().apply_to(&mut zero).unwrap();
        let mut negative = doc.clone();
        parse_line("set a h num -0").unwrap().unwrap().apply_to(&mut negative).unwrap();
        let lines: Vec<String> = diff_ops(&zero, &negative, &name).iter().map(ToString::to_string).collect();
        assert_eq!(lines, ["set a h num -0.0"]);
        // A deleted entity, an added one, a kind change.
        let mut other = doc.clone();
        parse_line("delete b").unwrap().unwrap().apply_to(&mut other).unwrap();
        parse_line("create c Slab").unwrap().unwrap().apply_to(&mut other).unwrap();
        let name3 = |id: EntityId| ["a", "b", "c"].iter().find(|n| EntityId::derive(n) == id).unwrap().to_string();
        let lines: Vec<String> = diff_ops(&doc, &other, &name3).iter().map(ToString::to_string).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.contains(&"delete b".to_string()) && lines.contains(&"create c Slab".to_string()), "{lines:?}");
    }

    #[test]
    fn the_marks_find_the_tail_after_the_last_sync() {
        let project = |text: &str| Project::from_journal(Path::new("p"), text.to_string()).unwrap();
        let journal = "create a Wall\nset a h num 1.0\n";
        let p = project(&format!("{journal}set a h num 2.0\ncreate b Wall\n"));
        let mark = Mark::of(journal.as_bytes());
        assert_eq!(marked_ops(&p, mark), Some(2), "the marked part holds two ops");
        assert_eq!(marked_ops(&project(journal), mark), Some(2), "the journal is the marked one");
        assert_eq!(marked_ops(&p, Mark::of(b"")), Some(0), "an empty mark is a prefix of everything");
        assert_eq!(marked_ops(&p, Mark::of(b"create a Wal")), None, "a cut inside a line");
        assert_eq!(marked_ops(&p, Mark::of(b"create a Wall\nset a h num 9.0\n")), None, "a different journal");
        assert_eq!(marked_ops(&project("create a Wall\n"), mark), None, "a shorter journal");
        let blank_lines = project("create a Wall\n\n\nset a h num 1.0\n\n");
        assert_eq!(marked_ops(&blank_lines, Mark::of(b"create a Wall\n\n\n")), Some(1), "blank lines are not ops");
        // Marks survive their text form.
        assert_eq!(Mark::decode(&mark.encode()), Some(mark));
        for bad in ["", "1", "1:2", "x:0000000000000000", "1:zz", "1:0000000000000000:1", "-1:0000000000000000"] {
            assert_eq!(Mark::decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn three_peers_through_a_server_converge_and_a_converged_sync_writes_nothing() {
        let tmp = TempDir::new();
        let (s, a, b, c) = (project(&tmp, "s"), project(&tmp, "a"), project(&tmp, "b"), project(&tmp, "c"));
        let mut server = DirBackend { dir: s.clone() };
        apply(&tmp, &a, BASE);
        for dir in [&a, &b, &c] {
            sync(dir, &mut server).unwrap();
        }
        assert_same(&[&s, &a, &b, &c]);
        assert_eq!(journal(&a), BASE.replace("num 0\n", "num 0.0\n").replace("num 3\n", "num 3.0\n").as_bytes(), "A's journal was never rewritten");
        apply(&tmp, &a, "set W1 height num 3.5\ndelete L1\ncreate P1 Space\nset P1 name text lobby\n");
        apply(&tmp, &b, "set W1 height num 4\ncreate W2 Wall\nset W2 level ref L1\nset L2 elevation num 3.2\n");
        apply(&tmp, &c, "set W1 height num 5\nset W1 name text north\nset L1 elevation num 0.5\ncreate D1 Opening\nset D1 host ref W1\n");
        let (ha, hb, hc) = (hash(&a), hash(&b), hash(&c));
        assert!(ha != hb && hb != hc && ha != hc);
        for _ in 0..2 {
            for dir in [&c, &a, &b] {
                sync(dir, &mut server).unwrap();
            }
        }
        assert_same(&[&s, &a, &b, &c]);
        for dir in [&s, &a, &b, &c] {
            let project = Project::open(dir).unwrap();
            assert!(project.doc.get(EntityId::derive("L1")).is_none(), "the delete wins over the concurrent set");
            let mut dangling: Vec<_> =
                project.dangling().iter().map(|d| format!("{} {} {}", project.name(d.source), d.key, project.name(d.target))).collect();
            dangling.sort();
            assert_eq!(dangling, ["W1 level L1", "W2 level L1"], "{}", dir.display());
        }
        // A third pass: not a byte of any file changes.
        let before: Vec<_> = [&s, &a, &b, &c].iter().map(|d| (journal(d), store(d))).collect();
        for dir in [&a, &b, &c] {
            sync(dir, &mut server).unwrap();
        }
        let after: Vec<_> = [&s, &a, &b, &c].iter().map(|d| (journal(d), store(d))).collect();
        assert_eq!(before, after);
        // bim-store is in step with the journals.
        for dir in [&s, &a, &b, &c] {
            let project = Project::open(dir).unwrap();
            let state = Store::open_existing(dir).unwrap().unwrap().state().unwrap();
            assert_eq!(state.len(), project.doc.len(), "{}", dir.display());
        }
    }

    #[test]
    fn the_order_of_the_syncs_does_not_change_the_result() {
        let mut hashes = Vec::new();
        for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
            let tmp = TempDir::new();
            let dirs: Vec<PathBuf> = ["s", "a", "b", "c"].iter().map(|n| project(&tmp, n)).collect();
            let mut server = DirBackend { dir: dirs[0].clone() };
            apply(&tmp, &dirs[1], BASE);
            for peer in 1..4 {
                sync(&dirs[peer], &mut server).unwrap();
            }
            // Peer ids differ between runs here; only the order-independence per run is checked, and
            // the closed-form winners (no ties are possible between different counters).
            apply(&tmp, &dirs[1], "set W1 height num 3.5\ndelete L1\n");
            apply(&tmp, &dirs[2], "set W1 height num 4\ncreate W2 Wall\nset W2 level ref L1\n");
            apply(&tmp, &dirs[3], "set W1 name text north\nset L1 elevation num 0.5\n");
            for _ in 0..2 {
                for &i in &order {
                    sync(&dirs[i + 1], &mut server).unwrap();
                }
            }
            let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
            assert_same(&refs);
            let p = Project::open(&dirs[1]).unwrap();
            assert_eq!(p.doc.get(EntityId::derive("W1")).unwrap().props["name"], Value::Text("north".into()));
            hashes.push(p.doc.get(EntityId::derive("W1")).unwrap().props["height"].clone());
        }
        // A tie on `height` (3.5 and 4 are both stamped at the same counter) goes to the larger
        // peer id, whichever order the peers synced in.
        assert!(hashes.iter().all(|h| matches!(h, Value::Num(x) if *x == 3.5 || *x == 4.0)));
    }

    #[test]
    fn a_sync_killed_before_the_journal_rename_is_repaired_by_the_next_one() {
        let tmp = TempDir::new();
        let (s, a, b) = (project(&tmp, "s"), project(&tmp, "a"), project(&tmp, "b"));
        let mut server = DirBackend { dir: s.clone() };
        apply(&tmp, &a, BASE);
        sync(&a, &mut server).unwrap();
        apply(&tmp, &b, "create extra Wall\nset extra h num 1\n");
        let b_before = journal(&b);
        sync(&b, &mut server).unwrap();
        let b_after = journal(&b);
        assert_ne!(b_before, b_after, "B received A's base");
        let log_len = Local::load(&b).unwrap().log.len();
        // Put the old journal back, as if the process had died between the transaction and the rename.
        fs::write(b.join(OPLOG), &b_before).unwrap();
        let reread = Local::load(&b).unwrap();
        assert!(reread.local.is_empty(), "the old journal is `journal_prev`: nothing in it is new");
        assert_eq!(reread.log.len(), log_len, "and nothing is stamped twice");
        drop(reread);
        sync(&b, &mut server).unwrap();
        assert_eq!(journal(&b), b_after, "the same merged journal is derived again");
        sync(&a, &mut server).unwrap();
        assert_same(&[&s, &a, &b]);
        assert_eq!(Local::load(&b).unwrap().log.len(), log_len, "still nothing stamped twice");
    }

    #[test]
    fn lines_applied_after_a_sync_are_the_next_tail_and_a_rewritten_journal_is_diffed() {
        let tmp = TempDir::new();
        let (s, a, b) = (project(&tmp, "s"), project(&tmp, "a"), project(&tmp, "b"));
        let mut server = DirBackend { dir: s.clone() };
        apply(&tmp, &a, BASE);
        sync(&a, &mut server).unwrap();
        sync(&b, &mut server).unwrap();
        let clock = Local::load(&a).unwrap().log.clock();
        apply(&tmp, &a, "set W1 height num 9\n");
        let local = Local::load(&a).unwrap();
        assert_eq!(local.local.len(), 1);
        assert_eq!(local.local[0].stamp.counter, clock + 1);
        drop(local);
        sync(&a, &mut server).unwrap();
        sync(&b, &mut server).unwrap();
        assert_same(&[&s, &a, &b]);
        // Somebody rewrites B's journal by hand: the difference becomes the edit.
        let rewritten = String::from_utf8(journal(&b)).unwrap().replace("height num 9.0", "height num 11.0") + "create fresh Slab\n";
        fs::write(b.join(OPLOG), rewritten).unwrap();
        assert_ne!(hash(&b), hash(&a));
        let local = Local::load(&b).unwrap();
        assert!(!local.local.is_empty());
        drop(local);
        sync(&b, &mut server).unwrap();
        sync(&a, &mut server).unwrap();
        assert_same(&[&s, &a, &b]);
        let a_doc = Project::open(&a).unwrap().doc;
        assert_eq!(a_doc.get(EntityId::derive("W1")).unwrap().props["height"], Value::Num(11.0));
        assert!(a_doc.get(EntityId::derive("fresh")).is_some());
    }

    #[test]
    fn a_project_that_predates_sync_gets_its_peer_at_the_first_successful_sync_only() {
        let tmp = TempDir::new();
        let (s, a) = (project(&tmp, "s"), tmp.path().join("old"));
        fs::create_dir(&a).unwrap();
        fs::write(a.join(OPLOG), BASE).unwrap();
        let loaded = Local::load(&a).unwrap();
        assert!(loaded.peer_is_new);
        assert!(!a.join(STORE_FILE).exists(), "loading creates nothing");
        let mut server = DirBackend { dir: s.clone() };
        drop(loaded);
        sync(&a, &mut server).unwrap();
        let again = Local::load(&a).unwrap();
        assert!(!again.peer_is_new);
        assert_eq!(hash(&a), hash(&s));
        // A store that exists but has no sync tables (it predates T3.3) is left alone until then.
        let b = tmp.path().join("old2");
        fs::create_dir(&b).unwrap();
        fs::write(b.join(OPLOG), "create z Wall\n").unwrap();
        Store::open_or_create(&b).unwrap();
        let before = store(&b);
        assert!(Local::load(&b).unwrap().peer_is_new);
        assert_eq!(store(&b), before);
        sync(&b, &mut server).unwrap();
        sync(&a, &mut server).unwrap();
        assert_same(&[&s, &a, &b]);
    }

    /// A random edit that is valid against `dir`'s current document: creates of names that are not
    /// live, sets and deletes of names that are.
    fn random_edit(rng: &mut Rng, dir: &Path) -> String {
        const POOL: [&str; 5] = ["a", "b", "c", "d", "e"];
        let doc = Project::open(dir).unwrap().doc;
        let mut alive: Vec<&str> = POOL.iter().copied().filter(|n| doc.get(EntityId::derive(n)).is_some()).collect();
        let mut text = String::new();
        for _ in 0..=rng.below(3) {
            let name = POOL[rng.below(POOL.len())];
            if alive.contains(&name) {
                if rng.below(6) == 0 {
                    text.push_str(&format!("delete {name}\n"));
                    alive.retain(|n| *n != name);
                } else {
                    let key = ["h", "r", "t"][rng.below(3)];
                    let value = match rng.below(4) {
                        0 => format!("num {}", [0.0, -0.0, 2.5, 1e21][rng.below(4)]),
                        1 => format!("text {}", ["x", "north", "ünï"][rng.below(3)]),
                        2 => format!("ref {}", POOL[rng.below(POOL.len())]),
                        _ => format!("num {}", rng.below(5)),
                    };
                    text.push_str(&format!("set {name} {key} {value}\n"));
                }
            } else {
                text.push_str(&format!("create {name} {}\n", ["Wall", "Slab", "Level"][rng.below(3)]));
                alive.push(name);
            }
        }
        text
    }

    #[test]
    fn random_histories_with_crashes_converge_and_a_converged_sync_writes_nothing() {
        for seed in 1..=6u64 {
            let mut rng = Rng(seed * 7919);
            let tmp = TempDir::new();
            let dirs: Vec<PathBuf> = ["s", "p0", "p1", "p2"].iter().map(|n| project(&tmp, n)).collect();
            let mut server = DirBackend { dir: dirs[0].clone() };
            for step in 0..60 {
                let who = 1 + rng.below(3);
                match rng.below(10) {
                    0..=4 => {
                        let text = random_edit(&mut rng, &dirs[who]);
                        apply(&tmp, &dirs[who], &text);
                    }
                    5..=8 => {
                        let before = journal(&dirs[who]);
                        sync(&dirs[who], &mut server).unwrap_or_else(|e| panic!("seed {seed} step {step}: {e}"));
                        assert!(journal(&dirs[who]).starts_with(&before), "seed {seed} step {step}: a sync rewrote the journal");
                        // Sometimes the process dies before the journal rename: the old journal is back.
                        if rng.below(5) == 0 {
                            fs::write(dirs[who].join(OPLOG), before).unwrap();
                        }
                    }
                    _ => {
                        // The server directory is edited locally too.
                        let text = random_edit(&mut rng, &dirs[0]);
                        apply(&tmp, &dirs[0], &text);
                    }
                }
            }
            for _ in 0..3 {
                for dir in &dirs[1..] {
                    sync(dir, &mut server).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
                }
            }
            let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
            assert_same(&refs);
            let before: Vec<_> = dirs.iter().map(|d| (journal(d), store(d))).collect();
            for dir in &dirs[1..] {
                sync(dir, &mut server).unwrap();
            }
            let after: Vec<_> = dirs.iter().map(|d| (journal(d), store(d))).collect();
            assert_eq!(before, after, "seed {seed}: a converged sync wrote something");
            // Every directory's journal replays to exactly the document the sync log stands for.
            let reference = Local::load(&dirs[0]).unwrap();
            for dir in &dirs {
                let local = Local::load(dir).unwrap();
                assert!(local.local.is_empty());
                assert_eq!(local.log.to_vec(), reference.log.to_vec(), "seed {seed}: logs differ in {}", dir.display());
                assert!(same_state(&local.log.materialize().doc, &Project::open(dir).unwrap().doc));
            }
        }
    }

    #[test]
    fn an_apply_that_lands_while_a_sync_runs_makes_the_sync_fail_and_is_merged_by_the_next_one() {
        let tmp = TempDir::new();
        let (s, a, b) = (project(&tmp, "s"), project(&tmp, "a"), project(&tmp, "b"));
        let mut server = DirBackend { dir: s.clone() };
        apply(&tmp, &a, BASE);
        sync(&a, &mut server).unwrap();
        apply(&tmp, &b, "create from-b Wall\n");
        // B's sync reads its project, then an apply gets in before the sync writes.
        let local = Local::load(&b).unwrap();
        let snapshot = server.snapshot().unwrap();
        let received = snapshot.missing_for(&local.log.vv());
        apply(&tmp, &b, "set from-b h num 5\n");
        let store_before = store(&b);
        let Err(CliError::Input(message)) = local.persist(received) else { panic!("the sync ignored the apply") };
        assert!(message.contains("changed while this command was running"), "{message}");
        assert_eq!(store(&b), store_before, "nothing was written");
        assert!(String::from_utf8(journal(&b)).unwrap().ends_with("set from-b h num 5.0\n"), "the apply's line is intact");
        // The next sync takes everything in: the apply's line is a local edit, stamped after the merge.
        sync(&b, &mut server).unwrap();
        sync(&a, &mut server).unwrap();
        assert_same(&[&s, &a, &b]);
        let doc = Project::open(&a).unwrap().doc;
        assert_eq!(doc.get(EntityId::derive("from-b")).unwrap().props["h"], Value::Num(5.0));
    }

    #[test]
    fn a_merge_the_journal_cannot_express_is_refused_before_anything_is_written() {
        let tmp = TempDir::new();
        let a = project(&tmp, "a");
        let (journal_before, store_before) = (journal(&a), store(&a));
        // An entity that is created and nothing else: it would lack `_name` and `_schema`, which
        // every create in a journal sets.
        let bare = StampedOp { stamp: Stamp { counter: 1, peer: 99 }, op: SyncOp::Create { name: "bare".into(), kind: "Wall".into() } };
        let Err(CliError::Input(message)) = Local::load(&a).unwrap().persist(vec![bare]) else { panic!("a bare create merged") };
        assert!(message.contains("cannot be written to the journal"), "{message}");
        assert_eq!((journal(&a), store(&a)), (journal_before, store_before));
    }

    #[test]
    fn a_conflicting_history_is_refused_before_anything_is_written() {
        let tmp = TempDir::new();
        let (s, a) = (project(&tmp, "s"), project(&tmp, "a"));
        let mut server = DirBackend { dir: s.clone() };
        apply(&tmp, &a, "create x Wall\n");
        sync(&a, &mut server).unwrap();
        // A copy of A made now shares its peer id; both then edit, and both try to sync.
        let copy = tmp.path().join("copy");
        fs::create_dir(&copy).unwrap();
        for file in [OPLOG, STORE_FILE] {
            fs::copy(a.join(file), copy.join(file)).unwrap();
        }
        apply(&tmp, &a, "set x h num 1\n");
        apply(&tmp, &copy, "set x h num 2\n");
        sync(&a, &mut server).unwrap();
        let (before_journal, before_store) = (journal(&copy), store(&copy));
        let local = Local::load(&copy).unwrap();
        let snapshot = server.snapshot().unwrap();
        let error = local.log.check_prefixes(&snapshot.summary()).unwrap_err();
        assert!(matches!(error, SyncError::Diverged(_)), "{error:?}");
        // And if the edits themselves were handed over, persist refuses them whole.
        let result = local.persist(snapshot.to_vec());
        let Err(CliError::Input(message)) = result else { panic!("{result:?}") };
        assert!(message.contains("carry the stamp"), "{message}");
        assert_eq!((journal(&copy), store(&copy)), (before_journal, before_store));
    }
}
