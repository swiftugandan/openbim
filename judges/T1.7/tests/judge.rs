// External judge for T1.7 (bim-crdt semantic layer). Hand-picked cases plus a proptest run of
// 10,000 random multi-peer histories (3-5 peers: creates, sets, deletes, constraint mutations,
// offline splits and merges). Expected diagnostics come from closed forms computed here from the
// merged syntactic document: dangling refs by lookup, dependency cycles by brute-force reachability,
// and constraint satisfiability of distance/coincident groups over at most three points by the
// triangle inequality (every value set below has a margin of at least 0.5 or is exactly degenerate).
use bim_crdt::Replica;
use bim_doc::{Document, EntityId, Value};
use proptest::prelude::*;
use proptest::test_runner::{Config as PConfig, RngSeed, TestCaseError};
use std::collections::{BTreeMap, BTreeSet};

fn id(s: &str) -> EntityId {
    EntityId::derive(s)
}

// ------------------------------------------------------------------ the entity universe

const GENERIC: [&str; 4] = ["g0", "g1", "g2", "g3"];
const PAIRS: [(usize, usize); 3] = [(0, 1), (0, 2), (1, 2)];
const XS: [[f64; 2]; 3] = [[0.0, 0.3], [5.0, 5.4], [1.7, 2.2]];
const YS: [[f64; 2]; 3] = [[0.0, 0.2], [0.1, 0.6], [4.0, 4.5]];
const VALUES: [f64; 3] = [1.0, 1.5, 4.0];

/// Six Points in two pools of three (pool b is shifted 100 m in x).
fn point_seed(p: usize) -> String {
    format!("p{}{}", ["a", "b"][p / 3], p % 3)
}
/// Twelve Constraints: two per point pair per pool.
fn constraint_seed(c: usize) -> String {
    format!("c{}{}_{}", ["a", "b"][c / 6], (c % 6) / 2, c % 2)
}
fn constraint_points(c: usize) -> (usize, usize) {
    let (i, j) = PAIRS[(c % 6) / 2];
    (c / 6 * 3 + i, c / 6 * 3 + j)
}
/// Initial guesses: every combination is a non-degenerate triangle with distinct vertices.
fn point_xy(p: usize, xi: usize, yi: usize) -> (f64, f64) {
    (XS[p % 3][xi] + 100.0 * (p / 3) as f64, YS[p % 3][yi])
}
fn all_seeds() -> Vec<String> {
    let mut v: Vec<String> = GENERIC.iter().map(|s| s.to_string()).collect();
    v.extend((0..6).map(point_seed));
    v.extend((0..12).map(constraint_seed));
    v
}

fn create_point(r: &mut Replica, p: usize, xi: usize, yi: usize) {
    let (x, y) = point_xy(p, xi, yi);
    let e = id(&point_seed(p));
    r.create(e, "Point".into());
    r.set(e, "x".into(), Value::Num(x));
    r.set(e, "y".into(), Value::Num(y));
}

fn create_constraint(r: &mut Replica, c: usize, coincident: bool, v: f64) {
    let e = id(&constraint_seed(c));
    let (a, b) = constraint_points(c);
    r.create(e, "Constraint".into());
    r.set(e, "kind".into(), Value::Text(if coincident { "coincident" } else { "distance" }.into()));
    r.set(e, "a".into(), Value::Ref(id(&point_seed(a))));
    r.set(e, "b".into(), Value::Ref(id(&point_seed(b))));
    r.set(e, "value".into(), Value::Num(if coincident { 0.0 } else { v }));
}

#[derive(Debug, Clone)]
enum Edit {
    CreateGeneric(usize, bool),
    DeleteGeneric(usize),
    SetRef(usize, bool, usize),
    SetNum(usize, u8),
    CreatePoint(usize, usize, usize),
    DeletePoint(usize),
    MovePoint(usize, usize, usize),
    CreateConstraint(usize, bool, usize),
    DeleteConstraint(usize),
    SetValue(usize, usize),
    SetKind(usize, bool),
    /// Malformed constraint/point data (only applied in cases that allow it).
    Garbage(usize, usize),
}

