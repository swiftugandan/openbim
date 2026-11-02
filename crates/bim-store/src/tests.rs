//! Tests of the store against real database files in scratch directories.

use std::sync::atomic::{AtomicUsize, Ordering};

use bim_doc::Value;

use super::*;

/// A fresh, empty directory removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("bim-store-unit-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entity(seed: &str, kind: &str, props: &[(&str, Value)]) -> Entity {
    Entity {
        id: EntityId::derive(seed),
        kind: kind.to_string(),
        props: props.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
    }
}

fn wall(seed: &str, height: f64) -> Entity {
    entity(
        seed,
        "Wall",
        &[("_name", Value::Text(seed.into())), ("_schema", Value::Num(1.0)), ("height", Value::Num(height))],
    )
}

fn integrity(store: &Store) -> Vec<String> {
    let mut stmt = store.conn.prepare("PRAGMA integrity_check").unwrap();
    let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

fn count(store: &Store, table: &str) -> i64 {
    store.conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap()
}

fn rev_of(n: u64) -> String {
    format!("{n:016x}")
}

#[test]
fn a_new_store_is_a_valid_empty_database_with_the_empty_mark() {
    let tmp = TempDir::new();
    let store = Store::open_or_create(tmp.path()).unwrap();
    assert_eq!(tmp.files(), [STORE_FILE]);
    assert_eq!(store.journal_mark().unwrap(), JournalMark::empty());
    assert_eq!(JournalMark::empty(), JournalMark { len: 0, hash: bytes_hash(b""), ops: 0 });
    assert!(store.revisions().unwrap().is_empty());
    assert!(store.state().unwrap().is_empty());
    assert_eq!(integrity(&store), ["ok"]);
    drop(store);
    assert!(fs::read(tmp.path().join(STORE_FILE)).unwrap().starts_with(b"SQLite format 3\0"));
}

#[test]
fn the_rollback_journal_leaves_nothing_but_the_database_file_when_idle() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    let mode: String = store.conn.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
    assert_eq!(mode, "delete");
    let sync: i64 = store.conn.query_row("PRAGMA synchronous", [], |row| row.get(0)).unwrap();
    assert_eq!(sync, 2, "FULL");
    let txn = store.transaction().unwrap();
    txn.append_ops(["create a Wall"]).unwrap();
    txn.commit().unwrap();
    assert_eq!(tmp.files(), [STORE_FILE]);
}

#[test]
fn reopening_keeps_everything_and_never_resets_the_mark() {
    let tmp = TempDir::new();
    let mark = JournalMark::of(b"create a Wall\n", 1);
    {
        let mut store = Store::open_or_create(tmp.path()).unwrap();
        let txn = store.transaction().unwrap();
        txn.append_ops(["create a Wall"]).unwrap();
        txn.put_entity("a", &wall("a", 3.0)).unwrap();
        txn.set_journal_mark(mark).unwrap();
        txn.record_revision(&rev_of(1), "first", [("a".to_string(), &wall("a", 3.0))]).unwrap();
        txn.commit().unwrap();
    }
    for open in [Store::open_or_create(tmp.path()).unwrap(), Store::open_existing(tmp.path()).unwrap().unwrap()] {
        assert_eq!(open.journal_mark().unwrap(), mark);
        assert_eq!(open.revisions().unwrap().len(), 1);
        assert_eq!(open.state().unwrap().len(), 1);
    }
}

#[test]
fn open_existing_does_not_create_a_missing_store() {
    let tmp = TempDir::new();
    assert!(Store::open_existing(tmp.path()).unwrap().is_none());
    assert_eq!(tmp.files(), Vec::<String>::new());
    fs::create_dir(tmp.path().join(STORE_FILE)).unwrap();
    assert!(matches!(Store::open_existing(tmp.path()), Err(StoreError::Corrupt(_))));
}

#[test]
fn a_file_that_is_not_a_database_is_an_error_not_a_panic() {
    let tmp = TempDir::new();
    let junk = "this is not a database, just some text, long enough to look odd";
    fs::write(tmp.path().join(STORE_FILE), junk).unwrap();
    assert!(matches!(Store::open_or_create(tmp.path()), Err(StoreError::Sqlite(_))));
    assert!(matches!(Store::open_existing(tmp.path()), Err(StoreError::Sqlite(_))));
    assert_eq!(fs::read(tmp.path().join(STORE_FILE)).unwrap(), junk.as_bytes(), "left alone");
}

