//! The replicated state: a grow-only set of stamped edits, and the document it materializes to.
//!
//! **Why a set of edits is a state-based CRDT.** [`OpLog`] is keyed by stamp, so merging two logs
//! is a set union: commutative, associative and idempotent. The observable document is a pure
//! function of the *set* ([`materialize_ops`] folds the edits in any order), and that function is
//! `bim-crdt`'s: an entity is live iff its greatest create stamp exceeds its greatest delete
//! stamp, a prop is visible iff its entity is live and the prop's stamp exceeds the greatest delete
//! stamp, props are last-writer-wins by stamp and the kind is last-writer-wins among creates. The
//! tests check the fold against `bim_crdt::Replica` on random histories.
//!
//! **Version vectors are exact.** A peer's own counters strictly increase, a sender always ships
//! the whole per-peer suffix `{ops of p with counter > vv[p]}` and a receiver stores a received
//! batch atomically, so every holder has a *prefix* of each peer's edits and "holds (p, c)" is
//! "c <= vv\[p\]".

use std::collections::BTreeMap;

use bim_doc::{Document, EntityId, Op, Value};

use crate::codec::{put_op, put_u64};
use crate::error::SyncError;
use crate::op::{StampedOp, SyncOp, MAX_COUNTER};
use crate::Stamp;

/// For each peer, the greatest counter held. A missing peer holds counter 0, i.e. nothing.
pub type VersionVector = BTreeMap<u32, u64>;

/// What one side tells the other it holds: the version vector, and for each peer a digest of that
/// peer's edits up to and including its greatest counter.
///
/// The vector alone says "I hold (p, c)" for every c up to `vv[p]`; the digests let the other side
/// confirm that what it holds under those stamps is the same edits. Without them, two copies of
/// one project directory (they share a peer id) that were edited independently would each believe
/// the other had their edits, and silently stay different forever.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Summary {
    /// The greatest counter held, per peer.
    pub vv: VersionVector,
    /// Per peer in `vv`: the digest of that peer's edits with a counter up to `vv[peer]`.
    pub digests: BTreeMap<u32, u64>,
}

/// The pointwise maximum of two version vectors.
pub fn vv_max(a: &VersionVector, b: &VersionVector) -> VersionVector {
    let mut out = a.clone();
    for (&peer, &counter) in b {
        let slot = out.entry(peer).or_insert(0);
        *slot = (*slot).max(counter);
    }
    out
}

/// A set of stamped edits. Two edits never share a stamp: inserting a different edit under a
/// stamp that is taken is a [`SyncError::Conflict`].
#[derive(Debug, Clone, Default)]
pub struct OpLog {
    ops: BTreeMap<(u32, u64), SyncOp>,
    vv: VersionVector,
    clock: u64,
}

impl OpLog {
    /// An empty log.
    pub fn new() -> OpLog {
        OpLog::default()
    }

    /// Number of edits.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// True when the log holds no edit.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// True when the log holds the edit with this stamp.
    pub fn contains(&self, stamp: Stamp) -> bool {
        self.ops.contains_key(&(stamp.peer, stamp.counter))
    }

    /// The edit with this stamp.
    pub fn get(&self, stamp: Stamp) -> Option<&SyncOp> {
        self.ops.get(&(stamp.peer, stamp.counter))
    }

    /// The greatest counter of any peer, 0 for an empty log: the Lamport clock.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// The version vector.
    pub fn vv(&self) -> VersionVector {
        self.vv.clone()
    }

    /// The digest of `peer`'s edits with a counter up to and including `upto`: a hash of their
    /// (counter, edit) sequence in counter order. Equal for two logs exactly when (up to hash
    /// collisions) they hold the same edits of that peer in that range.
    pub fn digest(&self, peer: u32, upto: u64) -> u64 {
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        let fold = |mut h: u64, bytes: &[u8]| {
            for &b in bytes {
                h ^= u64::from(b);
                h = h.wrapping_mul(FNV_PRIME);
            }
            h
        };
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        let mut scratch = Vec::new();
        for (&(_, counter), op) in self.ops.range((peer, 0)..=(peer, upto)) {
            scratch.clear();
            put_u64(&mut scratch, counter);
            put_op(&mut scratch, op);
            h = fold(h, &(scratch.len() as u64).to_le_bytes());
            h = fold(h, &scratch);
        }
        // A finalizer, so logs that differ in one byte do not differ in one hash bit.
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h
    }

