//! `bim-incr`: demand-driven incremental computation in the style of salsa / rustc's query system.
//!
//! A [`Db`] holds **inputs** (set from outside, each change starting a new *revision*) and
//! memoizes **derived queries** (pure functions of inputs and other queries). While a query
//! executes, every input and query it reads is recorded as a dependency. When a query is demanded
//! again at a later revision it is **validated red-green**: its recorded dependencies are visited
//! in order, derived ones are validated (and, if need be, re-executed) first, and only when one of
//! them changed after the query was last verified is the query itself re-executed. A re-executed
//! query whose new value equals the old one keeps its old "changed at" revision (**early cutoff**),
//! so nothing that depends on it is dirtied. Setting an input to a value equal to its current one
//! changes nothing at all.
//!
//! Queries and inputs are ordinary types implementing [`Query`] or [`Input`]; keys and values are
//! stored type-erased in one node table, values are returned behind [`Rc`]. The database keeps a
//! log of every query execution ([`Db::take_log`]) so a caller can report exactly what was
//! recomputed.
//!
//! The database is single-threaded (interior mutability through `RefCell`); a query that demands
//! itself, directly or through other queries, panics naming the query.

#![forbid(unsafe_code)]

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::rc::Rc;

/// An input of the database: a family of cells keyed by `Key`, each holding a `Value` or nothing.
pub trait Input: 'static {
    /// Which cell of the input family.
    type Key: Clone + Eq + Hash + 'static;
    /// The cell's value; a set to an equal value does not start a new revision.
    type Value: PartialEq + 'static;
}

/// A derived query: a pure function of the inputs and queries it reads through the [`Db`].
pub trait Query: 'static {
    /// Which instance of the query family.
    type Key: Clone + Eq + Hash + 'static;
    /// The result; compared with the previous one for early cutoff.
    type Value: PartialEq + 'static;
    /// Computes the value, reading inputs with [`Db::input`] and other queries with [`Db::get`]
    /// (every read is recorded as a dependency). Must not set inputs.
    fn execute(db: &Db, key: &Self::Key) -> Self::Value;
}

/// A revision counter: every effective input change increments it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Revision(pub u64);

/// One query execution, from the database's log.
#[derive(Clone)]
pub struct Executed {
    query: TypeId,
    name: &'static str,
    key: Rc<dyn Any>,
}

impl Executed {
    /// Whether this execution was of query `Q`.
    pub fn is<Q: Query>(&self) -> bool {
        self.query == TypeId::of::<Q>()
    }

    /// The key of the execution if it was of query `Q`.
    pub fn key<Q: Query>(&self) -> Option<&Q::Key> {
        if self.is::<Q>() {
            self.key.downcast_ref::<Q::Key>()
        } else {
            None
        }
    }

    /// The query's type name (for diagnostics).
    pub fn query_name(&self) -> &'static str {
        self.name
    }
}

impl std::fmt::Debug for Executed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Executed({})", self.name)
    }
}

type NodeId = usize;

/// Re-validates a derived node without knowing its query type at the call site.
trait DynQuery {
    fn ensure(&self, db: &Db);
    fn executed(&self) -> Executed;
    fn name(&self) -> &'static str;
}

struct Handle<Q: Query> {
    key: Q::Key,
}

impl<Q: Query> DynQuery for Handle<Q> {
    fn ensure(&self, db: &Db) {
        db.ensure::<Q>(&self.key);
    }
    fn executed(&self) -> Executed {
        Executed { query: TypeId::of::<Q>(), name: std::any::type_name::<Q>(), key: Rc::new(self.key.clone()) }
    }
    fn name(&self) -> &'static str {
        std::any::type_name::<Q>()
    }
}

enum Kind {
    Input,
    Derived(Rc<dyn DynQuery>),
}

