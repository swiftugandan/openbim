//! `bim-mep`: the MEP trade engine ([`MepEngine`], trade `mep`) — pipe and duct network
//! hydraulics.
//!
//! Every `MepSystem` (spec/schema.md) is a directed graph: its `Port`s are the nodes and the
//! `Segment`s between them the links. Sources hold their fixed `pressure`; sinks draw their `flow`
//! (non-source ports' `flow` is a demand, default 0). Each segment loses
//! `dp = (f·L/D + minor_k)·ρ·v²/2` (Darcy-Weisbach plus minor losses), signed with the flow, where
//! `L` is the distance between its ports' `x y z`, `D` the diameter (round) or the hydraulic
//! diameter `2wh/(w+h)` (rect, area `w·h`) and `f` is `64/Re` below Re 2000 and Colebrook-White
//! above ([`hydraulics`]). Each system is solved on its own ([`solver`]): by default with the
//! global gradient algorithm ([`Method::Newton`]), or with Hardy-Cross loop corrections
//! ([`Method::HardyCross`], `bim solve --method hardy-cross`). Elevation head is not modelled.
//!
//! The result is the `bim solve --trade mep --json` document of spec/cli.md:
//! `{"segments":[{"id","flow","velocity","dp","reynolds","friction_factor","size"}],
//! "ports":[{"id","pressure"}],"continuity_residual":r,
//! "circuits":[{"id","current","voltage_drop","voltage_drop_pct"}]}`, segments, ports and circuits
//! by name. `flow` is + from `from` to `to`, `dp` = p(from) − p(to), `velocity` = |flow|/area,
//! `ports[].pressure` the gauge static pressure (a source reports its fixed value), and
//! `continuity_residual` the largest |inflow − outflow − demand| over the non-source ports. At
//! zero flow a segment reports Re 0, friction factor 0 and dp 0. `size` is the recommended nominal
//! size ([`recommend_size`]).
//!
//! **Circuits** ([`circuits`]) are independent of the hydraulics: `current = Σ load_watts /
//! voltage`, `voltage_drop = current · 2 · resistivity · length / conductor_area` and
//! `voltage_drop_pct = 100 · voltage_drop / voltage`. A Port that only a Circuit refers to (as its
//! `panel` or one of its `loads`) and no Segment uses is not part of a hydraulic network: it is
//! neither validated nor reported. A `MepSystem` without Segments has nothing to solve (its ports
//! report their fixed pressure, else 0, and its demands count in `continuity_residual`).
//!
//! Invalid input is [`EngineError::Input`] (exit 2): a missing or wrong-kind `system`/`from`/`to`
//! Ref, a segment between two systems or from a port to itself, an unknown `medium`, `kind` or
//! `shape`, a non-positive or missing density, viscosity, diameter, width or height, a negative or
//! missing roughness, a negative minor_k, a source without `pressure`, a segment with no
//! resistance at all (zero length and no minor loss), a connected part of a network without a
//! source, or a Circuit whose `panel` or a load is not a Port, whose `loads` and `load_watts` are
//! not lists of the same length, whose `voltage` or `conductor_area` is not positive or whose
//! `resistivity` or `length` is negative or missing. A network whose iteration does not converge
//! is [`EngineError::Failed`] (exit 1); in particular one whose solution would put a segment
//! exactly at Re 2000, where the friction law jumps, fails naming the segments that keep
//! switching regime. A Port with no `system` that no segment uses is not part of any network and
//! not reported.

pub mod circuits;
pub mod hydraulics;
pub mod solver;

use std::collections::{HashMap, HashSet};

use bim_doc::{Entity, EntityId, Value};
use bim_plugin::{EngineContext, EngineError, TradeEngine};
use serde_json::{json, Map, Value as Json};

pub use hydraulics::{Fluid, Pipe, PipeState};
pub use solver::{Method, Network, Node, Solution, SolveError};

/// The `mep` trade engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct MepEngine {
    method: Method,
}

impl MepEngine {
    /// The engine with the default (Newton / global gradient) network solver.
    pub fn new() -> Self {
        MepEngine { method: Method::Newton }
    }

    /// The engine with network solver `method`.
    pub fn with_method(method: Method) -> Self {
        MepEngine { method }
    }

    /// The network solver this engine uses.
    pub fn method(&self) -> Method {
        self.method
    }
}

impl Method {
    /// The method named `name` on the command line: `newton` or `hardy-cross`.
    pub fn parse(name: &str) -> Option<Method> {
        match name {
            "newton" => Some(Method::Newton),
            "hardy-cross" => Some(Method::HardyCross),
            _ => None,
        }
    }
}

