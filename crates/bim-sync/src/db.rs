//! The replicated log inside `bim.sqlite`, next to `bim-store`'s tables.
//!
//! ```sql
//! sync_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL)          -- `peer`, and the journal marks
//! sync_ops(peer INTEGER, counter INTEGER, op BLOB, PRIMARY KEY(peer, counter))
//! ```
//!
//! `bim-store` checks only its own `meta.schema`, so it tolerates these tables, and this module
//! never touches `bim-store`'s. The connection uses the same settings as `bim-store`: the rollback
//! journal (`journal_mode = DELETE`, so an idle project directory holds no extra file),
//! `synchronous = FULL` (a committed transaction is on disk) and a 5 second busy timeout.
//!
//! The "ensure" operations read first and write only what is missing, so running them on a project
//! that is already set up leaves `bim.sqlite` byte for byte as it was.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};

use crate::codec::{op_bytes, op_from_bytes};
use crate::error::SyncError;
use crate::log::OpLog;
use crate::op::{StampedOp, MAX_COUNTER};
use crate::Stamp;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The meta key holding this project's peer id.
pub const PEER_KEY: &str = "peer";

const TABLES: &str = "
CREATE TABLE IF NOT EXISTS sync_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sync_ops(
    peer INTEGER NOT NULL, counter INTEGER NOT NULL, op BLOB NOT NULL, PRIMARY KEY(peer, counter)) WITHOUT ROWID;
";

/// An open `bim.sqlite`, seen through the sync tables.
pub struct SyncDb {
    conn: Connection,
    /// Whether the sync tables exist (they do not in a store that predates sync).
    tables: bool,
}

impl SyncDb {
    /// Opens the existing `bim.sqlite` at `path` without creating or changing anything. `Ok(None)`
    /// if there is no such file. A store without the sync tables reads as an empty log with no
    /// peer id.
    pub fn open_existing(path: &Path) -> Result<Option<SyncDb>, SyncError> {
        match fs::metadata(path) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(SyncError::Corrupt("exists and is not a file".to_string())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SyncError::Corrupt(e.to_string())),
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = configure(Connection::open_with_flags(path, flags)?)?;
        let tables = tables_present(&conn)?;
        Ok(Some(SyncDb { conn, tables }))
    }

    /// Opens `bim.sqlite` at `path`, creating the file and the sync tables if they are missing.
    pub fn open_or_create(path: &Path) -> Result<SyncDb, SyncError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = configure(Connection::open_with_flags(path, flags)?)?;
        let mut db = SyncDb { tables: tables_present(&conn)?, conn };
        if !db.tables {
            // Another process may be creating them right now: the statements are idempotent and the
            // immediate transaction makes the second one wait for the first.
            let tx = db.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(TABLES)?;
            tx.commit()?;
            db.tables = true;
        }
        Ok(db)
    }

    /// This project's peer id, if it has one.
    pub fn peer(&self) -> Result<Option<u32>, SyncError> {
        match self.meta(PEER_KEY)? {
            None => Ok(None),
            Some(text) => text
                .parse::<u32>()
                .map(Some)
                .map_err(|_| SyncError::Corrupt(format!("the stored peer id `{text}` is not a 32-bit number"))),
        }
    }

    /// This project's peer id, drawing a random one first if it has none. Writes nothing when the
    /// id is already there; two racing callers end up with the same id.
    pub fn ensure_peer(&mut self) -> Result<u32, SyncError> {
        if let Some(peer) = self.peer()? {
            return Ok(peer);
        }
        let txn = self.transaction()?;
        let peer = txn.set_peer(crate::random_peer())?;
        txn.commit()?;
        Ok(peer)
    }

    /// One meta value.
    pub fn meta(&self, key: &str) -> Result<Option<String>, SyncError> {
        if !self.tables {
            return Ok(None);
        }
        Ok(self.conn.query_row("SELECT value FROM sync_meta WHERE key = ?1", [key], |row| row.get(0)).optional()?)
    }

    /// Every stored edit.
    pub fn load_log(&self) -> Result<OpLog, SyncError> {
        let mut log = OpLog::new();
        if !self.tables {
            return Ok(log);
        }
        let mut stmt = self.conn.prepare("SELECT peer, counter, op FROM sync_ops ORDER BY peer, counter")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let (peer, counter, bytes): (i64, i64, Vec<u8>) = (row.get(0)?, row.get(1)?, row.get(2)?);
            let corrupt = |what: String| SyncError::Corrupt(format!("stored edit (counter {counter}, peer {peer}): {what}"));
            let peer = u32::try_from(peer).map_err(|_| corrupt("the peer id is out of range".to_string()))?;
            let counter = u64::try_from(counter).map_err(|_| corrupt("the counter is out of range".to_string()))?;
            let op = op_from_bytes(&bytes).map_err(corrupt)?;
            log.insert(StampedOp { stamp: Stamp { counter, peer }, op })
                .map_err(|e| SyncError::Corrupt(format!("stored edit (counter {counter}, peer {peer}): {e}")))?;
        }
        Ok(log)
    }

    /// Starts a write transaction (`BEGIN IMMEDIATE`: it waits for other writers). Dropping it
    /// without [`SyncTxn::commit`] rolls it back. The sync tables must exist.
    pub fn transaction(&mut self) -> Result<SyncTxn<'_>, SyncError> {
        if !self.tables {
            return Err(SyncError::Corrupt("the store has no sync tables".to_string()));
        }
        Ok(SyncTxn { tx: self.conn.transaction_with_behavior(TransactionBehavior::Immediate)? })
    }
}

