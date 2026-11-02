//! `bim-doc`: a content-addressed semantic document graph with an op log.
//!
//! A [`Document`] is a set of [`Entity`] nodes keyed by a 128-bit [`EntityId`].
//! Each entity has a `kind` and a sorted map of named [`Value`] props; a
//! [`Value::Ref`] prop is an edge to another entity. The document changes only
//! through [`Op`]s passed to [`Document::apply`]; every op that succeeds is
//! appended to the [`Document::log`], and ops that fail leave both the state and
//! the log untouched.
//!
//! [`Document::content_hash`] is a stable 64-bit digest of the document *state*
//! (not its history): two documents holding the same entities, kinds and props
//! hash equal regardless of the order of the ops that produced them. Following
//! spec/schema.md, the provenance props [`PROVENANCE_PROPS`] (`_created_by`,
//! `_stamp`) are excluded from the hash.
//!
//! All hashing here is hand-written (FNV-1a plus a fixed finaliser), so ids and
//! hashes are identical across runs, platforms and Rust versions.
//!
//! Schema version 1 (spec/schema.md, [`Document::SCHEMA_VERSION`]) adds:
//!
//! * identity: [`EntityId::parse`] reads a 26-character Crockford-base32 ULID as-is
//!   and maps any other string through [`EntityId::derive`]; [`EntityId::new_ulid`]
//!   builds a ULID from its timestamp and randomness fields;
//! * values: [`Value::Bool`] and [`Value::List`];
//! * the dependency graph: every `Ref` (also inside Lists, at any depth) is an edge
//!   from the entity holding it to its target. [`Document::deps`],
//!   [`Document::dependents`] and [`Document::topo_order`] query it.
//!
//! The reserved props `_name` and `_schema` (spec/schema.md) are ordinary props to
//! this crate: front ends set them with [`Op::Set`] right after a create, so the
//! kernel's op log holds exactly the ops it was given.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Props excluded from [`Document::content_hash`] (spec/schema.md: "content_hash
/// covers kinds and props except the provenance props `_created_by` and `_stamp`").
pub const PROVENANCE_PROPS: [&str; 2] = ["_created_by", "_stamp"];

const FNV128_OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
const FNV128_PRIME: u128 = 0x0000000001000000000000000000013B;
const FNV64_OFFSET: u64 = 0xcbf29ce484222325;
const FNV64_PRIME: u64 = 0x100000001b3;

/// 128-bit FNV-1a over `bytes`.
fn fnv1a_128(bytes: &[u8]) -> u128 {
    let mut h = FNV128_OFFSET;
    for &b in bytes {
        h ^= u128::from(b);
        h = h.wrapping_mul(FNV128_PRIME);
    }
    h
}

/// SplitMix64-style finaliser: a bijection on `u64` with good avalanche.
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// Bijective 128-bit finaliser built from [`mix64`] on both halves plus
/// invertible cross-half xors, so it never introduces collisions.
fn mix128(x: u128) -> u128 {
    let mut hi = (x >> 64) as u64;
    let mut lo = x as u64;
    lo = mix64(lo ^ hi);
    hi = mix64(hi ^ lo);
    lo = mix64(lo ^ hi);
    (u128::from(hi) << 64) | u128::from(lo)
}

/// Crockford base-32 digits in value order (ULID text encoding, uppercase only).
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Length of a ULID's text form.
const ULID_LEN: usize = 26;

/// Mask of the 80 random bits of a ULID.
const ULID_RAND_MASK: u128 = (1u128 << 80) - 1;

/// A 128-bit entity identifier.
///
/// Ordering is numeric on the inner `u128`; documents iterate entities in this order.
/// For ULIDs ([`EntityId::new_ulid`]) that is time order, then randomness order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(pub u128);

impl EntityId {
    /// Deterministically derives an id from a seed string.
    ///
    /// The result depends only on the UTF-8 bytes of `seed` (128-bit FNV-1a
    /// followed by a bijective finaliser), so it is stable across runs and
    /// platforms. Distinct seeds give distinct ids except with negligible
    /// (~2^-128 per pair) probability.
    pub fn derive(seed: &str) -> EntityId {
        EntityId(mix128(fnv1a_128(seed.as_bytes())))
    }