/// Applies an edit; returns true if it was a garbage edit that was applied.
fn apply_edit(r: &mut Replica, e: &Edit, allow_garbage: bool) -> bool {
    match *e {
        Edit::CreateGeneric(g, wall) => r.create(id(GENERIC[g]), if wall { "Wall" } else { "Level" }.into()),
        Edit::DeleteGeneric(g) => r.delete(id(GENERIC[g])),
        Edit::SetRef(g, host, t) => r.set(id(GENERIC[g]), if host { "host" } else { "ref" }.into(), Value::Ref(id(GENERIC[t]))),
        Edit::SetNum(g, x) => r.set(id(GENERIC[g]), "x".into(), Value::Num(x as f64)),
        Edit::CreatePoint(p, xi, yi) => create_point(r, p, xi, yi),
        Edit::DeletePoint(p) => r.delete(id(&point_seed(p))),
        Edit::MovePoint(p, xi, yi) => {
            let (x, y) = point_xy(p, xi, yi);
            r.set(id(&point_seed(p)), "x".into(), Value::Num(x));
            r.set(id(&point_seed(p)), "y".into(), Value::Num(y));
        }
        Edit::CreateConstraint(c, coincident, v) => create_constraint(r, c, coincident, VALUES[v]),
        Edit::DeleteConstraint(c) => r.delete(id(&constraint_seed(c))),
        Edit::SetValue(c, v) => r.set(id(&constraint_seed(c)), "value".into(), Value::Num(VALUES[v])),
        Edit::SetKind(c, coincident) => {
            r.set(id(&constraint_seed(c)), "kind".into(), Value::Text(if coincident { "coincident" } else { "distance" }.into()))
        }
        Edit::Garbage(t, kind) => {
            if !allow_garbage {
                return false;
            }
            let c = id(&constraint_seed(t % 12));
            let p = id(&point_seed(t % 6));
            match kind {
                0 => r.set(c, "a".into(), Value::Ref(id(GENERIC[t % 4]))),
                1 => r.set(c, "kind".into(), Value::Text("parallel".into())),
                2 => r.set(p, "x".into(), Value::Text("oops".into())),
                3 => r.set(c, "value".into(), Value::Text("1".into())),
                4 => r.set(c, "kind".into(), Value::Text("datum".into())),
                _ => r.set(p, "y".into(), Value::Bool(true)),
            }
            return true;
        }
    }
    false
}

#[derive(Debug, Clone)]
enum Action {
    Edit(usize, Edit),
    Merge(usize, usize),
    /// Offline split healing: every peer in the bitmask converges with the others in it.
    Gossip(u8),
}

// ------------------------------------------------------------------ reading documents

