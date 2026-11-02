//! How a [`Project`] and its SQLite store, `<dir>/bim.sqlite` (crate `bim-store`), are kept in step.
//!
//! **The journal `doc.oplog` is the source of truth for the document**: every read command replays
//! it and never opens the store, and a journal edited by hand is honoured. The store is a
//! transactional index that follows the journal: the content-addressed op log, the materialized
//! state, and the revisions `bim commit` records (which only the store holds).
//!
//! The store remembers how far it has read the journal, its *mark* ([`bim_store::JournalMark`]:
//! length, hash and op count). Every writing command (`apply`, `commit`) starts one SQLite
//! transaction, checks under its lock that the journal on disk is the one it replayed, and
//! **reconciles** the store with the journal (`Project::reconcile`) before doing its own work:
//!
//! * mark equals the journal: nothing to do;
//! * the journal is the marked journal plus whole new lines (a crash between the journal rename
//!   and the commit of `apply`, an edit by hand, a project that never had a store): the new ops
//!   are imported and the entities they touch refreshed;
//! * anything else (the journal was rewritten or truncated): the state is rebuilt from the
//!   journal and every journal op is logged again, so they all count for the next revision.
//!
//! `apply` ([`Project::apply_ops`]) orders its steps so a crash at any point leaves a state both
//! stores agree on or one the next reconcile repairs: store writes first, then the journal is
//! replaced (temporary file, sync, rename), and the SQLite commit comes last. Killed before the
//! rename, SQLite rolls the transaction back and the journal is still the old file; killed after
//! it, the journal is the new file and the store is merely behind. Either way the document is the
//! old one or the new one, never part of an apply, and `bim.sqlite` passes `integrity_check`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use bim_doc::{Document, EntityId};
use bim_store::{bytes_hash, JournalMark, Store, StoreError, Txn, STORE_FILE};

use crate::ops::{resolve_id, LineOp};
use crate::project::{io_error, replace_journal, shown_name, Project, OPLOG};
use crate::CliError;

/// The error a command reports for a store failure: always an input error (exit 2), never an
/// internal one, because the store lives in a user's directory and fails like any file there.
pub(crate) fn store_error(dir: &Path, error: StoreError) -> CliError {
    match error {
        StoreError::UnknownRevision(_) => CliError::Input(error.to_string()),
        other => CliError::Input(format!("{}: {other}", dir.join(STORE_FILE).display())),
    }
}

/// Opens `<dir>/bim.sqlite` if the project has one. Read commands use this: they never create it.
pub(crate) fn existing_store(dir: &Path) -> Result<Option<Store>, CliError> {
    Store::open_existing(dir).map_err(|e| store_error(dir, e))
}

fn canonical_lines(ops: &[LineOp]) -> Vec<String> {
    ops.iter().map(ToString::to_string).collect()
}

impl Project {
    /// Opens `<dir>/bim.sqlite`, creating it if the project has none yet.
    pub fn open_store(&self) -> Result<Store, CliError> {
        Store::open_or_create(&self.dir).map_err(|e| store_error(&self.dir, e))
    }

