//! The unit of replication: one stamped edit.
//!
//! A [`SyncOp`] is a `bim_doc::Op` that names its entity by the string it was written with (a
//! seed or a ULID, as the journal does), so a project journal and the replicated log convert into
//! each other without losing the names the user chose. [`Stamp`] is `bim-crdt`'s Lamport stamp, so
//! there is one definition of the ordering that decides every conflict.

use bim_doc::{EntityId, Op, Value};

pub use bim_crdt::Stamp;

/// Deepest List nesting a value may have (a List directly inside a prop is level 1). It matches the
/// nesting `bim-cli`'s journal parser accepts, so every value a journal can hold can be replicated.
pub const MAX_VALUE_DEPTH: usize = 128;

/// Largest Lamport counter: the store keeps counters as signed 64-bit integers.
pub const MAX_COUNTER: u64 = i64::MAX as u64;

/// A prop value. A `Ref` keeps its target's name as written.
#[derive(Debug, Clone)]
pub enum SyncValue {
    /// A finite number; compared and stored by bit pattern (`0.0` and `-0.0` are different edits).
    Num(f64),
    /// A text.
    Text(String),
    /// A reference to the entity this name stands for.
    Ref(String),
    /// A boolean.
    Bool(bool),
    /// An ordered list.
    List(Vec<SyncValue>),
}

