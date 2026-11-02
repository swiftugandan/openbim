//! `bim-store`: the SQLite store of a bim project, `<dir>/bim.sqlite`.
//!
//! The store holds, next to the project's text journal `doc.oplog`:
//!
//! * the **op log**: every op ever applied, content-addressed. An op's id ([`op_id`]) is the hash
//!   of its canonical text, so the same op has the same id wherever it is stored (`ops` maps an id
//!   to its text once; `oplog` lists the ids in apply order). Because the id is a function of the
//!   text alone, an op applied twice with byte-identical canonical text appears twice in `oplog`
//!   with one id;
//! * the **state**: the live document, entity by entity, prop by prop, in canonical text
//!   ([`encode_value`]);
//! * the **revisions**: for each [`Txn::record_revision`] the revision id (a content hash), its
//!   message, the stretch of the op log it closes, and a full snapshot of the document, so a
//!   revision diff never replays history. Revisions with the same id share one snapshot.
//!
//! # Transactions
//!
//! All writes go through one [`Txn`], an immediate SQLite transaction: it is all of it or none of
//! it, and a process killed halfway leaves the previous state (the next open rolls the hot
//! rollback journal back). The database uses the rollback journal (`journal_mode=DELETE`) with
//! `synchronous=FULL`, so when idle `bim.sqlite` is the only file and a committed transaction is
//! on disk.
//!
//! # The journal mark
//!
//! The store follows an external text journal and remembers how far it has read it, as a
//! [`JournalMark`]: the journal's length, the hash of its bytes and the number of ops in it. The
//! caller sets the mark in the same transaction that stores the journal's ops, and on the next
//! open compares it with the journal it finds to tell a store that is in step from one that is
//! behind (the journal grew) or unrelated (the journal was rewritten). Deciding what to do about
//! that is the caller's business; this crate only keeps the mark atomically with the rest.

#![forbid(unsafe_code)]

mod codec;
mod revision;
mod schema;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bim_doc::{Entity, EntityId};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};

pub use codec::{bytes_hash, encode_value, entity_key, op_id};
pub use revision::{diff_rows, EntityRow, RevDiff, Revision, Rows, KIND_KEY};

/// File name of the store inside a project directory.
pub const STORE_FILE: &str = "bim.sqlite";

/// The schema version this crate reads and writes.
pub const SCHEMA_VERSION: &str = "1";