fn prop<'a>(d: &'a Document, e: EntityId, k: &str) -> Option<&'a Value> {
    d.get(e).and_then(|x| x.props.get(k))
}
fn num(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Num(x)) => Some(*x),
        _ => None,
    }
}
fn text(v: Option<&Value>) -> Option<&str> {
    match v {
        Some(Value::Text(s)) => Some(s.as_str()),
        _ => None,
    }
}
fn reff(v: Option<&Value>) -> Option<EntityId> {
    match v {
        Some(Value::Ref(t)) => Some(*t),
        _ => None,
    }
}
fn user_props(d: &Document, e: EntityId) -> BTreeMap<String, Value> {
    d.get(e).map(|x| x.props.iter().filter(|(k, _)| !k.starts_with('_')).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Diag {
    entity: EntityId,
    key: Option<String>,
    target: Option<EntityId>,
    reason: String,
}

/// Finds the materialized ConflictDiagnostic entities of an evaluated document: each one
/// references the entity it is about, so it is a dependent of some known seed.
fn diagnostics_of(ev: &Document, known: &BTreeSet<EntityId>) -> Result<BTreeMap<EntityId, Diag>, String> {
    let mut out = BTreeMap::new();
    for s in known {
        for d in ev.dependents(*s) {
            if known.contains(&d) || out.contains_key(&d) {
                continue;
            }
            let e = ev.get(d).ok_or("dependents() returned a missing entity")?;
            if e.kind != "ConflictDiagnostic" {
                return Err(format!("evaluated document has an unexpected {} entity", e.kind));
            }
            let entity = reff(e.props.get("entity")).ok_or("ConflictDiagnostic without an `entity` Ref")?;
            let reason = text(e.props.get("reason")).ok_or("ConflictDiagnostic without a `reason` Text")?.to_string();
            if !["dangling_ref", "constraint_unsatisfiable", "dependency_cycle"].contains(&reason.as_str()) {
                return Err(format!("unknown ConflictDiagnostic reason {reason:?}"));
            }
            let key = match e.props.get("key") {
                None => None,
                Some(Value::Text(k)) => Some(k.clone()),
                Some(v) => return Err(format!("ConflictDiagnostic `key` must be Text, got {v:?}")),
            };
            let target = match e.props.get("target") {
                None => None,
                Some(Value::Ref(t)) => Some(*t),
                Some(v) => return Err(format!("ConflictDiagnostic `target` must be a Ref, got {v:?}")),
            };
            out.insert(d, Diag { entity, key, target, reason });
        }
    }
    Ok(out)
}

/// Brute-force: entities that reach themselves along Ref edges between live entities.
fn on_cycles(s: &Document, known: &[EntityId]) -> BTreeSet<EntityId> {
    let live: Vec<EntityId> = known.iter().copied().filter(|e| s.get(*e).is_some()).collect();
    let succ = |e: EntityId| -> Vec<EntityId> {
        s.get(e).unwrap().props.values().filter_map(|v| reff(Some(v))).filter(|t| s.get(*t).is_some()).collect()
    };
    let mut out = BTreeSet::new();
    for &start in &live {
        let mut seen = BTreeSet::new();
        let mut stack = succ(start);
        while let Some(x) = stack.pop() {
            if x == start {
                out.insert(start);
                break;
            }
            if seen.insert(x) {
                stack.extend(succ(x));
            }
        }
    }
    out
}

fn find(uf: &mut [usize], x: usize) -> usize {
    if uf[x] != x {
        let r = find(uf, uf[x]);
        uf[x] = r;
    }
    uf[x]
}

struct Group {
    points: BTreeSet<usize>,
    constraints: Vec<(usize, usize, usize, f64)>, // (constraint, point a, point b, required distance)
    inconsistent: bool,
    degenerate: bool,
}

/// Connected groups of well-formed constraints between live Points, with closed-form satisfiability.
fn constraint_groups(s: &Document) -> Vec<Group> {
    let mut uf: Vec<usize> = (0..6).collect();
    let mut cons = Vec::new();
    for c in 0..12 {
        let e = id(&constraint_seed(c));
        if s.get(e).is_none() {
            continue;
        }
        let (a, b) = constraint_points(c);
        if s.get(id(&point_seed(a))).is_none() || s.get(id(&point_seed(b))).is_none() {
            continue; // dangling: reported as dangling_ref, not solved
        }
        let v = match text(prop(s, e, "kind")) {
            Some("coincident") => 0.0,
            Some("distance") => num(prop(s, e, "value")).unwrap(),
            k => panic!("judge invariant: constraint kind {k:?}"),
        };
        cons.push((c, a, b, v));
        let (ra, rb) = (find(&mut uf, a), find(&mut uf, b));
        uf[ra] = rb;
    }
    let mut groups: BTreeMap<usize, Group> = BTreeMap::new();
    for &(c, a, b, v) in &cons {
        let root = find(&mut uf, a);
        let g = groups.entry(root).or_insert(Group { points: BTreeSet::new(), constraints: vec![], inconsistent: false, degenerate: false });
        g.points.insert(a);
        g.points.insert(b);
        g.constraints.push((c, a, b, v));
    }
    for g in groups.values_mut() {
        let mut per_pair: BTreeMap<(usize, usize), BTreeSet<u64>> = BTreeMap::new();
        for &(_, a, b, v) in &g.constraints {
            per_pair.entry((a, b)).or_default().insert(v.to_bits());
            g.degenerate |= v == 0.0;
        }
        g.inconsistent = per_pair.values().any(|vs| vs.len() > 1);
        if !g.inconsistent && per_pair.len() == 3 {
            let mut d: Vec<f64> = per_pair.values().map(|vs| f64::from_bits(*vs.iter().next().unwrap())).collect();
            d.sort_by(|x, y| x.partial_cmp(y).unwrap());
            g.inconsistent = d[2] > d[0] + d[1];
        }
    }
    groups.into_values().collect()
}

fn xy(d: &Document, p: usize) -> Option<(f64, f64)> {
    let e = id(&point_seed(p));
    Some((num(prop(d, e, "x"))?, num(prop(d, e, "y"))?))
}

/// All semantic checks on one merged replica. `strict_constraints` is false when malformed
/// constraint/point data was written (then only robustness, dangling and cycle checks apply).
fn check_semantics(merged: &Replica, strict_constraints: bool) -> Result<(), String> {
    let seeds = all_seeds();
    let known_vec: Vec<EntityId> = seeds.iter().map(|s| id(s)).collect();
    let known: BTreeSet<EntityId> = known_vec.iter().copied().collect();
    let s = merged.document();
    let live: Vec<EntityId> = known_vec.iter().copied().filter(|e| s.get(*e).is_some()).collect();

    // Syntactic layer: no diagnostic nodes, nothing but the authored entities.
    if s.len() != live.len() {
        return Err(format!("document() has {} entities, {} authored ones are live (diagnostic nodes belong only to evaluate())", s.len(), live.len()));
    }

    let ev = merged.evaluate();
    let e = &ev.document;
    let diags = diagnostics_of(e, &known)?;
    if diags.len() != ev.diagnostics.len() {
        return Err(format!("{} ConflictDiagnostic entities but {} diagnostics", diags.len(), ev.diagnostics.len()));
    }
    if e.len() != live.len() + diags.len() {
        return Err(format!("evaluated document has {} entities, want {} live + {} diagnostics", e.len(), live.len(), diags.len()));
    }
    for k in &known_vec {
        match (s.get(*k), e.get(*k)) {
            (None, None) => {}
            (Some(a), Some(b)) if a.kind == b.kind => {}
            _ => return Err("evaluated document does not have the same live entities as document()".into()),
        }
    }

    // dangling_ref: every live Ref whose target is not live, exactly once.
    let mut want_dangling = BTreeSet::new();
    for &x in &live {
        for (k, v) in &s.get(x).unwrap().props {
            if let Value::Ref(t) = v {
                if s.get(*t).is_none() {
                    want_dangling.insert((x, k.clone(), *t));
                }
            }
        }
    }
    let got_dangling: BTreeSet<(EntityId, String, EntityId)> = diags
        .values()
        .filter(|d| d.reason == "dangling_ref")
        .map(|d| (d.entity, d.key.clone().unwrap_or_default(), d.target.unwrap_or(EntityId::derive("<no target>"))))
        .collect();
    let n_dangling = diags.values().filter(|d| d.reason == "dangling_ref").count();
    if got_dangling != want_dangling || n_dangling != want_dangling.len() {
        return Err(format!("dangling_ref diagnostics {got_dangling:?}, want {want_dangling:?}"));
    }
    let t12: BTreeSet<(EntityId, String, EntityId)> = merged.diagnostics().into_iter().map(|d| (d.entity, d.key, d.target)).collect();
    if t12 != want_dangling {
        return Err("Replica::diagnostics() (T1.2) disagrees with the dangling refs of document()".into());
    }

    // dependency_cycle: exactly the entities lying on a Ref cycle.
    let want_cycle = on_cycles(&s, &known_vec);
    let got_cycle: BTreeSet<EntityId> = diags.values().filter(|d| d.reason == "dependency_cycle").map(|d| d.entity).collect();
    if got_cycle != want_cycle {
        return Err(format!("dependency_cycle diagnostics name {} entities, {} lie on Ref cycles", got_cycle.len(), want_cycle.len()));
    }

    // Authored props survive evaluation, except solved Point coordinates.
    for &x in &live {
        let kind = &s.get(x).unwrap().kind;
        let (a, b) = (user_props(&s, x), user_props(e, x));
        let strip = |m: BTreeMap<String, Value>| -> BTreeMap<String, Value> {
            if kind == "Point" { m.into_iter().filter(|(k, _)| k != "x" && k != "y").collect() } else { m }
        };
        if strip(a) != strip(b) {
            return Err(format!("evaluate() changed authored props of a {kind}"));
        }
    }

    if !strict_constraints {
        return Ok(());
    }
    let flagged: BTreeSet<EntityId> = diags.values().filter(|d| d.reason == "constraint_unsatisfiable").map(|d| d.entity).collect();
    let groups = constraint_groups(&s);
    let mut in_groups = BTreeSet::new();
    let mut grouped_points = BTreeSet::new();
    for g in &groups {
        let ids: BTreeSet<EntityId> = g.constraints.iter().map(|c| id(&constraint_seed(c.0))).collect();
        in_groups.extend(ids.iter().copied());
        grouped_points.extend(g.points.iter().copied());
        let n_flagged = ids.intersection(&flagged).count();
        let unchanged = g.points.iter().all(|&p| xy(e, p) == xy(&s, p));
        let satisfied = g.constraints.iter().all(|&(_, a, b, v)| match (xy(e, a), xy(e, b)) {
            (Some(p), Some(q)) => (((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt() - v).abs() <= 1e-6,
            _ => false,
        });
        let describe = || format!("group of {} constraints {:?} over points {:?}", g.constraints.len(),
            g.constraints.iter().map(|c| (constraint_seed(c.0), c.3)).collect::<Vec<_>>(), g.points);
        if g.inconsistent {
            if n_flagged != ids.len() {
                return Err(format!("unsatisfiable {}: {} of {} constraints flagged constraint_unsatisfiable", describe(), n_flagged, ids.len()));
            }
            if !unchanged {
                return Err(format!("unsatisfiable {}: geometry was changed", describe()));
            }
        } else if n_flagged == 0 {
            if !satisfied {
                return Err(format!("satisfiable {}: evaluated Points do not satisfy it within 1e-6", describe()));
            }
        } else if g.degenerate && n_flagged == ids.len() && unchanged {
            // A coincident-degenerate group a solver may legitimately fail to close; tolerated.
        } else {
            return Err(format!("satisfiable {}: {} constraints flagged constraint_unsatisfiable", describe(), n_flagged));
        }
    }
    if !flagged.is_subset(&in_groups) {
        return Err("constraint_unsatisfiable reported for a constraint outside any solvable group".into());
    }
    for p in 0..6 {
        if !grouped_points.contains(&p) && xy(e, p) != xy(&s, p) {
            return Err(format!("unconstrained point {} moved", point_seed(p)));
        }
    }
    Ok(())
}

fn fold(peers: &[Replica], order: &[usize]) -> Replica {
    let mut acc = peers[order[0]].clone();
    for &i in &order[1..] {
        acc.merge(&peers[i]);
    }
    acc
}

fn eval_hash(r: &Replica) -> u64 {
    r.evaluate().document.content_hash()
}

fn run_history(n: usize, setup: &[Edit], actions: &[Action], perm_seed: u64, allow_garbage: bool) -> Result<(), String> {
    let mut garbage = false;
    let mut peers: Vec<Replica> = (1..=n as u32).map(Replica::new).collect();
    for e in setup {
        garbage |= apply_edit(&mut peers[0], e, allow_garbage);
    }
    let base = peers[0].clone();
    for p in peers.iter_mut().skip(1) {
        p.merge(&base);
    }
    for a in actions {
        match a {
            Action::Edit(p, e) => garbage |= apply_edit(&mut peers[p % n], e, allow_garbage),
            Action::Merge(p, q) => {
                let other = peers[q % n].clone();
                peers[p % n].merge(&other);
            }
            Action::Gossip(mask) => {
                let members: Vec<usize> = (0..n).filter(|i| mask >> i & 1 == 1).collect();
                if members.len() >= 2 {
                    let acc = fold(&peers, &members);
                    for &m in &members {
                        peers[m].merge(&acc);
                    }
                }
            }
        }
        // Semantic evaluation must also be safe on partially merged peers (sampled: evaluate()
        // runs the solver, and the final checks below already call it five times per history).
        if let Action::Merge(p, _) = a {
            if perm_seed % 4 == 0 {
                let _ = peers[p % n].evaluate();
            }
        }
    }
    // Final merge in three orders.
    let forward: Vec<usize> = (0..n).collect();
    let backward: Vec<usize> = (0..n).rev().collect();
    let mut shuffled = forward.clone();
    let mut x = perm_seed | 1;
    for i in (1..n).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        shuffled.swap(i, (x % (i as u64 + 1)) as usize);
    }
    let merged = [fold(&peers, &forward), fold(&peers, &backward), fold(&peers, &shuffled)];
    let syn: Vec<(u64, usize)> = merged.iter().map(|m| (m.document().content_hash(), m.diagnostics().len())).collect();
    if syn[0] != syn[1] || syn[0] != syn[2] {
        return Err("syntactic merge is order dependent (T1.2 guarantee broken)".into());
    }
    let h: Vec<u64> = merged.iter().map(eval_hash).collect();
    if h[0] != h[1] || h[0] != h[2] {
        return Err("evaluate() differs between merge orders of the same state".into());
    }
    if eval_hash(&merged[0]) != h[0] {
        return Err("evaluate() is not deterministic".into());
    }
    let before = merged[0].document().content_hash();
    check_semantics(&merged[0], !garbage)?;
    if merged[0].document().content_hash() != before {
        return Err("evaluate() changed the syntactic state".into());
    }
    Ok(())
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        2 => (0..4usize, any::<bool>()).prop_map(|(g, w)| Edit::CreateGeneric(g, w)),
        1 => (0..4usize).prop_map(Edit::DeleteGeneric),
        3 => (0..4usize, any::<bool>(), 0..4usize).prop_map(|(g, h, t)| Edit::SetRef(g, h, t)),
        1 => (0..4usize, 0..3u8).prop_map(|(g, x)| Edit::SetNum(g, x)),
        3 => (0..6usize, 0..2usize, 0..2usize).prop_map(|(p, a, b)| Edit::CreatePoint(p, a, b)),
        1 => (0..6usize).prop_map(Edit::DeletePoint),
        2 => (0..6usize, 0..2usize, 0..2usize).prop_map(|(p, a, b)| Edit::MovePoint(p, a, b)),
        5 => (0..12usize, prop::bool::weighted(0.2), 0..3usize).prop_map(|(c, k, v)| Edit::CreateConstraint(c, k, v)),
        1 => (0..12usize).prop_map(Edit::DeleteConstraint),
        2 => (0..12usize, 0..3usize).prop_map(|(c, v)| Edit::SetValue(c, v)),
        1 => (0..12usize, prop::bool::weighted(0.2)).prop_map(|(c, k)| Edit::SetKind(c, k)),
        1 => (0..12usize, 0..6usize).prop_map(|(t, k)| Edit::Garbage(t, k)),
    ]
}

fn setup_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        2 => (0..4usize, any::<bool>()).prop_map(|(g, w)| Edit::CreateGeneric(g, w)),
        2 => (0..4usize, any::<bool>(), 0..4usize).prop_map(|(g, h, t)| Edit::SetRef(g, h, t)),
        4 => (0..6usize, 0..2usize, 0..2usize).prop_map(|(p, a, b)| Edit::CreatePoint(p, a, b)),
        3 => (0..12usize, prop::bool::weighted(0.15), 0..3usize).prop_map(|(c, k, v)| Edit::CreateConstraint(c, k, v)),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        6 => (0..5usize, edit()).prop_map(|(p, e)| Action::Edit(p, e)),
        2 => (0..5usize, 0..5usize).prop_map(|(p, q)| Action::Merge(p, q)),
        1 => (1..32u8).prop_map(Action::Gossip),
    ]
}

