//! `bim-crdt`: a state-based (convergent) CRDT replica of a [`bim_doc::Document`].
//!
//! Each [`Replica`] belongs to one peer and records local edits
//! ([`Replica::create`], [`Replica::set`], [`Replica::delete`]) stamped with a
//! Lamport clock [`Stamp`] `(counter, peer)`. Replicas exchange whole states
//! with [`Replica::merge`], which is a join on a semilattice and therefore
//! commutative, associative and idempotent.
//!
//! Per entity the replica keeps:
//! - the greatest create stamp together with that create's kind (so the kind is
//!   last-writer-wins among creates),
//! - the greatest delete stamp (a tombstone),
//! - a last-writer-wins register per prop key: `(stamp, value)`.
//!
//! Semantics (all derived from stamps alone, so every replica that has seen the
//! same set of edits agrees):
//! - an entity is **live** iff its greatest create stamp exceeds its greatest
//!   delete stamp (an entity never created is not live);
//! - a prop is **visible** iff its entity is live and the prop's stamp exceeds the
//!   entity's greatest delete stamp. So props written before a delete stay dead
//!   even when the entity is later re-created, while props written after the
//!   revival are visible.
//!
//! [`Replica::document`] materializes the live state as a plain
//! [`bim_doc::Document`] (no provenance props are injected), and
//! [`Replica::diagnostics`] reports every visible [`Value::Ref`] whose target is
//! not live — the conflicts a concurrent delete can leave behind.
//!
//! [`Replica::evaluate`] adds the semantic layer on top of the merged state:
//! dangling refs, Ref cycles and unsatisfiable constraint groups become
//! materialized `ConflictDiagnostic` entities (see [`Evaluated`]).
//!
//! Local edits never fail: a `set` or `delete` on an unknown id is simply
//! recorded and takes effect according to the rules above once merged.

use std::collections::BTreeMap;

use bim_doc::{Document, EntityId, Op, Value};

mod semantic;

pub use semantic::{Diagnostic, Evaluated, Reason, CONFLICT_DIAGNOSTIC_KIND, MAX_MAGNITUDE, UNSATISFIABLE_KEY};

/// A Lamport timestamp: `(counter, peer)`, ordered lexicographically.
///
/// Stamps issued by distinct edits are distinct, because each peer increments
/// its counter for every local edit and the peer id breaks ties between peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    /// Lamport counter.
    pub counter: u64,
    /// Id of the peer that issued the edit.
    pub peer: u32,
}

/// A visible reference prop whose target entity is tombstoned or missing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConflictDiagnostic {
    /// The live entity holding the reference.
    pub entity: EntityId,
    /// The prop key holding the reference.
    pub key: String,
    /// The referenced entity, which is not live.
    pub target: EntityId,
}

/// A last-writer-wins prop register.
#[derive(Debug, Clone)]
struct PropReg {
    stamp: Stamp,
    value: Value,
}

impl PropReg {
    /// True when `other` should replace `self` in a join.
    ///
    /// Higher stamp wins. Equal stamps normally denote the same edit (and hence
    /// the same value); should they ever differ, a deterministic order on the
    /// value's debug encoding decides, keeping the join commutative.
    fn loses_to(&self, other: &PropReg) -> bool {
        match other.stamp.cmp(&self.stamp) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => value_key(&other.value) > value_key(&self.value),
        }
    }
}

/// A total, deterministic encoding of a value used only to break stamp ties.
fn value_key(v: &Value) -> String {
    format!("{v:?}")
}

/// Join-semilattice state of one entity.
#[derive(Debug, Clone, Default)]
struct EntityState {
    /// Greatest create stamp and the kind given by that create.
    created: Option<(Stamp, String)>,
    /// Greatest delete stamp.
    deleted: Option<Stamp>,
    /// Last-writer-wins registers per prop key.
    props: BTreeMap<String, PropReg>,
}

impl EntityState {
    fn is_live(&self) -> bool {
        match &self.created {
            Some((c, _)) => Some(*c) > self.deleted,
            None => false,
        }
    }

    fn prop_visible(&self, reg: &PropReg) -> bool {
        self.is_live() && Some(reg.stamp) > self.deleted
    }

    /// Visible props in key order (empty when the entity is not live).
    fn visible_props(&self) -> impl Iterator<Item = (&String, &Value)> {
        let live = self.is_live();
        self.props
            .iter()
            .filter(move |(_, r)| live && Some(r.stamp) > self.deleted)
            .map(|(k, r)| (k, &r.value))
    }

