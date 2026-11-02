//! Semantic evaluation of a merged replica (see [`Replica::evaluate`]).
//!
//! The syntactic merge (T1.2) never fails and never consults meaning. This
//! layer runs on top of a materialized [`Document`] and reports the conflicts
//! that only show up once concurrent edits are combined:
//!
//! - `dangling_ref`: a visible top-level `Ref` prop whose target is not live
//!   (exactly the set of [`Replica::diagnostics`]);
//! - `dependency_cycle`: an entity lying on a cycle of top-level `Ref` edges
//!   between live entities (a self-reference is a cycle);
//! - `constraint_unsatisfiable`: a `Constraint` in a connected group of
//!   constraints that the 2D solver reports as [`Status::Inconsistent`].
//!
//! Each diagnostic is materialized as a `ConflictDiagnostic` entity whose id is
//! `EntityId::derive("conflict:" + entity + key + target)` (ids in their
//! 32-hex-digit display form, `target` omitted when there is none), so every
//! replica that has merged the same state produces the same nodes.

use std::collections::{BTreeMap, BTreeSet};

use bim_doc::{Document, EntityId, Op, Value};
use bim_solver::{Constraint as SolverConstraint, Sketch, Status};

use crate::Replica;

/// Kind of the materialized diagnostic entities.
pub const CONFLICT_DIAGNOSTIC_KIND: &str = "ConflictDiagnostic";

/// Prop key recorded on `constraint_unsatisfiable` diagnostics: the
/// constraint's `kind` is what cannot hold together with its group.
pub const UNSATISFIABLE_KEY: &str = "kind";

/// Coordinates and constraint values whose magnitude exceeds this are treated
/// as malformed (the constraint is skipped), so solver arithmetic on them can
/// never overflow to infinities or NaN.
pub const MAX_MAGNITUDE: f64 = 1e100;

/// Why a [`Diagnostic`] was raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// A top-level `Ref` prop whose target is not live.
    DanglingRef,
    /// A constraint in a group the solver found inconsistent.
    ConstraintUnsatisfiable,
    /// An entity on a cycle of `Ref` edges between live entities.
    DependencyCycle,
}

impl Reason {
    /// The `reason` Text stored on the materialized node.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::DanglingRef => "dangling_ref",
            Reason::ConstraintUnsatisfiable => "constraint_unsatisfiable",
            Reason::DependencyCycle => "dependency_cycle",
        }
    }
}

/// One semantic conflict, materialized as a `ConflictDiagnostic` node with id
/// [`Diagnostic::id`] in [`Evaluated::document`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    /// Id of the materialized `ConflictDiagnostic` entity.
    pub id: EntityId,
    /// The entity the conflict is about.
    pub entity: EntityId,
    /// The prop key involved (`dangling_ref`: the dangling prop;
    /// `dependency_cycle`: the prop whose Ref continues the cycle;
    /// `constraint_unsatisfiable`: [`UNSATISFIABLE_KEY`]).
    pub key: String,
    /// The referenced entity, when there is one (`dangling_ref`: the missing
    /// target; `dependency_cycle`: the next entity on the cycle).
    pub target: Option<EntityId>,
    /// Why the diagnostic was raised.
    pub reason: Reason,
}

impl Diagnostic {
    /// The id derived from `"conflict:" + entity + key + target`.
    pub fn derive_id(entity: EntityId, key: &str, target: Option<EntityId>) -> EntityId {
        EntityId::derive(&seed(entity, key, target))
    }
}

fn seed(entity: EntityId, key: &str, target: Option<EntityId>) -> String {
    match target {
        Some(t) => format!("conflict:{entity}{key}{t}"),
        None => format!("conflict:{entity}{key}"),
    }
}

/// Result of [`Replica::evaluate`].
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated {
    /// The merged document with solved Point coordinates and one
    /// `ConflictDiagnostic` entity per entry of `diagnostics`.
    pub document: Document,
    /// The materialized diagnostics, sorted by node id.
    pub diagnostics: Vec<Diagnostic>,
}