    /// The version vector with the digest of each peer's edits.
    pub fn summary(&self) -> Summary {
        let digests = self.vv.iter().map(|(&peer, &counter)| (peer, self.digest(peer, counter))).collect();
        Summary { vv: self.vv.clone(), digests }
    }

    /// Checks `theirs` against this log: wherever this log holds at least as much of a peer's edits
    /// as `theirs` does, the same edits must be there, else [`SyncError::Diverged`]. (Where this
    /// log holds less, the other side makes the same check against our summary.)
    pub fn check_prefixes(&self, theirs: &Summary) -> Result<(), SyncError> {
        for (&peer, &counter) in &theirs.vv {
            let Some(&their_digest) = theirs.digests.get(&peer) else {
                return Err(SyncError::Protocol(format!("the summary of peer {peer} has no digest")));
            };
            if self.vv.get(&peer).is_some_and(|&mine| mine >= counter) && self.digest(peer, counter) != their_digest {
                return Err(SyncError::Diverged(peer));
            }
        }
        Ok(())
    }

    /// Every edit, ordered by (peer, counter).
    pub fn iter(&self) -> impl Iterator<Item = (Stamp, &SyncOp)> + '_ {
        self.ops.iter().map(|(&(peer, counter), op)| (Stamp { counter, peer }, op))
    }

    /// Every edit, cloned, ordered by (peer, counter).
    pub fn to_vec(&self) -> Vec<StampedOp> {
        self.iter().map(|(stamp, op)| StampedOp { stamp, op: op.clone() }).collect()
    }

    /// The edits `vv` does not cover: for each peer, every edit with a counter above `vv[peer]`
    /// (a missing peer is 0). The complete per-peer suffix, ordered by (peer, counter).
    pub fn missing_for(&self, vv: &VersionVector) -> Vec<StampedOp> {
        let mut out = Vec::new();
        for (&peer, &held) in &self.vv {
            let Some(from) = vv.get(&peer).copied().unwrap_or(0).checked_add(1) else { continue };
            if from > held {
                continue;
            }
            out.extend(
                self.ops
                    .range((peer, from)..=(peer, u64::MAX))
                    .map(|(&(peer, counter), op)| StampedOp { stamp: Stamp { counter, peer }, op: op.clone() }),
            );
        }
        out
    }

    fn note(&mut self, stamp: Stamp) {
        let slot = self.vv.entry(stamp.peer).or_insert(0);
        *slot = (*slot).max(stamp.counter);
        self.clock = self.clock.max(stamp.counter);
    }

    /// Adds an edit. `Ok(true)` if it is new, `Ok(false)` if the log already holds this very edit
    /// (idempotence), an error if the edit is invalid or its stamp names a different edit.
    pub fn insert(&mut self, op: StampedOp) -> Result<bool, SyncError> {
        op.check().map_err(|why| SyncError::Protocol(format!("invalid edit: {why}")))?;
        let key = (op.stamp.peer, op.stamp.counter);
        match self.ops.get(&key) {
            Some(held) if *held == op.op => Ok(false),
            Some(_) => Err(SyncError::Conflict(op.stamp)),
            None => {
                self.note(op.stamp);
                self.ops.insert(key, op.op);
                Ok(true)
            }
        }
    }

    /// Adds many edits, all or none: on an error the log is unchanged. Returns how many were new.
    pub fn insert_all(&mut self, ops: impl IntoIterator<Item = StampedOp>) -> Result<usize, SyncError> {
        self.insert_new(ops).map(|fresh| fresh.len())
    }

    /// Like [`OpLog::insert_all`], but returns the edits that were new (ordered by peer and
    /// counter), so a caller can store exactly those.
    pub fn insert_new(&mut self, ops: impl IntoIterator<Item = StampedOp>) -> Result<Vec<StampedOp>, SyncError> {
        let mut fresh: BTreeMap<(u32, u64), SyncOp> = BTreeMap::new();
        for op in ops {
            op.check().map_err(|why| SyncError::Protocol(format!("invalid edit: {why}")))?;
            let key = (op.stamp.peer, op.stamp.counter);
            let held = self.ops.get(&key).or_else(|| fresh.get(&key));
            match held {
                Some(held) if *held == op.op => {}
                Some(_) => return Err(SyncError::Conflict(op.stamp)),
                None => {
                    fresh.insert(key, op.op);
                }
            }
        }
        let mut added = Vec::with_capacity(fresh.len());
        for ((peer, counter), op) in fresh {
            let stamp = Stamp { counter, peer };
            self.note(stamp);
            self.ops.insert((peer, counter), op.clone());
            added.push(StampedOp { stamp, op });
        }
        Ok(added)
    }

    /// Stamps `ops` as this peer's next edits: counters `clock + 1`, `clock + 2`, … in order. The
    /// log itself is not changed; insert the result. Fails only when the counters would run out.
    pub fn stamp_local(&self, peer: u32, ops: Vec<SyncOp>) -> Result<Vec<StampedOp>, SyncError> {
        let mut counter = self.clock;
        let mut out = Vec::with_capacity(ops.len());
        for op in ops {
            counter = counter
                .checked_add(1)
                .filter(|&c| c <= MAX_COUNTER)
                .ok_or_else(|| SyncError::Protocol("the Lamport clock is exhausted".to_string()))?;
            out.push(StampedOp { stamp: Stamp { counter, peer }, op });
        }
        Ok(out)
    }

    /// The document this log stands for. See the module docs.
    pub fn materialize(&self) -> Materialized {
        materialize_ops(self.iter())
    }
}