    /// The id a string names (spec/schema.md, "Identity"). Infallible.
    ///
    /// A ULID — exactly 26 characters, each an uppercase Crockford base-32 digit
    /// (`0-9 A-Z` without `I L O U`), the first at most `7` so the value fits in 128
    /// bits — is used as-is: its 128-bit value. Any other string (lowercase, aliases,
    /// other lengths, punctuation, the empty string) is a *seed*: [`EntityId::derive`].
    pub fn parse(s: &str) -> EntityId {
        Self::parse_ulid(s).unwrap_or_else(|| EntityId::derive(s))
    }

    /// The value of `s` if it is a ULID in canonical text form, else `None`.
    pub fn parse_ulid(s: &str) -> Option<EntityId> {
        let bytes = s.as_bytes();
        if bytes.len() != ULID_LEN || bytes[0] > b'7' {
            return None;
        }
        let mut v: u128 = 0;
        for &c in bytes {
            let digit = CROCKFORD.iter().position(|&d| d == c)?;
            v = (v << 5) | digit as u128;
        }
        Some(EntityId(v))
    }

    /// The ULID with timestamp `ms` (milliseconds; only the low 48 bits are kept) and
    /// randomness `rand` (only the low 80 bits are kept): `(ms << 80) | rand`.
    /// Deterministic: equal inputs give equal ids.
    pub fn new_ulid(ms: u64, rand: u128) -> EntityId {
        EntityId(((ms as u128) << 80) | (rand & ULID_RAND_MASK))
    }

    /// This id as 26 Crockford base-32 digits, most significant first. Every id has
    /// one; [`EntityId::parse`] of it gives the id back.
    pub fn to_ulid_string(&self) -> String {
        let mut v = self.0;
        let mut out = [0u8; ULID_LEN];
        for slot in out.iter_mut().rev() {
            *slot = CROCKFORD[(v & 31) as usize];
            v >>= 5;
        }
        out.iter().map(|&b| b as char).collect()
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// A prop value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A number (SI units).
    Num(f64),
    /// A text string.
    Text(String),
    /// A reference to another entity (a dependency edge).
    Ref(EntityId),
    /// A boolean.
    Bool(bool),
    /// An ordered list of values; Refs inside it (at any depth) are dependency edges too.
    List(Vec<Value>),
}

impl Value {
    /// Calls `f` with every Ref target in this value: the value itself if it is a
    /// `Ref`, and every Ref nested in Lists at any depth, in document order.
    pub fn for_each_ref(&self, f: &mut impl FnMut(EntityId)) {
        // Iterative, so a deeply nested List cannot overflow the stack.
        let mut stack = vec![self];
        while let Some(v) = stack.pop() {
            match v {
                Value::Ref(id) => f(*id),
                Value::List(items) => stack.extend(items.iter().rev()),
                Value::Num(_) | Value::Text(_) | Value::Bool(_) => {}
            }
        }
    }

    /// Every Ref target in this value (see [`Value::for_each_ref`]), in document order,
    /// with repeats.
    pub fn refs(&self) -> Vec<EntityId> {
        let mut out = Vec::new();
        self.for_each_ref(&mut |id| out.push(id));
        out
    }
}

/// A node of the document graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    /// The entity's id.
    pub id: EntityId,
    /// The entity's kind, e.g. `"Wall"`.
    pub kind: String,
    /// Named props, sorted by key.
    pub props: BTreeMap<String, Value>,
}

/// An edit to a [`Document`].
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Create a new entity with no props. Fails if `id` already exists.
    Create {
        /// Id of the new entity.
        id: EntityId,
        /// Kind of the new entity.
        kind: String,
    },
    /// Insert or overwrite prop `key` of entity `id`. Fails if `id` does not exist.
    Set {
        /// Target entity.
        id: EntityId,
        /// Prop name.
        key: String,
        /// New value.
        value: Value,
    },
    /// Remove entity `id` and all its props. Fails if `id` does not exist.
    ///
    /// Refs from other entities to `id` are left in place and become dangling
    /// (see [`Document::dangling_refs`]).
    Delete {
        /// Entity to remove.
        id: EntityId,
    },
}