impl TradeEngine for MepEngine {
    fn name(&self) -> &str {
        "mep"
    }

    fn solve(&self, ctx: &EngineContext<'_>) -> Result<Json, EngineError> {
        solve_document(ctx, self.method)
    }
}

/// The medium of a system, for the sizing limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Medium {
    Water,
    Air,
}

/// A segment's cross-section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Round { diameter: f64 },
    Rect { width: f64, height: f64 },
}

impl Shape {
    /// Flow area and hydraulic diameter.
    pub fn area_dh(self) -> (f64, f64) {
        match self {
            Shape::Round { diameter } => (std::f64::consts::PI * diameter * diameter / 4.0, diameter),
            Shape::Rect { width, height } => (width * height, 2.0 * width * height / (width + height)),
        }
    }
}

/// Nominal round sizes (DN, inside diameter in mm).
pub const DN: [u32; 14] = [15, 20, 25, 32, 40, 50, 65, 80, 100, 125, 150, 200, 250, 300];

/// The recommended nominal size for a segment of `shape` carrying `flow` (spec/cli.md `size`):
/// round, `DN<n>` with the smallest DN keeping velocity ≤ 2.0 m/s (water) / 6.0 m/s (air) and the
/// Darcy-Weisbach friction gradient `f/D·ρv²/2` (Colebrook/64-Re at the segment's roughness)
/// ≤ 400 Pa/m (water) / 1.5 Pa/m (air), else the largest DN; rect, `<w>x<h>` in mm keeping the
/// authored height, the width the smallest multiple of 50 mm meeting the same limits.
pub fn recommend_size(shape: Shape, flow: f64, roughness: f64, fluid: &Fluid, medium: Medium) -> String {
    let (v_max, g_max) = match medium {
        Medium::Water => (2.0, 400.0),
        Medium::Air => (6.0, 1.5),
    };
    let fits = |s: Shape| {
        let (area, dh) = s.area_dh();
        let v = flow.abs() / area;
        let re = fluid.density * v * dh / fluid.viscosity;
        let (f, _) = hydraulics::friction_factor(re, roughness / dh);
        v <= v_max && f / dh * fluid.density * v * v / 2.0 <= g_max
    };
    match shape {
        Shape::Round { .. } => {
            let dn = DN
                .iter()
                .copied()
                .find(|&dn| fits(Shape::Round { diameter: f64::from(dn) / 1000.0 }))
                .unwrap_or(DN[DN.len() - 1]);
            format!("DN{dn}")
        }
        Shape::Rect { height, .. } => {
            // Velocity and gradient fall as the width grows; bound the search at 100 m.
            let k = (1..=2000u32).find(|&k| fits(Shape::Rect { width: f64::from(k) * 0.05, height })).unwrap_or(2000);
            format!("{}x{}", k * 50, mm(height))
        }
    }
}

/// `metres` in millimetres, as an integer when it is one (to 1e-6 mm).
fn mm(metres: f64) -> String {
    let x = metres * 1000.0;
    if (x - x.round()).abs() < 1e-6 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x}")
    }
}

/// A finite number as JSON, else null.
fn num(x: f64) -> Json {
    serde_json::Number::from_f64(x).map_or(Json::Null, Json::Number)
}

fn input(msg: String) -> EngineError {
    EngineError::Input(msg)
}

/// The numeric prop `key` of `e`: `Ok(None)` if absent, an error if present but not a finite number.
fn opt_num(name: &str, e: &Entity, key: &str) -> Result<Option<f64>, EngineError> {
    match e.props.get(key) {
        None => Ok(None),
        Some(Value::Num(x)) if x.is_finite() => Ok(Some(*x)),
        Some(_) => Err(input(format!("{} `{name}`: `{key}` must be a finite number", e.kind))),
    }
}

fn req_num(name: &str, e: &Entity, key: &str) -> Result<f64, EngineError> {
    opt_num(name, e, key)?.ok_or_else(|| input(format!("{} `{name}`: `{key}` is required", e.kind)))
}

fn positive(name: &str, e: &Entity, key: &str) -> Result<f64, EngineError> {
    let x = req_num(name, e, key)?;
    if x > 0.0 {
        Ok(x)
    } else {
        Err(input(format!("{} `{name}`: `{key}` must be positive, got {x}", e.kind)))
    }
}

/// Like [`req_num`], but zero is allowed.
fn non_negative(name: &str, e: &Entity, key: &str) -> Result<f64, EngineError> {
    let x = req_num(name, e, key)?;
    if x >= 0.0 {
        Ok(x)
    } else {
        Err(input(format!("{} `{name}`: `{key}` must not be negative, got {x}", e.kind)))
    }
}