impl PartialEq for SyncValue {
    /// Bitwise for numbers, so equality is exactly "encodes to the same bytes".
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (SyncValue::Num(a), SyncValue::Num(b)) => a.to_bits() == b.to_bits(),
            (SyncValue::Text(a), SyncValue::Text(b)) | (SyncValue::Ref(a), SyncValue::Ref(b)) => a == b,
            (SyncValue::Bool(a), SyncValue::Bool(b)) => a == b,
            (SyncValue::List(a), SyncValue::List(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for SyncValue {}

impl SyncValue {
    /// The document value: a Ref's name resolved to the id it stands for.
    pub fn to_value(&self) -> Value {
        match self {
            SyncValue::Num(x) => Value::Num(*x),
            SyncValue::Text(s) => Value::Text(s.clone()),
            SyncValue::Ref(name) => Value::Ref(EntityId::parse(name)),
            SyncValue::Bool(b) => Value::Bool(*b),
            SyncValue::List(items) => Value::List(items.iter().map(SyncValue::to_value).collect()),
        }
    }

    /// Checks the value can be carried: numbers finite, Lists nested at most [`MAX_VALUE_DEPTH`].
    pub fn check(&self) -> Result<(), String> {
        self.check_at(0)
    }

    fn check_at(&self, depth: usize) -> Result<(), String> {
        match self {
            SyncValue::Num(x) if !x.is_finite() => Err(format!("number {x} is not finite")),
            SyncValue::Num(_) | SyncValue::Text(_) | SyncValue::Ref(_) | SyncValue::Bool(_) => Ok(()),
            SyncValue::List(items) => {
                if depth >= MAX_VALUE_DEPTH {
                    return Err(format!("lists nested deeper than {MAX_VALUE_DEPTH}"));
                }
                items.iter().try_for_each(|item| item.check_at(depth + 1))
            }
        }
    }

    /// Calls `f` with every Ref name in the value, at any depth, in order.
    pub fn for_each_ref<'a>(&'a self, f: &mut impl FnMut(&'a str)) {
        match self {
            SyncValue::Ref(name) => f(name),
            SyncValue::List(items) => items.iter().for_each(|item| item.for_each_ref(f)),
            SyncValue::Num(_) | SyncValue::Text(_) | SyncValue::Bool(_) => {}
        }
    }
}

/// One edit of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp {
    /// Creates (or re-creates) the entity. Carries no props: `bim` follows a create with the
    /// `_name` and `_schema` sets, which are ops of their own.
    Create {
        /// The entity, as written.
        name: String,
        /// Its kind; never empty.
        kind: String,
    },
    /// Sets one prop.
    Set {
        /// The entity, as written.
        name: String,
        /// The prop.
        key: String,
        /// Its new value.
        value: SyncValue,
    },
    /// Deletes the entity.
    Delete {
        /// The entity, as written.
        name: String,
    },
}

impl SyncOp {
    /// The name of the entity this op acts on, as written.
    pub fn name(&self) -> &str {
        match self {
            SyncOp::Create { name, .. } | SyncOp::Set { name, .. } | SyncOp::Delete { name } => name,
        }
    }

    /// The id of the entity this op acts on.
    pub fn id(&self) -> EntityId {
        EntityId::parse(self.name())
    }

    /// The document op: the same edit with names resolved to ids.
    pub fn to_doc_op(&self) -> Op {
        let id = self.id();
        match self {
            SyncOp::Create { kind, .. } => Op::Create { id, kind: kind.clone() },
            SyncOp::Set { key, value, .. } => Op::Set { id, key: key.clone(), value: value.to_value() },
            SyncOp::Delete { .. } => Op::Delete { id },
        }
    }

    /// Calls `f` with every entity name the op mentions: its target, then each Ref in its value.
    pub fn for_each_name<'a>(&'a self, f: &mut impl FnMut(&'a str)) {
        f(self.name());
        if let SyncOp::Set { value, .. } = self {
            value.for_each_ref(f);
        }
    }

    /// Checks the op can be carried and journalled: a create has a kind, numbers are finite,
    /// Lists are not nested too deep.
    pub fn check(&self) -> Result<(), String> {
        match self {
            SyncOp::Create { kind, .. } if kind.is_empty() => Err("a create needs a kind".to_string()),
            SyncOp::Set { value, .. } => value.check(),
            _ => Ok(()),
        }
    }
}

/// An edit and the stamp that orders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StampedOp {
    /// The edit's Lamport stamp; unique per edit.
    pub stamp: Stamp,
    /// The edit.
    pub op: SyncOp,
}

impl StampedOp {
    /// Checks the stamp (counter from 1 to [`MAX_COUNTER`]) and the op.
    pub fn check(&self) -> Result<(), String> {
        if self.stamp.counter == 0 || self.stamp.counter > MAX_COUNTER {
            return Err(format!("counter {} is out of range", self.stamp.counter));
        }
        self.op.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_is_bitwise_for_numbers() {
        assert_ne!(SyncValue::Num(0.0), SyncValue::Num(-0.0));
        assert_eq!(SyncValue::Num(1.5), SyncValue::Num(1.5));
        assert_ne!(SyncValue::Text("a".into()), SyncValue::Ref("a".into()));
        assert_eq!(
            SyncValue::List(vec![SyncValue::Num(-0.0)]),
            SyncValue::List(vec![SyncValue::Num(-0.0)])
        );
    }

    #[test]
    fn check_rejects_what_cannot_be_journalled() {
        assert!(SyncValue::Num(f64::NAN).check().is_err());
        assert!(SyncValue::Num(f64::INFINITY).check().is_err());
        assert!(SyncOp::Create { name: "a".into(), kind: String::new() }.check().is_err());
        let mut deep = SyncValue::Num(1.0);
        for _ in 0..MAX_VALUE_DEPTH {
            deep = SyncValue::List(vec![deep]);
        }
        assert!(deep.check().is_ok(), "exactly the maximum depth");
        assert!(SyncValue::List(vec![deep]).check().is_err(), "one level more");
        let nan_inside = SyncValue::List(vec![SyncValue::List(vec![SyncValue::Num(f64::NAN)])]);
        assert!(nan_inside.check().is_err());
        let good = StampedOp { stamp: Stamp { counter: 1, peer: 0 }, op: SyncOp::Delete { name: "a".into() } };
        assert!(good.check().is_ok());
        let zero = StampedOp { stamp: Stamp { counter: 0, peer: 0 }, ..good.clone() };
        assert!(zero.check().is_err());
        let huge = StampedOp { stamp: Stamp { counter: MAX_COUNTER + 1, peer: 0 }, ..good };
        assert!(huge.check().is_err());
    }

    #[test]
    fn names_resolve_like_the_journal_does() {
        let ulid = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        assert_eq!(SyncOp::Delete { name: ulid.into() }.id(), EntityId::parse(ulid));
        assert_eq!(SyncOp::Delete { name: "wall".into() }.id(), EntityId::derive("wall"));
        let op = SyncOp::Set {
            name: "w".into(),
            key: "k".into(),
            value: SyncValue::List(vec![SyncValue::Ref("a".into()), SyncValue::List(vec![SyncValue::Ref("b".into())])]),
        };
        let mut names = Vec::new();
        op.for_each_name(&mut |n| names.push(n.to_string()));
        assert_eq!(names, ["w", "a", "b"]);
        assert_eq!(
            op.to_doc_op(),
            Op::Set {
                id: EntityId::derive("w"),
                key: "k".into(),
                value: Value::List(vec![
                    Value::Ref(EntityId::derive("a")),
                    Value::List(vec![Value::Ref(EntityId::derive("b"))])
                ]),
            }
        );
    }
}