/// How long a connection waits for another process's transaction before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite refused: the file is not a database, is locked, is read-only, the disk is full…
    Sqlite(rusqlite::Error),
    /// The file system refused.
    Io(io::Error),
    /// A well-formed revision id that no revision has.
    UnknownRevision(String),
    /// The store holds something this crate cannot make sense of: a schema it does not know, a
    /// damaged mark, two different ops with one id.
    Corrupt(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "{e}"),
            StoreError::Io(e) => write!(f, "{e}"),
            StoreError::UnknownRevision(rev) => write!(f, "unknown revision `{rev}`"),
            StoreError::Corrupt(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Sqlite(e) => Some(e),
            StoreError::Io(e) => Some(e),
            StoreError::UnknownRevision(_) | StoreError::Corrupt(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// What the store last read of the text journal it follows. See the crate docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalMark {
    /// Length of the journal in bytes.
    pub len: u64,
    /// [`bytes_hash`] of the journal's bytes.
    pub hash: u64,
    /// Number of ops the journal holds.
    pub ops: u64,
}

impl JournalMark {
    /// The mark of the journal `bytes`, which holds `ops` ops.
    pub fn of(bytes: &[u8], ops: u64) -> JournalMark {
        JournalMark { len: bytes.len() as u64, hash: bytes_hash(bytes), ops }
    }

    /// The mark of an empty journal: what a new store starts with.
    pub fn empty() -> JournalMark {
        JournalMark::of(b"", 0)
    }
}

/// An open `bim.sqlite`.
pub struct Store {
    conn: Connection,
}

/// The path of the store inside `dir`.
///
/// SQLite is built to read a file name that starts with `file:` as a URI, so a relative project
/// directory with that name would silently open a different file; `./` takes the name out of the
/// URI syntax.
pub fn store_path(dir: &Path) -> PathBuf {
    let path = dir.join(STORE_FILE);
    if path.to_string_lossy().starts_with("file:") {
        Path::new(".").join(path)
    } else {
        path
    }
}

impl Store {
    /// Opens `<dir>/bim.sqlite`, creating the file and its tables if they are missing. A new store
    /// starts with the mark of an empty journal, no ops and no revisions; an existing one is left
    /// as it is.
    pub fn open_or_create(dir: &Path) -> Result<Store, StoreError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        Store::configure(Connection::open_with_flags(store_path(dir), flags)?)
    }

    /// Opens `<dir>/bim.sqlite` if it exists (`Ok(None)` if not), without creating anything. The
    /// connection is read-write all the same: it is what lets SQLite roll back the hot journal a
    /// killed process left behind.
    pub fn open_existing(dir: &Path) -> Result<Option<Store>, StoreError> {
        let path = store_path(dir);
        match fs::metadata(&path) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(StoreError::Corrupt("exists and is not a file".to_string())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        Store::configure(Connection::open_with_flags(path, flags)?).map(Some)
    }

    fn configure(mut conn: Connection) -> Result<Store, StoreError> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // `journal_mode` answers with a row, which `execute_batch` drops.
        conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA synchronous = FULL; PRAGMA cache_size = -65536;")?;
        schema::ensure(&mut conn)?;
        Ok(Store { conn })
    }

    /// The mark the store holds.
    pub fn journal_mark(&self) -> Result<JournalMark, StoreError> {
        read_mark(&self.conn)
    }

    /// Begins a write transaction (`BEGIN IMMEDIATE`: it waits for other writers, and readers keep
    /// reading the old state). Dropping the [`Txn`] without [`Txn::commit`] rolls it back.
    pub fn transaction(&mut self) -> Result<Txn<'_>, StoreError> {
        Ok(Txn { tx: self.conn.transaction_with_behavior(TransactionBehavior::Immediate)? })
    }

    /// Every revision, oldest first.
    pub fn revisions(&self) -> Result<Vec<Revision>, StoreError> {
        revision::revisions(&self.conn)
    }

    /// What differs between two revisions: [`StoreError::UnknownRevision`] if the store has no
    /// revision with either id. A revision never differs from itself.
    pub fn diff(&self, rev_a: &str, rev_b: &str) -> Result<RevDiff, StoreError> {
        revision::diff(&self.conn, rev_a, rev_b)
    }

    /// The live state the store holds, by [`entity_key`]. Mainly for tests and tools: it is what
    /// the document looked like after the last transaction that updated it.
    pub fn state(&self) -> Result<Rows, StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        revision::load_rows(
            &tx,
            "SELECT entity, name, kind FROM state_entities",
            "SELECT entity, key, value FROM state_props",
            &[],
        )
    }
}

/// A write transaction on a [`Store`]; see [`Store::transaction`].
pub struct Txn<'a> {
    tx: Transaction<'a>,
}

