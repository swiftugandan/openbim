//! Revisions: what `bim log` lists and what `bim diff <dir> <revA> <revB>` compares.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::types::ToSql;
use rusqlite::{params, Connection};

use crate::StoreError;

/// A recorded revision, as `bim log` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    /// The revision id: the content hash of the document when it was recorded, 16 hex digits.
    pub rev: String,
    /// The message given to `bim commit`.
    pub message: String,
    /// Ids of the ops recorded since the previous revision, in apply order. An op that was applied
    /// twice with the same canonical text has the same id both times, so an id can repeat.
    pub ops: Vec<String>,
}

/// One entity of a state or a snapshot: its shown name, its kind and its props in canonical text
/// (see [`crate::encode_value`]), keyed by prop name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityRow {
    /// The name the entity is shown under (its `_name` prop).
    pub name: String,
    /// The entity's kind.
    pub kind: String,
    /// Props by key, values in canonical text.
    pub props: BTreeMap<String, String>,
}

/// Entities by [`crate::entity_key`].
pub type Rows = BTreeMap<String, EntityRow>;

/// The difference between two revisions, entities named by [`EntityRow::name`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RevDiff {
    /// Entities only the second revision has, sorted.
    pub added: Vec<String>,
    /// Entities only the first revision has, sorted.
    pub removed: Vec<String>,
    /// Entities both have whose kind or props differ, sorted by name, each with the sorted keys
    /// that differ: present on one side only, or on both with different values. A changed kind is
    /// reported as the key `_kind`.
    pub changed: Vec<(String, Vec<String>)>,
}

impl RevDiff {
    /// True when the two revisions hold the same entities, kinds and props.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// The key `diff` reports when an entity's kind changed.
pub const KIND_KEY: &str = "_kind";

/// Compares two snapshots. Entities are matched by key (their id), not by name.
pub fn diff_rows(a: &Rows, b: &Rows) -> RevDiff {
    let mut diff = RevDiff::default();
    for (key, after) in b {
        let Some(before) = a.get(key) else {
            diff.added.push(after.name.clone());
            continue;
        };
        let mut keys: BTreeSet<&str> = BTreeSet::new();
        if before.kind != after.kind {
            keys.insert(KIND_KEY);
        }
        for (k, v) in &before.props {
            if after.props.get(k) != Some(v) {
                keys.insert(k);
            }
        }
        keys.extend(after.props.keys().filter(|k| !before.props.contains_key(*k)).map(String::as_str));
        if !keys.is_empty() {
            diff.changed.push((after.name.clone(), keys.into_iter().map(str::to_string).collect()));
        }
    }
    diff.removed.extend(a.iter().filter(|(key, _)| !b.contains_key(*key)).map(|(_, e)| e.name.clone()));
    diff.added.sort();
    diff.removed.sort();
    diff.changed.sort();
    diff
}

/// Reads the entity rows of `entities_sql` and the prop rows of `props_sql`, both bound to `args`.
pub(crate) fn load_rows(
    conn: &Connection,
    entities_sql: &str,
    props_sql: &str,
    args: &[&dyn ToSql],
) -> Result<Rows, StoreError> {
    let mut rows = Rows::new();
    let mut stmt = conn.prepare(entities_sql)?;
    let mut found = stmt.query(args)?;
    while let Some(row) = found.next()? {
        let (key, name, kind): (String, String, String) = (row.get(0)?, row.get(1)?, row.get(2)?);
        rows.insert(key, EntityRow { name, kind, props: BTreeMap::new() });
    }
    let mut stmt = conn.prepare(props_sql)?;
    let mut found = stmt.query(args)?;
    while let Some(row) = found.next()? {
        let (key, prop, value): (String, String, String) = (row.get(0)?, row.get(1)?, row.get(2)?);
        let Some(entity) = rows.get_mut(&key) else {
            return Err(StoreError::Corrupt(format!("a prop row names the unknown entity {key}")));
        };
        entity.props.insert(prop, value);
    }
    Ok(rows)
}

/// The snapshot recorded for `rev`.
fn snapshot(conn: &Connection, rev: &str) -> Result<Rows, StoreError> {
    load_rows(
        conn,
        "SELECT entity, name, kind FROM snap_entities WHERE rev = ?1",
        "SELECT entity, key, value FROM snap_props WHERE rev = ?1",
        &[&rev],
    )
}

fn revision_exists(conn: &Connection, rev: &str) -> Result<bool, StoreError> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM revisions WHERE rev = ?1)", [rev], |row| row.get(0))?)
}