/// Why [`Document::apply`] rejected an op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocError {
    /// A `Set` or `Delete` targeted an id that is not in the document.
    NotFound(EntityId),
    /// A `Create` used an id that is already in the document.
    AlreadyExists(EntityId),
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocError::NotFound(id) => write!(f, "entity {id} not found"),
            DocError::AlreadyExists(id) => write!(f, "entity {id} already exists"),
        }
    }
}

impl std::error::Error for DocError {}

/// [`Document::topo_order`] found a dependency cycle among live entities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    /// One cycle: each entity depends on the next, and the last on the first. A
    /// self-reference is a one-entity cycle.
    pub path: Vec<EntityId>,
}

impl fmt::Display for Cycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dependency cycle: ")?;
        for id in &self.path {
            write!(f, "{id} -> ")?;
        }
        match self.path.first() {
            Some(first) => write!(f, "{first}"),
            None => f.write_str("(empty)"),
        }
    }
}

impl std::error::Error for Cycle {}

/// A semantic document: entities keyed by id, plus the log of applied ops.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    entities: BTreeMap<EntityId, Entity>,
    log: Vec<Op>,
}

impl Document {
    /// The schema version this crate implements (spec/schema.md); front ends store it
    /// in each entity's reserved `_schema` prop.
    pub const SCHEMA_VERSION: u32 = 1;

    /// An empty document with an empty log.
    pub fn new() -> Document {
        Document::default()
    }

    /// Applies `op`. On success the op is appended to the log; on error neither
    /// the state nor the log changes.
    pub fn apply(&mut self, op: Op) -> Result<(), DocError> {
        match &op {
            Op::Create { id, kind } => {
                if self.entities.contains_key(id) {
                    return Err(DocError::AlreadyExists(*id));
                }
                self.entities.insert(
                    *id,
                    Entity { id: *id, kind: kind.clone(), props: BTreeMap::new() },
                );
            }
            Op::Set { id, key, value } => {
                let entity = self.entities.get_mut(id).ok_or(DocError::NotFound(*id))?;
                entity.props.insert(key.clone(), value.clone());
            }
            Op::Delete { id } => {
                self.entities.remove(id).ok_or(DocError::NotFound(*id))?;
            }
        }
        self.log.push(op);
        Ok(())
    }

