//! The 2D frame: model building, direct-stiffness assembly, solution and recovery.
//!
//! Degrees of freedom are numbered node by node, `3 * node + {0: ux, 1: uy, 2: rz}`.
//! Members are Euler–Bernoulli beam-columns with six local DOFs
//! `(u1, v1, θ1, u2, v2, θ2)`, mapped to global axes by `k_g = Tᵀ k T`.

use crate::linalg::{solve_spd, SolveError};

/// Degrees of freedom per node: `ux`, `uy`, `rz`.
const NDOF: usize = 3;
const DOF_NAMES: [&str; NDOF] = ["ux", "uy", "rz"];

/// Reported when finite inputs still overflow to infinity or NaN somewhere.
const OVERFLOW: &str =
    "numerical overflow: check e, a, iz, lengths and loads for absurd magnitudes or units";

type Mat6 = [[f64; 6]; 6];
type Vec6 = [f64; 6];

/// A prismatic member between two nodes (indices into `Frame::nodes`).
#[derive(Debug, Clone, Copy)]
struct Member {
    i: usize,
    j: usize,
    e: f64,
    a: f64,
    iz: f64,
}

/// Validated geometry of a member: length and the direction cosines of local x.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    length: f64,
    c: f64,
    s: f64,
}

/// A plane frame: nodes, prismatic Euler–Bernoulli beam-column members, supports
/// and nodal loads.
///
/// Build the model with [`add_node`](Frame::add_node), [`add_member`](Frame::add_member),
/// [`fix`](Frame::fix) and [`load`](Frame::load), then call
/// [`analyze`](Frame::analyze). Units are the caller's, as long as they are
/// consistent (for example N and m, or kN and m).
///
/// # Conventions
///
/// * Global x points right, y up; rotations are counter-clockwise positive.
/// * A member's local x runs from its node `i` to its node `j`; local y is local x
///   rotated 90° counter-clockwise, so local z coincides with global z and moments
///   are counter-clockwise positive in local and global axes alike.
///
/// # Contract
///
/// * The builder methods never fail for data problems. They panic only on
///   programming errors: a node index that does not exist, or a member from a
///   node to itself.
/// * [`fix`](Frame::fix) *sets* a node's three restraint flags (a second call
///   replaces the first; `fix(n, false, false, false)` frees the node again).
/// * [`load`](Frame::load) *accumulates*: repeated calls on a node add up.
/// * Everything else is checked by [`analyze`](Frame::analyze), which reports it as
///   an `Err` and never panics or returns NaN results.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    nodes: Vec<[f64; 2]>,
    members: Vec<Member>,
    fixed: Vec<[bool; NDOF]>,
    loads: Vec<[f64; NDOF]>,
}

impl Frame {
    /// An empty frame.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of nodes added so far (the next [`add_node`](Frame::add_node) returns this index).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of members added so far.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Adds a node at global `(x, y)` and returns its index (0, 1, 2, … in order of
    /// creation). A new node is free and unloaded.
    ///
    /// Non-finite coordinates are accepted here and rejected by [`analyze`](Frame::analyze).
    pub fn add_node(&mut self, x: f64, y: f64) -> usize {
        self.nodes.push([x, y]);
        self.fixed.push([false; NDOF]);
        self.loads.push([0.0; NDOF]);
        self.nodes.len() - 1
    }

    /// Adds a prismatic Euler–Bernoulli beam-column from node `i` to node `j` with
    /// Young's modulus `e`, cross-section area `a` and second moment of area `iz`
    /// (about the axis normal to the plane), and returns its index (0, 1, 2, … in
    /// order of creation). Both ends are rigidly connected to their nodes.
    ///
    /// Coincident nodes, and `e`, `a` or `iz` that are not finite and positive, are
    /// reported by [`analyze`](Frame::analyze), not here.
    ///
    /// # Panics
    ///
    /// If `i` or `j` is not an existing node, or `i == j`.
    pub fn add_member(&mut self, i: usize, j: usize, e: f64, a: f64, iz: f64) -> usize {
        self.check_node(i);
        self.check_node(j);
        assert!(
            i != j,
            "member must connect two different nodes (got node {i} twice)"
        );
        self.members.push(Member { i, j, e, a, iz });
        self.members.len() - 1
    }