struct Node {
    kind: Kind,
    /// The revision at which the value last changed (0: never had a value).
    changed_at: u64,
    /// The revision at which the value was last known to be up to date.
    verified_at: u64,
    /// Recorded dependencies of the last execution, in read order (derived nodes only).
    deps: Vec<NodeId>,
    /// The value (`None` for an input that was never set or was set to nothing, and for a
    /// derived node that never executed).
    value: Option<Rc<dyn Any>>,
    executing: bool,
}

#[derive(Default)]
struct Frame {
    deps: Vec<NodeId>,
    seen: HashSet<NodeId>,
}

#[derive(Default)]
struct Inner {
    revision: u64,
    /// Per input or query type: its key → node table (`HashMap<K, NodeId>`).
    tables: HashMap<TypeId, Box<dyn Any>>,
    nodes: Vec<Node>,
    stack: Vec<Frame>,
    log: Vec<Executed>,
}

/// The incremental database: inputs, memoized queries, the dependency graph and the execution log.
#[derive(Default)]
pub struct Db {
    inner: RefCell<Inner>,
}

impl Db {
    /// An empty database at revision 0.
    pub fn new() -> Db {
        Db::default()
    }

    /// The current revision.
    pub fn revision(&self) -> Revision {
        Revision(self.inner.borrow().revision)
    }

