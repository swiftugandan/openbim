//! The `struct` trade engine: analytical model extraction from the document, tributary
//! distribution of area loads, linear analysis with [`Frame3`] and the `solve --trade struct`
//! output of spec/cli.md.
//!
//! # Model extraction
//!
//! * Members: every `Column` (axis from `(x, y, base elevation)` to `(x, y, top elevation)`,
//!   reference vector +y) and every `Beam` (axis from `(x1, y1)` to `(x2, y2)` at level
//!   elevation − `z_offset` − `section_h`/2, reference vector +z), in the order of
//!   [`EngineContext::entities`] (by id). `section_b` lies along local y, `section_h` along
//!   local z: `A = b·h`, `Iy = b·h³/12`, `Iz = h·b³/12`, `Wy = b·h²/6`, `Wz = h·b²/6`,
//!   `J = a·c³·(1/3 − 0.21·(c/a)·(1 − c⁴/(12·a⁴)))` with `a ≥ c` the sides; Euler–Bernoulli
//!   (no shear areas); no self-weight.
//! * Nodes: member ends, each merged into the first node created within 1e-6 (3D); ids `n1, n2,
//!   …` in that order of first appearance (start before end).
//! * Splitting: a member is split at every node lying on its axis (within 1e-6, more than 1e-6
//!   from both ends); the pieces are `<id>#1 … <id>#k` from its start. Members that merely cross
//!   are not connected.
//! * Supports: the base nodes of the columns whose base is the lowest column base elevation
//!   (within 1e-6) are fully fixed. A `Support` applies to the node within 1e-6 of its point and
//!   replaces that node's restraints with its six flags (several Supports on one node: a degree
//!   of freedom is restrained if any of them restrains it; a missing flag counts as `false`). A
//!   Support with no node within 1e-6 is an input error.
//! * `PointLoad`: all six components at the node nearest to its point (ties: the first node).
//! * `AreaLoad`: on its host slab's net plan region (boundary minus `SlabOpening`s), each point
//!   goes to its nearest supporting beam, i.e. the nearest analytical piece of a Beam on the
//!   slab's Level (see the `tributary` module). The load reaches the beam as the distributed
//!   load `pressure · width(s)` (−z), `width(s)` being the width of its nearest-beam region on
//!   the perpendicular through station `s`; points nearest to a beam end load that node. An
//!   AreaLoad on a slab whose Level has no Beam, or whose boundary is not a polygon, is an
//!   input error.
//! * Combinations: one per `LoadCombination` (`name`; `cases` and `factors` zipped, equal lengths
//!   required, duplicate names rejected), in entity order. Cases in no combination are ignored.
//!   Without any LoadCombination the envelopes and displacements are zero and there are no
//!   reaction rows.
//!
//! # Analysis and output
//!
//! Span loads enter [`Frame3`] as consistent nodal loads; the member end forces are then
//! corrected by them, and the internal actions at the [`ENVELOPE_STATIONS`] stations include the
//! span load (the T4.4 convention: actions of the part beyond the station on the part from the
//! start, tension positive). The deflection adds the clamped-beam deflection under the span load
//! to the end-displacement field, relative to the chord. `utilization` is the maximum over
//! stations and combinations of `|N|/(A·fy) + |My|/(Wy·fy) + |Mz|/(Wz·fy)`.
//! `tributary[]` lists every analytical piece of a Beam on a Level that has a Slab, with its
//! area summed over those slabs (strip plus credited end regions).

use std::collections::HashMap;

use bim_doc::{Entity, EntityId, Value};
use bim_plugin::{EngineContext, EngineError, TradeEngine};
use serde_json::{json, Value as Json};

use crate::frame3d::{Frame3, LoadCombination, Section3, ENVELOPE_STATIONS};
use crate::span;
use crate::tributary::{self, Segment, P2};

/// Node merge, split and Support matching tolerance (m).
const TOL: f64 = 1e-6;

type V3 = [f64; 3];
type Mat3 = [V3; 3];

/// The `struct` trade engine (spec/cli.md `solve --trade struct`).
#[derive(Debug, Clone, Copy, Default)]
pub struct StructEngine;

impl StructEngine {
    /// The engine.
    pub fn new() -> Self {
        StructEngine
    }
}

impl TradeEngine for StructEngine {
    fn name(&self) -> &str {
        "struct"
    }