impl Replica {
    /// Semantic evaluation of the merged state. Pure: the replica is not
    /// changed and [`Replica::document`] never contains diagnostic nodes.
    ///
    /// The result depends only on [`Replica::document`] (not on the peer, the
    /// clock or the merge order), so replicas that have seen the same edits
    /// produce identical results. Never panics.
    ///
    /// Constraint evaluation: a `Constraint` is solvable when it is live, its
    /// `kind` is Text `distance` (with a finite Num `value`) or `coincident`
    /// (any `value` ignored), and its top-level `a`/`b` Refs name live `Point`
    /// entities with finite Num `x`/`y`. Anything else is skipped (dangling
    /// refs are still reported as `dangling_ref`). Solvable constraints are
    /// grouped into connected components over shared Points; each group is
    /// solved with [`bim_solver::Sketch`] from the merged coordinates. A group
    /// reported [`Status::Inconsistent`] flags every constraint in it with
    /// `constraint_unsatisfiable` and leaves its Points untouched; any other
    /// group writes its solved `x`/`y` back. Other Points keep their merged
    /// coordinates.
    pub fn evaluate(&self) -> Evaluated {
        let mut document = self.document();
        let mut found: Vec<(EntityId, String, Option<EntityId>, Reason)> = Vec::new();

        for d in self.diagnostics() {
            found.push((d.entity, d.key, Some(d.target), Reason::DanglingRef));
        }
        for (e, key, t) in cycle_members(&document) {
            found.push((e, key, Some(t), Reason::DependencyCycle));
        }
        let (solved, unsatisfiable) = solve_constraints(&document);
        for c in unsatisfiable {
            found.push((c, UNSATISFIABLE_KEY.to_string(), None, Reason::ConstraintUnsatisfiable));
        }

        // Geometry first (only Points, which are never diagnostic nodes).
        for (p, (x, y)) in solved {
            let _ = document.apply(Op::Set { id: p, key: "x".into(), value: Value::Num(x) });
            let _ = document.apply(Op::Set { id: p, key: "y".into(), value: Value::Num(y) });
        }

        // One node per distinct derived id; the first (in the deterministic
        // order above) wins should two diagnostics ever derive the same id.
        let mut by_id: BTreeMap<EntityId, Diagnostic> = BTreeMap::new();
        for (entity, key, target, reason) in found {
            let id = Diagnostic::derive_id(entity, &key, target);
            by_id.entry(id).or_insert(Diagnostic { id, entity, key, target, reason });
        }

        let mut diagnostics = Vec::with_capacity(by_id.len());
        for (id, d) in by_id {
            // An authored entity that happens to hold this id keeps it; the
            // diagnostic is then not materialized (and not reported).
            if document.apply(Op::Create { id, kind: CONFLICT_DIAGNOSTIC_KIND.into() }).is_err() {
                continue;
            }
            let mut props: Vec<(&str, Value)> = vec![
                ("_name", Value::Text(seed(d.entity, &d.key, d.target))),
                ("entity", Value::Ref(d.entity)),
                ("key", Value::Text(d.key.clone())),
                ("reason", Value::Text(d.reason.as_str().into())),
            ];
            if let Some(t) = d.target {
                props.push(("target", Value::Ref(t)));
            }
            for (k, v) in props {
                let _ = document.apply(Op::Set { id, key: k.into(), value: v });
            }
            diagnostics.push(d);
        }
        Evaluated { document, diagnostics }
    }
}

