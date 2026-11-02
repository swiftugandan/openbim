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

use std::collections::BTreeMap;
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

/// A 128-bit entity identifier.
///
/// Ordering is numeric on the inner `u128`; documents iterate entities in this order.
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

/// A semantic document: entities keyed by id, plus the log of applied ops.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    entities: BTreeMap<EntityId, Entity>,
    log: Vec<Op>,
}

impl Document {
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
                }
            }
        }
        h.finish()
    }

    /// Props whose [`Value::Ref`] target is not a live entity, as
    /// `(source entity id, prop key)`, sorted by id then key.
    pub fn dangling_refs(&self) -> Vec<(EntityId, String)> {
        let mut out = Vec::new();
        for e in self.entities.values() {
            for (k, v) in &e.props {
                if let Value::Ref(target) = v {
                    if !self.entities.contains_key(target) {
                        out.push((e.id, k.clone()));
                    }
                }
            }
        }
        out
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
}