    /// The entity with this id, if present.
    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(&id)
    }

    /// Number of live entities.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// True when the document has no live entities (the log may be non-empty).
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Live entities in ascending id order.
    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }

    /// The ops applied successfully so far, in application order.
    pub fn log(&self) -> &[Op] {
        &self.log
    }

    /// A stable 64-bit digest of the document state.
    ///
    /// Covers every live entity's id, kind and props (keys, value types and
    /// values), except the provenance props in [`PROVENANCE_PROPS`]. Entities are
    /// visited in id order and props in key order, so the hash is independent of
    /// op order. The encoding is unambiguous: strings are length-prefixed, every
    /// value carries a type tag, and numbers are hashed by bit pattern with
    /// `-0.0` normalised to `0.0` and every NaN to one canonical NaN (so values
    /// that compare equal under [`PartialEq`] hash equal).
    pub fn content_hash(&self) -> u64 {
        let mut h = Hasher64::new();
        h.u64(self.entities.len() as u64);
        for e in self.entities.values() {
            h.u128(e.id.0);
            h.str(&e.kind);
            let props: Vec<(&String, &Value)> = e
                .props
                .iter()
                .filter(|(k, _)| !PROVENANCE_PROPS.contains(&k.as_str()))
                .collect();
            h.u64(props.len() as u64);
            for (k, v) in props {
                h.str(k);
                hash_value(&mut h, v);
            }
        }
        h.finish()
    }

    /// Props holding a Ref (directly, or nested in Lists at any depth) whose target
    /// is not a live entity, as `(source entity id, prop key)`, each prop once,
    /// sorted by id then key.
    pub fn dangling_refs(&self) -> Vec<(EntityId, String)> {
        let mut out = Vec::new();
        for e in self.entities.values() {
            for (k, v) in &e.props {
                let mut dangling = false;
                v.for_each_ref(&mut |target| dangling |= !self.entities.contains_key(&target));
                if dangling {
                    out.push((e.id, k.clone()));
                }
            }
        }
        out
    }

    /// The entities `id` depends on: every Ref target in its props, including Refs
    /// nested in Lists at any depth, sorted and deduplicated. Targets that are not
    /// live are included. Empty when `id` is not live.
    pub fn deps(&self, id: EntityId) -> Vec<EntityId> {
        self.dep_set(id).into_iter().collect()
    }

    /// The live entities that depend on `id` (hold a Ref to it, directly or inside a
    /// List), sorted and deduplicated. `id` need not be live.
    pub fn dependents(&self, id: EntityId) -> Vec<EntityId> {
        self.entities
            .values()
            .filter(|e| e.props.values().any(|v| {
                let mut hit = false;
                v.for_each_ref(&mut |t| hit |= t == id);
                hit
            }))
            .map(|e| e.id)
            .collect()
    }

    /// Every live entity exactly once, each after all of its live dependencies
    /// (edges to missing targets are ignored). Among entities that are ready at the
    /// same time the smallest id comes first, so the order is deterministic.
    ///
    /// Fails with a [`Cycle`] when the live dependency graph has one; a
    /// self-reference is a cycle.
    pub fn topo_order(&self) -> Result<Vec<EntityId>, Cycle> {
        // Live out-edges (entity -> its live deps) and the reverse adjacency.
        let mut live_deps: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
        let mut users: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
        let mut pending: BTreeMap<EntityId, usize> = BTreeMap::new();
        for &id in self.entities.keys() {
            let deps: Vec<EntityId> =
                self.dep_set(id).into_iter().filter(|t| self.entities.contains_key(t)).collect();
            for &t in &deps {
                users.entry(t).or_default().push(id);
            }
            pending.insert(id, deps.len());
            live_deps.insert(id, deps);
        }
        let mut ready: BTreeSet<EntityId> =
            pending.iter().filter(|(_, &n)| n == 0).map(|(&id, _)| id).collect();
        let mut order = Vec::with_capacity(self.entities.len());
        while let Some(id) = ready.pop_first() {
            order.push(id);
            for &user in users.get(&id).map(Vec::as_slice).unwrap_or(&[]) {
                let n = pending.get_mut(&user).expect("every user is live");
                *n -= 1;
                if *n == 0 {
                    ready.insert(user);
                }
            }
        }
        if order.len() == self.entities.len() {
            return Ok(order);
        }
        // Every unordered entity has an unordered live dep, so walking unordered deps
        // from any of them must revisit an entity: the walk from there is a cycle.
        let unordered = |id: &EntityId| pending[id] > 0;
        let start = *pending.keys().find(|id| unordered(id)).expect("some entity is unordered");
        let mut seen: BTreeMap<EntityId, usize> = BTreeMap::new();
        let mut walk = Vec::new();
        let mut at = start;
        while !seen.contains_key(&at) {
            seen.insert(at, walk.len());
            walk.push(at);
            at = *live_deps[&at].iter().find(|t| unordered(t)).expect("an unordered entity has an unordered dep");
        }
        Err(Cycle { path: walk.split_off(seen[&at]) })
    }

    /// The deps of `id` as a set (see [`Document::deps`]).
    fn dep_set(&self, id: EntityId) -> BTreeSet<EntityId> {
        let mut set = BTreeSet::new();
        if let Some(e) = self.entities.get(&id) {
            for v in e.props.values() {
                v.for_each_ref(&mut |t| {
                    set.insert(t);
                });
            }
        }
        set
    }
}