/// Top-level Ref edges `(key, target)` of `e` whose target is live, in key order.
fn live_ref_edges(doc: &Document, e: EntityId) -> Vec<(String, EntityId)> {
    match doc.get(e) {
        Some(ent) => ent
            .props
            .iter()
            .filter_map(|(k, v)| match v {
                Value::Ref(t) if doc.get(*t).is_some() => Some((k.clone(), *t)),
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    }
}

/// Every live entity on a cycle of top-level Ref edges between live entities,
/// with the first prop (in key order) whose Ref target lies on the same cycle
/// (the same strongly connected component). Iterative Tarjan, so deep graphs
/// cannot overflow the stack.
fn cycle_members(doc: &Document) -> Vec<(EntityId, String, EntityId)> {
    let ids: Vec<EntityId> = doc.entities().map(|e| e.id).collect();
    let index_of: BTreeMap<EntityId, usize> = ids.iter().enumerate().map(|(i, e)| (*e, i)).collect();
    let edges: Vec<Vec<(String, usize)>> = ids
        .iter()
        .map(|e| {
            live_ref_edges(doc, *e)
                .into_iter()
                .filter_map(|(k, t)| index_of.get(&t).map(|&j| (k, j)))
                .collect()
        })
        .collect();

    let n = ids.len();
    const UNVISITED: usize = usize::MAX;
    let mut index = vec![UNVISITED; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut comp = vec![UNVISITED; n];
    let mut comp_size: Vec<usize> = Vec::new();
    let mut next = 0usize;

    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        // (node, next edge position)
        let mut call: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&(v, pos)) = call.last() {
            if let Some(&(_, w)) = edges[v].get(pos) {
                if let Some(top) = call.last_mut() {
                    top.1 += 1;
                }
                if index[w] == UNVISITED {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            call.pop();
            if let Some(&(parent, _)) = call.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let c = comp_size.len();
                let mut size = 0;
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    comp[w] = c;
                    size += 1;
                    if w == v {
                        break;
                    }
                }
                comp_size.push(size);
            }
        }
    }

    let mut out = Vec::new();
    for v in 0..n {
        let c = comp[v];
        let on_cycle = comp_size.get(c).is_some_and(|&s| s > 1) || edges[v].iter().any(|(_, w)| *w == v);
        if !on_cycle {
            continue;
        }
        if let Some((k, w)) = edges[v].iter().find(|(_, w)| comp[*w] == c) {
            out.push((ids[v], k.clone(), ids[*w]));
        }
    }
    out
}

fn finite_num(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Num(x)) if x.is_finite() && x.abs() <= MAX_MAGNITUDE => Some(*x),
        _ => None,
    }
}

/// `(x, y)` of a live `Point` with well-formed coordinates.
fn point_xy(doc: &Document, p: EntityId) -> Option<(f64, f64)> {
    let e = doc.get(p)?;
    if e.kind != "Point" {
        return None;
    }
    Some((finite_num(e.props.get("x"))?, finite_num(e.props.get("y"))?))
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Coincident,
    Distance(f64),
}

/// A well-formed constraint between two well-formed Points, or `None`.
fn solvable(doc: &Document, c: &bim_doc::Entity) -> Option<(EntityId, EntityId, Kind)> {
    if c.kind != "Constraint" {
        return None;
    }
    let kind = match c.props.get("kind") {
        Some(Value::Text(k)) if k == "coincident" => Kind::Coincident,
        Some(Value::Text(k)) if k == "distance" => Kind::Distance(finite_num(c.props.get("value"))?),
        _ => return None,
    };
    let (a, b) = match (c.props.get("a"), c.props.get("b")) {
        (Some(Value::Ref(a)), Some(Value::Ref(b))) => (*a, *b),
        _ => return None,
    };
    point_xy(doc, a)?;
    point_xy(doc, b)?;
    Some((a, b, kind))
}

fn find(parent: &mut BTreeMap<EntityId, EntityId>, x: EntityId) -> EntityId {
    let mut root = x;
    while let Some(&p) = parent.get(&root) {
        if p == root {
            break;
        }
        root = p;
    }
    // Path compression.
    let mut cur = x;
    while let Some(&p) = parent.get(&cur) {
        if p == root {
            break;
        }
        parent.insert(cur, root);
        cur = p;
    }
    root
}

