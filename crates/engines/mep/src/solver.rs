//! Network solvers. A network is nodes (fixed-pressure sources, or nodes with a demand) and
//! [`Pipe`]s; the solution is the signed flow of every pipe and the pressure at every node such
//! that, at every non-source node, inflow − outflow = demand (continuity) and, for every pipe,
//! `p(a) − p(b) = dp(Q)` (energy).
//!
//! * [`Method::Newton`] — the global gradient algorithm (Todini–Pilati): Newton's method on flows
//!   and pressures together. Each iteration linearizes every pipe, `Q' = y + w·(p'_a − p'_b)`
//!   with `w = 1/(dp/dQ)` and `y = Q − w·dp(Q)`, substitutes that into continuity at the
//!   non-source nodes — an SPD weighted Laplacian in the new pressures, solved by dense Cholesky
//!   per connected component — and recovers the flows. Flows satisfy continuity to roundoff after
//!   every iteration; the iteration stops when the flow update is below 1e-12 of the flow scale.
//! * [`Method::HardyCross`] — the reference loop method: continuity-satisfying initial flows on a
//!   spanning tree rooted at a source, then Newton corrections of one loop flow at a time
//!   (Gauss-Seidel sweeps) over the fundamental cycles of the tree and one pseudo-loop per further
//!   source (the tree path from the root source to it, which must drop the difference of the two
//!   fixed pressures); pressures by a walk of the tree from the root.
//!
//! A component without a source fails ([`SolveError::NoSource`]) unless it is a lone node with
//! no demand (its pressure is reported as 0). The friction law jumps at Re 2000 (no transition
//! interpolation), so a network can have no solution: one that would need a pipe exactly on the
//! jump. Both methods then stall with that pipe switching regime and fail with
//! [`SolveError::Transition`] naming it.

use crate::hydraulics::{Fluid, Pipe};

/// The network solver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    /// Global gradient algorithm (Newton on flows and pressures).
    #[default]
    Newton,
    /// Hardy-Cross loop corrections.
    HardyCross,
}

/// A network node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    /// The fixed pressure of a source (Pa), `None` for every other node.
    pub fixed: Option<f64>,
    /// Flow leaving the network at this node (m³/s); ignored at a source.
    pub demand: f64,
}

/// A network to solve.
#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub fluid: Fluid,
    pub nodes: Vec<Node>,
    pub pipes: Vec<Pipe>,
}

/// A solved network.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    /// Signed flow per pipe, + from `a` to `b`.
    pub flows: Vec<f64>,
    /// Pressure per node.
    pub pressures: Vec<f64>,
}

/// Why a network has no solution.
#[derive(Debug, Clone, PartialEq)]
pub enum SolveError {
    /// The connected component containing this node has no source.
    NoSource(usize),
    /// The iteration stalled with these pipes (indices) switching between laminar and turbulent:
    /// the solution would put them exactly on the friction-factor jump at Re 2000, where the
    /// discontinuous law (64/Re below, Colebrook-White above) has no pressure drop that balances
    /// the network.
    Transition(Vec<usize>),
    /// The iteration did not converge for another reason.
    NotConverged(String),
}

/// Relative flow-update tolerance.
const TOL: f64 = 1e-12;
/// Below this relative update a stalled iteration counts as converged (roundoff floor).
const STALL_TOL: f64 = 1e-11;

/// Solves `net` with `method`.
pub fn solve(net: &Network, method: Method) -> Result<Solution, SolveError> {
    let n = net.nodes.len();
    let mut flows = vec![0.0; net.pipes.len()];
    let mut pressures = vec![0.0; n];
    for comp in components(n, &net.pipes) {
        let root = comp.nodes.iter().copied().find(|&i| net.nodes[i].fixed.is_some());
        let Some(root) = root else {
            if comp.pipes.is_empty() && net.nodes[comp.nodes[0]].demand == 0.0 {
                continue; // a lone node without demand: nothing flows, pressure 0
            }
            return Err(SolveError::NoSource(comp.nodes[0]));
        };
        match method {
            Method::Newton => newton(net, &comp, &mut flows, &mut pressures)?,
            Method::HardyCross => hardy_cross(net, &comp, root, &mut flows, &mut pressures)?,
        }
        for &i in &comp.nodes {
            if let Some(p) = net.nodes[i].fixed {
                pressures[i] = p;
            }
        }
    }
    Ok(Solution { flows, pressures })
}

/// A connected component: its nodes (ascending) and pipes (ascending).
struct Component {
    nodes: Vec<usize>,
    pipes: Vec<usize>,
}