fn text<'e>(name: &str, e: &'e Entity, key: &str) -> Result<&'e str, EngineError> {
    match e.props.get(key) {
        Some(Value::Text(s)) => Ok(s),
        None => Err(input(format!("{} `{name}`: `{key}` is required", e.kind))),
        Some(_) => Err(input(format!("{} `{name}`: `{key}` must be text", e.kind))),
    }
}

/// The target of the Ref prop `key` of `e`, which must be a live entity of kind `kind`.
fn reference(ctx: &EngineContext<'_>, name: &str, e: &Entity, key: &str, kind: &str) -> Result<EntityId, EngineError> {
    match e.props.get(key) {
        Some(Value::Ref(id)) => match ctx.document().get(*id) {
            Some(t) if t.kind == kind => Ok(*id),
            Some(t) => Err(input(format!("{} `{name}`: `{key}` refers to {} `{}`, not a {kind}", e.kind, t.kind, ctx.name(*id)))),
            None => Err(input(format!("{} `{name}`: `{key}` refers to missing entity `{}`", e.kind, ctx.name(*id)))),
        },
        None => Err(input(format!("{} `{name}`: `{key}` is required", e.kind))),
        Some(_) => Err(input(format!("{} `{name}`: `{key}` must be a Ref to a {kind}", e.kind))),
    }
}

/// The Refs in the List prop `key` of `e`, each of which must be a live entity of kind `kind`.
fn reference_list(ctx: &EngineContext<'_>, name: &str, e: &Entity, key: &str, kind: &str) -> Result<Vec<EntityId>, EngineError> {
    let Some(Value::List(items)) = e.props.get(key) else {
        return Err(input(format!("{} `{name}`: `{key}` must be a list of Refs to {kind}s", e.kind)));
    };
    items
        .iter()
        .map(|item| match item {
            Value::Ref(id) => match ctx.document().get(*id) {
                Some(t) if t.kind == kind => Ok(*id),
                Some(t) => Err(input(format!("{} `{name}`: `{key}` holds {} `{}`, not a {kind}", e.kind, t.kind, ctx.name(*id)))),
                None => Err(input(format!("{} `{name}`: `{key}` holds missing entity `{}`", e.kind, ctx.name(*id)))),
            },
            _ => Err(input(format!("{} `{name}`: `{key}` must be a list of Refs to {kind}s", e.kind))),
        })
        .collect()
}

/// The finite numbers in the List prop `key` of `e`.
fn number_list(name: &str, e: &Entity, key: &str) -> Result<Vec<f64>, EngineError> {
    let Some(Value::List(items)) = e.props.get(key) else {
        return Err(input(format!("{} `{name}`: `{key}` must be a list of numbers", e.kind)));
    };
    items
        .iter()
        .map(|item| match item {
            Value::Num(x) if x.is_finite() => Ok(*x),
            _ => Err(input(format!("{} `{name}`: `{key}` must be a list of finite numbers", e.kind))),
        })
        .collect()
}

/// Every Port that a Circuit refers to as its `panel` or among its `loads` (whatever else is wrong
/// with the circuit is reported when it is solved).
fn circuit_ports(ents: &[(String, &Entity)]) -> HashSet<EntityId> {
    let mut ports = HashSet::new();
    for (_, e) in ents.iter().filter(|(_, e)| e.kind == "Circuit") {
        if let Some(Value::Ref(id)) = e.props.get("panel") {
            ports.insert(*id);
        }
        if let Some(Value::List(loads)) = e.props.get("loads") {
            ports.extend(loads.iter().filter_map(|v| if let Value::Ref(id) = v { Some(*id) } else { None }));
        }
    }
    ports
}

/// The solved `circuits` array: every `Circuit`, by name.
fn solve_circuits(ctx: &EngineContext<'_>, ents: &[(String, &Entity)]) -> Result<Vec<Json>, EngineError> {
    let mut out = Vec::new();
    for (name, e) in ents.iter().filter(|(_, e)| e.kind == "Circuit") {
        reference(ctx, name, e, "panel", "Port")?;
        let loads = reference_list(ctx, name, e, "loads", "Port")?;
        let watts = number_list(name, e, "load_watts")?;
        if watts.len() != loads.len() {
            return Err(input(format!(
                "Circuit `{name}`: `load_watts` has {} entries but `loads` has {}",
                watts.len(),
                loads.len()
            )));
        }
        let voltage = positive(name, e, "voltage")?;
        let area = positive(name, e, "conductor_area")?;
        let resistivity = non_negative(name, e, "resistivity")?;
        let length = non_negative(name, e, "length")?;
        let r = circuits::circuit(&watts, voltage, area, resistivity, length);
        out.push(json!({
            "id": name,
            "current": num(r.current),
            "voltage_drop": num(r.voltage_drop),
            "voltage_drop_pct": num(r.voltage_drop_pct),
        }));
    }
    Ok(out)
}

