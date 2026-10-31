// External judge for T3.1 (bim-incr through bim_model::Session). Asserts the Spec bullet in TASKS.md.
//
// Ground truth never comes from the incremental path itself: every expectation is derived from
// fresh from-scratch Sessions (a new Session fed the whole op history), from the joins/openings the
// fixture authors by construction, and from which entities each from-scratch drawing references
// (`data-entity`, spec/cli.md).
// Only `Session` is named; the stats type is used through its `solids` / `views` fields (TASKS.md
// T3.1 Details), so where the builder defines it does not matter.
use bim_doc::{EntityId, Op, Value};
use bim_model::Session;
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------- fixture

const VIEWS: [&str; 4] = [
    "plan:L1@1.2",
    "plan:L2@1.2",
    // Looks toward +y: the L1 cluster (y >= -3); the L2 cluster (y <= -20) is behind the plane.
    "section:-5,-1,30,-1",
    // Looks toward -y: the L2 cluster; the L1 cluster (y >= -3) is behind the plane.
    "section:30,-15,-5,-15",
];

/// Entities whose solids the judge compares (openings are cutters, compared through their hosts).
const ELEMENTS: [&str; 11] = ["WA", "WB", "WC", "WD", "WT", "WI", "S1", "WE", "S2", "K1", "BM"];

fn id(s: &str) -> EntityId {
    EntityId::derive(s)
}
fn num(x: f64) -> Value {
    Value::Num(x)
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}
fn rf(s: &str) -> Value {
    Value::Ref(id(s))
}
fn list(xs: &[f64]) -> Value {
    Value::List(xs.iter().map(|x| Value::Num(*x)).collect())
}

fn create(ops: &mut Vec<Op>, seed: &str, kind: &str, props: Vec<(&str, Value)>) {
    ops.push(Op::Create { id: id(seed), kind: kind.into() });
    ops.push(set(seed, "_name", text(seed)));
    for (k, v) in props {
        ops.push(set(seed, k, v));
    }
}
fn set(seed: &str, key: &str, value: Value) -> Op {
    Op::Set { id: id(seed), key: key.into(), value }
}

fn wall(ops: &mut Vec<Op>, seed: &str, level: &str, p: [f64; 4], h: f64) {
    create(
        ops,
        seed,
        "Wall",
        vec![
            ("level", rf(level)),
            ("x1", num(p[0])),
            ("y1", num(p[1])),
            ("x2", num(p[2])),
            ("y2", num(p[3])),
            ("height", num(h)),
            ("type", rf("WT200")),
        ],
    );
}

fn opening(ops: &mut Vec<Op>, seed: &str, host: &str, kind: &str, off: f64, sill: f64, w: f64, h: f64) {
    create(
        ops,
        seed,
        "Opening",
        vec![
            ("host", rf(host)),
            ("kind", text(kind)),
            ("offset", num(off)),
            ("sill", num(sill)),
            ("width", num(w)),
            ("height", num(h)),
        ],
    );
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    list(&[x0, y0, x1, y0, x1, y1, x0, y1])
}