    /// Sets the restraints of `node`: a `true` flag holds that global degree of
    /// freedom (`ux`, `uy`, `rz`) at zero displacement. The call *replaces* all
    /// three flags of the node, so `fix(n, true, false, false)` after
    /// `fix(n, false, true, false)` leaves only `ux` held, and
    /// `fix(n, false, false, false)` frees the node.
    ///
    /// # Panics
    ///
    /// If `node` does not exist.
    pub fn fix(&mut self, node: usize, ux: bool, uy: bool, rz: bool) {
        self.check_node(node);
        self.fixed[node] = [ux, uy, rz];
    }

    /// Applies a nodal load: force `(fx, fy)` along the global axes and moment `mz`
    /// (counter-clockwise positive). Loads *accumulate*: calling `load` twice on a
    /// node applies the sum of both.
    ///
    /// A load on a restrained degree of freedom is allowed; the support absorbs it
    /// and the reaction there is reduced by it. A non-finite load is accepted here
    /// and rejected by [`analyze`](Frame::analyze).
    ///
    /// # Panics
    ///
    /// If `node` does not exist.
    pub fn load(&mut self, node: usize, fx: f64, fy: f64, mz: f64) {
        self.check_node(node);
        let l = &mut self.loads[node];
        l[0] += fx;
        l[1] += fy;
        l[2] += mz;
    }

    fn check_node(&self, node: usize) {
        assert!(
            node < self.nodes.len(),
            "node index {node} out of range (the frame has {} nodes)",
            self.nodes.len()
        );
    }