proptest! {
    #![proptest_config(PConfig {
        cases: 10_000,
        rng_seed: RngSeed::Fixed(0x7_1_7_2026),
        failure_persistence: None,
        max_shrink_iters: 2_000,
        max_shrink_time: 20_000,
        ..PConfig::default()
    })]
    #[test]
    fn ten_thousand_multi_peer_histories(
        n in 3..=5usize,
        setup in prop::collection::vec(setup_edit(), 0..16),
        actions in prop::collection::vec(action(), 5..40),
        perm_seed in any::<u64>(),
        allow_garbage in prop::bool::weighted(0.25),
    ) {
        run_history(n, &setup, &actions, perm_seed, allow_garbage).map_err(TestCaseError::fail)?;
    }
}

// ------------------------------------------------------------------ hand-picked cases

fn diags_of(r: &Replica) -> BTreeMap<EntityId, Diag> {
    let known: BTreeSet<EntityId> = all_seeds().iter().map(|s| id(s)).chain(["wall", "door"].map(id)).collect();
    diagnostics_of(&r.evaluate().document, &known).unwrap()
}

fn three_points(r: &mut Replica) {
    for p in 0..6 {
        create_point(r, p, 0, 0);
    }
}

#[test]
fn dangling_ref_is_materialized() {
    let mut p1 = Replica::new(1);
    p1.create(id("wall"), "Wall".into());
    p1.create(id("door"), "Opening".into());
    p1.set(id("door"), "host".into(), Value::Ref(id("wall")));
    let mut p2 = Replica::new(2);
    p2.merge(&p1);
    p2.delete(id("wall"));
    p1.merge(&p2);
    let d: Vec<Diag> = diags_of(&p1).into_values().collect();
    assert_eq!(d, vec![Diag { entity: id("door"), key: Some("host".into()), target: Some(id("wall")), reason: "dangling_ref".into() }]);
    let ev = p1.evaluate();
    assert_eq!(ev.diagnostics.len(), 1);
    assert_eq!(ev.document.len(), 2, "door + one ConflictDiagnostic");
    assert_eq!(p1.document().len(), 1, "document() stays syntactic");

    // Diagnostic ids are derived from (entity, key, target), not from peers or stamps.
    let mut q = Replica::new(9);
    q.create(id("door"), "Opening".into());
    q.set(id("door"), "x".into(), Value::Num(1.0));
    q.set(id("door"), "host".into(), Value::Ref(id("wall")));
    let ids_p: Vec<EntityId> = diags_of(&p1).into_keys().collect();
    let ids_q: Vec<EntityId> = diags_of(&q).into_keys().collect();
    assert_eq!(ids_p, ids_q, "the same conflict must get the same ConflictDiagnostic id on every replica");
}