/// A write transaction on the sync tables.
pub struct SyncTxn<'a> {
    tx: Transaction<'a>,
}

impl SyncTxn<'_> {
    /// Stores edits, skipping those already stored. An edit whose stamp is taken by a *different*
    /// edit is a [`SyncError::Conflict`] (and the caller should drop the transaction).
    pub fn insert_ops(&self, ops: &[StampedOp]) -> Result<(), SyncError> {
        let mut insert = self.tx.prepare_cached(
            "INSERT INTO sync_ops(peer, counter, op) VALUES(?1, ?2, ?3) ON CONFLICT(peer, counter) DO NOTHING",
        )?;
        let mut stored = self.tx.prepare_cached("SELECT op FROM sync_ops WHERE peer = ?1 AND counter = ?2")?;
        for op in ops {
            op.check().map_err(|why| SyncError::Protocol(format!("invalid edit: {why}")))?;
            let (peer, counter) = (i64::from(op.stamp.peer), op.stamp.counter.min(MAX_COUNTER) as i64);
            let bytes = op_bytes(&op.op);
            if insert.execute(params![peer, counter, bytes])? == 0 {
                let held: Vec<u8> = stored.query_row(params![peer, counter], |row| row.get(0))?;
                if held != bytes {
                    return Err(SyncError::Conflict(op.stamp));
                }
            }
        }
        Ok(())
    }

    /// Sets a meta value.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), SyncError> {
        self.tx.execute(
            "INSERT INTO sync_meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Stores `peer` as the peer id unless one is stored already; returns the id that is stored.
    pub fn set_peer(&self, peer: u32) -> Result<u32, SyncError> {
        self.tx.execute(
            "INSERT OR IGNORE INTO sync_meta(key, value) VALUES(?1, ?2)",
            params![PEER_KEY, peer.to_string()],
        )?;
        let text: String = self.tx.query_row("SELECT value FROM sync_meta WHERE key = ?1", [PEER_KEY], |row| row.get(0))?;
        text.parse::<u32>().map_err(|_| SyncError::Corrupt(format!("the stored peer id `{text}` is not a 32-bit number")))
    }

    /// Commits: everything this transaction did becomes durable at once.
    pub fn commit(self) -> Result<(), SyncError> {
        Ok(self.tx.commit()?)
    }
}

fn configure(conn: Connection) -> Result<Connection, SyncError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // `journal_mode` answers with a row, which `execute_batch` drops.
    conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA synchronous = FULL;")?;
    Ok(conn)
}