    /// Starts the write transaction of a command: waits for other writers, checks that
    /// `doc.oplog` is still the journal this project replayed (a writer that got in first would
    /// otherwise lose its ops), and reconciles the store with it.
    fn begin<'s>(&self, store: &'s mut Store) -> Result<Txn<'s>, CliError> {
        let txn = store.transaction().map_err(|e| store_error(&self.dir, e))?;
        let path = self.dir.join(OPLOG);
        if fs::read(&path).map_err(|e| io_error(&path, &e))? != self.journal.as_bytes() {
            return Err(CliError::Input(format!(
                "{}: changed while this command was running; run it again",
                path.display()
            )));
        }
        self.reconcile(&txn)?;
        Ok(txn)
    }

    /// If the journal is the marked journal plus whole new lines, the number of ops the marked
    /// part held (so `self.ops[n..]` are the new ones).
    fn ops_marked_in_prefix(&self, mark: JournalMark) -> Option<usize> {
        let bytes = self.journal.as_bytes();
        let len = usize::try_from(mark.len).ok().filter(|&len| len < bytes.len())?;
        let ops = usize::try_from(mark.ops).ok().filter(|&ops| ops <= self.ops.len())?;
        let ends_a_line = len == 0 || bytes[len - 1] == b'\n';
        (ends_a_line && bytes_hash(&bytes[..len]) == mark.hash).then_some(ops)
    }

    /// Brings the store in line with this project's journal, inside `txn`. See the module docs.
    pub(crate) fn reconcile(&self, txn: &Txn<'_>) -> Result<(), CliError> {
        let fail = |e| store_error(&self.dir, e);
        let now = JournalMark::of(self.journal.as_bytes(), self.ops.len() as u64);
        let marked = txn.journal_mark().map_err(fail)?;
        if marked == now {
            return Ok(());
        }
        if let Some(known) = self.ops_marked_in_prefix(marked) {
            let tail = &self.ops[known..];
            txn.append_ops(canonical_lines(tail).iter().map(String::as_str)).map_err(fail)?;
            self.write_entities(txn, &self.doc, touched(tail))?;
        } else {
            txn.clear_state().map_err(fail)?;
            for entity in self.doc.entities() {
                txn.put_entity(&shown_name(entity), entity).map_err(fail)?;
            }
            txn.append_ops(canonical_lines(&self.ops).iter().map(String::as_str)).map_err(fail)?;
        }
        txn.set_journal_mark(now).map_err(fail)
    }

    /// Makes the store's state agree with `doc` for each of `ids`: the entity as it is there, or
    /// gone if `doc` no longer has it.
    fn write_entities(&self, txn: &Txn<'_>, doc: &Document, ids: BTreeSet<EntityId>) -> Result<(), CliError> {
        for id in ids {
            match doc.get(id) {
                Some(entity) => txn.put_entity(&shown_name(entity), entity),
                None => txn.remove_entity(id),
            }
            .map_err(|e| store_error(&self.dir, e))?;
        }
        Ok(())
    }

    /// The transactional apply of `bim apply`: appends `ops` to the journal and the store in one
    /// SQLite transaction. The caller has checked the ops against the document and passes `after`,
    /// the document with them applied. Does nothing for an empty `ops`.
    ///
    /// Any failure before the commit leaves both the journal and the store as they were (the
    /// journal is put back if the commit itself fails). See the module docs for the crash cases.
    pub fn apply_ops(&self, ops: &[LineOp], after: &Document) -> Result<(), CliError> {
        if ops.is_empty() {
            return Ok(());
        }
        self.ensure_journal_writable()?;
        let fail = |e| store_error(&self.dir, e);
        let mut store = self.open_store()?;
        let txn = self.begin(&mut store)?;
        txn.append_ops(canonical_lines(ops).iter().map(String::as_str)).map_err(fail)?;
        self.write_entities(&txn, after, touched(ops))?;
        let text = self.appended(ops);
        txn.set_journal_mark(JournalMark::of(text.as_bytes(), (self.ops.len() + ops.len()) as u64)).map_err(fail)?;
        let path = self.dir.join(OPLOG);
        replace_journal(&self.dir, text.as_bytes()).map_err(|e| io_error(&path, &e))?;
        if let Err(e) = txn.commit() {
            // The journal already holds the new ops; the store does not. Put the old journal back.
            let _ = replace_journal(&self.dir, self.journal.as_bytes());
            return Err(fail(e));
        }
        Ok(())
    }

    /// Records a revision of the document as it is now and returns its id, the document's content
    /// hash as 16 hex digits. The revision lists the ops logged since the previous one.
    pub fn commit_revision(&self, message: &str) -> Result<String, CliError> {
        let rev = format!("{:016x}", self.doc.content_hash());
        let fail = |e| store_error(&self.dir, e);
        let mut store = self.open_store()?;
        let txn = self.begin(&mut store)?;
        let snapshot = self.doc.entities().map(|entity| (shown_name(entity), entity));
        txn.record_revision(&rev, message, snapshot).map_err(fail)?;
        txn.commit().map_err(fail)?;
        Ok(rev)
    }
}