#[test]
fn a_store_of_another_schema_version_is_refused() {
    let tmp = TempDir::new();
    drop(Store::open_or_create(tmp.path()).unwrap());
    let conn = Connection::open(tmp.path().join(STORE_FILE)).unwrap();
    conn.execute("UPDATE meta SET value = '99' WHERE key = 'schema'", []).unwrap();
    drop(conn);
    let Err(StoreError::Corrupt(message)) = Store::open_or_create(tmp.path()) else { panic!("schema 99 accepted") };
    assert!(message.contains("`99`"), "{message}");
}

#[test]
fn a_damaged_mark_is_reported() {
    let tmp = TempDir::new();
    let store = Store::open_or_create(tmp.path()).unwrap();
    store.conn.execute("UPDATE meta SET value = 'xyz' WHERE key = 'journal_len'", []).unwrap();
    assert!(matches!(store.journal_mark(), Err(StoreError::Corrupt(_))));
    store.conn.execute("UPDATE meta SET value = '12' WHERE key = 'journal_len'", []).unwrap();
    store.conn.execute("DELETE FROM meta WHERE key = 'journal_hash'", []).unwrap();
    assert!(matches!(store.journal_mark(), Err(StoreError::Corrupt(_))));
}

#[test]
fn store_paths_never_read_as_uris() {
    assert_eq!(store_path(Path::new("proj")), Path::new("proj").join(STORE_FILE));
    assert_eq!(store_path(Path::new("/abs/file:x")), Path::new("/abs/file:x").join(STORE_FILE));
    assert_eq!(store_path(Path::new("file:x?mode=memory")), Path::new("./file:x?mode=memory").join(STORE_FILE));
    assert!(!store_path(Path::new("file:x")).to_string_lossy().starts_with("file:"));
}

#[test]
fn directories_with_odd_names_work() {
    let tmp = TempDir::new();
    for name in ["file:odd?mode=memory&x=1", "sp ace", "ünï", "a#b"] {
        let dir = tmp.path().join(name);
        fs::create_dir(&dir).unwrap();
        let mut store = Store::open_or_create(&dir).unwrap();
        let txn = store.transaction().unwrap();
        txn.append_ops(["create a Wall"]).unwrap();
        txn.commit().unwrap();
        drop(store);
        assert!(dir.join(STORE_FILE).is_file(), "{name}");
        assert_eq!(Store::open_existing(&dir).unwrap().unwrap().journal_mark().unwrap(), JournalMark::empty());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let dir = tmp.path().join(std::ffi::OsStr::from_bytes(b"p\xff\xfe"));
        fs::create_dir(&dir).unwrap();
        drop(Store::open_or_create(&dir).unwrap());
        assert!(Store::open_existing(&dir).unwrap().is_some(), "a directory name that is not UTF-8");
    }
}

#[test]
fn ops_are_content_addressed_and_logged_in_order() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    let lines = ["create a Wall", "set a h num 3.0", "set a h num 4.0", "set a h num 3.0"];
    let txn = store.transaction().unwrap();
    txn.append_ops(lines).unwrap();
    txn.record_revision(&rev_of(1), "r", []).unwrap();
    txn.commit().unwrap();
    assert_eq!(count(&store, "ops"), 3, "a repeated op text is stored once");
    assert_eq!(count(&store, "oplog"), 4, "but logged every time it was applied");
    let ids: Vec<String> = lines.iter().map(|l| op_id(l)).collect();
    assert_eq!(ids[1], ids[3]);
    assert_eq!(store.revisions().unwrap()[0].ops, ids);
    let text: String = store.conn.query_row("SELECT line FROM ops WHERE id = ?1", [&ids[2]], |row| row.get(0)).unwrap();
    assert_eq!(text, "set a h num 4.0");
}

#[test]
fn two_different_ops_with_one_id_are_refused() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    store.conn.execute("INSERT INTO ops(id, line) VALUES(?1, 'something else')", [op_id("create a Wall")]).unwrap();
    let txn = store.transaction().unwrap();
    let Err(StoreError::Corrupt(message)) = txn.append_ops(["create a Wall"]) else { panic!("collision accepted") };
    assert!(message.contains(&op_id("create a Wall")), "{message}");
}

#[test]
fn a_revision_covers_the_ops_since_the_previous_one() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    let ids = |lines: &[&str]| lines.iter().map(|l| op_id(l)).collect::<Vec<_>>();
    let txn = store.transaction().unwrap();
    txn.append_ops(["create a Wall", "set a h num 1.0"]).unwrap();
    txn.record_revision(&rev_of(1), "first", []).unwrap();
    txn.commit().unwrap();
    let txn = store.transaction().unwrap();
    txn.record_revision(&rev_of(1), "nothing new", []).unwrap();
    txn.append_ops(["set a h num 2.0"]).unwrap();
    txn.commit().unwrap();
    let txn = store.transaction().unwrap();
    txn.append_ops(["delete a", "create a Slab"]).unwrap();
    txn.record_revision(&rev_of(3), "third, with \"quotes\" and ünïcode", []).unwrap();
    txn.commit().unwrap();
    let revisions = store.revisions().unwrap();
    assert_eq!(
        revisions,
        [
            Revision { rev: rev_of(1), message: "first".into(), ops: ids(&["create a Wall", "set a h num 1.0"]) },
            Revision { rev: rev_of(1), message: "nothing new".into(), ops: vec![] },
            Revision {
                rev: rev_of(3),
                message: "third, with \"quotes\" and ünïcode".into(),
                ops: ids(&["set a h num 2.0", "delete a", "create a Slab"])
            },
        ]
    );
}