    /// Solves the linear static problem and returns displacements, support
    /// reactions and member end forces.
    ///
    /// `analyze` does not modify the frame; calling it twice gives identical
    /// results. The solution is by the direct-stiffness method on a dense matrix
    /// (cost grows with the cube of the number of free degrees of freedom).
    ///
    /// # Errors
    ///
    /// Returns `Err` with a message naming the culprit when
    ///
    /// * a node coordinate or a load is NaN or infinite;
    /// * a member has zero (or non-finite) length, or `e`, `a`, `iz` is not finite
    ///   and positive;
    /// * the structure is unstable or singular: a rigid-body motion or mechanism
    ///   remains after the supports are applied, or a free node is not connected
    ///   to any member (a node with no member must be fully fixed).
    ///
    /// Singularity is judged against each degree of freedom's own stiffness, so the
    /// verdict does not depend on the units used or on very stiff members next to
    /// flexible ones: a fixed-base portal whose beam is 10⁷ times stiffer in bending
    /// than its columns is analysed normally. What cannot be told apart from a
    /// mechanism is a restraint more than about 10¹⁰ times stiffer than the stiffness
    /// it restrains (an axially rigid link that much stiffer than the columns it
    /// ties together, say): such a structure is reported as singular too. That
    /// threshold is a deliberate limit, not a bug.
    ///
    /// Other degenerate-but-valid models are accepted: a structure whose every
    /// degree of freedom is fixed gives zero displacements, reactions equal to the
    /// negated loads and zero end forces; so does an empty frame.
    pub fn analyze(&self) -> Result<Results, String> {
        let geoms = self.validate()?;
        let n = NDOF * self.nodes.len();

        // Assemble the global stiffness matrix (dense, row-major) and load vector.
        let mut k = vec![0.0; n * n];
        for (mem, g) in self.members.iter().zip(&geoms) {
            let kg = global_stiffness(mem, g);
            let dofs = member_dofs(mem);
            for (r, &dr) in dofs.iter().enumerate() {
                for (c, &dc) in dofs.iter().enumerate() {
                    k[dr * n + dc] += kg[r][c];
                }
            }
        }
        if !k.iter().all(|v| v.is_finite()) {
            return Err(OVERFLOW.to_string());
        }
        let f: Vec<f64> = self.loads.iter().flatten().copied().collect();

        // Partition into free and restrained degrees of freedom.
        let restrained: Vec<bool> = self.fixed.iter().flatten().copied().collect();
        let free: Vec<usize> = (0..n).filter(|&d| !restrained[d]).collect();
        let nf = free.len();

        // Solve K_ff u_f = F_f (restrained displacements are zero).
        let mut u = vec![0.0; n];
        if nf > 0 {
            let mut kff = Vec::with_capacity(nf * nf);
            for &r in &free {
                kff.extend(free.iter().map(|&c| k[r * n + c]));
            }
            let ff: Vec<f64> = free.iter().map(|&d| f[d]).collect();
            let uf = solve_spd(kff, nf, &ff).map_err(|e| explain(e, &free))?;
            for (&d, v) in free.iter().zip(uf) {
                u[d] = v;
            }
        }

        // Reactions: the force the support exerts on the structure, from node
        // equilibrium K u = F + R. Free degrees of freedom carry none.
        let mut react = vec![[0.0; NDOF]; self.nodes.len()];
        for (node, r) in react.iter_mut().enumerate() {
            for (d, rd) in r.iter_mut().enumerate() {
                let row = NDOF * node + d;
                if restrained[row] {
                    let ku: f64 = (0..n).map(|c| k[row * n + c] * u[c]).sum();
                    *rd = ku - f[row];
                }
            }
        }

        // End forces: the forces the nodes exert on the member, in local axes.
        let end_forces: Vec<Vec6> = self
            .members
            .iter()
            .zip(&geoms)
            .map(|(mem, g)| {
                let dofs = member_dofs(mem);
                let ug: Vec6 = dofs.map(|d| u[d]);
                let ul = mat_vec(&transformation(g.c, g.s), &ug);
                mat_vec(&local_stiffness(mem.e, mem.a, mem.iz, g.length), &ul)
            })
            .collect();

        let disp: Vec<[f64; NDOF]> = u.chunks_exact(NDOF).map(|c| [c[0], c[1], c[2]]).collect();
        let results = Results {
            disp,
            react,
            end_forces,
        };
        if !results.is_finite() {
            return Err(OVERFLOW.to_string());
        }
        Ok(results)
    }

    /// Checks every input the solver relies on and returns each member's geometry.
    fn validate(&self) -> Result<Vec<Geometry>, String> {
        for (k, p) in self.nodes.iter().enumerate() {
            if !(p[0].is_finite() && p[1].is_finite()) {
                return Err(format!(
                    "node {k} has non-finite coordinates ({}, {})",
                    p[0], p[1]
                ));
            }
        }
        for (k, l) in self.loads.iter().enumerate() {
            if !l.iter().all(|v| v.is_finite()) {
                return Err(format!(
                    "the load on node {k} is not finite ({}, {}, {})",
                    l[0], l[1], l[2]
                ));
            }
        }
        let mut geoms = Vec::with_capacity(self.members.len());
        for (m, mem) in self.members.iter().enumerate() {
            for (name, v) in [("e", mem.e), ("a", mem.a), ("iz", mem.iz)] {
                if !(v.is_finite() && v > 0.0) {
                    return Err(format!(
                        "member {m}: {name} must be finite and positive (got {v})"
                    ));
                }
            }
            let [xi, yi] = self.nodes[mem.i];
            let [xj, yj] = self.nodes[mem.j];
            let (dx, dy) = (xj - xi, yj - yi);
            let length = dx.hypot(dy);
            if length == 0.0 {
                return Err(format!(
                    "member {m} has zero length (nodes {} and {} coincide)",
                    mem.i, mem.j
                ));
            }
            if !length.is_finite() {
                return Err(format!("member {m} has a non-finite length"));
            }
            geoms.push(Geometry {
                length,
                c: dx / length,
                s: dy / length,
            });
        }
        Ok(geoms)
    }
}