/// Whether both sync tables exist. One without the other is a damaged store.
fn tables_present(conn: &Connection) -> Result<bool, SyncError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('sync_meta', 'sync_ops')",
        [],
        |row| row.get(0),
    )?;
    match count {
        0 => Ok(false),
        2 => Ok(true),
        _ => Err(SyncError::Corrupt("the store has only one of the two sync tables".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::{SyncOp, SyncValue};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!("bim-sync-db-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn db(&self) -> PathBuf {
            self.0.join("bim.sqlite")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn op(counter: u64, peer: u32, name: &str) -> StampedOp {
        StampedOp { stamp: Stamp { counter, peer }, op: SyncOp::Delete { name: name.into() } }
    }

    #[test]
    fn open_existing_creates_nothing() {
        let s = Scratch::new();
        assert!(SyncDb::open_existing(&s.db()).unwrap().is_none());
        assert!(!s.db().exists());
        fs::create_dir(s.db()).unwrap();
        assert!(SyncDb::open_existing(&s.db()).is_err(), "a directory is not a store");
    }

    #[test]
    fn a_store_without_sync_tables_reads_as_empty_and_stays_untouched() {
        let s = Scratch::new();
        Connection::open(s.db()).unwrap().execute_batch("CREATE TABLE meta(key TEXT, value TEXT);").unwrap();
        let before = fs::read(s.db()).unwrap();
        let db = SyncDb::open_existing(&s.db()).unwrap().unwrap();
        assert_eq!(db.peer().unwrap(), None);
        assert_eq!(db.meta("anything").unwrap(), None);
        assert!(db.load_log().unwrap().is_empty());
        drop(db);
        assert_eq!(fs::read(s.db()).unwrap(), before);
    }

    #[test]
    fn setting_up_twice_leaves_the_file_byte_for_byte_alone() {
        let s = Scratch::new();
        let mut db = SyncDb::open_or_create(&s.db()).unwrap();
        let peer = db.ensure_peer().unwrap();
        drop(db);
        let before = fs::read(s.db()).unwrap();
        let mut db = SyncDb::open_or_create(&s.db()).unwrap();
        assert_eq!(db.ensure_peer().unwrap(), peer);
        assert_eq!(db.peer().unwrap(), Some(peer));
        drop(db);
        assert_eq!(fs::read(s.db()).unwrap(), before);
        assert_eq!(fs::read_dir(&s.0).unwrap().count(), 1, "no journal file is left behind");
    }

    #[test]
    fn the_log_round_trips_and_inserting_twice_is_harmless() {
        let s = Scratch::new();
        let mut db = SyncDb::open_or_create(&s.db()).unwrap();
        let ops = vec![
            op(1, 5, "a"),
            StampedOp {
                stamp: Stamp { counter: 2, peer: 5 },
                op: SyncOp::Set { name: "a".into(), key: "k".into(), value: SyncValue::List(vec![SyncValue::Num(-0.0)]) },
            },
            op(1, u32::MAX, "b"),
            op(MAX_COUNTER, 0, "c"),
        ];
        let txn = db.transaction().unwrap();
        txn.insert_ops(&ops).unwrap();
        txn.insert_ops(&ops).unwrap();
        txn.set_meta("journal_cur", "1:2:3").unwrap();
        txn.set_meta("journal_cur", "4:5:6").unwrap();
        txn.commit().unwrap();
        let log = db.load_log().unwrap();
        assert_eq!(log.len(), 4);
        let mut expected = ops.clone();
        expected.sort_by_key(|o| (o.stamp.peer, o.stamp.counter));
        assert_eq!(log.to_vec(), expected);
        assert_eq!(db.meta("journal_cur").unwrap().as_deref(), Some("4:5:6"));
        // Reopened, it is all still there.
        drop(db);
        let db = SyncDb::open_existing(&s.db()).unwrap().unwrap();
        assert_eq!(db.load_log().unwrap().to_vec(), expected);
    }

    #[test]
    fn a_different_edit_under_a_taken_stamp_is_a_conflict_and_rolls_back() {
        let s = Scratch::new();
        let mut db = SyncDb::open_or_create(&s.db()).unwrap();
        let txn = db.transaction().unwrap();
        txn.insert_ops(&[op(1, 1, "a")]).unwrap();
        txn.commit().unwrap();
        let txn = db.transaction().unwrap();
        txn.insert_ops(&[op(2, 1, "new")]).unwrap();
        let error = txn.insert_ops(&[op(1, 1, "different")]).unwrap_err();
        assert!(matches!(error, SyncError::Conflict(s) if s == Stamp { counter: 1, peer: 1 }));
        drop(txn); // rolled back
        assert_eq!(db.load_log().unwrap().len(), 1);
    }

    #[test]
    fn the_first_peer_id_sticks() {
        let s = Scratch::new();
        let mut db = SyncDb::open_or_create(&s.db()).unwrap();
        let txn = db.transaction().unwrap();
        assert_eq!(txn.set_peer(11).unwrap(), 11);
        assert_eq!(txn.set_peer(22).unwrap(), 11, "an id that is stored is never replaced");
        txn.commit().unwrap();
        assert_eq!(db.ensure_peer().unwrap(), 11);
    }

    #[test]
    fn damaged_rows_are_corrupt_not_panics() {
        let s = Scratch::new();
        let mut db = SyncDb::open_or_create(&s.db()).unwrap();
        db.ensure_peer().unwrap();
        db.conn.execute("INSERT INTO sync_ops(peer, counter, op) VALUES(1, 1, x'ff')", []).unwrap();
        assert!(matches!(db.load_log(), Err(SyncError::Corrupt(_))));
        db.conn.execute("DELETE FROM sync_ops", []).unwrap();
        db.conn.execute("INSERT INTO sync_ops(peer, counter, op) VALUES(-1, 1, x'0200000000')", []).unwrap();
        assert!(matches!(db.load_log(), Err(SyncError::Corrupt(_))));
        db.conn.execute("DELETE FROM sync_ops", []).unwrap();
        db.conn.execute("INSERT INTO sync_ops(peer, counter, op) VALUES(1, 0, x'0200000000')", []).unwrap();
        assert!(matches!(db.load_log(), Err(SyncError::Corrupt(_))), "counter 0 is not a stamp");
        db.conn.execute("UPDATE sync_meta SET value = 'x' WHERE key = 'peer'", []).unwrap();
        assert!(matches!(db.peer(), Err(SyncError::Corrupt(_))));
        let s2 = Scratch::new();
        Connection::open(s2.db()).unwrap().execute_batch("CREATE TABLE sync_meta(key TEXT, value TEXT);").unwrap();
        assert!(matches!(SyncDb::open_existing(&s2.db()), Err(SyncError::Corrupt(_))));
        fs::write(s2.db(), "this is not a database, but it is long enough to be taken for one").unwrap();
        assert!(SyncDb::open_existing(&s2.db()).is_err());
    }
}