    /// The node of `key` in the table of type `T` (an input or a query type), created with
    /// `make` when absent.
    fn node_of<T: 'static, K: Clone + Eq + Hash + 'static>(&self, key: &K, make: impl FnOnce() -> Kind) -> NodeId {
        let mut inner = self.inner.borrow_mut();
        let table = inner
            .tables
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(HashMap::<K, NodeId>::new()))
            .downcast_mut::<HashMap<K, NodeId>>()
            .expect("one key type per table");
        if let Some(&id) = table.get(key) {
            return id;
        }
        let id = inner.nodes.len();
        let table = inner
            .tables
            .get_mut(&TypeId::of::<T>())
            .and_then(|t| t.downcast_mut::<HashMap<K, NodeId>>())
            .expect("table exists");
        table.insert(key.clone(), id);
        inner.nodes.push(Node { kind: make(), changed_at: 0, verified_at: 0, deps: Vec::new(), value: None, executing: false });
        id
    }

    /// Sets input cell `key` to `value` (`None` clears it). Starts a new revision only when the
    /// value differs from the current one.
    pub fn set<I: Input>(&self, key: &I::Key, value: Option<I::Value>) {
        let id = self.node_of::<I, I::Key>(key, || Kind::Input);
        let mut inner = self.inner.borrow_mut();
        let same = match (&inner.nodes[id].value, &value) {
            (None, None) => true,
            (Some(old), Some(new)) => old.downcast_ref::<I::Value>().map_or(false, |old| old == new),
            _ => false,
        };
        if same {
            return;
        }
        inner.revision += 1;
        let rev = inner.revision;
        let node = &mut inner.nodes[id];
        node.value = value.map(|v| Rc::new(v) as Rc<dyn Any>);
        node.changed_at = rev;
        node.verified_at = rev;
    }

    /// Reads input cell `key`, recording the dependency of the executing query (if any). `None`
    /// when the cell was never set or was cleared.
    pub fn input<I: Input>(&self, key: &I::Key) -> Option<Rc<I::Value>> {
        let id = self.node_of::<I, I::Key>(key, || Kind::Input);
        let mut inner = self.inner.borrow_mut();
        let value = inner.nodes[id].value.clone().and_then(|v| Rc::downcast::<I::Value>(v).ok());
        record(&mut inner, id);
        value
    }

    /// The value of query `Q` at `key`, memoized; executes it only when it has never run or one
    /// of its dependencies changed since. Records the dependency of the executing query (if any).
    pub fn get<Q: Query>(&self, key: &Q::Key) -> Rc<Q::Value> {
        let (id, value) = self.ensure::<Q>(key);
        record(&mut self.inner.borrow_mut(), id);
        value
    }

    /// Brings the node of `Q`/`key` up to date without recording a dependency on it.
    fn ensure<Q: Query>(&self, key: &Q::Key) -> (NodeId, Rc<Q::Value>) {
        let id = self.node_of::<Q, Q::Key>(key, || Kind::Derived(Rc::new(Handle::<Q> { key: key.clone() })));
        let (fresh, has_value, verified_at, deps) = {
            let inner = self.inner.borrow();
            let node = &inner.nodes[id];
            if node.executing {
                panic!("bim-incr: query cycle: {} demands itself", std::any::type_name::<Q>());
            }
            (node.verified_at == inner.revision && node.value.is_some(), node.value.is_some(), node.verified_at, node.deps.clone())
        };
        if !fresh {
            let mut dirty = !has_value;
            if !dirty {
                for d in deps {
                    self.ensure_node(d);
                    if self.inner.borrow().nodes[d].changed_at > verified_at {
                        dirty = true;
                        break;
                    }
                }
            }
            if dirty {
                self.execute::<Q>(id, key);
            } else {
                let mut inner = self.inner.borrow_mut();
                let rev = inner.revision;
                inner.nodes[id].verified_at = rev;
            }
        }
        let value = self.inner.borrow().nodes[id].value.clone().expect("executed query has a value");
        (id, Rc::downcast::<Q::Value>(value).expect("one value type per query"))
    }

    /// Brings any node up to date (inputs are always up to date).
    fn ensure_node(&self, id: NodeId) {
        let handle = match &self.inner.borrow().nodes[id].kind {
            Kind::Input => return,
            Kind::Derived(h) => Rc::clone(h),
        };
        handle.ensure(self);
    }

    fn execute<Q: Query>(&self, id: NodeId, key: &Q::Key) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.nodes[id].executing = true;
            inner.stack.push(Frame::default());
        }
        let value = Q::execute(self, key);
        let mut inner = self.inner.borrow_mut();
        let frame = inner.stack.pop().expect("frame pushed before execution");
        let rev = inner.revision;
        let node = &mut inner.nodes[id];
        node.executing = false;
        let unchanged = node.value.as_ref().and_then(|old| old.downcast_ref::<Q::Value>()).map_or(false, |old| *old == value);
        if !unchanged {
            node.changed_at = rev;
        }
        node.verified_at = rev;
        node.value = Some(Rc::new(value));
        node.deps = frame.deps;
        let executed = match &node.kind {
            Kind::Derived(h) => h.executed(),
            Kind::Input => unreachable!("inputs are not executed"),
        };
        inner.log.push(executed);
    }

    /// Takes the log of every query execution since the last call, in execution order.
    pub fn take_log(&self) -> Vec<Executed> {
        std::mem::take(&mut self.inner.borrow_mut().log)
    }

    /// Whether query `Q` at `key` holds a memoized value (fresh or not).
    pub fn is_memoized<Q: Query>(&self, key: &Q::Key) -> bool {
        let inner = self.inner.borrow();
        inner
            .tables
            .get(&TypeId::of::<Q>())
            .and_then(|t| t.downcast_ref::<HashMap<Q::Key, NodeId>>())
            .and_then(|t| t.get(key))
            .map_or(false, |&id| inner.nodes[id].value.is_some())
    }

    /// The names of the queries currently executing, outermost first (for diagnostics).
    pub fn executing(&self) -> Vec<&'static str> {
        let inner = self.inner.borrow();
        inner
            .nodes
            .iter()
            .filter(|n| n.executing)
            .filter_map(|n| match &n.kind {
                Kind::Derived(h) => Some(h.name()),
                Kind::Input => None,
            })
            .collect()
    }
}

/// Records `id` as a dependency of the innermost executing query.
fn record(inner: &mut Inner, id: NodeId) {
    if let Some(frame) = inner.stack.last_mut() {
        if frame.seen.insert(id) {
            frame.deps.push(id);
        }
    }
}

#[cfg(test)]
mod tests;