/// Names the structural cause of a failed solve, mapping the position in the
/// reduced system back to a node and degree of freedom.
fn explain(err: SolveError, free: &[usize]) -> String {
    let label = |index: usize| {
        let dof = free[index];
        format!("{} at node {}", DOF_NAMES[dof % NDOF], dof / NDOF)
    };
    match err {
        SolveError::NoStiffness { index } => format!(
            "structure is unstable or singular: {} has no stiffness (a node that no member \
             reaches must be fully fixed)",
            label(index)
        ),
        SolveError::Singular { index, .. } => format!(
            "structure is unstable or singular: a rigid-body motion or mechanism involves {} \
             (check the supports and the connectivity; member stiffnesses that differ by more \
             than about 1e10 also look singular)",
            label(index)
        ),
        SolveError::NonFinite => OVERFLOW.to_string(),
    }
}

/// Global DOF numbers of a member's six end DOFs, in the order `(i: ux uy rz, j: ux uy rz)`.
fn member_dofs(mem: &Member) -> [usize; 6] {
    let (i, j) = (NDOF * mem.i, NDOF * mem.j);
    [i, i + 1, i + 2, j, j + 1, j + 2]
}

/// Local stiffness of a prismatic Euler–Bernoulli beam-column for the DOFs
/// `(u1, v1, θ1, u2, v2, θ2)` along and across its own axis.
fn local_stiffness(e: f64, a: f64, iz: f64, l: f64) -> Mat6 {
    let ax = e * a / l; // EA/L
    let ei = e * iz;
    let b = 12.0 * ei / (l * l * l); // 12EI/L³
    let c = 6.0 * ei / (l * l); // 6EI/L²
    let d = 4.0 * ei / l; // 4EI/L
    let h = 2.0 * ei / l; // 2EI/L
    [
        [ax, 0.0, 0.0, -ax, 0.0, 0.0],
        [0.0, b, c, 0.0, -b, c],
        [0.0, c, d, 0.0, -c, h],
        [-ax, 0.0, 0.0, ax, 0.0, 0.0],
        [0.0, -b, -c, 0.0, b, -c],
        [0.0, c, h, 0.0, -c, d],
    ]
}

/// Global-to-local transformation `T` of a member whose local x has direction
/// cosines `(c, s)`: `u_local = T u_global`. Local y is local x rotated 90°
/// counter-clockwise, so the rotation DOF is the same in both systems.
fn transformation(c: f64, s: f64) -> Mat6 {
    [
        [c, s, 0.0, 0.0, 0.0, 0.0],
        [-s, c, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, c, s, 0.0],
        [0.0, 0.0, 0.0, -s, c, 0.0],
        [0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    ]
}

/// Member stiffness in global axes, `Tᵀ k T`, symmetrised against round-off so the
/// assembled matrix is exactly symmetric.
fn global_stiffness(mem: &Member, g: &Geometry) -> Mat6 {
    let t = transformation(g.c, g.s);
    let k = local_stiffness(mem.e, mem.a, mem.iz, g.length);
    let kg = mat_mul(&transpose(&t), &mat_mul(&k, &t));
    let mut sym = [[0.0; 6]; 6];
    for (r, row) in sym.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = 0.5 * (kg[r][c] + kg[c][r]);
        }
    }
    sym
}

fn mat_mul(a: &Mat6, b: &Mat6) -> Mat6 {
    let mut out = [[0.0; 6]; 6];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..6).map(|m| a[r][m] * b[m][c]).sum();
        }
    }
    out
}