    fn solve(&self, ctx: &EngineContext<'_>) -> Result<Json, EngineError> {
        solve(ctx)
    }
}

fn input(msg: String) -> EngineError {
    EngineError::Input(format!("struct: {msg}"))
}

/// Typed prop access with messages naming the entity.
struct Props<'c, 'a> {
    ctx: &'c EngineContext<'a>,
    name: &'c str,
    e: &'a Entity,
}

impl<'c, 'a> Props<'c, 'a> {
    fn get(&self, key: &str) -> Option<&'a Value> {
        self.e.props.get(key)
    }

    fn num(&self, key: &str) -> Result<f64, EngineError> {
        match self.get(key) {
            Some(Value::Num(x)) if x.is_finite() => Ok(*x),
            Some(_) => Err(input(format!("{} {}: `{key}` must be a finite number", self.e.kind, self.name))),
            None => Err(input(format!("{} {}: missing `{key}`", self.e.kind, self.name))),
        }
    }

    fn num_or(&self, key: &str, default: f64) -> Result<f64, EngineError> {
        if self.get(key).is_none() {
            Ok(default)
        } else {
            self.num(key)
        }
    }

    fn positive(&self, key: &str) -> Result<f64, EngineError> {
        let v = self.num(key)?;
        if v > 0.0 {
            Ok(v)
        } else {
            Err(input(format!("{} {}: `{key}` must be positive (got {v})", self.e.kind, self.name)))
        }
    }

    fn flag(&self, key: &str) -> Result<bool, EngineError> {
        match self.get(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(input(format!("{} {}: `{key}` must be a Bool", self.e.kind, self.name))),
        }
    }

    fn text(&self, key: &str) -> Result<String, EngineError> {
        match self.get(key) {
            Some(Value::Text(s)) => Ok(s.clone()),
            Some(_) => Err(input(format!("{} {}: `{key}` must be Text", self.e.kind, self.name))),
            None => Err(input(format!("{} {}: missing `{key}`", self.e.kind, self.name))),
        }
    }

    fn list(&self, key: &str) -> Result<&'a [Value], EngineError> {
        match self.get(key) {
            Some(Value::List(v)) => Ok(v),
            Some(_) => Err(input(format!("{} {}: `{key}` must be a List", self.e.kind, self.name))),
            None => Err(input(format!("{} {}: missing `{key}`", self.e.kind, self.name))),
        }
    }

    fn nums(&self, key: &str) -> Result<Vec<f64>, EngineError> {
        self.list(key)?
            .iter()
            .map(|v| match v {
                Value::Num(x) if x.is_finite() => Ok(*x),
                _ => Err(input(format!("{} {}: `{key}` must hold finite numbers", self.e.kind, self.name))),
            })
            .collect()
    }

    fn texts(&self, key: &str) -> Result<Vec<String>, EngineError> {
        self.list(key)?
            .iter()
            .map(|v| match v {
                Value::Text(s) => Ok(s.clone()),
                _ => Err(input(format!("{} {}: `{key}` must hold Text", self.e.kind, self.name))),
            })
            .collect()
    }

    /// The live entity of kind `kind` that `key` references.
    fn target(&self, key: &str, kind: &str) -> Result<&'a Entity, EngineError> {
        let id = match self.get(key) {
            Some(Value::Ref(id)) => *id,
            Some(_) => return Err(input(format!("{} {}: `{key}` must be a Ref", self.e.kind, self.name))),
            None => return Err(input(format!("{} {}: missing `{key}`", self.e.kind, self.name))),
        };
        match self.ctx.document().get(id) {
            Some(t) if t.kind == kind => Ok(t),
            Some(t) => Err(input(format!(
                "{} {}: `{key}` references {} {}, not a {kind}",
                self.e.kind,
                self.name,
                t.kind,
                self.ctx.name(id)
            ))),
            None => Err(input(format!("{} {}: `{key}` references a missing entity", self.e.kind, self.name))),
        }
    }

    fn of<'n>(&self, e: &'a Entity, name: &'n str) -> Props<'n, 'a>
    where
        'c: 'n,
    {
        Props { ctx: self.ctx, name, e }
    }
}

/// A Column or Beam as authored.
struct Physical {
    id: String,
    start: V3,
    end: V3,
    b: f64,
    h: f64,
    e: f64,
    g: f64,
    fy: f64,
    reference: V3,
    /// The Beam's Level (None for a Column).
    level: Option<EntityId>,
    column: bool,
}