/// A materialized log.
#[derive(Debug, Clone)]
pub struct Materialized {
    /// The live entities with their visible props, entities in id order, props in key order.
    pub doc: Document,
    /// For every id any edit mentions (as a target or inside a Ref), the lexicographically smallest
    /// written name that stands for it. Deterministic, so every peer shows the same name.
    pub names: BTreeMap<EntityId, String>,
}

/// One entity's join-semilattice state, as in `bim-crdt`.
#[derive(Default)]
struct EntityState<'a> {
    /// Greatest create stamp and the kind that create gave.
    created: Option<(Stamp, &'a str)>,
    /// Greatest delete stamp.
    deleted: Option<Stamp>,
    /// Last-writer-wins registers.
    props: BTreeMap<&'a str, (Stamp, Value)>,
}

/// Folds edits into the document they stand for. The result depends only on *which* edits are
/// given, never on their order or on repeats of an edit.
pub fn materialize_ops<'a>(ops: impl IntoIterator<Item = (Stamp, &'a SyncOp)>) -> Materialized {
    let mut states: BTreeMap<EntityId, EntityState<'a>> = BTreeMap::new();
    let mut names: BTreeMap<EntityId, &'a str> = BTreeMap::new();
    let mut note = |id: EntityId, name: &'a str| match names.get_mut(&id) {
        Some(held) => {
            if name < *held {
                *held = name;
            }
        }
        None => {
            names.insert(id, name);
        }
    };
    for (stamp, op) in ops {
        let id = op.id();
        note(id, op.name());
        if let SyncOp::Set { value, .. } = op {
            value.for_each_ref(&mut |name| note(EntityId::parse(name), name));
        }
        let state = states.entry(id).or_default();
        match op {
            SyncOp::Create { kind, .. } => {
                let new = Some((stamp, kind.as_str()));
                if new > state.created {
                    state.created = new;
                }
            }
            SyncOp::Delete { .. } => {
                if Some(stamp) > state.deleted {
                    state.deleted = Some(stamp);
                }
            }
            SyncOp::Set { key, value, .. } => {
                let value = value.to_value();
                match state.props.get_mut(key.as_str()) {
                    Some((held_stamp, held_value)) => {
                        let wins = match stamp.cmp(held_stamp) {
                            std::cmp::Ordering::Greater => true,
                            std::cmp::Ordering::Less => false,
                            // Cannot happen in an `OpLog` (one edit per stamp); breaks the tie the
                            // way `bim-crdt` does so the fold stays order-insensitive regardless.
                            std::cmp::Ordering::Equal => format!("{value:?}") > format!("{held_value:?}"),
                        };
                        if wins {
                            *held_stamp = stamp;
                            *held_value = value;
                        }
                    }
                    None => {
                        state.props.insert(key.as_str(), (stamp, value));
                    }
                }
            }
        }
    }
    let mut doc = Document::new();
    for (id, state) in &states {
        let Some((_, kind)) = state.created.filter(|(created, _)| Some(*created) > state.deleted) else { continue };
        // Each id is created once into a fresh document and only live ids receive sets, so these
        // applies cannot fail.
        let _ = doc.apply(Op::Create { id: *id, kind: kind.to_string() });
        for (key, (stamp, value)) in &state.props {
            if Some(*stamp) > state.deleted {
                let _ = doc.apply(Op::Set { id: *id, key: key.to_string(), value: value.clone() });
            }
        }
    }
    let names = names.into_iter().map(|(id, name)| (id, name.to_string())).collect();
    Materialized { doc, names }
}