/// `Store::revisions`: every revision, oldest first, each with the ops recorded since the one before.
pub(crate) fn revisions(conn: &Connection) -> Result<Vec<Revision>, StoreError> {
    // One read transaction, so a concurrent commit cannot show up halfway.
    let tx = conn.unchecked_transaction()?;
    let rows: Vec<(String, String, i64)> = {
        let mut stmt = tx.prepare("SELECT rev, message, last_seq FROM revisions ORDER BY seq")?;
        let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        mapped.collect::<Result<_, _>>()?
    };
    let mut ops = tx.prepare("SELECT op_id FROM oplog WHERE seq > ?1 AND seq <= ?2 ORDER BY seq")?;
    let mut out = Vec::with_capacity(rows.len());
    let mut previous = 0;
    for (rev, message, last_seq) in rows {
        let ids = ops.query_map(params![previous, last_seq], |row| row.get::<_, String>(0))?;
        out.push(Revision { rev, message, ops: ids.collect::<Result<_, _>>()? });
        previous = last_seq;
    }
    Ok(out)
}

/// `Store::diff`.
pub(crate) fn diff(conn: &Connection, rev_a: &str, rev_b: &str) -> Result<RevDiff, StoreError> {
    let tx = conn.unchecked_transaction()?;
    for rev in [rev_a, rev_b] {
        if !revision_exists(&tx, rev)? {
            return Err(StoreError::UnknownRevision(rev.to_string()));
        }
    }
    if rev_a == rev_b {
        return Ok(RevDiff::default());
    }
    Ok(diff_rows(&snapshot(&tx, rev_a)?, &snapshot(&tx, rev_b)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, kind: &str, props: &[(&str, &str)]) -> EntityRow {
        EntityRow {
            name: name.into(),
            kind: kind.into(),
            props: props.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn rows(entries: &[(&str, EntityRow)]) -> Rows {
        entries.iter().map(|(key, e)| (key.to_string(), e.clone())).collect()
    }

    #[test]
    fn identical_snapshots_do_not_differ() {
        let a = rows(&[("1", row("W1", "Wall", &[("h", "3.0")])), ("2", row("L1", "Level", &[]))]);
        assert!(diff_rows(&a, &a).is_empty());
        assert!(diff_rows(&Rows::new(), &Rows::new()).is_empty());
    }

    #[test]
    fn added_removed_and_changed_are_sorted_by_name() {
        let a = rows(&[
            ("1", row("zed", "Wall", &[("h", "3.0"), ("gone", "1.0"), ("same", "1.0")])),
            ("2", row("alpha", "Wall", &[("h", "1.0")])),
            ("3", row("removed-b", "Wall", &[])),
            ("4", row("removed-a", "Wall", &[])),
        ]);
        let b = rows(&[
            ("1", row("zed", "Wall", &[("h", "3.5"), ("new", "x"), ("same", "1.0")])),
            ("2", row("alpha", "Wall", &[("h", "2.0")])),
            ("5", row("added-b", "Wall", &[])),
            ("6", row("added-a", "Wall", &[])),
        ]);
        let d = diff_rows(&a, &b);
        assert_eq!(d.added, ["added-a", "added-b"]);
        assert_eq!(d.removed, ["removed-a", "removed-b"]);
        assert_eq!(
            d.changed,
            [
                ("alpha".to_string(), vec!["h".to_string()]),
                ("zed".to_string(), vec!["gone".to_string(), "h".to_string(), "new".to_string()]),
            ]
        );
        assert!(!d.is_empty());
        let back = diff_rows(&b, &a);
        assert_eq!((back.added.clone(), back.removed.clone()), (d.removed.clone(), d.added.clone()));
    }

    #[test]
    fn a_changed_kind_is_reported_as_the_reserved_key() {
        let a = rows(&[("1", row("w", "Wall", &[("h", "1.0")]))]);
        let b = rows(&[("1", row("w", "Slab", &[("h", "1.0")]))]);
        assert_eq!(diff_rows(&a, &b).changed, [("w".to_string(), vec![KIND_KEY.to_string()])]);
    }

    #[test]
    fn entities_are_matched_by_id_not_by_name() {
        let a = rows(&[("1", row("w", "Wall", &[]))]);
        let b = rows(&[("2", row("w", "Wall", &[]))]);
        let d = diff_rows(&a, &b);
        assert_eq!((d.added, d.removed), (vec!["w".to_string()], vec!["w".to_string()]));
    }
}