/// One analytical member.
struct Member {
    id: String,
    i: usize,
    j: usize,
    s: Section3,
    wy: f64,
    wz: f64,
    fy: f64,
    r: Mat3,
    length: f64,
    level: Option<EntityId>,
    /// Span loads: (case, local load per unit profile value, profile, local equivalent nodal loads).
    span: Vec<SpanLoad>,
    /// Tributary area (None when the member supports no slab).
    tributary: Option<f64>,
}

struct SpanLoad {
    case: String,
    local: V3,
    profile: Vec<(f64, f64)>,
    equivalent: [f64; 12],
}

fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot3(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dist3(a: V3, b: V3) -> f64 {
    let d = sub3(a, b);
    dot3(d, d).sqrt()
}

/// Rows x, y, z of the local axes (frame3d's convention).
fn axes(pi: V3, pj: V3, reference: V3) -> Option<(Mat3, f64)> {
    let d = sub3(pj, pi);
    let l = dot3(d, d).sqrt();
    if l == 0.0 {
        return None;
    }
    let x = [d[0] / l, d[1] / l, d[2] / l];
    let along = dot3(reference, x);
    let perp = [reference[0] - along * x[0], reference[1] - along * x[1], reference[2] - along * x[2]];
    let pn = dot3(perp, perp).sqrt();
    if pn <= 1e-6 * dot3(reference, reference).sqrt() {
        return None;
    }
    let z = [perp[0] / pn, perp[1] / pn, perp[2] / pn];
    Some(([x, cross3(z, x), z], l))
}

fn mul(r: &Mat3, v: V3) -> V3 {
    [dot3(r[0], v), dot3(r[1], v), dot3(r[2], v)]
}

fn mul_t(r: &Mat3, v: V3) -> V3 {
    [
        r[0][0] * v[0] + r[1][0] * v[1] + r[2][0] * v[2],
        r[0][1] * v[0] + r[1][1] * v[1] + r[2][1] * v[2],
        r[0][2] * v[0] + r[1][2] * v[1] + r[2][2] * v[2],
    ]
}

fn torsion_constant(b: f64, h: f64) -> f64 {
    let (a, c) = (b.max(h), b.min(h));
    a * c * c * c * (1.0 / 3.0 - 0.21 * (c / a) * (1.0 - c.powi(4) / (12.0 * a.powi(4))))
}

fn node_of(nodes: &mut Vec<V3>, p: V3) -> usize {
    if let Some(k) = nodes.iter().position(|q| dist3(*q, p) <= TOL) {
        return k;
    }
    nodes.push(p);
    nodes.len() - 1
}

fn polygon(values: &[f64], what: &str) -> Result<Vec<P2>, String> {
    if values.len() % 2 != 0 || values.len() < 6 {
        return Err(format!("{what}: `boundary` must hold at least three x, y pairs"));
    }
    Ok(values.chunks_exact(2).map(|c| [c[0], c[1]]).collect())
}

/// Replaces "node <k>" by the node id and "member <m>" by the member id in a solver message.
fn translate(msg: &str, members: &[Member]) -> String {
    let mut out = String::new();
    let mut rest = msg;
    loop {
        let next = ["node ", "member "].iter().filter_map(|w| rest.find(w).map(|p| (p, *w))).min();
        let Some((pos, word)) = next else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..pos + word.len()]);
        rest = &rest[pos + word.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let k: usize = rest[..digits].parse().unwrap_or(usize::MAX);
            if word == "node " {
                out.push_str(&format!("n{}", k.saturating_add(1)));
            } else if let Some(m) = members.get(k) {
                out.push_str(&m.id);
            } else {
                out.push_str(&rest[..digits]);
            }
            rest = &rest[digits..];
        }
    }
}