struct System {
    fluid: Fluid,
    medium: Medium,
    /// Port indices (into `ports`) of this system, by name.
    ports: Vec<usize>,
    /// Segment indices (into `segments`) of this system, by name.
    segments: Vec<usize>,
}

struct PortIn {
    name: String,
    xyz: [f64; 3],
    fixed: Option<f64>,
    demand: f64,
}

struct SegmentIn {
    name: String,
    from: usize,
    to: usize,
    shape: Shape,
    roughness: f64,
    k: f64,
    /// Distance between the two ports (m).
    length: f64,
}

impl SegmentIn {
    /// The segment as a pipe between nodes `a` and `b`.
    fn pipe(&self, a: usize, b: usize) -> Pipe {
        let (area, dh) = self.shape.area_dh();
        Pipe { a, b, area, dh, length: self.length, roughness: self.roughness, k: self.k }
    }
}

fn solve_document(ctx: &EngineContext<'_>, method: Method) -> Result<Json, EngineError> {
    let ents = ctx.entities();
    // Systems.
    let mut systems: Vec<System> = Vec::new();
    let mut system_index: HashMap<EntityId, usize> = HashMap::new();
    for (name, e) in ents.iter().filter(|(_, e)| e.kind == "MepSystem") {
        let medium = match text(name, e, "medium")? {
            "water" => Medium::Water,
            "air" => Medium::Air,
            other => return Err(input(format!("MepSystem `{name}`: unknown medium `{other}` (water, air)"))),
        };
        let fluid = Fluid { density: positive(name, e, "density")?, viscosity: positive(name, e, "viscosity")? };
        system_index.insert(e.id, systems.len());
        systems.push(System { fluid, medium, ports: Vec::new(), segments: Vec::new() });
    }
    // Segments reference ports: a port without `system` is only an error if a segment uses it.
    let used: HashSet<EntityId> = ents
        .iter()
        .filter(|(_, e)| e.kind == "Segment")
        .flat_map(|(_, e)| ["from", "to"].into_iter().filter_map(|k| match e.props.get(k) {
            Some(Value::Ref(id)) => Some(*id),
            _ => None,
        }))
        .collect();
    // A port that only a circuit refers to is electrical, not part of a hydraulic network.
    let electrical = circuit_ports(&ents);
    // Ports.
    let mut ports: Vec<PortIn> = Vec::new();
    let mut port_index: HashMap<EntityId, (usize, usize)> = HashMap::new(); // id -> (port, system)
    for (name, e) in ents.iter().filter(|(_, e)| e.kind == "Port") {
        if !used.contains(&e.id) && (!e.props.contains_key("system") || electrical.contains(&e.id)) {
            continue;
        }
        let sys = system_index[&reference(ctx, name, e, "system", "MepSystem")?];
        let xyz = [req_num(name, e, "x")?, req_num(name, e, "y")?, req_num(name, e, "z")?];
        let (fixed, demand) = match text(name, e, "kind")? {
            "source" => {
                let p = opt_num(name, e, "pressure")?
                    .ok_or_else(|| input(format!("Port `{name}`: a source needs a `pressure`")))?;
                (Some(p), 0.0)
            }
            "sink" | "junction" => (None, opt_num(name, e, "flow")?.unwrap_or(0.0)),
            other => return Err(input(format!("Port `{name}`: unknown kind `{other}` (source, sink, junction)"))),
        };
        port_index.insert(e.id, (ports.len(), sys));
        systems[sys].ports.push(ports.len());
        ports.push(PortIn { name: name.clone(), xyz, fixed, demand });
    }
    // Segments.
    let mut segments: Vec<SegmentIn> = Vec::new();
    for (name, e) in ents.iter().filter(|(_, e)| e.kind == "Segment") {
        let (from, sys_a) = port_index[&reference(ctx, name, e, "from", "Port")?];
        let (to, sys_b) = port_index[&reference(ctx, name, e, "to", "Port")?];
        if sys_a != sys_b {
            return Err(input(format!("Segment `{name}`: connects ports of two different systems")));
        }
        if from == to {
            return Err(input(format!("Segment `{name}`: runs from a port to itself")));
        }
        let shape = match text(name, e, "shape")? {
            "round" => Shape::Round { diameter: positive(name, e, "diameter")? },
            "rect" => Shape::Rect { width: positive(name, e, "width")?, height: positive(name, e, "height")? },
            other => return Err(input(format!("Segment `{name}`: unknown shape `{other}` (round, rect)"))),
        };
        let roughness = req_num(name, e, "roughness")?;
        let k = opt_num(name, e, "minor_k")?.unwrap_or(0.0);
        if roughness < 0.0 || k < 0.0 {
            return Err(input(format!("Segment `{name}`: roughness and minor_k must not be negative")));
        }
        let (pa, pb) = (ports[from].xyz, ports[to].xyz);
        let length = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2) + (pa[2] - pb[2]).powi(2)).sqrt();
        if length == 0.0 && k == 0.0 {
            return Err(input(format!("Segment `{name}`: zero length and no minor loss (no resistance)")));
        }
        systems[sys_a].segments.push(segments.len());
        segments.push(SegmentIn { name: name.clone(), from, to, shape, roughness, k, length });
    }

    // Solve every system.
    let mut flow = vec![0.0; segments.len()];
    let mut pressure = vec![0.0; ports.len()];
    let mut system_of = vec![0; segments.len()];
    for (si, sys) in systems.iter().enumerate() {
        if sys.segments.is_empty() {
            // Nothing connects the ports: nothing flows and only a source has a pressure to report.
            for &p in &sys.ports {
                pressure[p] = ports[p].fixed.unwrap_or(0.0);
            }
            continue;
        }
        let local: HashMap<usize, usize> = sys.ports.iter().enumerate().map(|(k, &p)| (p, k)).collect();
        let nodes = sys.ports.iter().map(|&p| Node { fixed: ports[p].fixed, demand: ports[p].demand }).collect();
        let pipes = sys.segments.iter().map(|&s| segments[s].pipe(local[&segments[s].from], local[&segments[s].to])).collect();
        for &s in &sys.segments {
            system_of[s] = si;
        }
        let net = Network { fluid: sys.fluid, nodes, pipes };
        let sol = solver::solve(&net, method).map_err(|e| match e {
            SolveError::NoSource(k) => input(format!(
                "Port `{}`: its part of the network has no source (a fixed pressure)",
                ports[sys.ports[k]].name
            )),
            SolveError::Transition(pipes) => {
                let names: Vec<&str> = pipes.iter().map(|&k| segments[sys.segments[k]].name.as_str()).collect();
                EngineError::Failed(format!(
                    "no solution: segment(s) {} would sit on the laminar/turbulent jump of the friction factor at Re 2000",
                    names.join(", ")
                ))
            }
            SolveError::NotConverged(why) => EngineError::Failed(why),
        })?;
        for (k, &s) in sys.segments.iter().enumerate() {
            flow[s] = sol.flows[k];
        }
        for (k, &p) in sys.ports.iter().enumerate() {
            pressure[p] = sol.pressures[k];
        }
    }

    // Continuity at every non-source port, from the reported flows.
    let mut net_in: Vec<f64> = ports.iter().map(|p| -p.demand).collect();
    for (s, seg) in segments.iter().enumerate() {
        net_in[seg.from] -= flow[s];
        net_in[seg.to] += flow[s];
    }
    let residual = ports.iter().zip(&net_in).filter(|(p, _)| p.fixed.is_none()).map(|(_, r)| r.abs()).fold(0.0, f64::max);

    let mut seg_json = Vec::with_capacity(segments.len());
    for (s, seg) in segments.iter().enumerate() {
        let System { fluid, medium, .. } = systems[system_of[s]];
        let st = seg.pipe(seg.from, seg.to).state(&fluid, flow[s]);
        seg_json.push(json!({
            "id": seg.name,
            "flow": num(flow[s]),
            "velocity": num(st.velocity),
            "dp": num(st.dp),
            "reynolds": num(st.reynolds),
            "friction_factor": num(st.friction_factor),
            "size": recommend_size(seg.shape, flow[s], seg.roughness, &fluid, medium),
        }));
    }
    let port_json: Vec<Json> = ports.iter().zip(&pressure).map(|(p, &pr)| json!({"id": p.name, "pressure": num(pr)})).collect();
    let mut out = Map::new();
    out.insert("segments".into(), Json::Array(seg_json));
    out.insert("ports".into(), Json::Array(port_json));
    out.insert("continuity_residual".into(), num(residual));
    out.insert("circuits".into(), Json::Array(solve_circuits(ctx, &ents)?));
    Ok(Json::Object(out))
}

#[cfg(test)]
mod tests;