/// Solves every connected group of solvable constraints. Returns the solved
/// coordinates of Points in satisfiable groups and the constraints of
/// inconsistent groups.
#[allow(clippy::type_complexity)]
fn solve_constraints(doc: &Document) -> (BTreeMap<EntityId, (f64, f64)>, BTreeSet<EntityId>) {
    let mut cons: Vec<(EntityId, EntityId, EntityId, Kind)> = Vec::new();
    let mut parent: BTreeMap<EntityId, EntityId> = BTreeMap::new();
    for e in doc.entities() {
        if let Some((a, b, k)) = solvable(doc, e) {
            cons.push((e.id, a, b, k));
            parent.entry(a).or_insert(a);
            parent.entry(b).or_insert(b);
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                // Deterministic: the smaller id becomes the root.
                let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
                parent.insert(hi, lo);
            }
        }
    }

    // Group constraints (in id order) by the root of their points.
    let mut groups: BTreeMap<EntityId, Vec<(EntityId, EntityId, EntityId, Kind)>> = BTreeMap::new();
    for c in cons {
        let r = find(&mut parent, c.1);
        groups.entry(r).or_default().push(c);
    }

    let mut solved = BTreeMap::new();
    let mut unsatisfiable = BTreeSet::new();
    for group in groups.values() {
        let points: BTreeSet<EntityId> = group.iter().flat_map(|c| [c.1, c.2]).collect();
        let mut sketch = Sketch::new();
        let mut handle = BTreeMap::new();
        for &p in &points {
            // Every point passed `point_xy` in `solvable`.
            let (x, y) = point_xy(doc, p).unwrap_or((0.0, 0.0));
            handle.insert(p, sketch.add_point(x, y));
        }
        for &(_, a, b, k) in group {
            let (Some(&pa), Some(&pb)) = (handle.get(&a), handle.get(&b)) else {
                continue;
            };
            sketch.add(match k {
                Kind::Coincident => SolverConstraint::Coincident(pa, pb),
                Kind::Distance(d) => SolverConstraint::Distance(pa, pb, d),
            });
        }
        let report = sketch.solve();
        let coords: Vec<(EntityId, (f64, f64))> = handle.iter().map(|(p, h)| (*p, sketch.point(*h))).collect();
        let finite = coords.iter().all(|(_, (x, y))| x.is_finite() && y.is_finite());
        if report.status == Status::Inconsistent || !finite {
            unsatisfiable.extend(group.iter().map(|c| c.0));
        } else {
            solved.extend(coords);
        }
    }
    (solved, unsatisfiable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> EntityId {
        EntityId::derive(s)
    }

    fn point(r: &mut Replica, s: &str, x: f64, y: f64) {
        r.create(id(s), "Point".into());
        r.set(id(s), "x".into(), Value::Num(x));
        r.set(id(s), "y".into(), Value::Num(y));
    }

    fn constraint(r: &mut Replica, s: &str, kind: &str, a: &str, b: &str, v: f64) {
        r.create(id(s), "Constraint".into());
        r.set(id(s), "kind".into(), Value::Text(kind.into()));
        r.set(id(s), "a".into(), Value::Ref(id(a)));
        r.set(id(s), "b".into(), Value::Ref(id(b)));
        r.set(id(s), "value".into(), Value::Num(v));
    }

    fn xy(d: &Document, s: &str) -> (f64, f64) {
        let e = d.get(id(s)).unwrap();
        match (e.props.get("x"), e.props.get("y")) {
            (Some(Value::Num(x)), Some(Value::Num(y))) => (*x, *y),
            _ => panic!("no coordinates"),
        }
    }

    fn dist(d: &Document, a: &str, b: &str) -> f64 {
        let (p, q) = (xy(d, a), xy(d, b));
        (p.0 - q.0).hypot(p.1 - q.1)
    }

    fn flagged(ev: &Evaluated, reason: Reason) -> BTreeSet<EntityId> {
        ev.diagnostics.iter().filter(|d| d.reason == reason).map(|d| d.entity).collect()
    }

    /// Every diagnostic is materialized exactly once with the spec'd props.
    fn check_nodes(r: &Replica, ev: &Evaluated) {
        let syn = r.document();
        assert_eq!(ev.document.len(), syn.len() + ev.diagnostics.len());
        assert!(syn.entities().all(|e| e.kind != CONFLICT_DIAGNOSTIC_KIND || ev.document.get(e.id).is_some()));
        for d in &ev.diagnostics {
            assert_eq!(d.id, Diagnostic::derive_id(d.entity, &d.key, d.target));
            let n = ev.document.get(d.id).unwrap();
            assert_eq!(n.kind, CONFLICT_DIAGNOSTIC_KIND);
            assert_eq!(n.props.get("entity"), Some(&Value::Ref(d.entity)));
            assert_eq!(n.props.get("key"), Some(&Value::Text(d.key.clone())));
            assert_eq!(n.props.get("reason"), Some(&Value::Text(d.reason.as_str().into())));
            assert_eq!(n.props.get("target"), d.target.map(Value::Ref).as_ref());
            assert_eq!(n.props.get("_name"), Some(&Value::Text(seed(d.entity, &d.key, d.target))));
            assert_eq!(EntityId::derive(&seed(d.entity, &d.key, d.target)), d.id);
        }
        let mut sorted = ev.diagnostics.clone();
        sorted.sort_by_key(|d| d.id);
        sorted.dedup_by_key(|d| d.id);
        assert_eq!(sorted, ev.diagnostics);
    }

    #[test]
    fn dangling_refs_are_materialized_with_peer_independent_ids() {
        let mut a = Replica::new(1);
        a.create(id("wall"), "Wall".into());
        a.create(id("door"), "Opening".into());
        a.set(id("door"), "host".into(), Value::Ref(id("wall")));
        let mut b = Replica::new(2);
        b.merge(&a);
        b.delete(id("wall"));
        a.merge(&b);
        let before = a.document();
        let ev = a.evaluate();
        assert_eq!(a.document(), before, "evaluate is pure");
        check_nodes(&a, &ev);
        assert_eq!(ev.diagnostics.len(), 1);
        let d = &ev.diagnostics[0];
        assert_eq!((d.entity, d.key.as_str(), d.target, d.reason), (id("door"), "host", Some(id("wall")), Reason::DanglingRef));
        assert_eq!(d.id, EntityId::derive(&format!("conflict:{}host{}", id("door"), id("wall"))));

        let mut q = Replica::new(9);
        q.create(id("door"), "Opening".into());
        q.set(id("door"), "host".into(), Value::Ref(id("wall")));
        assert_eq!(q.evaluate().diagnostics[0].id, d.id);
        assert_eq!(a.diagnostics().len(), 1, "T1.2 diagnostics unchanged");
    }

    #[test]
    fn ref_cycles_name_every_member_and_ignore_list_refs() {
        let mut r = Replica::new(1);
        for g in ["g0", "g1", "g2", "g3", "g4"] {
            r.create(id(g), "Wall".into());
        }
        r.set(id("g0"), "ref".into(), Value::Ref(id("g1")));
        r.set(id("g1"), "ref".into(), Value::Ref(id("g2")));
        r.set(id("g2"), "host".into(), Value::Ref(id("g0")));
        r.set(id("g3"), "ref".into(), Value::Ref(id("g0")));
        // A List-nested ref back is not a top-level Ref edge.
        r.set(id("g4"), "ref".into(), Value::Ref(id("g3")));
        r.set(id("g3"), "list".into(), Value::List(vec![Value::Ref(id("g4"))]));
        let ev = r.evaluate();
        check_nodes(&r, &ev);
        assert_eq!(flagged(&ev, Reason::DependencyCycle), ["g0", "g1", "g2"].map(id).into_iter().collect());
        for d in ev.diagnostics.iter().filter(|d| d.reason == Reason::DependencyCycle) {
            let t = d.target.unwrap();
            assert!(["g0", "g1", "g2"].map(id).contains(&t), "target continues the cycle");
        }
        r.set(id("g1"), "ref".into(), Value::Num(0.0));
        r.set(id("g3"), "self".into(), Value::Ref(id("g3")));
        let ev = r.evaluate();
        check_nodes(&r, &ev);
        assert_eq!(flagged(&ev, Reason::DependencyCycle), [id("g3")].into_iter().collect());
        // A cycle through a deleted entity is not a cycle among live entities.
        r.delete(id("g3"));
        assert!(flagged(&r.evaluate(), Reason::DependencyCycle).is_empty());
    }

    #[test]
    fn deep_chains_and_long_cycles_do_not_overflow() {
        let mut r = Replica::new(1);
        let n = 20_000;
        for i in 0..n {
            r.create(id(&format!("e{i}")), "Wall".into());
            r.set(id(&format!("e{i}")), "next".into(), Value::Ref(id(&format!("e{}", (i + 1) % n))));
        }
        let ev = r.evaluate();
        assert_eq!(flagged(&ev, Reason::DependencyCycle).len(), n);
    }

    #[test]
    fn concurrent_constraints_flag_the_whole_group_and_keep_geometry() {
        let mut base = Replica::new(1);
        for (s, x, y) in [("p0", 0.0, 0.0), ("p1", 5.0, 0.1), ("p2", 1.7, 4.0), ("q0", 100.0, 0.0), ("q1", 105.0, 0.1), ("free", 7.0, 7.0)] {
            point(&mut base, s, x, y);
        }
        let mut a = base.clone();
        let mut b = Replica::new(2);
        b.merge(&base);
        constraint(&mut a, "c0", "distance", "p0", "p1", 1.0);
        constraint(&mut a, "c1", "distance", "p1", "p2", 1.0);
        constraint(&mut b, "c2", "distance", "p0", "p1", 1.5);
        constraint(&mut b, "d0", "distance", "q0", "q1", 4.0);
        for r in [&a, &b] {
            let ev = r.evaluate();
            check_nodes(r, &ev);
            assert!(ev.diagnostics.is_empty());
        }
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        let ev = ab.evaluate();
        assert_eq!(ev, ba.evaluate(), "merge order does not matter");
        check_nodes(&ab, &ev);
        assert_eq!(flagged(&ev, Reason::ConstraintUnsatisfiable), ["c0", "c1", "c2"].map(id).into_iter().collect());
        let syn = ab.document();
        for p in ["p0", "p1", "p2", "free"] {
            assert_eq!(xy(&ev.document, p), xy(&syn, p));
        }
        assert!((dist(&ev.document, "q0", "q1") - 4.0).abs() < 1e-9);
        // Removing the conflicting constraint makes the group solvable again.
        ab.delete(id("c2"));
        let ev = ab.evaluate();
        assert!(ev.diagnostics.is_empty());
        assert!((dist(&ev.document, "p0", "p1") - 1.0).abs() < 1e-9);
        assert!((dist(&ev.document, "p1", "p2") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn redundant_consistent_and_coincident_constraints() {
        let mut r = Replica::new(1);
        point(&mut r, "p0", 0.0, 0.0);
        point(&mut r, "p1", 2.0, 0.5);
        point(&mut r, "p2", 0.3, 3.0);
        constraint(&mut r, "c0", "distance", "p0", "p1", 1.0);
        constraint(&mut r, "c1", "distance", "p0", "p1", 1.0);
        constraint(&mut r, "c2", "coincident", "p1", "p2", 42.0); // value ignored
        let ev = r.evaluate();
        assert!(ev.diagnostics.is_empty(), "{:?}", ev.diagnostics);
        assert!((dist(&ev.document, "p0", "p1") - 1.0).abs() < 1e-9);
        assert!(dist(&ev.document, "p1", "p2") < 1e-9);
        constraint(&mut r, "c3", "distance", "p0", "p2", 3.0);
        let ev = r.evaluate();
        check_nodes(&r, &ev);
        assert_eq!(flagged(&ev, Reason::ConstraintUnsatisfiable).len(), 4);
        for d in &ev.diagnostics {
            assert_eq!((d.key.as_str(), d.target), (UNSATISFIABLE_KEY, None));
        }
    }

    #[test]
    fn malformed_and_dangling_constraints_are_skipped() {
        let mut r = Replica::new(1);
        point(&mut r, "p0", 0.0, 0.0);
        point(&mut r, "p1", 2.0, 0.0);
        r.create(id("wall"), "Wall".into());
        constraint(&mut r, "parallel", "parallel", "p0", "p1", 1.0);
        constraint(&mut r, "textval", "distance", "p0", "p1", 1.0);
        r.set(id("textval"), "value".into(), Value::Text("1".into()));
        constraint(&mut r, "nanval", "distance", "p0", "p1", f64::NAN);
        constraint(&mut r, "infval", "distance", "p0", "p1", f64::INFINITY);
        constraint(&mut r, "notpoint", "distance", "p0", "wall", 1.0);
        constraint(&mut r, "dangling", "distance", "p0", "gone", 1.0);
        constraint(&mut r, "listb", "distance", "p0", "p1", 1.0);
        r.set(id("listb"), "b".into(), Value::List(vec![Value::Ref(id("p1"))]));
        let syn = r.document();
        let ev = r.evaluate();
        check_nodes(&r, &ev);
        assert!(flagged(&ev, Reason::ConstraintUnsatisfiable).is_empty());
        assert_eq!(flagged(&ev, Reason::DanglingRef), [id("dangling")].into_iter().collect());
        assert_eq!(xy(&ev.document, "p0"), xy(&syn, "p0"));
        assert_eq!(xy(&ev.document, "p1"), xy(&syn, "p1"));
    }

    #[test]
    fn non_finite_and_huge_values_never_panic() {
        let specials = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, f64::MIN, 1e308, -1e308, 1e100, 1e99, -0.0, 0.0, 1e-300];
        for (i, &x) in specials.iter().enumerate() {
            for &v in &specials {
                let mut r = Replica::new(1);
                point(&mut r, "p0", x, -x);
                point(&mut r, "p1", -x, specials[(i + 3) % specials.len()]);
                point(&mut r, "p2", 1.0, 2.0);
                constraint(&mut r, "c0", "distance", "p0", "p1", v);
                constraint(&mut r, "c1", "distance", "p1", "p2", v);
                constraint(&mut r, "c2", "coincident", "p0", "p2", v);
                constraint(&mut r, "c3", "distance", "p2", "p2", v);
                let ev = r.evaluate();
                check_nodes(&r, &ev);
                // Points are either untouched (bitwise) or moved to finite solved values.
                let syn = r.document();
                for e in ev.document.entities().filter(|e| e.kind == "Point") {
                    for k in ["x", "y"] {
                        let (Some(Value::Num(n)), Some(Value::Num(o))) = (e.props.get(k), syn.get(e.id).and_then(|s| s.props.get(k))) else {
                            panic!("coordinates lost");
                        };
                        assert!(n.to_bits() == o.to_bits() || (n.is_finite() && o.is_finite()));
                    }
                }
            }
        }
    }

    #[test]
    fn diagnostic_id_collision_with_authored_entity_is_not_materialized() {
        let mut r = Replica::new(1);
        r.create(id("door"), "Opening".into());
        r.set(id("door"), "host".into(), Value::Ref(id("wall")));
        let clash = Diagnostic::derive_id(id("door"), "host", Some(id("wall")));
        r.create(clash, "Wall".into());
        let ev = r.evaluate();
        check_nodes(&r, &ev);
        assert!(ev.diagnostics.is_empty());
        assert_eq!(ev.document.get(clash).unwrap().kind, "Wall");
    }
}
