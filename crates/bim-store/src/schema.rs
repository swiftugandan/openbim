//! The tables of `bim.sqlite`.
//!
//! * `meta`: `schema`, and the journal mark (`journal_len`, `journal_hash`, `journal_ops`);
//! * `ops` (id → canonical text) and `oplog` (`seq` → op id): the op log. `oplog` only ever gains
//!   rows, which is what keeps a revision's stretch of it (`last_seq`) valid for good;
//! * `state_entities` / `state_props`: the live document;
//! * `revisions`: one row per recorded revision, `last_seq` the last `oplog.seq` it covers;
//! * `snap_entities` / `snap_props`: the document as of a revision id, written once per distinct id.
//!
//! Entities are keyed by [`crate::entity_key`].

use rusqlite::{Connection, TransactionBehavior};

use crate::{bytes_hash, StoreError, SCHEMA_VERSION};

const TABLES: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS ops(id TEXT PRIMARY KEY, line TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS oplog(seq INTEGER PRIMARY KEY, op_id TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS state_entities(
    entity TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS state_props(
    entity TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(entity, key)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS revisions(
    seq INTEGER PRIMARY KEY, rev TEXT NOT NULL, message TEXT NOT NULL, last_seq INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS revisions_rev ON revisions(rev);
CREATE TABLE IF NOT EXISTS snap_entities(
    rev TEXT NOT NULL, entity TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL,
    PRIMARY KEY(rev, entity)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS snap_props(
    rev TEXT NOT NULL, entity TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY(rev, entity, key)) WITHOUT ROWID;
";

/// Makes sure `conn` holds a store of the schema this crate knows, creating it in one transaction
/// if the file is new. A store that is already complete is only read.
pub(crate) fn ensure(conn: &mut Connection) -> Result<(), StoreError> {
    let created: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |row| row.get(0),
    )?;
    if !created {
        // Another process may be creating the same store right now: the statements are
        // idempotent, and the immediate transaction makes the second one wait for the first.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(TABLES)?;
        let empty = bytes_hash(b"");
        for (key, value) in [
            ("schema", SCHEMA_VERSION.to_string()),
            ("journal_len", "0".to_string()),
            ("journal_hash", format!("{empty:016x}")),
            ("journal_ops", "0".to_string()),
        ] {
            tx.execute("INSERT OR IGNORE INTO meta(key, value) VALUES(?1, ?2)", [key, value.as_str()])?;
        }
        tx.commit()?;
    }
    let found: Option<String> =
        optional_value(conn, "SELECT value FROM meta WHERE key = 'schema'")?;
    match found.as_deref() {
        Some(SCHEMA_VERSION) => Ok(()),
        Some(other) => Err(StoreError::Corrupt(format!(
            "unsupported store schema version `{other}` (this bim reads version {SCHEMA_VERSION})"
        ))),
        None => Err(StoreError::Corrupt("the store has no schema version".to_string())),
    }
}

fn optional_value(conn: &Connection, sql: &str) -> Result<Option<String>, StoreError> {
    use rusqlite::OptionalExtension;
    Ok(conn.query_row(sql, [], |row| row.get(0)).optional()?)
}