#[test]
fn concurrent_constraints_become_unsatisfiable() {
    let mut base = Replica::new(1);
    three_points(&mut base);
    let mut p1 = base.clone();
    let mut p2 = Replica::new(2);
    p2.merge(&base);
    create_constraint(&mut p1, 0, false, 1.0); // pa0-pa1 = 1
    create_constraint(&mut p2, 1, false, 1.5); // pa0-pa1 = 1.5, concurrently
    create_constraint(&mut p2, 6, false, 4.0); // pool b: pb0-pb1 = 4, alone
    for r in [&p1, &p2] {
        check_semantics(r, true).unwrap(); // each peer alone is consistent
    }
    p1.merge(&p2);
    let flagged: BTreeSet<EntityId> = diags_of(&p1).into_values().filter(|d| d.reason == "constraint_unsatisfiable").map(|d| d.entity).collect();
    assert_eq!(flagged, [0, 1].map(|c| id(&constraint_seed(c))).into_iter().collect());
    let ev = p1.evaluate().document;
    let s = p1.document();
    assert_eq!(xy(&ev, 0), xy(&s, 0), "no geometry change in an unsatisfiable group");
    assert_eq!(xy(&ev, 1), xy(&s, 1));
    let (a, b) = (xy(&ev, 3).unwrap(), xy(&ev, 4).unwrap());
    assert!((((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() - 4.0).abs() < 1e-6, "the satisfiable group is solved");
    check_semantics(&p1, true).unwrap();
}

#[test]
fn triangle_inequality_across_three_peers() {
    let mut base = Replica::new(100);
    three_points(&mut base);
    let mut peers: Vec<Replica> = (1..=4).map(|k| {
        let mut r = Replica::new(k);
        r.merge(&base);
        r
    }).collect();
    create_constraint(&mut peers[0], 0, false, 1.0); // a0-a1 = 1
    create_constraint(&mut peers[1], 2, false, 1.0); // a0-a2 = 1
    create_constraint(&mut peers[2], 4, false, 4.0); // a1-a2 = 4 > 1 + 1
    create_constraint(&mut peers[3], 8, false, 1.5); // pool b, unrelated
    let mut all = peers[0].clone();
    for p in &peers[1..] {
        all.merge(p);
    }
    let flagged: BTreeSet<EntityId> = diags_of(&all).into_values().filter(|d| d.reason == "constraint_unsatisfiable").map(|d| d.entity).collect();
    assert_eq!(flagged, [0, 2, 4].map(|c| id(&constraint_seed(c))).into_iter().collect());
    check_semantics(&all, true).unwrap();

    // Coincident plus a non-zero distance on the same pair.
    let mut q = base.clone();
    create_constraint(&mut q, 6, true, 0.0);
    create_constraint(&mut q, 7, false, 1.0);
    let flagged: BTreeSet<EntityId> = diags_of(&q).into_values().filter(|d| d.reason == "constraint_unsatisfiable").map(|d| d.entity).collect();
    assert_eq!(flagged, [6, 7].map(|c| id(&constraint_seed(c))).into_iter().collect());
    check_semantics(&q, true).unwrap();
}

#[test]
fn ref_cycles_are_reported() {
    let mut r = Replica::new(1);
    for g in GENERIC {
        r.create(id(g), "Wall".into());
    }
    r.set(id("g0"), "ref".into(), Value::Ref(id("g1")));
    r.set(id("g1"), "ref".into(), Value::Ref(id("g2")));
    r.set(id("g2"), "host".into(), Value::Ref(id("g0")));
    r.set(id("g3"), "ref".into(), Value::Ref(id("g0"))); // leads into the cycle, not on it
    let on: BTreeSet<EntityId> = diags_of(&r).into_values().filter(|d| d.reason == "dependency_cycle").map(|d| d.entity).collect();
    assert_eq!(on, ["g0", "g1", "g2"].map(id).into_iter().collect());
    check_semantics(&r, true).unwrap();
    r.set(id("g1"), "ref".into(), Value::Num(0.0));
    r.set(id("g3"), "host".into(), Value::Ref(id("g3")));
    let on: BTreeSet<EntityId> = diags_of(&r).into_values().filter(|d| d.reason == "dependency_cycle").map(|d| d.entity).collect();
    assert_eq!(on, [id("g3")].into_iter().collect(), "a self-reference is a cycle");
}

#[test]
fn garbage_never_panics() {
    let mut r = Replica::new(1);
    three_points(&mut r);
    for c in 0..12 {
        create_constraint(&mut r, c, c % 3 == 0, VALUES[c % 3]);
    }
    for k in 0..6 {
        apply_edit(&mut r, &Edit::Garbage(k * 2 + 1, k), true);
    }
    r.set(id(&constraint_seed(5)), "b".into(), Value::List(vec![Value::Ref(id("pa0"))]));
    r.set(id(&constraint_seed(9)), "a".into(), Value::Ref(id(&constraint_seed(9))));
    r.set(id(&point_seed(4)), "x".into(), Value::Num(f64::NAN));
    let _ = r.evaluate();
    check_semantics(&r, false).unwrap();
}