/// True when two documents hold the same entities, kinds and props, numbers compared by bit
/// pattern. (`Document`'s `==` also compares the op logs, which differ with how a state was
/// reached; `Value`'s `==` equates `0.0` and `-0.0`.)
pub fn same_state(a: &Document, b: &Document) -> bool {
    a.len() == b.len()
        && a.entities().zip(b.entities()).all(|(x, y)| {
            x.id == y.id
                && x.kind == y.kind
                && x.props.len() == y.props.len()
                && x.props.iter().zip(&y.props).all(|((kx, vx), (ky, vy))| kx == ky && same_value(vx, vy))
        })
}

/// Value equality with numbers compared by bit pattern.
pub fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => x.to_bits() == y.to_bits(),
        (Value::Text(x), Value::Text(y)) => x == y,
        (Value::Ref(x), Value::Ref(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::List(x), Value::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_value(p, q)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::SyncValue;

    fn st(counter: u64, peer: u32) -> Stamp {
        Stamp { counter, peer }
    }

    fn put(log: &mut OpLog, counter: u64, peer: u32, op: SyncOp) {
        assert!(log.insert(StampedOp { stamp: st(counter, peer), op }).unwrap());
    }

    fn create(name: &str, kind: &str) -> SyncOp {
        SyncOp::Create { name: name.into(), kind: kind.into() }
    }

    fn set(name: &str, key: &str, value: SyncValue) -> SyncOp {
        SyncOp::Set { name: name.into(), key: key.into(), value }
    }

    fn del(name: &str) -> SyncOp {
        SyncOp::Delete { name: name.into() }
    }

    fn num(x: f64) -> SyncValue {
        SyncValue::Num(x)
    }

    fn prop(m: &Materialized, name: &str, key: &str) -> Option<Value> {
        m.doc.get(EntityId::parse(name)).and_then(|e| e.props.get(key).cloned())
    }

    #[test]
    fn insert_distinguishes_new_duplicate_and_conflict() {
        let mut log = OpLog::new();
        let a = StampedOp { stamp: st(1, 7), op: create("x", "Wall") };
        assert!(log.insert(a.clone()).unwrap());
        assert!(!log.insert(a.clone()).unwrap(), "the same edit again is a no-op");
        let other = StampedOp { stamp: st(1, 7), op: create("x", "Slab") };
        assert!(matches!(log.insert(other), Err(SyncError::Conflict(s)) if s == st(1, 7)));
        let zero_vs_negative = StampedOp { stamp: st(2, 7), op: set("x", "k", num(0.0)) };
        log.insert(zero_vs_negative).unwrap();
        let flipped = StampedOp { stamp: st(2, 7), op: set("x", "k", num(-0.0)) };
        assert!(matches!(log.insert(flipped), Err(SyncError::Conflict(_))), "0.0 and -0.0 are different edits");
        assert_eq!(log.len(), 2);
        let bad = StampedOp { stamp: st(0, 7), op: del("x") };
        assert!(matches!(log.insert(bad), Err(SyncError::Protocol(_))));
        let nan = StampedOp { stamp: st(9, 7), op: set("x", "k", num(f64::NAN)) };
        assert!(matches!(log.insert(nan), Err(SyncError::Protocol(_))));
    }

    #[test]
    fn insert_all_is_atomic() {
        let mut log = OpLog::new();
        put(&mut log, 1, 1, create("a", "W"));
        let before = log.clone();
        let batch = vec![
            StampedOp { stamp: st(2, 1), op: del("a") },
            StampedOp { stamp: st(3, 2), op: create("b", "W") },
            StampedOp { stamp: st(1, 1), op: create("a", "Other") }, // conflicts with the held edit
        ];
        assert!(matches!(log.insert_all(batch), Err(SyncError::Conflict(_))));
        assert_eq!(log.to_vec(), before.to_vec());
        assert_eq!(log.vv(), before.vv());
        assert_eq!(log.clock(), 1);
        // A conflict inside the batch itself is caught too.
        let twice = vec![
            StampedOp { stamp: st(5, 3), op: del("a") },
            StampedOp { stamp: st(5, 3), op: del("b") },
        ];
        assert!(matches!(log.insert_all(twice), Err(SyncError::Conflict(_))));
        assert_eq!(log.len(), 1);
        // Repeats of one edit are fine and counted once.
        let same = vec![StampedOp { stamp: st(5, 3), op: del("a") }, StampedOp { stamp: st(5, 3), op: del("a") }];
        assert_eq!(log.insert_all(same).unwrap(), 1);
        assert_eq!(log.insert_all(log.to_vec()).unwrap(), 0, "re-inserting everything adds nothing");
    }

    #[test]
    fn vv_clock_and_missing_for_are_the_per_peer_suffixes() {
        let mut log = OpLog::new();
        assert_eq!((log.clock(), log.vv()), (0, VersionVector::new()));
        for (c, p) in [(1, 1), (2, 1), (5, 1), (3, 2), (4, 2), (9, 3)] {
            put(&mut log, c, p, del("x"));
        }
        assert_eq!(log.clock(), 9);
        assert_eq!(log.vv(), VersionVector::from([(1, 5), (2, 4), (3, 9)]));
        let stamps = |vv: &VersionVector| log.missing_for(vv).iter().map(|o| (o.stamp.counter, o.stamp.peer)).collect::<Vec<_>>();
        assert_eq!(stamps(&VersionVector::new()).len(), 6);
        assert_eq!(stamps(&VersionVector::from([(1, 2), (2, 4)])), [(5, 1), (9, 3)]);
        assert_eq!(stamps(&log.vv()), []);
        assert_eq!(stamps(&VersionVector::from([(1, u64::MAX), (2, 0), (3, 99)])), [(3, 2), (4, 2)]);
        // A vv that is ahead of the log asks for nothing.
        assert_eq!(stamps(&VersionVector::from([(1, 50), (2, 50), (3, 50)])), []);
    }

    #[test]
    fn digests_see_any_difference_in_the_range_and_nothing_outside_it() {
        let build = |tweak: bool| {
            let mut log = OpLog::new();
            put(&mut log, 1, 1, create("a", "W"));
            put(&mut log, 2, 1, set("a", "k", num(1.0)));
            put(&mut log, 3, 1, set("a", "k", num(if tweak { 2.0 } else { 3.0 })));
            put(&mut log, 1, 2, del("b"));
            log
        };
        let (x, y) = (build(false), build(true));
        assert_eq!(x.digest(1, 2), y.digest(1, 2), "the logs agree up to counter 2");
        assert_ne!(x.digest(1, 3), y.digest(1, 3));
        assert_eq!(x.digest(2, 1), y.digest(2, 1));
        assert_eq!(x.digest(9, 100), y.digest(9, 100), "an unknown peer has the empty digest");
        assert_ne!(x.digest(1, 1), x.digest(1, 2));
        assert_ne!(x.digest(1, 3), x.digest(2, 3), "different peers hold different edits");
        // The summary pairs the vector with the tip digests.
        let s = x.summary();
        assert_eq!(s.vv, x.vv());
        assert_eq!(s.digests[&1], x.digest(1, 3));
        assert_eq!(s.digests.keys().collect::<Vec<_>>(), s.vv.keys().collect::<Vec<_>>());
        // A longer history that extends a shorter one is consistent with it, both ways round.
        let mut longer = x.clone();
        put(&mut longer, 4, 1, del("a"));
        assert!(longer.check_prefixes(&x.summary()).is_ok());
        assert!(x.check_prefixes(&longer.summary()).is_ok(), "the shorter side has nothing to compare, and is not wrong");
        assert!(x.check_prefixes(&x.summary()).is_ok());
        // Different edits under the same stamps are found, from whichever side has the longer prefix.
        assert!(matches!(x.check_prefixes(&y.summary()), Err(SyncError::Diverged(1))));
        assert!(matches!(longer.check_prefixes(&y.summary()), Err(SyncError::Diverged(1))));
        let mut shorter_twin = OpLog::new();
        put(&mut shorter_twin, 1, 1, create("a", "OTHER"));
        assert!(matches!(x.check_prefixes(&shorter_twin.summary()), Err(SyncError::Diverged(1))));
        assert!(matches!(shorter_twin.check_prefixes(&x.summary()), Ok(())), "this side holds less than x: x does the check");
    }

    #[test]
    fn stamp_local_continues_the_clock_and_is_deterministic() {
        let mut log = OpLog::new();
        put(&mut log, 7, 1, create("a", "W"));
        let ops = vec![set("a", "k", num(1.0)), del("a")];
        let a = log.stamp_local(9, ops.clone()).unwrap();
        assert_eq!(a.iter().map(|o| (o.stamp.counter, o.stamp.peer)).collect::<Vec<_>>(), [(8, 9), (9, 9)]);
        assert_eq!(a, log.stamp_local(9, ops).unwrap());
        assert!(log.stamp_local(9, vec![]).unwrap().is_empty());
        let mut near_end = OpLog::new();
        near_end.insert(StampedOp { stamp: st(MAX_COUNTER, 1), op: del("a") }).unwrap();
        assert!(near_end.stamp_local(2, vec![del("b")]).is_err());
    }

    #[test]
    fn delete_beats_an_older_create_and_a_concurrent_lower_set() {
        let mut log = OpLog::new();
        put(&mut log, 1, 1, create("e", "Wall"));
        put(&mut log, 2, 1, set("e", "k", num(1.0))); // concurrent with the delete, lower stamp
        put(&mut log, 2, 2, del("e"));
        let m = log.materialize();
        assert!(m.doc.is_empty());
        // A later re-create revives the entity without the props older than the delete.
        put(&mut log, 3, 1, create("e", "Wall"));
        let m = log.materialize();
        assert!(m.doc.get(EntityId::derive("e")).unwrap().props.is_empty());
        put(&mut log, 4, 1, set("e", "k", num(5.0)));
        assert_eq!(prop(&log.materialize(), "e", "k"), Some(Value::Num(5.0)));
    }

    #[test]
    fn a_set_after_the_delete_does_not_revive_and_a_set_before_the_create_shows() {
        let mut log = OpLog::new();
        put(&mut log, 1, 1, create("e", "Wall"));
        put(&mut log, 2, 1, del("e"));
        put(&mut log, 3, 2, set("e", "k", num(1.0)));
        assert!(log.materialize().doc.is_empty(), "sets never revive");
        let mut early = OpLog::new();
        put(&mut early, 1, 1, set("e", "k", num(2.0)));
        assert!(early.materialize().doc.is_empty(), "never created");
        put(&mut early, 2, 1, create("e", "Wall"));
        assert_eq!(prop(&early.materialize(), "e", "k"), Some(Value::Num(2.0)));
    }

    #[test]
    fn kind_and_props_are_last_writer_wins_by_stamp() {
        let mut log = OpLog::new();
        put(&mut log, 1, 1, create("e", "Wall"));
        put(&mut log, 1, 2, create("e", "Slab"));
        put(&mut log, 2, 1, set("e", "k", num(1.0)));
        put(&mut log, 2, 2, set("e", "k", num(2.0)));
        put(&mut log, 3, 1, set("e", "k", num(3.0)));
        put(&mut log, 3, 1 + 1, set("e", "j", SyncValue::Text("x".into())));
        let m = log.materialize();
        let e = m.doc.get(EntityId::derive("e")).unwrap();
        assert_eq!(e.kind, "Slab", "the greater create stamp (1, peer 2) wins");
        assert_eq!(e.props["k"], Value::Num(3.0));
        assert_eq!(e.props["j"], Value::Text("x".into()));
    }

    #[test]
    fn names_are_the_smallest_written_name_of_every_mentioned_id() {
        let ulid = EntityId::derive("wall").to_ulid_string();
        let mut log = OpLog::new();
        put(&mut log, 1, 1, create("wall", "Wall"));
        put(&mut log, 2, 1, set(&ulid, "k", SyncValue::List(vec![SyncValue::Ref("ghost".into())])));
        let m = log.materialize();
        let wall = EntityId::derive("wall");
        assert_eq!(EntityId::parse(&ulid), wall, "the ULID text names the same entity");
        assert_eq!(m.names[&wall], std::cmp::min(ulid.as_str(), "wall"));
        assert_eq!(m.names[&EntityId::derive("ghost")], "ghost");
        assert_eq!(m.names.len(), 2);
    }

    #[test]
    fn the_fold_does_not_depend_on_order_or_repeats() {
        let mut log = OpLog::new();
        for (c, p, op) in [
            (1, 1, create("a", "W")),
            (2, 1, set("a", "k", num(1.0))),
            (3, 1, set("a", "r", SyncValue::Ref("b".into()))),
            (1, 2, create("b", "W")),
            (2, 2, del("b")),
            (3, 2, set("a", "k", num(2.0))),
            (4, 2, create("b", "S")),
        ] {
            put(&mut log, c, p, op);
        }
        let all = log.to_vec();
        let reference = materialize_ops(all.iter().map(|o| (o.stamp, &o.op)));
        let mut seed = 0x1234_5678_u64;
        for _ in 0..50 {
            // Fisher-Yates with an LCG, plus a repeated prefix.
            let mut shuffled = all.clone();
            for i in (1..shuffled.len()).rev() {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                shuffled.swap(i, (seed >> 33) as usize % (i + 1));
            }
            shuffled.extend(all.iter().take(3).cloned());
            let m = materialize_ops(shuffled.iter().map(|o| (o.stamp, &o.op)));
            assert!(same_state(&m.doc, &reference.doc));
            assert_eq!(m.doc.content_hash(), reference.doc.content_hash());
            assert_eq!(m.names, reference.names);
        }
    }

    #[test]
    fn same_state_compares_bits_not_logs() {
        let mut a = Document::new();
        let mut b = Document::new();
        let id = EntityId::derive("e");
        for d in [&mut a, &mut b] {
            d.apply(Op::Create { id, kind: "W".into() }).unwrap();
        }
        a.apply(Op::Set { id, key: "k".into(), value: Value::Num(0.0) }).unwrap();
        b.apply(Op::Set { id, key: "k".into(), value: Value::Num(-0.0) }).unwrap();
        assert_eq!(a.get(id), b.get(id), "Value's == equates the zeros");
        assert!(!same_state(&a, &b));
        b.apply(Op::Set { id, key: "k".into(), value: Value::Num(0.0) }).unwrap();
        assert!(same_state(&a, &b), "logs differ, state does not");
        assert_ne!(a, b);
    }
}