/// Feeds the unambiguous encoding of `v` to `h`: a type tag, then the payload.
/// Tags: 0 Num (canonical bits), 1 Text (length-prefixed), 2 Ref (128 bits),
/// 3 Bool (one byte), 4 List (element count, then each element's encoding).
fn hash_value(h: &mut Hasher64, v: &Value) {
    // Iterative, so a deeply nested List cannot overflow the stack.
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            Value::Num(x) => {
                h.byte(0);
                h.u64(canonical_f64_bits(*x));
            }
            Value::Text(s) => {
                h.byte(1);
                h.str(s);
            }
            Value::Ref(id) => {
                h.byte(2);
                h.u128(id.0);
            }
            Value::Bool(b) => {
                h.byte(3);
                h.byte(u8::from(*b));
            }
            Value::List(items) => {
                h.byte(4);
                h.u64(items.len() as u64);
                stack.extend(items.iter().rev());
            }
        }
    }
}

/// Bit pattern of `x` with `-0.0` mapped to `0.0` and all NaNs to one NaN.
fn canonical_f64_bits(x: f64) -> u64 {
    if x.is_nan() {
        f64::NAN.to_bits()
    } else if x == 0.0 {
        0
    } else {
        x.to_bits()
    }
}

/// Streaming 64-bit FNV-1a with a final avalanche mix.
struct Hasher64(u64);