/// The entities `ops` act on.
fn touched(ops: &[LineOp]) -> BTreeSet<EntityId> {
    ops.iter().map(|op| resolve_id(op.seed())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{parse_text, Located};
    use crate::project;
    use crate::testutil::TempDir;
    use bim_store::{encode_value, entity_key, op_id, EntityRow, Rows};

    fn init(tmp: &TempDir) {
        project::init(tmp.path()).unwrap();
        Store::open_or_create(tmp.path()).unwrap();
    }

    /// What `bim apply` does with an ops file holding `text`.
    fn apply(dir: &Path, text: &str) -> Result<(), CliError> {
        let project = Project::open(dir)?;
        let ops: Vec<LineOp> = parse_text(text).unwrap().into_iter().map(|Located { op, .. }| op).collect();
        let mut scratch = project.doc.clone();
        for op in &ops {
            op.apply_to(&mut scratch).unwrap();
        }
        project.apply_ops(&ops, &scratch)
    }

    fn journal(dir: &Path) -> String {
        fs::read_to_string(dir.join(OPLOG)).unwrap()
    }

    fn ids(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| op_id(line)).collect()
    }

    /// The rows the store should hold for `project`'s document.
    fn expected_state(project: &Project) -> Rows {
        project
            .doc
            .entities()
            .map(|entity| {
                let props = entity
                    .props
                    .iter()
                    .map(|(key, value)| {
                        let mut text = String::new();
                        encode_value(value, &mut text);
                        (key.clone(), text)
                    })
                    .collect();
                (entity_key(entity.id), EntityRow { name: shown_name(entity), kind: entity.kind.clone(), props })
            })
            .collect()
    }

    /// The store's state and mark are exactly those of the journal on disk.
    fn assert_in_step(dir: &Path) {
        let project = Project::open(dir).unwrap();
        let store = Store::open_existing(dir).unwrap().expect("a store");
        assert_eq!(store.state().unwrap(), expected_state(&project));
        assert_eq!(store.journal_mark().unwrap(), JournalMark::of(project.journal.as_bytes(), project.ops.len() as u64));
    }

    fn revisions(dir: &Path) -> Vec<(String, Vec<String>)> {
        let store = Store::open_existing(dir).unwrap().expect("a store");
        store.revisions().unwrap().into_iter().map(|r| (r.message, r.ops)).collect()
    }

    #[test]
    fn a_sequence_of_applies_keeps_the_state_equal_to_the_replayed_journal() {
        let tmp = TempDir::new();
        init(&tmp);
        assert_in_step(tmp.path());
        let batches = [
            "create L Level\nset L elevation num 0\ncreate w Wall\nset w host ref L\nset w h num 3\n",
            // delete and re-create inside one batch, a set after the re-create
            "delete w\ncreate w Slab\nset w t num 0.2\nset w h num 1\n",
            // a set on an entity another batch created, a ref to something that does not exist
            "set L elevation num -0\nset L ghost ref nobody\n",
            r#"{"op":"set","id":"L","key":"tags","value":["a b",true,[{"ref":"w"},2]]}"#,
            "delete L\n",
            "create L Level\n",
        ];
        for batch in batches {
            apply(tmp.path(), &format!("{batch}\n")).unwrap();
            assert_in_step(tmp.path());
        }
        let store = Store::open_existing(tmp.path()).unwrap().unwrap();
        let state = store.state().unwrap();
        assert_eq!(state.len(), 2);
        let w = &state[&entity_key(bim_doc::EntityId::derive("w"))];
        assert_eq!((w.kind.as_str(), w.props["t"].as_str(), w.props["h"].as_str()), ("Slab", "0.2", "1.0"));
    }

    #[test]
    fn a_rejected_or_empty_apply_leaves_the_store_alone() {
        let tmp = TempDir::new();
        init(&tmp);
        apply(tmp.path(), "create a W\n").unwrap();
        let before = fs::read(tmp.path().join(bim_store::STORE_FILE)).unwrap();
        let project = Project::open(tmp.path()).unwrap();
        project.apply_ops(&[], &project.doc).unwrap();
        assert_eq!(fs::read(tmp.path().join(bim_store::STORE_FILE)).unwrap(), before);
        assert_in_step(tmp.path());
    }

    #[test]
    fn the_ops_of_an_apply_are_logged_by_their_canonical_text() {
        let tmp = TempDir::new();
        init(&tmp);
        apply(tmp.path(), "create  a   Wall\n\n set a  h num 3\n").unwrap();
        Project::open(tmp.path()).unwrap().commit_revision("one").unwrap();
        assert_eq!(revisions(tmp.path()), [("one".to_string(), ids(&["create a Wall", "set a h num 3.0"]))]);
    }

    #[test]
    fn a_journal_that_grew_is_imported_as_a_tail() {
        // The state a crash after the journal rename (before the SQLite commit) leaves, or an edit by hand.
        let tmp = TempDir::new();
        init(&tmp);
        apply(tmp.path(), "create a Wall\nset a h num 1\n").unwrap();
        let grown = format!("{}\r\n\r\nset a h num 2\ncreate  b   Wall\nset b p ref a\ndelete a\n", journal(tmp.path()));
        fs::write(tmp.path().join(OPLOG), &grown).unwrap();
        let rev = Project::open(tmp.path()).unwrap().commit_revision("after the edit").unwrap();
        assert_eq!(rev.len(), 16);
        assert_in_step(tmp.path());
        assert_eq!(
            revisions(tmp.path()),
            [(
                "after the edit".to_string(),
                ids(&["create a Wall", "set a h num 1.0", "set a h num 2.0", "create b Wall", "set b p ref a", "delete a"])
            )]
        );
        assert_eq!(journal(tmp.path()), grown, "reconciling reads the journal, it never rewrites it");
    }

    #[test]
    fn apply_catches_a_store_that_is_behind_up_before_adding_its_own_ops() {
        let tmp = TempDir::new();
        init(&tmp);
        apply(tmp.path(), "create a Wall\n").unwrap();
        fs::write(tmp.path().join(OPLOG), "create a Wall\nset a h num 1.0\ncreate b Wall\n").unwrap();
        apply(tmp.path(), "set b h num 2\ndelete a\n").unwrap();
        assert_in_step(tmp.path());
        Project::open(tmp.path()).unwrap().commit_revision("r").unwrap();
        assert_eq!(
            revisions(tmp.path())[0].1,
            ids(&["create a Wall", "set a h num 1.0", "create b Wall", "set b h num 2.0", "delete a"])
        );
    }

    #[test]
    fn a_project_that_never_had_a_store_gets_one_from_its_journal() {
        let tmp = TempDir::new();
        project::init(tmp.path()).unwrap();
        fs::write(tmp.path().join(OPLOG), "create a Wall\nset a h num 1\n").unwrap();
        assert!(Store::open_existing(tmp.path()).unwrap().is_none());
        Project::open(tmp.path()).unwrap().commit_revision("first").unwrap();
        assert_in_step(tmp.path());
        assert_eq!(revisions(tmp.path())[0].1, ids(&["create a Wall", "set a h num 1.0"]));
    }

    #[test]
    fn a_rewritten_journal_rebuilds_the_state_and_keeps_the_revisions() {
        let tmp = TempDir::new();
        init(&tmp);
        apply(tmp.path(), "create a Wall\nset a h num 1\ncreate b Wall\n").unwrap();
        Project::open(tmp.path()).unwrap().commit_revision("old").unwrap();
        // Same length, different content; then a shorter journal; then an unrelated one; then none.
        for rewritten in [
            "create a Wall\nset a h num 2.0\ncreate b Wall\n",
            "create a Wall\n",
            "create zzz Slab\nset zzz t num 1\n",
            "",
        ] {
            fs::write(tmp.path().join(OPLOG), rewritten).unwrap();
            let project = Project::open(tmp.path()).unwrap();
            project.commit_revision("rewritten").unwrap();
            assert_in_step(tmp.path());
        }
        let log = revisions(tmp.path());
        assert_eq!(log.len(), 5);
        assert_eq!(log[0], ("old".to_string(), ids(&["create a Wall", "set a h num 1.0", "create b Wall"])));
        // After a rewrite every op of the new journal is logged again, so the next revision lists them all.
        assert_eq!(log[1].1, ids(&["create a Wall", "set a h num 2.0", "create b Wall"]));
        assert_eq!(log[2].1, ids(&["create a Wall"]));
        assert_eq!(log[3].1, ids(&["create zzz Slab", "set zzz t num 1.0"]));
        assert!(log[4].1.is_empty());
    }

    #[test]
    fn a_journal_extended_after_an_unterminated_last_line_is_rebuilt_not_trusted() {
        let tmp = TempDir::new();
        project::init(tmp.path()).unwrap();
        fs::write(tmp.path().join(OPLOG), "create a Wall").unwrap();
        Project::open(tmp.path()).unwrap().commit_revision("one").unwrap();
        // The marked bytes end mid-line: the "extension" may have changed that line (Wall -> Wallet).
        fs::write(tmp.path().join(OPLOG), "create a Wallet\nset a h num 2\n").unwrap();
        Project::open(tmp.path()).unwrap().commit_revision("two").unwrap();
        assert_in_step(tmp.path());
        let log = revisions(tmp.path());
        assert_eq!(log[0].1, ids(&["create a Wall"]));
        assert_eq!(log[1].1, ids(&["create a Wallet", "set a h num 2.0"]));
        let state = Store::open_existing(tmp.path()).unwrap().unwrap().state().unwrap();
        assert_eq!(state.values().next().unwrap().kind, "Wallet");
    }

    #[test]
    fn two_writers_that_read_the_same_journal_cannot_both_append() {
        let tmp = TempDir::new();
        init(&tmp);
        apply(tmp.path(), "create a Wall\n").unwrap();
        let (first, second) = (Project::open(tmp.path()).unwrap(), Project::open(tmp.path()).unwrap());
        let ops = |text: &str| -> Vec<LineOp> { parse_text(text).unwrap().into_iter().map(|l| l.op).collect() };
        let (one, two) = (ops("set a h num 1\n"), ops("set a h num 2\n"));
        let after = |project: &Project, ops: &[LineOp]| {
            let mut doc = project.doc.clone();
            ops.iter().for_each(|op| op.apply_to(&mut doc).unwrap());
            doc
        };
        first.apply_ops(&one, &after(&first, &one)).unwrap();
        let Err(CliError::Input(message)) = second.apply_ops(&two, &after(&second, &two)) else {
            panic!("the second writer overwrote the first one's ops")
        };
        assert!(message.contains("changed while this command was running"), "{message}");
        let Err(CliError::Input(_)) = second.commit_revision("late") else { panic!("commit on a stale project") };
        assert_eq!(journal(tmp.path()), "create a Wall\nset a h num 1.0\n");
        assert_in_step(tmp.path());
        assert!(revisions(tmp.path()).is_empty(), "the refused commit recorded nothing");
    }

    #[test]
    fn a_missing_journal_at_write_time_is_an_input_error() {
        let tmp = TempDir::new();
        init(&tmp);
        let project = Project::open(tmp.path()).unwrap();
        fs::remove_file(tmp.path().join(OPLOG)).unwrap();
        let ops: Vec<LineOp> = parse_text("create a Wall\n").unwrap().into_iter().map(|l| l.op).collect();
        assert!(matches!(project.apply_ops(&ops, &project.doc), Err(CliError::Input(_))));
        assert!(!tmp.path().join(OPLOG).exists(), "nothing recreated it");
    }

    #[test]
    fn store_failures_are_input_errors_that_name_the_file() {
        let tmp = TempDir::new();
        init(&tmp);
        fs::write(tmp.path().join(bim_store::STORE_FILE), "not a database at all, but long enough to be looked at").unwrap();
        let before = journal(tmp.path());
        let Err(CliError::Input(message)) = apply(tmp.path(), "create a Wall\n") else { panic!("apply into a broken store") };
        assert!(message.contains("bim.sqlite"), "{message}");
        assert_eq!(journal(tmp.path()), before, "the journal is untouched");
        assert!(matches!(Project::open(tmp.path()).unwrap().commit_revision("x"), Err(CliError::Input(_))));
        assert!(matches!(existing_store(tmp.path()), Err(CliError::Input(_))));
        let unknown = store_error(tmp.path(), StoreError::UnknownRevision("0123456789abcdef".into()));
        assert_eq!(unknown, CliError::Input("unknown revision `0123456789abcdef`".into()));
    }
}