impl Txn<'_> {
    /// The mark the store holds, as of this transaction.
    pub fn journal_mark(&self) -> Result<JournalMark, StoreError> {
        read_mark(&self.tx)
    }

    /// Replaces the mark.
    pub fn set_journal_mark(&self, mark: JournalMark) -> Result<(), StoreError> {
        let mut put = self.tx.prepare_cached(
            "INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )?;
        put.execute(params!["journal_len", mark.len.to_string()])?;
        put.execute(params!["journal_hash", format!("{:016x}", mark.hash)])?;
        put.execute(params!["journal_ops", mark.ops.to_string()])?;
        Ok(())
    }

    /// Appends ops to the op log, in order. Each is given as its canonical text; its id is
    /// [`op_id`] of it. A text the store already has is not stored again, but still takes its
    /// place in the log. Two different texts with one id are an error ([`StoreError::Corrupt`]).
    pub fn append_ops<'s>(&self, canonical: impl IntoIterator<Item = &'s str>) -> Result<(), StoreError> {
        let mut store = self.tx.prepare_cached("INSERT INTO ops(id, line) VALUES(?1, ?2) ON CONFLICT(id) DO NOTHING")?;
        let mut stored = self.tx.prepare_cached("SELECT line FROM ops WHERE id = ?1")?;
        let mut log = self.tx.prepare_cached("INSERT INTO oplog(op_id) VALUES(?1)")?;
        for line in canonical {
            let id = op_id(line);
            if store.execute(params![id, line])? == 0 {
                let known: String = stored.query_row([&id], |row| row.get(0))?;
                if known != line {
                    return Err(StoreError::Corrupt(format!("op id {id} belongs to two different ops")));
                }
            }
            log.execute([&id])?;
        }
        Ok(())
    }

    /// Makes the state hold `entity` as it is now, under the shown name `name`: its old rows (if
    /// any) are replaced.
    pub fn put_entity(&self, name: &str, entity: &Entity) -> Result<(), StoreError> {
        let key = entity_key(entity.id);
        self.tx
            .prepare_cached(
                "INSERT INTO state_entities(entity, name, kind) VALUES(?1, ?2, ?3) \
                 ON CONFLICT(entity) DO UPDATE SET name = excluded.name, kind = excluded.kind",
            )?
            .execute(params![key, name, entity.kind])?;
        self.tx.prepare_cached("DELETE FROM state_props WHERE entity = ?1")?.execute([&key])?;
        let mut insert = self.tx.prepare_cached("INSERT INTO state_props(entity, key, value) VALUES(?1, ?2, ?3)")?;
        let mut text = String::new();
        for (prop, value) in &entity.props {
            text.clear();
            encode_value(value, &mut text);
            insert.execute(params![key, prop, text])?;
        }
        Ok(())
    }

    /// Removes `id` from the state.
    pub fn remove_entity(&self, id: EntityId) -> Result<(), StoreError> {
        let key = entity_key(id);
        self.tx.prepare_cached("DELETE FROM state_props WHERE entity = ?1")?.execute([&key])?;
        self.tx.prepare_cached("DELETE FROM state_entities WHERE entity = ?1")?.execute([&key])?;
        Ok(())
    }

    /// Empties the state (not the op log, not the revisions).
    pub fn clear_state(&self) -> Result<(), StoreError> {
        self.tx.execute_batch("DELETE FROM state_props; DELETE FROM state_entities;")?;
        Ok(())
    }

    /// Records a revision `rev` with `message`. Its ops are the op-log entries since the previous
    /// revision. `snapshot` lists the document's entities with their shown names; it is stored
    /// unless an earlier revision with the same id has stored one (equal ids mean equal content).
    /// Recording a revision with nothing new, or one that repeats an earlier id, is allowed.
    pub fn record_revision<'e>(
        &self,
        rev: &str,
        message: &str,
        snapshot: impl IntoIterator<Item = (String, &'e Entity)>,
    ) -> Result<(), StoreError> {
        let seen: bool = self.tx.query_row("SELECT EXISTS(SELECT 1 FROM revisions WHERE rev = ?1)", [rev], |row| row.get(0))?;
        let last_seq: i64 = self.tx.query_row("SELECT COALESCE(MAX(seq), 0) FROM oplog", [], |row| row.get(0))?;
        self.tx.execute(
            "INSERT INTO revisions(rev, message, last_seq) VALUES(?1, ?2, ?3)",
            params![rev, message, last_seq],
        )?;
        if seen {
            return Ok(());
        }
        let mut entities = self.tx.prepare_cached("INSERT INTO snap_entities(rev, entity, name, kind) VALUES(?1, ?2, ?3, ?4)")?;
        let mut props = self.tx.prepare_cached("INSERT INTO snap_props(rev, entity, key, value) VALUES(?1, ?2, ?3, ?4)")?;
        let mut text = String::new();
        for (name, entity) in snapshot {
            let key = entity_key(entity.id);
            entities.execute(params![rev, key, name, entity.kind])?;
            for (prop, value) in &entity.props {
                text.clear();
                encode_value(value, &mut text);
                props.execute(params![rev, key, prop, text])?;
            }
        }
        Ok(())
    }

    /// Commits: everything this transaction did becomes durable at once.
    pub fn commit(self) -> Result<(), StoreError> {
        Ok(self.tx.commit()?)
    }
}

/// Reads one `meta` value.
fn read_meta(conn: &Connection, key: &str) -> Result<String, StoreError> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| row.get(0))
        .optional()?
        .ok_or_else(|| StoreError::Corrupt(format!("the store lacks its `{key}` entry")))
}

fn read_mark(conn: &Connection) -> Result<JournalMark, StoreError> {
    let number = |key: &str| -> Result<u64, StoreError> {
        let text = read_meta(conn, key)?;
        text.parse().map_err(|_| StoreError::Corrupt(format!("the store's `{key}` entry is not a number: `{text}`")))
    };
    let hash = read_meta(conn, "journal_hash")?;
    Ok(JournalMark {
        len: number("journal_len")?,
        hash: u64::from_str_radix(&hash, 16)
            .map_err(|_| StoreError::Corrupt(format!("the store's `journal_hash` entry is not a hash: `{hash}`")))?,
        ops: number("journal_ops")?,
    })
}

#[cfg(test)]
mod tests;