fn components(n: usize, pipes: &[Pipe]) -> Vec<Component> {
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for p in pipes {
        let (ra, rb) = (find(&mut parent, p.a), find(&mut parent, p.b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    let mut index = vec![usize::MAX; n];
    let mut out: Vec<Component> = Vec::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        if index[r] == usize::MAX {
            index[r] = out.len();
            out.push(Component { nodes: Vec::new(), pipes: Vec::new() });
        }
        out[index[r]].nodes.push(i);
    }
    for (j, p) in pipes.iter().enumerate() {
        let r = find(&mut parent, p.a);
        out[index[r]].pipes.push(j);
    }
    out
}

/// The flow scale of a component: the largest of its flows and its total |demand|.
fn flow_scale(net: &Network, comp: &Component, flows: &[f64]) -> f64 {
    let demand: f64 = comp.nodes.iter().filter(|&&i| net.nodes[i].fixed.is_none()).map(|&i| net.nodes[i].demand.abs()).sum();
    comp.pipes.iter().map(|&j| flows[j].abs()).fold(demand, f64::max)
}

/// What an iteration does after an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Continue,
    Converged,
    /// No new smallest update for the whole stall window while still above [`STALL_TOL`].
    Stalled,
}

/// Tracks convergence of an iteration from its successive update sizes, and which pipes switched
/// between laminar and turbulent while it made no progress.
struct Convergence {
    best: f64,
    stalled: usize,
    /// Iterations without a new smallest update after which a stall above [`STALL_TOL`] is final.
    window: usize,
    /// Per pipe: was it laminar at the last update.
    laminar: Vec<bool>,
    /// Pipes that changed regime since the last new smallest update.
    flipped: Vec<bool>,
}

impl Convergence {
    fn new(window: usize, pipes: usize) -> Self {
        Convergence { best: f64::INFINITY, stalled: 0, window, laminar: vec![false; pipes], flipped: vec![false; pipes] }
    }

    /// Records the regime of `comp`'s pipes at `flows` and judges an update of relative size
    /// `rel`: converged below [`TOL`], or at the roundoff floor (below [`STALL_TOL`] without a new
    /// smallest update for `window`/2 iterations); stalled without a new smallest update for
    /// `window` iterations. Every deterministic contraction, however slow, keeps setting new minima.
    fn update(&mut self, net: &Network, comp: &Component, flows: &[f64], rel: f64) -> Verdict {
        for &j in &comp.pipes {
            let p = &net.pipes[j];
            let laminar = p.state(&net.fluid, flows[j]).reynolds < crate::hydraulics::RE_LAMINAR;
            self.flipped[j] |= laminar != self.laminar[j];
            self.laminar[j] = laminar;
        }
        if rel <= TOL {
            return Verdict::Converged;
        }
        if rel < self.best {
            self.best = rel;
            self.stalled = 0;
            self.flipped.iter_mut().for_each(|f| *f = false);
        } else {
            self.stalled += 1;
        }
        if self.best <= STALL_TOL && self.stalled >= self.window / 2 {
            Verdict::Converged
        } else if self.stalled >= self.window {
            Verdict::Stalled
        } else {
            Verdict::Continue
        }
    }

    /// The error for a stalled iteration.
    fn stall_error(&self, comp: &Component, what: &str) -> SolveError {
        let flipped: Vec<usize> = comp.pipes.iter().copied().filter(|&j| self.flipped[j]).collect();
        if flipped.is_empty() {
            SolveError::NotConverged(format!("the {what} iteration stalled"))
        } else {
            SolveError::Transition(flipped)
        }
    }
}

fn newton(net: &Network, comp: &Component, flows: &mut [f64], pressures: &mut [f64]) -> Result<(), SolveError> {
    // Unknown-pressure nodes of the component, indexed 0..m.
    let mut unknown = vec![usize::MAX; net.nodes.len()];
    let mut m = 0;
    for &i in &comp.nodes {
        if net.nodes[i].fixed.is_none() {
            unknown[i] = m;
            m += 1;
        }
    }
    // Start from 1 m/s in every pipe, in its authored direction.
    for &j in &comp.pipes {
        flows[j] = net.pipes[j].area;
    }
    let mut conv = Convergence::new(30, net.pipes.len());
    const MAX_ITER: usize = 300;
    for _ in 0..MAX_ITER {
        // Every step is a full Newton step: the flows satisfy continuity from the first one on.
        let (next, heads) = gga_step(net, comp, flows, &unknown, m)?;
        for &i in &comp.nodes {
            if unknown[i] != usize::MAX {
                pressures[i] = heads[unknown[i]];
            }
        }
        let mut delta: f64 = 0.0;
        for (k, &j) in comp.pipes.iter().enumerate() {
            delta = delta.max((next[k] - flows[j]).abs());
            flows[j] = next[k];
        }
        if !delta.is_finite() {
            return Err(SolveError::NotConverged("the Newton iteration diverged".to_string()));
        }
        let scale = flow_scale(net, comp, flows);
        if scale == 0.0 {
            return Ok(());
        }
        match conv.update(net, comp, flows, delta / scale) {
            Verdict::Converged => return Ok(()),
            Verdict::Stalled => return Err(conv.stall_error(comp, "Newton")),
            Verdict::Continue => {}
        }
    }
    Err(SolveError::NotConverged(format!("the Newton iteration did not converge in {MAX_ITER} iterations")))
}

/// One GGA step from `flows`: the new flows of the component's pipes (in `comp.pipes` order) and
/// the new pressures of its unknown nodes (indexed by `unknown`). Each pipe is linearized at its
/// flow, `Q' = y + w·(p_a − p_b)` with `w = 1/(dp/dQ)`, `y = Q − w·dp(Q)`; continuity at the `m`
/// unknown nodes in terms of the new pressures is an SPD weighted Laplacian (every component has a
/// source), solved by Cholesky. Pressures are assembled relative to a source's pressure, which
/// keeps the `w·p` terms (and so the roundoff in the new flows) small.
fn gga_step(
    net: &Network,
    comp: &Component,
    flows: &[f64],
    unknown: &[usize],
    m: usize,
) -> Result<(Vec<f64>, Vec<f64>), SolveError> {
    let datum = comp.nodes.iter().find_map(|&i| net.nodes[i].fixed).unwrap_or(0.0);
    let fixed = |i: usize| net.nodes[i].fixed.map(|p| p - datum);
    let mut a = vec![0.0; m * m];
    let mut rhs = vec![0.0; m];
    for &i in &comp.nodes {
        if unknown[i] != usize::MAX {
            rhs[unknown[i]] -= net.nodes[i].demand;
        }
    }
    let mut wy = Vec::with_capacity(comp.pipes.len());
    for &j in &comp.pipes {
        let p = &net.pipes[j];
        let (dp, slope) = p.dp_slope_regularized(&net.fluid, flows[j]);
        let (w, y) = (1.0 / slope, flows[j] - dp / slope);
        wy.push((w, y));
        // Continuity, inflow − outflow = demand: −Q' at `a`, +Q' at `b`.
        let (ua, ub) = (unknown[p.a], unknown[p.b]);
        if ua != usize::MAX {
            a[ua * m + ua] += w;
            rhs[ua] -= y;
            match fixed(p.b) {
                Some(pb) => rhs[ua] += w * pb,
                None => a[ua * m + ub] -= w,
            }
        }
        if ub != usize::MAX {
            a[ub * m + ub] += w;
            rhs[ub] += y;
            match fixed(p.a) {
                Some(pa) => rhs[ub] += w * pa,
                None => a[ub * m + ua] -= w,
            }
        }
    }
    if !cholesky_solve(&mut a, &mut rhs, m) {
        return Err(SolveError::NotConverged("singular network matrix".to_string()));
    }
    let head = |i: usize| fixed(i).unwrap_or_else(|| rhs[unknown[i]]);
    let next = comp
        .pipes
        .iter()
        .zip(&wy)
        .map(|(&j, &(w, y))| y + w * (head(net.pipes[j].a) - head(net.pipes[j].b)))
        .collect();
    Ok((next, rhs.into_iter().map(|h| h + datum).collect()))
}

/// Solves `a·x = b` in place (`x` left in `b`) for a symmetric positive definite `n×n` row-major
/// `a`; false if a pivot is not positive.
fn cholesky_solve(a: &mut [f64], b: &mut [f64], n: usize) -> bool {
    // a = L·Lᵀ, L stored in the lower triangle.
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if !(d > 0.0) || !d.is_finite() {
            return false;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    true
}

/// A loop of the Hardy-Cross method: pipes with their traversal sign (+1 along `a` → `b`) and the
/// pressure drop the loop must have (0 for a closed loop, `p(root) − p(source)` for a pseudo-loop).
struct Loop {
    pipes: Vec<(usize, f64)>,
    target: f64,
}

fn hardy_cross(
    net: &Network,
    comp: &Component,
    root: usize,
    flows: &mut [f64],
    pressures: &mut [f64],
) -> Result<(), SolveError> {
    let fluid = &net.fluid;
    let n = net.nodes.len();
    // Adjacency of the component.
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n]; // node -> (pipe, other node)
    for &j in &comp.pipes {
        let p = &net.pipes[j];
        adj[p.a].push((j, p.b));
        adj[p.b].push((j, p.a));
    }
    // Breadth-first spanning tree from the root source.
    let mut parent_pipe = vec![usize::MAX; n];
    let mut depth = vec![usize::MAX; n];
    let mut order = vec![root];
    depth[root] = 0;
    let mut in_tree = vec![false; net.pipes.len()];
    let mut head = 0;
    while head < order.len() {
        let u = order[head];
        head += 1;
        for &(j, v) in &adj[u] {
            if depth[v] == usize::MAX {
                depth[v] = depth[u] + 1;
                parent_pipe[v] = j;
                in_tree[j] = true;
                order.push(v);
            }
        }
    }
    let parent = |v: usize| {
        let p = &net.pipes[parent_pipe[v]];
        if p.a == v { p.b } else { p.a }
    };
    // Initial flows: 1 m/s in every chord, the tree carrying whatever continuity then requires
    // (sources other than the root take no flow initially).
    let mut need = vec![0.0; n]; // flow the tree must bring into the node's subtree
    for &i in &comp.nodes {
        if net.nodes[i].fixed.is_none() {
            need[i] = net.nodes[i].demand;
        }
    }
    for &j in &comp.pipes {
        if !in_tree[j] {
            let p = &net.pipes[j];
            flows[j] = p.area;
            need[p.a] += flows[j];
            need[p.b] -= flows[j];
        }
    }
    for &v in order.iter().skip(1).rev() {
        let j = parent_pipe[v];
        let u = parent(v);
        flows[j] = if net.pipes[j].b == v { need[v] } else { -need[v] };
        need[u] += need[v];
    }
    // Tree path from u up to the ancestor at depth d, as (pipe, sign) walked upward.
    let path = |from: usize, to: usize| -> Vec<(usize, f64)> {
        // pipes walked from `from` to `to` through the tree
        let (mut x, mut z) = (from, to);
        let mut up: Vec<(usize, f64)> = Vec::new();
        let mut down: Vec<(usize, f64)> = Vec::new();
        while x != z {
            if depth[x] >= depth[z] {
                let j = parent_pipe[x];
                // walking x -> parent(x)
                up.push((j, if net.pipes[j].a == x { 1.0 } else { -1.0 }));
                x = parent(x);
            } else {
                let j = parent_pipe[z];
                // walking parent(z) -> z (prepended in reverse later)
                down.push((j, if net.pipes[j].b == z { 1.0 } else { -1.0 }));
                z = parent(z);
            }
        }
        up.extend(down.into_iter().rev());
        up
    };
    let mut loops: Vec<Loop> = Vec::new();
    for &j in &comp.pipes {
        if !in_tree[j] {
            let p = &net.pipes[j];
            let mut pipes = vec![(j, 1.0)];
            pipes.extend(path(p.b, p.a));
            loops.push(Loop { pipes, target: 0.0 });
        }
    }
    let p_root = net.nodes[root].fixed.expect("the root is a source");
    for &i in &comp.nodes {
        if i != root {
            if let Some(p) = net.nodes[i].fixed {
                loops.push(Loop { pipes: path(root, i), target: p_root - p });
            }
        }
    }
    // Gauss-Seidel sweeps of single-loop Newton corrections.
    let mut conv = Convergence::new(200, net.pipes.len());
    const MAX_SWEEPS: usize = 200_000;
    let mut converged = loops.is_empty();
    for _ in 0..MAX_SWEEPS {
        if converged {
            break;
        }
        let mut delta: f64 = 0.0;
        for lp in &loops {
            let (mut sum, mut den) = (-lp.target, 0.0);
            for &(j, s) in &lp.pipes {
                let (dp, slope) = net.pipes[j].dp_slope_regularized(fluid, flows[j]);
                sum += s * dp;
                den += slope;
            }
            let dq = -sum / den;
            for &(j, s) in &lp.pipes {
                flows[j] += s * dq;
            }
            delta = delta.max(dq.abs());
        }
        if !delta.is_finite() {
            return Err(SolveError::NotConverged("the Hardy-Cross iteration diverged".to_string()));
        }
        let scale = flow_scale(net, comp, flows);
        if scale == 0.0 {
            break;
        }
        match conv.update(net, comp, flows, delta / scale) {
            Verdict::Converged => converged = true,
            Verdict::Stalled => return Err(conv.stall_error(comp, "Hardy-Cross")),
            Verdict::Continue => {}
        }
    }
    if !converged {
        return Err(SolveError::NotConverged(format!("Hardy-Cross did not converge in {MAX_SWEEPS} sweeps")));
    }
    // Pressures by a walk of the tree from the root.
    pressures[root] = p_root;
    for &v in order.iter().skip(1) {
        let j = parent_pipe[v];
        let dp = net.pipes[j].state(fluid, flows[j]).dp;
        let u = parent(v);
        pressures[v] = if net.pipes[j].a == u { pressures[u] - dp } else { pressures[u] + dp };
    }
    Ok(())
}