#[test]
fn a_dropped_transaction_changes_nothing() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    {
        let txn = store.transaction().unwrap();
        txn.append_ops(["create a Wall"]).unwrap();
        txn.put_entity("a", &wall("a", 1.0)).unwrap();
        txn.set_journal_mark(JournalMark::of(b"x", 1)).unwrap();
        txn.record_revision(&rev_of(1), "lost", [("a".to_string(), &wall("a", 1.0))]).unwrap();
    }
    assert_eq!((count(&store, "ops"), count(&store, "oplog"), count(&store, "state_entities")), (0, 0, 0));
    assert_eq!((count(&store, "revisions"), count(&store, "snap_entities")), (0, 0));
    assert_eq!(store.journal_mark().unwrap(), JournalMark::empty());
    assert_eq!(integrity(&store), ["ok"]);
}

#[test]
fn a_failing_step_rolls_the_whole_transaction_back() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    store.conn.execute("INSERT INTO ops(id, line) VALUES(?1, 'other')", [op_id("set a h num 2.0")]).unwrap();
    let txn = store.transaction().unwrap();
    txn.put_entity("a", &wall("a", 1.0)).unwrap();
    let result = txn.append_ops(["create a Wall", "set a h num 2.0"]);
    assert!(result.is_err());
    drop(txn);
    assert_eq!((count(&store, "oplog"), count(&store, "state_entities"), count(&store, "ops")), (0, 0, 1));
}

#[test]
fn a_second_writer_cannot_start_while_a_transaction_is_open() {
    let tmp = TempDir::new();
    let mut first = Store::open_or_create(tmp.path()).unwrap();
    let mut second = Store::open_or_create(tmp.path()).unwrap();
    second.conn.busy_timeout(Duration::ZERO).unwrap();
    let txn = first.transaction().unwrap();
    let Err(StoreError::Sqlite(error)) = second.transaction() else { panic!("two writers at once") };
    assert!(error.to_string().contains("locked"), "{error}");
    // Readers are not blocked: they see the old state.
    assert!(second.revisions().unwrap().is_empty());
    txn.commit().unwrap();
    second.transaction().unwrap().commit().unwrap();
}

#[test]
fn put_entity_replaces_the_old_rows_and_remove_entity_deletes_them() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    let txn = store.transaction().unwrap();
    let first = entity("a", "Wall", &[("_name", Value::Text("a".into())), ("h", Value::Num(1.0)), ("old", Value::Bool(true))]);
    txn.put_entity("a", &first).unwrap();
    txn.put_entity("b", &wall("b", 2.0)).unwrap();
    txn.commit().unwrap();
    let key = entity_key(EntityId::derive("a"));
    let state = store.state().unwrap();
    assert_eq!(state[&key].props.keys().collect::<Vec<_>>(), ["_name", "h", "old"]);
    assert_eq!(state[&key].props["old"], "true");

    let second = entity("a", "Slab", &[("_name", Value::Text("a".into())), ("h", Value::Num(2.5))]);
    let txn = store.transaction().unwrap();
    txn.put_entity("a", &second).unwrap();
    txn.remove_entity(EntityId::derive("b")).unwrap();
    txn.remove_entity(EntityId::derive("never-there")).unwrap();
    txn.commit().unwrap();
    let state = store.state().unwrap();
    assert_eq!(state.len(), 1);
    let row = &state[&key];
    assert_eq!((row.name.as_str(), row.kind.as_str()), ("a", "Slab"));
    assert_eq!(row.props.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>(), [("_name", "\"a\""), ("h", "2.5")]);
    assert_eq!(count(&store, "state_props"), 2, "no prop row of a removed entity or of a dropped key is left");

    let txn = store.transaction().unwrap();
    txn.clear_state().unwrap();
    txn.commit().unwrap();
    assert_eq!((count(&store, "state_entities"), count(&store, "state_props")), (0, 0));
}