/// Two levels. L1: a closed square WA–WB–WC–WD (four L-joins), a wall WT abutting WA at a T-join,
/// openings OA (in WA) and OC (in WC), an isolated wall WI, slab S1. L2 (far away in -y): wall WE
/// with opening OE, slab S2, beam BM, and column K1 standing on L1 up to L2.
fn fixture() -> Vec<Op> {
    let mut o = Vec::new();
    create(&mut o, "L1", "Level", vec![("elevation", num(0.0))]);
    create(&mut o, "L2", "Level", vec![("elevation", num(4.0))]);
    create(
        &mut o,
        "WT200",
        "WallType",
        vec![
            ("layer_thickness", list(&[0.2])),
            ("layer_material", Value::List(vec![text("concrete")])),
            ("layer_conductivity", list(&[1.6])),
        ],
    );
    create(
        &mut o,
        "M1",
        "Material",
        vec![
            ("name", text("S355")),
            ("E", num(210e9)),
            ("G", num(81e9)),
            ("density", num(7850.0)),
            ("fy", num(355e6)),
        ],
    );
    wall(&mut o, "WA", "L1", [0.0, 0.0, 6.0, 0.0], 3.0);
    wall(&mut o, "WB", "L1", [6.0, 0.0, 6.0, 4.0], 3.0);
    wall(&mut o, "WC", "L1", [6.0, 4.0, 0.0, 4.0], 3.0);
    wall(&mut o, "WD", "L1", [0.0, 4.0, 0.0, 0.0], 3.0);
    wall(&mut o, "WT", "L1", [3.0, 0.0, 3.0, -3.0], 3.0);
    wall(&mut o, "WI", "L1", [20.0, 0.0, 26.0, 0.0], 3.0);
    opening(&mut o, "OA", "WA", "door", 1.0, 0.0, 0.9, 2.1);
    opening(&mut o, "OC", "WC", "window", 2.0, 1.0, 1.2, 1.2);
    create(
        &mut o,
        "S1",
        "Slab",
        vec![("level", rf("L1")), ("boundary", rect(0.0, 0.0, 6.0, 4.0)), ("thickness", num(0.2))],
    );
    wall(&mut o, "WE", "L2", [0.0, -20.0, 6.0, -20.0], 3.0);
    opening(&mut o, "OE", "WE", "window", 2.0, 1.0, 1.0, 1.0);
    create(
        &mut o,
        "S2",
        "Slab",
        vec![("level", rf("L2")), ("boundary", rect(0.0, -30.0, 6.0, -22.0)), ("thickness", num(0.25))],
    );
    create(
        &mut o,
        "BM",
        "Beam",
        vec![
            ("level", rf("L2")),
            ("x1", num(0.0)),
            ("y1", num(-26.0)),
            ("x2", num(6.0)),
            ("y2", num(-26.0)),
            ("section_b", num(0.2)),
            ("section_h", num(0.4)),
            ("material", rf("M1")),
        ],
    );
    create(
        &mut o,
        "K1",
        "Column",
        vec![
            ("x", num(10.0)),
            ("y", num(-25.0)),
            ("base", rf("L1")),
            ("top", rf("L2")),
            ("section_b", num(0.3)),
            ("section_h", num(0.3)),
            ("material", rf("M1")),
        ],
    );
    o
}

// ---------------------------------------------------------------- harness

/// The incremental session under test plus the full op history (for from-scratch rebuilds) and
/// the from-scratch snapshot of the current history (so each edit costs one fresh compile).
struct Harness {
    live: Session,
    history: Vec<Op>,
    scratch: Snapshot,
}

/// A from-scratch compile of a document: per-element (volume, area) and per-view SVG.
#[derive(Clone)]
struct Snapshot {
    solids: BTreeMap<String, (f64, f64)>,
    svgs: BTreeMap<String, String>,
}

fn fresh(history: &[Op]) -> Snapshot {
    let mut s = Session::new();
    s.apply(history.to_vec());
    for v in VIEWS {
        s.add_view(v);
    }
    snapshot(&s)
}

fn snapshot(s: &Session) -> Snapshot {
    let mut solids = BTreeMap::new();
    for e in ELEMENTS {
        if let Some(sol) = s.solid(e) {
            solids.insert(e.to_string(), (sol.volume(), sol.area()));
        }
    }
    let mut svgs = BTreeMap::new();
    for v in VIEWS {
        let svg = s.svg(v).unwrap_or_else(|| panic!("svg({v}) is None for a registered view"));
        svgs.insert(v.to_string(), svg);
    }
    Snapshot { solids, svgs }
}

impl Harness {
    fn new() -> Harness {
        let history = fixture();
        let mut live = Session::new();
        let first = live.apply(history.clone());
        let created: BTreeSet<String> = first.solids.iter().cloned().collect();
        for e in ELEMENTS {
            assert!(created.contains(e), "initial apply did not compute the solid of {e}: {:?}", first.solids);
        }
        for v in VIEWS {
            live.add_view(v);
        }
        let scratch = fresh(&history);
        Harness { live, history, scratch }
    }