fn transpose(a: &Mat6) -> Mat6 {
    let mut out = [[0.0; 6]; 6];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = a[c][r];
        }
    }
    out
}

fn mat_vec(a: &Mat6, x: &Vec6) -> Vec6 {
    a.map(|row| row.iter().zip(x).map(|(m, v)| m * v).sum())
}

/// The outcome of [`Frame::analyze`]: displacements, support reactions and
/// member end forces.
///
/// `Results` owns its data and does not borrow the [`Frame`]. Node and member
/// indices are those returned by [`Frame::add_node`] and [`Frame::add_member`];
/// an index that does not exist makes the accessors panic.
///
/// # Conventions
///
/// * Displacements and reactions are in global axes (x right, y up, rotation and
///   moment counter-clockwise positive).
/// * A *reaction* is the force the support exerts on the structure, so it
///   balances the applied load: a cantilever carrying a downward tip load has an
///   upward reaction force and a counter-clockwise reaction moment at its root.
///   Degrees of freedom that are not restrained have a reaction of exactly `0.0`.
/// * *Member end forces* are the forces the nodes exert on the member, in the
///   member's local axes (x from node `i` to node `j`, y 90° counter-clockwise
///   from x): `[N1, V1, M1, N2, V2, M2]` with axial force and shear along local x
///   and y, and moment about local z (counter-clockwise positive), at end 1 (node
///   `i`) and end 2 (node `j`). A member in tension has `N1 < 0` and `N2 > 0`.
///   Each member is in equilibrium: `N1 + N2 = 0`, `V1 + V2 = 0` and
///   `M1 + M2 + V2·L = 0` for member length `L`.
#[derive(Debug, Clone, PartialEq)]
pub struct Results {
    disp: Vec<[f64; NDOF]>,
    react: Vec<[f64; NDOF]>,
    end_forces: Vec<Vec6>,
}

impl Results {
    /// Number of nodes in the analysed frame.
    pub fn node_count(&self) -> usize {
        self.disp.len()
    }

    /// Number of members in the analysed frame.
    pub fn member_count(&self) -> usize {
        self.end_forces.len()
    }

    /// Displacement of `node` in global axes: `(ux, uy, rz)`.
    ///
    /// # Panics
    ///
    /// If `node` does not exist.
    pub fn displacement(&self, node: usize) -> (f64, f64, f64) {
        let [ux, uy, rz] = self.node_entry(&self.disp, node);
        (ux, uy, rz)
    }

    /// Support reaction at `node` in global axes: `(fx, fy, mz)`, the force and
    /// moment the support exerts on the structure; `0.0` for every degree of
    /// freedom that is not restrained.
    ///
    /// # Panics
    ///
    /// If `node` does not exist.
    pub fn reaction(&self, node: usize) -> (f64, f64, f64) {
        let [fx, fy, mz] = self.node_entry(&self.react, node);
        (fx, fy, mz)
    }

    /// End forces of member `m` in its local axes: `[N1, V1, M1, N2, V2, M2]`,
    /// the forces the nodes exert on the member (see the type-level docs).
    ///
    /// # Panics
    ///
    /// If `m` is not a member of the analysed frame.
    pub fn member_end_forces(&self, m: usize) -> [f64; 6] {
        assert!(
            m < self.end_forces.len(),
            "member index {m} out of range (the frame has {} members)",
            self.end_forces.len()
        );
        self.end_forces[m]
    }

    fn node_entry(&self, table: &[[f64; NDOF]], node: usize) -> [f64; NDOF] {
        assert!(
            node < table.len(),
            "node index {node} out of range (the frame has {} nodes)",
            table.len()
        );
        table[node]
    }

    fn is_finite(&self) -> bool {
        self.disp
            .iter()
            .chain(&self.react)
            .flatten()
            .all(|v| v.is_finite())
            && self.end_forces.iter().flatten().all(|v| v.is_finite())
    }
}

#[cfg(test)]
mod tests;