fn solve(ctx: &EngineContext<'_>) -> Result<Json, EngineError> {
    let entities = ctx.entities();
    let of_kind = |kind: &'static str| entities.iter().filter(move |(_, e)| e.kind == kind);

    // ---------------------------------------------------------------- members as authored
    let level_elevation = |p: &Props<'_, '_>, key: &str| -> Result<(EntityId, f64), EngineError> {
        let l = p.target(key, "Level")?;
        let name = ctx.name(l.id);
        Ok((l.id, p.of(l, &name).num("elevation")?))
    };
    let material = |p: &Props<'_, '_>| -> Result<(f64, f64, f64), EngineError> {
        let m = p.target("material", "Material")?;
        let name = ctx.name(m.id);
        let mp = p.of(m, &name);
        Ok((mp.positive("E")?, mp.positive("G")?, mp.positive("fy")?))
    };
    let mut physical: Vec<Physical> = Vec::new();
    for (name, e) in entities.iter().filter(|(_, e)| e.kind == "Column" || e.kind == "Beam") {
        let p = Props { ctx, name, e };
        let (b, h) = (p.positive("section_b")?, p.positive("section_h")?);
        let (em, gm, fy) = material(&p)?;
        let column = e.kind == "Column";
        let (start, end, reference, level) = if column {
            let (x, y) = (p.num("x")?, p.num("y")?);
            let (_, zb) = level_elevation(&p, "base")?;
            let (_, zt) = level_elevation(&p, "top")?;
            ([x, y, zb], [x, y, zt], [0.0, 1.0, 0.0], None)
        } else {
            let (lid, z0) = level_elevation(&p, "level")?;
            let z = z0 - p.num_or("z_offset", 0.0)? - h / 2.0;
            ([p.num("x1")?, p.num("y1")?, z], [p.num("x2")?, p.num("y2")?, z], [0.0, 0.0, 1.0], Some(lid))
        };
        physical.push(Physical { id: name.clone(), start, end, b, h, e: em, g: gm, fy, reference, level, column });
    }

    // ---------------------------------------------------------------- nodes and splitting
    let mut nodes: Vec<V3> = Vec::new();
    let ends: Vec<(usize, usize)> = physical
        .iter()
        .map(|m| {
            let i = node_of(&mut nodes, m.start);
            let j = node_of(&mut nodes, m.end);
            (i, j)
        })
        .collect();
    let mut members: Vec<Member> = Vec::new();
    for (m, &(i, j)) in physical.iter().zip(&ends) {
        if i == j {
            return Err(input(format!("{} has zero length (its ends merge within {TOL})", m.id)));
        }
        let (pi, pj) = (nodes[i], nodes[j]);
        let d = sub3(pj, pi);
        let l = dot3(d, d).sqrt();
        let mut inner: Vec<(f64, usize)> = Vec::new();
        for (k, &p) in nodes.iter().enumerate() {
            if k == i || k == j {
                continue;
            }
            let t = dot3(sub3(p, pi), d) / l;
            if t > TOL && t < l - TOL {
                let foot = [pi[0] + d[0] * t / l, pi[1] + d[1] * t / l, pi[2] + d[2] * t / l];
                if dist3(foot, p) <= TOL {
                    inner.push((t, k));
                }
            }
        }
        inner.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut chain = vec![i];
        chain.extend(inner.iter().map(|&(_, k)| k));
        chain.push(j);
        let k_total = chain.len() - 1;
        let (b, h) = (m.b, m.h);
        let s = Section3 {
            e: m.e,
            g: m.g,
            a: b * h,
            iy: b * h * h * h / 12.0,
            iz: h * b * b * b / 12.0,
            j: torsion_constant(b, h),
            asy: None,
            asz: None,
        };
        for (n, w) in chain.windows(2).enumerate() {
            let id = if k_total == 1 { m.id.clone() } else { format!("{}#{}", m.id, n + 1) };
            let (r, length) = axes(nodes[w[0]], nodes[w[1]], m.reference)
                .ok_or_else(|| input(format!("{id}: degenerate axis or reference vector")))?;
            members.push(Member {
                id,
                i: w[0],
                j: w[1],
                s,
                wy: b * h * h / 6.0,
                wz: h * b * b / 6.0,
                fy: m.fy,
                r,
                length,
                level: m.level,
                span: Vec::new(),
                tributary: None,
            });
        }
    }

    let mut frame = Frame3::new();
    for p in &nodes {
        frame.add_node(p[0], p[1], p[2]);
    }
    for m in &members {
        // The local z row is the member's reference direction made orthogonal to its axis.
        frame.add_member(m.i, m.j, m.s, m.r[2]);
    }

    // ---------------------------------------------------------------- supports
    let mut restraints: Vec<[bool; 6]> = vec![[false; 6]; nodes.len()];
    let base = physical.iter().filter(|m| m.column).map(|m| m.start[2]).fold(f64::INFINITY, f64::min);
    for (m, &(i, _)) in physical.iter().zip(&ends) {
        if m.column && (m.start[2] - base).abs() <= TOL {
            restraints[i] = [true; 6];
        }
    }
    let mut overrides: HashMap<usize, [bool; 6]> = HashMap::new();
    for (name, e) in of_kind("Support") {
        let p = Props { ctx, name, e };
        let pt = [p.num("x")?, p.num("y")?, p.num("z")?];
        let k = nodes
            .iter()
            .enumerate()
            .map(|(k, q)| (k, dist3(*q, pt)))
            .filter(|&(_, d)| d <= TOL)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k)
            .ok_or_else(|| input(format!("Support {name}: no analytical node within {TOL} of {pt:?}")))?;
        let mut flags = [false; 6];
        for (f, key) in flags.iter_mut().zip(["ux", "uy", "uz", "rx", "ry", "rz"]) {
            *f = p.flag(key)?;
        }
        let entry = overrides.entry(k).or_insert([false; 6]);
        for (a, b) in entry.iter_mut().zip(flags) {
            *a |= b;
        }
    }
    for (k, f) in overrides {
        restraints[k] = f;
    }
    for (k, r) in restraints.iter().enumerate() {
        if r.iter().any(|&b| b) {
            frame.fix(k, *r);
        }
    }

    // ---------------------------------------------------------------- point loads
    for (name, e) in of_kind("PointLoad") {
        let p = Props { ctx, name, e };
        let pt = [p.num("x")?, p.num("y")?, p.num("z")?];
        let mut f = [0.0; 6];
        for (v, key) in f.iter_mut().zip(["fx", "fy", "fz", "mx", "my", "mz"]) {
            *v = p.num_or(key, 0.0)?;
        }
        let case = p.text("case")?;
        let k = (0..nodes.len())
            .min_by(|&a, &b| dist3(nodes[a], pt).total_cmp(&dist3(nodes[b], pt)))
            .ok_or_else(|| input(format!("PointLoad {name}: the model has no analytical node")))?;
        frame.load(k, &case, f);
    }

    // ---------------------------------------------------------------- slabs and tributary areas
    struct SlabInfo {
        /// Supporting members (indices into `members`) and their partition.
        part: Result<Option<(Vec<usize>, tributary::Partition)>, String>,
    }
    let mut slabs: HashMap<EntityId, SlabInfo> = HashMap::new();
    for (name, e) in of_kind("Slab") {
        let p = Props { ctx, name, e };
        let part = (|| -> Result<Option<(Vec<usize>, tributary::Partition)>, EngineError> {
            let level = p.target("level", "Level")?.id;
            let outer = polygon(&p.nums("boundary")?, &format!("Slab {name}")).map_err(input)?;
            let mut holes = Vec::new();
            for (hn, he) in of_kind("SlabOpening") {
                if matches!(he.props.get("host"), Some(Value::Ref(h)) if *h == e.id) {
                    let hp = Props { ctx, name: hn, e: he };
                    holes.push(polygon(&hp.nums("boundary")?, &format!("SlabOpening {hn}")).map_err(input)?);
                }
            }
            let support: Vec<usize> = (0..members.len()).filter(|&k| members[k].level == Some(level)).collect();
            if support.is_empty() {
                return Ok(None);
            }
            let segs: Vec<Segment> = support
                .iter()
                .map(|&k| {
                    let (a, b) = (nodes[members[k].i], nodes[members[k].j]);
                    Segment { a: [a[0], a[1]], b: [b[0], b[1]] }
                })
                .collect();
            let part = tributary::partition(&outer, &holes, &segs);
            Ok(Some((support, part)))
        })();
        let part = part.map_err(|e| e.message().to_string());
        if let Ok(Some((support, part))) = &part {
            for (n, &k) in support.iter().enumerate() {
                *members[k].tributary.get_or_insert(0.0) += part.area(n);
            }
        }
        slabs.insert(e.id, SlabInfo { part });
    }

    // ---------------------------------------------------------------- area loads
    for (name, e) in of_kind("AreaLoad") {
        let p = Props { ctx, name, e };
        let slab = p.target("host", "Slab")?;
        let q = p.num("pressure")?;
        let case = p.text("case")?;
        let info = &slabs[&slab.id];
        let (support, part) = match &info.part {
            Err(msg) => return Err(EngineError::Input(msg.clone())),
            Ok(None) => {
                return Err(input(format!(
                    "AreaLoad {name}: slab {} has no supporting Beam on its Level",
                    ctx.name(slab.id)
                )))
            }
            Ok(Some(x)) => x,
        };
        let dir = [0.0, 0.0, -q];
        for (n, &k) in support.iter().enumerate() {
            let m = &mut members[k];
            let profile = &part.profiles[n];
            if profile.is_empty() {
                continue;
            }
            let c = mul(&m.r, dir);
            let w = span::nodal_weights(profile, m.length);
            let eq = [
                c[0] * w[0],
                c[1] * w[2],
                c[2] * w[2],
                0.0,
                -c[2] * w[3],
                c[1] * w[3],
                c[0] * w[1],
                c[1] * w[4],
                c[2] * w[4],
                0.0,
                -c[2] * w[5],
                c[1] * w[5],
            ];
            for (node, off) in [(m.i, 0), (m.j, 6)] {
                let f = mul_t(&m.r, [eq[off], eq[off + 1], eq[off + 2]]);
                let mm = mul_t(&m.r, [eq[off + 3], eq[off + 4], eq[off + 5]]);
                frame.load(node, &case, [f[0], f[1], f[2], mm[0], mm[1], mm[2]]);
            }
            m.span.push(SpanLoad { case: case.clone(), local: c, profile: profile.clone(), equivalent: eq });
        }
        for &(n, end, area) in &part.caps {
            let m = &members[support[n]];
            let node = if end == 0 { m.i } else { m.j };
            frame.load(node, &case, [0.0, 0.0, -q * area, 0.0, 0.0, 0.0]);
        }
    }

    // ---------------------------------------------------------------- combinations
    let mut combos: Vec<LoadCombination> = Vec::new();
    for (name, e) in of_kind("LoadCombination") {
        let p = Props { ctx, name, e };
        let cname = p.text("name")?;
        let cases = p.texts("cases")?;
        let factors = p.nums("factors")?;
        if cases.len() != factors.len() {
            return Err(input(format!(
                "LoadCombination {name}: {} cases but {} factors",
                cases.len(),
                factors.len()
            )));
        }
        if combos.iter().any(|c| c.name == cname) {
            return Err(input(format!("two LoadCombinations are named {cname:?}")));
        }
        let c = LoadCombination { name: cname, factors: cases.into_iter().zip(factors).collect() };
        frame.add_combination(c.clone());
        combos.push(c);
    }

    let results = frame
        .analyze()
        .map_err(|e| EngineError::Failed(format!("struct: analysis failed: {}", translate(&e, &members))))?;

    // ---------------------------------------------------------------- recovery
    let factor = |c: &LoadCombination, case: &str| -> f64 {
        c.factors.iter().filter(|(n, _)| n == case).map(|(_, f)| f).sum()
    };
    let stations = ENVELOPE_STATIONS;
    let mut member_rows = Vec::with_capacity(members.len());
    for (k, m) in members.iter().enumerate() {
        let l = m.length;
        let xs: Vec<f64> = (0..stations).map(|n| l * n as f64 / (stations - 1) as f64).collect();
        // Per span load and station: (∫w, ∫(x−s)w, clamped deflection).
        let pre: Vec<Vec<(f64, f64, f64)>> = m
            .span
            .iter()
            .map(|sl| {
                xs.iter()
                    .map(|&x| {
                        let (r0, r1) = span::resultants(&sl.profile, x);
                        (r0, r1, span::clamped_deflection(&sl.profile, l, x))
                    })
                    .collect()
            })
            .collect();
        let mut lo = [0.0f64; 6];
        let mut hi = [0.0f64; 6];
        let (mut deflection, mut util) = (0.0f64, 0.0f64);
        for (ci, c) in combos.iter().enumerate() {
            let mut f = results.member_end_forces(&c.name, k);
            let fac: Vec<f64> = m.span.iter().map(|sl| factor(c, &sl.case)).collect();
            for (sl, &a) in m.span.iter().zip(&fac) {
                for (v, e) in f.iter_mut().zip(sl.equivalent) {
                    *v -= a * e;
                }
            }
            let (di, dj) = (results.displacement(&c.name, m.i), results.displacement(&c.name, m.j));
            let (u1, t1) = (mul(&m.r, [di[0], di[1], di[2]]), mul(&m.r, [di[3], di[4], di[5]]));
            let (u2, t2) = (mul(&m.r, [dj[0], dj[1], dj[2]]), mul(&m.r, [dj[3], dj[4], dj[5]]));
            for (n, &x) in xs.iter().enumerate() {
                let mut act = [-f[0], -f[1], -f[2], -f[3], -f[4] - x * f[2], -f[5] + x * f[1]];
                let (mut vp, mut wp) = (0.0, 0.0);
                for ((sl, &a), p) in m.span.iter().zip(&fac).zip(&pre) {
                    let (r0, r1, cd) = p[n];
                    let c3 = [a * sl.local[0], a * sl.local[1], a * sl.local[2]];
                    act[0] -= c3[0] * r0;
                    act[1] -= c3[1] * r0;
                    act[2] -= c3[2] * r0;
                    act[4] -= c3[2] * r1;
                    act[5] += c3[1] * r1;
                    vp += c3[1] * cd;
                    wp += c3[2] * cd;
                }
                for q in 0..6 {
                    if ci == 0 && n == 0 {
                        lo[q] = act[q];
                        hi[q] = act[q];
                    } else {
                        lo[q] = lo[q].min(act[q]);
                        hi[q] = hi[q].max(act[q]);
                    }
                }
                util = util.max(
                    act[0].abs() / (m.s.a * m.fy) + act[4].abs() / (m.wy * m.fy) + act[5].abs() / (m.wz * m.fy),
                );
                // Transverse field relative to the chord: Hermite part plus the clamped span part.
                let xi = x / l;
                let (h2, h4) = (l * (xi - 2.0 * xi * xi + xi * xi * xi), l * (xi * xi * xi - xi * xi));
                let (h1, h3) = (1.0 - 3.0 * xi * xi + 2.0 * xi * xi * xi, 3.0 * xi * xi - 2.0 * xi * xi * xi);
                let v = (h1 - (1.0 - xi)) * u1[1] + (h3 - xi) * u2[1] + h2 * t1[2] + h4 * t2[2] + vp / (m.s.e * m.s.iz);
                let w = (h1 - (1.0 - xi)) * u1[2] + (h3 - xi) * u2[2] - h2 * t1[1] - h4 * t2[1] + wp / (m.s.e * m.s.iy);
                deflection = deflection.max(v.hypot(w));
            }
        }
        member_rows.push(json!({
            "id": m.id,
            "N": [lo[0], hi[0]],
            "Vy": [lo[1], hi[1]],
            "Vz": [lo[2], hi[2]],
            "T": [lo[3], hi[3]],
            "My": [lo[4], hi[4]],
            "Mz": [lo[5], hi[5]],
            "deflection": deflection,
            "utilization": util,
        }));
    }

    let first = combos.first().map(|c| c.name.as_str());
    let node_rows: Vec<Json> = nodes
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let d = first.map_or([0.0; 6], |c| results.displacement(c, k));
            json!({"id": format!("n{}", k + 1), "x": p[0], "y": p[1], "z": p[2],
                   "ux": d[0], "uy": d[1], "uz": d[2], "rx": d[3], "ry": d[4], "rz": d[5]})
        })
        .collect();
    let mut reaction_rows = Vec::new();
    for c in &combos {
        for (k, r) in restraints.iter().enumerate() {
            if r.iter().any(|&b| b) {
                let f = results.reaction(&c.name, k);
                reaction_rows.push(json!({"node": format!("n{}", k + 1), "fx": f[0], "fy": f[1], "fz": f[2],
                                          "mx": f[3], "my": f[4], "mz": f[5], "combination": c.name}));
            }
        }
    }
    let tributary_rows: Vec<Json> = members
        .iter()
        .filter_map(|m| m.tributary.map(|a| json!({"member": m.id, "area": a})))
        .collect();

    let out = json!({"nodes": node_rows, "members": member_rows, "reactions": reaction_rows, "tributary": tributary_rows});
    if !all_finite(&out) {
        return Err(EngineError::Failed(
            "struct: the analysis produced non-finite numbers (check units and magnitudes)".to_string(),
        ));
    }
    Ok(out)
}

fn all_finite(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Array(a) => a.iter().all(all_finite),
        Json::Object(o) => o.values().all(all_finite),
        _ => true,
    }
}