impl Hasher64 {
    fn new() -> Self {
        Hasher64(FNV64_OFFSET)
    }
    fn byte(&mut self, b: u8) {
        self.0 ^= u64::from(b);
        self.0 = self.0.wrapping_mul(FNV64_PRIME);
    }
    fn bytes(&mut self, bs: &[u8]) {
        for &b in bs {
            self.byte(b);
        }
    }
    fn u64(&mut self, x: u64) {
        self.bytes(&x.to_le_bytes());
    }
    fn u128(&mut self, x: u128) {
        self.bytes(&x.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }
    fn finish(&self) -> u64 {
        mix64(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn id(s: &str) -> EntityId {
        EntityId::derive(s)
    }

    fn doc(ops: Vec<Op>) -> Document {
        let mut d = Document::new();
        for op in ops {
            d.apply(op).unwrap();
        }
        d
    }

    fn create(s: &str, kind: &str) -> Op {
        Op::Create { id: id(s), kind: kind.into() }
    }

    fn set(s: &str, key: &str, value: Value) -> Op {
        Op::Set { id: id(s), key: key.into(), value }
    }

    #[test]
    fn derive_is_stable_and_distinct() {
        assert_eq!(id("wall-1"), id("wall-1"));
        assert_ne!(id(""), id("a"));
        assert_ne!(id("ab"), id("ba"));
        let n = 100_000;
        let ids: HashSet<EntityId> = (0..n).map(|i| id(&format!("e{i}"))).collect();
        assert_eq!(ids.len(), n);
        // Pinned value: guards against accidental algorithm changes.
        assert_eq!(id("wall-1"), EntityId(PINNED_WALL_1));
    }

    const PINNED_WALL_1: u128 = 254873895639115463180665199976636620755;

    #[test]
    fn mix128_is_injective_on_samples() {
        let outs: HashSet<u128> = (0u128..10_000).map(|i| mix128(i << 64 | i)).collect();
        assert_eq!(outs.len(), 10_000);
    }

    #[test]
    fn errors_leave_state_and_log_untouched() {
        let mut d = doc(vec![create("a", "Wall"), set("a", "h", Value::Num(3.0))]);
        let before = d.clone();
        assert_eq!(d.apply(create("a", "Door")), Err(DocError::AlreadyExists(id("a"))));
        assert_eq!(d.apply(set("b", "h", Value::Num(1.0))), Err(DocError::NotFound(id("b"))));
        assert_eq!(d.apply(Op::Delete { id: id("b") }), Err(DocError::NotFound(id("b"))));
        assert_eq!(d, before);
        assert_eq!(d.get(id("a")).unwrap().kind, "Wall");
    }

    #[test]
    fn delete_then_recreate_starts_fresh() {
        let mut d = doc(vec![create("a", "Wall"), set("a", "h", Value::Num(3.0))]);
        d.apply(Op::Delete { id: id("a") }).unwrap();
        assert!(d.is_empty());
        d.apply(create("a", "Slab")).unwrap();
        let e = d.get(id("a")).unwrap();
        assert_eq!(e.kind, "Slab");
        assert!(e.props.is_empty());
        assert_eq!(d.log().len(), 4);
        assert_eq!(d.content_hash(), doc(vec![create("a", "Slab")]).content_hash());
    }

    #[test]
    fn hash_basics() {
        let empty = Document::new().content_hash();
        assert_eq!(empty, Document::new().content_hash());
        let one = doc(vec![create("a", "Wall")]);
        assert_ne!(empty, one.content_hash());
        // Same kind, different id.
        assert_ne!(one.content_hash(), doc(vec![create("b", "Wall")]).content_hash());
        // Created then deleted equals empty.
        let gone = doc(vec![create("a", "Wall"), Op::Delete { id: id("a") }]);
        assert_eq!(gone.content_hash(), empty);
        // Prop moved between entities changes the hash.
        let x = doc(vec![create("a", "W"), create("b", "W"), set("a", "k", Value::Num(1.0))]);
        let y = doc(vec![create("a", "W"), create("b", "W"), set("b", "k", Value::Num(1.0))]);
        assert_ne!(x.content_hash(), y.content_hash());
    }

    #[test]
    fn hash_encoding_is_unambiguous() {
        // Key/value boundary shifts must not collide.
        let a = doc(vec![create("e", "W"), set("e", "ab", Value::Text("c".into()))]);
        let b = doc(vec![create("e", "W"), set("e", "a", Value::Text("bc".into()))]);
        assert_ne!(a.content_hash(), b.content_hash());
        // Kind/key boundary.
        let a = doc(vec![create("e", "Wa"), set("e", "b", Value::Num(1.0))]);
        let b = doc(vec![create("e", "W"), set("e", "ab", Value::Num(1.0))]);
        assert_ne!(a.content_hash(), b.content_hash());
        // Value types are distinguished.
        let r = EntityId(1.0f64.to_bits() as u128);
        let n = doc(vec![create("e", "W"), set("e", "k", Value::Num(1.0))]);
        let rf = doc(vec![create("e", "W"), set("e", "k", Value::Ref(r))]);
        let t = doc(vec![create("e", "W"), set("e", "k", Value::Text("1".into()))]);
        assert_ne!(n.content_hash(), rf.content_hash());
        assert_ne!(n.content_hash(), t.content_hash());
        assert_ne!(t.content_hash(), rf.content_hash());
    }

    #[test]
    fn hash_normalises_zero_and_nan() {
        let p = doc(vec![create("e", "W"), set("e", "k", Value::Num(0.0))]);
        let m = doc(vec![create("e", "W"), set("e", "k", Value::Num(-0.0))]);
        assert_eq!(p.content_hash(), m.content_hash());
        let n1 = doc(vec![create("e", "W"), set("e", "k", Value::Num(f64::NAN))]);
        let n2 = doc(vec![create("e", "W"), set("e", "k", Value::Num(-f64::NAN))]);
        assert_eq!(n1.content_hash(), n2.content_hash());
        assert_ne!(n1.content_hash(), p.content_hash());
    }

    #[test]
    fn hash_ignores_provenance_props_only() {
        let base = doc(vec![create("e", "W"), set("e", "h", Value::Num(3.0))]);
        let prov = doc(vec![
            create("e", "W"),
            set("e", "_created_by", Value::Num(7.0)),
            set("e", "h", Value::Num(3.0)),
            set("e", "_stamp", Value::Num(42.0)),
        ]);
        assert_eq!(base.content_hash(), prov.content_hash());
        let named = doc(vec![
            create("e", "W"),
            set("e", "h", Value::Num(3.0)),
            set("e", "_name", Value::Text("e".into())),
        ]);
        assert_ne!(base.content_hash(), named.content_hash());
    }

    #[test]
    fn dangling_refs_sorted_and_self_ref_ok() {
        let mut d = doc(vec![
            create("a", "W"),
            create("b", "W"),
            set("a", "z", Value::Ref(id("gone"))),
            set("a", "m", Value::Ref(id("gone2"))),
            set("b", "x", Value::Ref(id("gone"))),
            set("b", "self", Value::Ref(id("b"))),
            set("b", "t", Value::Text("gone".into())),
        ]);
        let mut expected = vec![
            (id("a"), "m".to_string()),
            (id("a"), "z".to_string()),
            (id("b"), "x".to_string()),
        ];
        expected.sort();
        assert_eq!(d.dangling_refs(), expected);
        d.apply(set("a", "z", Value::Num(0.0))).unwrap();
        assert_eq!(d.dangling_refs().len(), 2);
    }

    #[test]
    fn error_display() {
        let e = DocError::NotFound(EntityId(0xab));
        assert!(e.to_string().contains("000000000000000000000000000000ab"));
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn ulids_parse_and_print() {
        let u = EntityId::new_ulid(1_469_918_176_385, 0x1234_5678_9abc_def0_1234);
        assert_eq!(EntityId::parse(&u.to_ulid_string()), u);
        assert_eq!(EntityId::parse_ulid("01ARYZ6S41TSV4RRFFQ69G5FAV").map(|e| (e.0 >> 80) as u64), Some(1_469_918_176_385));
        assert_eq!(EntityId::new_ulid(1, u128::MAX), EntityId((1u128 << 80) | ((1u128 << 80) - 1)));
        for s in ["01aryz6s41tsv4rrffq69g5fav", "01ARYZ6S41TSV4RRFFQ69G5FAI", "81ARYZ6S41TSV4RRFFQ69G5FAV", "é1ARYZ6S41TSV4RRFFQ69G5FA"] {
            assert_eq!(EntityId::parse(s), EntityId::derive(s), "{s}");
        }
    }

    #[test]
    fn bool_and_list_hash_distinctly_and_deps_see_nested_refs() {
        let a = doc(vec![create("e", "W"), set("e", "k", Value::List(vec![Value::Bool(true)]))]);
        let b = doc(vec![create("e", "W"), set("e", "k", Value::Bool(true))]);
        let c = doc(vec![create("e", "W"), set("e", "k", Value::List(vec![]))]);
        assert_ne!(a.content_hash(), b.content_hash());
        assert_ne!(a.content_hash(), c.content_hash());
        let nested = Value::List(vec![Value::List(vec![Value::Ref(id("x")), Value::Ref(id("y"))]), Value::Ref(id("x"))]);
        let d = doc(vec![create("e", "W"), create("x", "W"), set("e", "k", nested)]);
        let mut want = vec![id("x"), id("y")];
        want.sort();
        assert_eq!(d.deps(id("e")), want);
        assert_eq!(d.dependents(id("y")), vec![id("e")]);
        assert_eq!(d.dangling_refs(), vec![(id("e"), "k".to_string())]);
        assert_eq!(d.topo_order(), Ok(vec![id("x"), id("e")]));
    }

    #[test]
    fn cycles_are_reported_with_a_path() {
        let d = doc(vec![
            create("a", "W"),
            create("b", "W"),
            create("c", "W"),
            set("a", "r", Value::Ref(id("b"))),
            set("b", "r", Value::List(vec![Value::Ref(id("a"))])),
            set("c", "r", Value::Ref(id("a"))),
        ]);
        let cycle = d.topo_order().unwrap_err();
        let mut members = cycle.path.clone();
        members.sort();
        let mut want = vec![id("a"), id("b")];
        want.sort();
        assert_eq!(members, want);
        assert!(cycle.to_string().starts_with("dependency cycle: "));
        let selfie = doc(vec![create("s", "W"), set("s", "me", Value::Ref(id("s")))]);
        assert_eq!(selfie.topo_order(), Err(Cycle { path: vec![id("s")] }));
        assert_eq!(Document::SCHEMA_VERSION, 1);
    }
}