    fn join(&mut self, other: &EntityState) {
        if other.created > self.created {
            self.created = other.created.clone();
        }
        if other.deleted > self.deleted {
            self.deleted = other.deleted;
        }
        for (k, theirs) in &other.props {
            match self.props.get_mut(k) {
                Some(mine) => {
                    if mine.loses_to(theirs) {
                        *mine = theirs.clone();
                    }
                }
                None => {
                    self.props.insert(k.clone(), theirs.clone());
                }
            }
        }
    }
}

/// One peer's replica of the shared document state.
#[derive(Debug, Clone)]
pub struct Replica {
    peer: u32,
    /// Greatest Lamport counter seen locally or via merge.
    clock: u64,
    entities: BTreeMap<EntityId, EntityState>,
}

impl Replica {
    /// An empty replica owned by `peer`.
    pub fn new(peer: u32) -> Replica {
        Replica { peer, clock: 0, entities: BTreeMap::new() }
    }

    /// The peer id this replica stamps local edits with.
    pub fn peer(&self) -> u32 {
        self.peer
    }

    /// The greatest Lamport counter this replica has seen.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    fn tick(&mut self) -> Stamp {
        self.clock = self.clock.saturating_add(1);
        Stamp { counter: self.clock, peer: self.peer }
    }

    /// Creates (or re-creates) entity `id` with `kind`. Never fails.
    ///
    /// A create with a stamp above every delete seen so far revives a deleted
    /// entity; props older than the latest delete stay invisible.
    pub fn create(&mut self, id: EntityId, kind: String) {
        let s = self.tick();
        let e = self.entities.entry(id).or_default();
        let new = Some((s, kind));
        if new > e.created {
            e.created = new;
        }
    }

    /// Sets prop `key` of entity `id` to `value`. Never fails. On an entity that
    /// is not (yet) live the write is recorded; it becomes visible once the
    /// entity is live, provided no delete with a stamp above this write is seen.
    pub fn set(&mut self, id: EntityId, key: String, value: Value) {
        let s = self.tick();
        let e = self.entities.entry(id).or_default();
        let reg = PropReg { stamp: s, value };
        match e.props.get_mut(&key) {
            Some(mine) => {
                if mine.loses_to(&reg) {
                    *mine = reg;
                }
            }
            None => {
                e.props.insert(key, reg);
            }
        }
    }

    /// Deletes entity `id` by recording a tombstone. Never fails.
    pub fn delete(&mut self, id: EntityId) {
        let s = self.tick();
        let e = self.entities.entry(id).or_default();
        if Some(s) > e.deleted {
            e.deleted = Some(s);
        }
    }

    /// Joins `other`'s state into this replica.
    ///
    /// Commutative, associative and idempotent (with respect to observable
    /// state). The Lamport clock advances to the maximum of both clocks, so later
    /// local edits are stamped above everything merged.
    pub fn merge(&mut self, other: &Replica) {
        self.clock = self.clock.max(other.clock);
        for (id, theirs) in &other.entities {
            self.entities.entry(*id).or_default().join(theirs);
        }
    }

    /// True iff `id` is live in this replica.
    pub fn is_live(&self, id: EntityId) -> bool {
        self.entities.get(&id).is_some_and(EntityState::is_live)
    }

    /// Materializes the live state: every live entity (in id order) with its
    /// winning kind and its visible props. No provenance props are added.
    pub fn document(&self) -> Document {
        let mut doc = Document::new();
        for (id, e) in &self.entities {
            let Some((_, kind)) = e.created.as_ref().filter(|_| e.is_live()) else {
                continue;
            };
            // Each id is created once into a fresh document and only live ids
            // receive sets, so these applies cannot fail; ignore defensively.
            let _ = doc.apply(Op::Create { id: *id, kind: kind.clone() });
            for (k, v) in e.visible_props() {
                let _ = doc.apply(Op::Set { id: *id, key: k.clone(), value: v.clone() });
            }
        }
        doc
    }

    /// Every visible `Ref` prop of a live entity whose target is not live
    /// (tombstoned or never created), sorted. Never panics.
    ///
    /// The result corresponds one-to-one with
    /// [`Document::dangling_refs`] of [`Replica::document`].
    pub fn diagnostics(&self) -> Vec<ConflictDiagnostic> {
        let mut out = Vec::new();
        for (id, e) in &self.entities {
            for (k, reg) in &e.props {
                if !e.prop_visible(reg) {
                    continue;
                }
                if let Value::Ref(target) = &reg.value {
                    if !self.is_live(*target) {
                        out.push(ConflictDiagnostic { entity: *id, key: k.clone(), target: *target });
                    }
                }
            }
        }
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> EntityId {
        EntityId::derive(s)
    }

    fn merged(a: &Replica, b: &Replica) -> Replica {
        let mut m = a.clone();
        m.merge(b);
        m
    }

    #[test]
    fn local_edits_never_fail_on_unknown_ids() {
        let mut r = Replica::new(1);
        r.set(id("x"), "k".into(), Value::Num(1.0));
        r.delete(id("y"));
        assert!(r.document().is_empty());
        assert!(r.diagnostics().is_empty());
    }

    #[test]
    fn set_before_create_becomes_visible() {
        let mut r = Replica::new(1);
        r.set(id("e"), "k".into(), Value::Num(1.0));
        r.create(id("e"), "W".into());
        assert_eq!(r.document().get(id("e")).unwrap().props.get("k"), Some(&Value::Num(1.0)));
    }

    #[test]
    fn concurrent_sets_resolve_by_stamp() {
        let mut a = Replica::new(1);
        a.create(id("w"), "Wall".into());
        let mut b = Replica::new(2);
        b.merge(&a);
        a.set(id("w"), "h".into(), Value::Num(1.0)); // (2,1)
        b.set(id("w"), "h".into(), Value::Num(2.0)); // (2,2) wins
        let ab = merged(&a, &b);
        let ba = merged(&b, &a);
        assert_eq!(ab.document(), ba.document());
        assert_eq!(ab.document().get(id("w")).unwrap().props.get("h"), Some(&Value::Num(2.0)));
    }

    #[test]
    fn concurrent_creates_pick_kind_by_stamp() {
        let mut a = Replica::new(1);
        let mut b = Replica::new(2);
        a.create(id("e"), "Wall".into());
        b.create(id("e"), "Slab".into());
        let m = merged(&a, &b);
        assert_eq!(m.document().get(id("e")).unwrap().kind, "Slab");
        assert_eq!(merged(&b, &a).document(), m.document());
    }

    #[test]
    fn delete_wins_over_older_create_and_concurrent_lower_set() {
        let mut a = Replica::new(1);
        a.create(id("e"), "Wall".into()); // (1,1)
        let mut b = Replica::new(2);
        b.merge(&a);
        a.set(id("e"), "k".into(), Value::Num(1.0)); // (2,1)
        b.delete(id("e")); // (2,2) beats the set
        let m = merged(&a, &b);
        assert!(!m.is_live(id("e")));
        // A later re-create revives without the old prop.
        let mut m2 = m.clone();
        m2.create(id("e"), "Wall".into());
        assert!(m2.document().get(id("e")).unwrap().props.is_empty());
        m2.set(id("e"), "k".into(), Value::Num(5.0));
        assert_eq!(m2.document().get(id("e")).unwrap().props.get("k"), Some(&Value::Num(5.0)));
    }

    #[test]
    fn diagnostics_match_dangling_refs_and_are_sorted() {
        let mut r = Replica::new(1);
        r.create(id("a"), "W".into());
        r.create(id("b"), "W".into());
        r.set(id("a"), "z".into(), Value::Ref(id("gone")));
        r.set(id("a"), "m".into(), Value::Ref(id("b")));
        r.set(id("b"), "self".into(), Value::Ref(id("b")));
        r.delete(id("b"));
        let d = r.diagnostics();
        let mut sorted = d.clone();
        sorted.sort();
        assert_eq!(d, sorted);
        assert_eq!(d.len(), 2);
        assert_eq!(r.document().dangling_refs().len(), d.len());
        assert!(d.contains(&ConflictDiagnostic { entity: id("a"), key: "m".into(), target: id("b") }));
    }

    #[test]
    fn merge_is_idempotent_and_advances_clock() {
        let mut a = Replica::new(1);
        for _ in 0..5 {
            a.create(id("e"), "W".into());
        }
        let mut b = Replica::new(2);
        b.merge(&a);
        assert_eq!(b.clock(), 5);
        b.delete(id("e"));
        assert!(!b.is_live(id("e")));
        let bb = merged(&b, &b);
        assert_eq!(bb.document(), b.document());
        assert_eq!(bb.diagnostics(), b.diagnostics());
    }
}