    /// Applies `ops` to the live session; returns the reported recomputed solids and views, and
    /// the from-scratch snapshots before and after the edit.
    fn apply(&mut self, ops: Vec<Op>) -> (Vec<String>, Vec<String>, Snapshot, Snapshot) {
        let before = self.scratch.clone();
        self.history.extend(ops.iter().cloned());
        let stats = self.live.apply(ops);
        let after = fresh(&self.history);
        self.scratch = after.clone();
        (stats.solids, stats.views, before, after)
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// Element ids drawn in an SVG (`data-entity` attributes, spec/cli.md).
fn drawn(svg: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let pat = "data-entity=\"";
    let mut rest = svg;
    while let Some(i) = rest.find(pat) {
        rest = &rest[i + pat.len()..];
        let end = rest.find('"').expect("unterminated data-entity attribute");
        out.insert(rest[..end].to_string());
        rest = &rest[end..];
    }
    out
}

/// SVG compared as a multiset of tags, so element order is not part of the contract.
fn normalized(svg: &str) -> Vec<String> {
    let mut tags: Vec<String> = svg.split('<').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    tags.sort();
    tags
}

fn set_of(v: &[String]) -> BTreeSet<String> {
    v.iter().cloned().collect()
}

fn names(xs: &[&str]) -> BTreeSet<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

/// Soundness and minimality that hold for every edit:
/// - every element whose from-scratch solid changed is in `solids`;
/// - every view whose from-scratch drawing changed is in `views`;
/// - a recomputed view draws (before or after the edit) at least one recomputed element.
/// Then checks the live session equals the from-scratch compile.
fn check_bounds(label: &str, h: &Harness, so: &[String], vi: &[String], before: &Snapshot, after: &Snapshot) {
    let solids = set_of(so);
    let views = set_of(vi);
    for (e, (v1, a1)) in &after.solids {
        let changed = match before.solids.get(e) {
            Some((v0, a0)) => !close(*v0, *v1) || !close(*a0, *a1),
            None => true,
        };
        if changed {
            assert!(solids.contains(e), "{label}: solid of {e} changed but was not reported as recomputed ({solids:?})");
        }
    }
    for v in &views {
        assert!(VIEWS.contains(&v.as_str()), "{label}: reported view {v:?} was never registered");
    }
    for v in VIEWS {
        let (b, a) = (&before.svgs[v], &after.svgs[v]);
        if normalized(b) != normalized(a) {
            assert!(views.contains(v), "{label}: drawing {v} changed but was not reported as recomputed ({views:?})");
        }
        if views.contains(v) {
            let shows: BTreeSet<String> = drawn(b).union(&drawn(a)).cloned().collect();
            assert!(
                shows.iter().any(|e| solids.contains(e)),
                "{label}: view {v} was recomputed but shows none of the recomputed elements {solids:?} (it shows {shows:?})"
            );
        }
    }
    assert_matches_scratch(label, &h.live, after);
}

fn assert_matches_scratch(label: &str, live: &Session, scratch: &Snapshot) {
    let mine = snapshot(live);
    assert_eq!(
        mine.solids.keys().collect::<Vec<_>>(),
        scratch.solids.keys().collect::<Vec<_>>(),
        "{label}: incremental and from-scratch sessions have different element solids"
    );
    for (e, (v, a)) in &scratch.solids {
        let (mv, ma) = mine.solids[e];
        assert!(close(mv, *v) && close(ma, *a), "{label}: {e} incremental (V={mv}, A={ma}) != from-scratch (V={v}, A={a})");
    }
    for v in VIEWS {
        assert!(
            normalized(&mine.svgs[v]) == normalized(&scratch.svgs[v]),
            "{label}: incremental drawing {v} differs from the from-scratch drawing"
        );
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn fixture_sanity_from_scratch() {
    // The fixture must produce what the targeted assertions below rely on.
    let s = fresh(&fixture());
    for e in ELEMENTS {
        let (v, _) = s.solids.get(e).copied().unwrap_or_else(|| panic!("no solid for {e}"));
        assert!(v > 0.0, "{e} has non-positive volume {v}");
    }
    // Isolated wall WI: 6 m x 0.2 m x 3 m, no joins, no openings (closed form).
    assert!(close(s.solids["WI"].0, 6.0 * 0.2 * 3.0), "WI volume {} != 3.6", s.solids["WI"].0);
    // Column K1: 0.3 x 0.3 from L1 (0) to L2 (4).
    assert!(close(s.solids["K1"].0, 0.3 * 0.3 * 4.0), "K1 volume {} != 0.36", s.solids["K1"].0);
    let l2_cluster = names(&["WE", "OE", "S2", "BM", "K1"]);
    let section_l2 = drawn(&s.svgs["section:30,-15,-5,-15"]);
    assert!(section_l2.contains("WE"), "section looking at the L2 cluster does not draw WE: {section_l2:?}");
    assert!(
        section_l2.iter().all(|e| l2_cluster.contains(e)),
        "section looking -y from y=-15 draws entities behind its plane: {section_l2:?}"
    );
    let section_l1 = drawn(&s.svgs["section:-5,-1,30,-1"]);
    assert!(section_l1.contains("WA"), "section looking at the L1 cluster does not draw WA: {section_l1:?}");
    assert!(
        section_l1.iter().all(|e| !l2_cluster.contains(e)),
        "section looking +y from y=-1 draws L2-cluster entities behind its plane: {section_l1:?}"
    );
    assert!(drawn(&s.svgs["plan:L1@1.2"]).contains("WA"), "plan:L1@1.2 does not draw WA");
}

#[test]
fn wall_height_recomputes_wall_joined_walls_and_openings() {
    let mut h = Harness::new();
    let (so, vi, b, a) = h.apply(vec![set("WA", "height", num(3.5))]);
    // WA, its L-joined neighbours WB and WD, the T-joined WT, and its opening OA. Nothing else.
    assert_eq!(set_of(&so), names(&["WA", "WB", "WD", "WT", "OA"]), "height edit on WA");
    assert!(!set_of(&vi).contains("section:30,-15,-5,-15"), "WA edit recomputed the L2-cluster section");
    check_bounds("WA height", &h, &so, &vi, &b, &a);

    // The abutting wall of a T-join: WT and the through wall WA.
    let (so, vi, b, a) = h.apply(vec![set("WT", "height", num(2.5))]);
    assert_eq!(set_of(&so), names(&["WT", "WA"]), "height edit on WT");
    check_bounds("WT height", &h, &so, &vi, &b, &a);

    // An isolated wall without joins or openings recomputes alone, and only views drawing it.
    let (so, vi, b, a) = h.apply(vec![set("WI", "height", num(3.3))]);
    assert_eq!(set_of(&so), names(&["WI"]), "height edit on isolated WI");
    let views = set_of(&vi);
    assert!(views.contains("section:-5,-1,30,-1"), "WI height change must redraw the section that shows it");
    assert!(!views.contains("section:30,-15,-5,-15"), "WI edit recomputed the L2-cluster section");
    check_bounds("WI height", &h, &so, &vi, &b, &a);
}

#[test]
fn level_elevation_recomputes_elements_on_that_level() {
    let mut h = Harness::new();
    let (so, vi, b, a) = h.apply(vec![set("L2", "elevation", num(4.5))]);
    let solids = set_of(&so);
    // Elements on L2: WE, S2, BM, and K1 (its top is L2). OE (opening hosted on WE) may re-run.
    let must = names(&["WE", "S2", "BM", "K1"]);
    let may = names(&["WE", "S2", "BM", "K1", "OE"]);
    assert!(must.is_subset(&solids), "L2 elevation edit: {solids:?} misses some of {must:?}");
    assert!(solids.is_subset(&may), "L2 elevation edit recomputed elements not on L2: {solids:?}");
    let views = set_of(&vi);
    assert!(views.contains("section:30,-15,-5,-15"), "L2 edit must redraw the L2-cluster section");
    assert!(!views.contains("section:-5,-1,30,-1"), "L2 edit recomputed the L1-cluster section");
    check_bounds("L2 elevation", &h, &so, &vi, &b, &a);

    let (so, vi, b, a) = h.apply(vec![set("L1", "elevation", num(-0.5))]);
    let solids = set_of(&so);
    let must = names(&["WA", "WB", "WC", "WD", "WT", "WI", "S1", "K1"]);
    let may = names(&["WA", "WB", "WC", "WD", "WT", "WI", "S1", "K1", "OA", "OC"]);
    assert!(must.is_subset(&solids), "L1 elevation edit: {solids:?} misses some of {must:?}");
    assert!(solids.is_subset(&may), "L1 elevation edit recomputed elements not on L1: {solids:?}");
    check_bounds("L1 elevation", &h, &so, &vi, &b, &a);
}

#[test]
fn no_op_edit_recomputes_nothing() {
    let mut h = Harness::new();
    let (so, vi, b, a) = h.apply(vec![set("WA", "height", num(3.0))]);
    assert!(so.is_empty() && vi.is_empty(), "no-op edit recomputed {:?} / {:?}", so, vi);
    check_bounds("no-op", &h, &so, &vi, &b, &a);
    let (so, vi, _, _) = h.apply(vec![set("L2", "elevation", num(4.0)), set("OA", "width", num(0.9))]);
    assert!(so.is_empty() && vi.is_empty(), "no-op edits recomputed {:?} / {:?}", so, vi);
    let (so, vi, _, _) = h.apply(vec![]);
    assert!(so.is_empty() && vi.is_empty(), "empty apply recomputed {:?} / {:?}", so, vi);
}

#[test]
fn opening_edit_reaches_its_host() {
    let mut h = Harness::new();
    let (so, vi, b, a) = h.apply(vec![set("OC", "width", num(1.5))]);
    let solids = set_of(&so);
    assert!(solids.contains("OC") && solids.contains("WC"), "opening edit must recompute OC and host WC: {solids:?}");
    for e in ["WI", "WE", "S2", "K1", "BM", "S1"] {
        assert!(!solids.contains(e), "opening edit on WC recomputed unrelated {e}");
    }
    check_bounds("OC width", &h, &so, &vi, &b, &a);
}

/// Small deterministic PRNG (64-bit LCG, fixed seed) so the judge needs no external crates.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[(self.next() as usize) % xs.len()]
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        // Quantized to 1 cm so the values are exact in decimal and edits are never accidental no-ops.
        let steps = ((hi - lo) * 100.0).round() as u64;
        lo + ((self.next() % (steps + 1)) as f64) / 100.0
    }
}

#[test]
fn random_edit_sequence_matches_from_scratch() {
    let mut h = Harness::new();
    let mut rng = Lcg(0x5EED_3_1);
    let walls = ["WA", "WB", "WC", "WD", "WT", "WI", "WE"];
    let mut extra: Vec<String> = Vec::new();
    for step in 0..12 {
        let ops: Vec<Op> = match rng.next() % 7 {
            0 => vec![set(rng.pick(&walls), "height", num(rng.range(2.6, 3.6)))],
            1 => {
                // Keep L1 below L2: L1 in [-0.5, 0.5], L2 in [3.5, 4.5].
                if rng.next() % 2 == 0 {
                    vec![set("L1", "elevation", num(rng.range(-0.5, 0.5)))]
                } else {
                    vec![set("L2", "elevation", num(rng.range(3.5, 4.5)))]
                }
            }
            2 => vec![set(rng.pick(&["OA", "OC", "OE"]), "offset", num(rng.range(1.0, 1.8)))],
            3 => vec![set(rng.pick(&["S1", "S2"]), "thickness", num(rng.range(0.15, 0.3)))],
            4 => vec![set("K1", "section_b", num(rng.range(0.2, 0.4)))],
            5 => {
                let seed = format!("ON{step}");
                let mut o = Vec::new();
                opening(&mut o, &seed, rng.pick(&["WB", "WD", "WI"]), "window", 0.2 + 0.25 * step as f64, 1.0, 0.2, 0.6);
                extra.push(seed);
                o
            }
            _ => match extra.pop() {
                Some(seed) => vec![Op::Delete { id: id(&seed) }],
                None => vec![set("BM", "section_h", num(rng.range(0.3, 0.5)))],
            },
        };
        let (so, vi, b, a) = h.apply(ops);
        check_bounds(&format!("random step {step}"), &h, &so, &vi, &b, &a);
    }
}