#[test]
fn state_rows_encode_values_canonically() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    let e = entity(
        "e",
        "K",
        &[
            ("n", Value::Num(-0.0)),
            ("t", Value::Text("a\"b".into())),
            ("b", Value::Bool(false)),
            ("r", Value::Ref(EntityId(10))),
            ("l", Value::List(vec![Value::Num(1.0), Value::List(vec![])])),
        ],
    );
    let txn = store.transaction().unwrap();
    txn.put_entity("e", &e).unwrap();
    txn.commit().unwrap();
    let props = &store.state().unwrap()[&entity_key(e.id)].props;
    let got: Vec<(&str, &str)> = props.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        got,
        [
            ("b", "false"),
            ("l", "[1.0,[]]"),
            ("n", "0.0"),
            ("r", "{\"ref\":\"0000000000000000000000000000000a\"}"),
            ("t", "\"a\\\"b\""),
        ]
    );
}

#[test]
fn revisions_diff_by_snapshot_not_by_history() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    let w1 = wall("W1", 3.0);
    let w3 = wall("W3", 3.0);
    let docs: [(u64, Vec<Entity>); 3] = [
        (1, vec![w1.clone(), w3.clone()]),
        (2, vec![wall("W1", 3.5), wall("W2", 3.0)]),
        (3, vec![wall("W1", 3.5), entity("W2", "Wall", &[("_name", Value::Text("W2".into())), ("x2", Value::Num(6.0))])]),
    ];
    for (n, entities) in &docs {
        let txn = store.transaction().unwrap();
        let named: Vec<(String, &Entity)> = entities
            .iter()
            .map(|e| (if let Some(Value::Text(name)) = e.props.get("_name") { name.clone() } else { String::new() }, e))
            .collect();
        txn.record_revision(&rev_of(*n), &format!("r{n}"), named).unwrap();
        txn.commit().unwrap();
    }
    let d = store.diff(&rev_of(1), &rev_of(2)).unwrap();
    assert_eq!(d.added, ["W2"]);
    assert_eq!(d.removed, ["W3"]);
    assert_eq!(d.changed, [("W1".to_string(), vec!["height".to_string()])]);
    let d = store.diff(&rev_of(2), &rev_of(3)).unwrap();
    assert_eq!((d.added.len(), d.removed.len()), (0, 0));
    assert_eq!(d.changed, [("W2".to_string(), vec!["_schema".to_string(), "height".to_string(), "x2".to_string()])]);
    let d = store.diff(&rev_of(1), &rev_of(3)).unwrap();
    assert_eq!((d.added, d.removed), (vec!["W2".to_string()], vec!["W3".to_string()]));
    assert!(store.diff(&rev_of(2), &rev_of(2)).unwrap().is_empty());
    assert!(!store.diff(&rev_of(2), &rev_of(1)).unwrap().is_empty());
}

#[test]
fn an_unknown_revision_is_named_in_the_error() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    let txn = store.transaction().unwrap();
    txn.record_revision(&rev_of(1), "one", []).unwrap();
    txn.commit().unwrap();
    for (a, b, unknown) in [(1, 2, 2), (2, 1, 2), (2, 2, 2), (1, 1, 0)] {
        let result = store.diff(&rev_of(a), &rev_of(b));
        if unknown == 0 {
            assert!(result.unwrap().is_empty());
        } else {
            let Err(StoreError::UnknownRevision(rev)) = result else { panic!("{a} {b}") };
            assert_eq!(rev, rev_of(unknown));
            assert_eq!(StoreError::UnknownRevision(rev).to_string(), format!("unknown revision `{}`", rev_of(unknown)));
        }
    }
}

#[test]
fn a_repeated_revision_id_shares_the_first_snapshot() {
    let tmp = TempDir::new();
    let mut store = Store::open_or_create(tmp.path()).unwrap();
    let w = wall("W", 1.0);
    for message in ["a", "b"] {
        let txn = store.transaction().unwrap();
        txn.record_revision(&rev_of(7), message, [("W".to_string(), &w)]).unwrap();
        txn.commit().unwrap();
    }
    assert_eq!(count(&store, "revisions"), 2);
    assert_eq!(count(&store, "snap_entities"), 1);
    assert_eq!(count(&store, "snap_props"), 3);
    // An empty document is a revision like any other, even though it has no snapshot rows.
    for _ in 0..2 {
        let txn = store.transaction().unwrap();
        txn.record_revision(&rev_of(8), "empty", []).unwrap();
        txn.commit().unwrap();
    }
    assert!(store.diff(&rev_of(8), &rev_of(8)).unwrap().is_empty());
    let d = store.diff(&rev_of(8), &rev_of(7)).unwrap();
    assert_eq!(d.added, ["W"]);
    assert_eq!(integrity(&store), ["ok"]);
}
