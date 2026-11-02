//! The 3D space frame: model building, direct-stiffness assembly, solution per
//! load combination, and recovery of end forces, internal actions and envelopes.
//!
//! Degrees of freedom are numbered node by node,
//! `6 * node + {0: ux, 1: uy, 2: uz, 3: rx, 4: ry, 5: rz}`. Members are prismatic
//! Timoshenko beam-columns with twelve local DOFs
//! `(u1, v1, w1, θx1, θy1, θz1, u2, …, θz2)`, mapped to global axes by
//! `k_g = Tᵀ k T` with `T = diag(R, R, R, R)` and `R` the rows `[x; y; z]` of the
//! member's local axes.

use crate::linalg::{Cholesky, SolveError};

/// Degrees of freedom per node: `ux uy uz rx ry rz`.
const NDOF: usize = 6;
const DOF_NAMES: [&str; NDOF] = ["ux", "uy", "uz", "rx", "ry", "rz"];

/// Number of equally spaced stations (both ends included) at which the
/// envelopes are sampled along every member.
pub const ENVELOPE_STATIONS: usize = 101;

/// A reference vector whose component orthogonal to the member axis is shorter
/// than this fraction of its own length counts as parallel to the axis.
const PARALLEL_TOL: f64 = 1e-6;

/// Reported when finite inputs still overflow to infinity or NaN somewhere.
const OVERFLOW: &str = "numerical overflow: check section properties, lengths, loads and \
                        combination factors for absurd magnitudes or units";

type Mat3 = [[f64; 3]; 3];
type Mat12 = [[f64; 12]; 12];
type Vec12 = [f64; 12];

/// Cross-section and material of a 3D member.
///
/// All values in consistent units. `iz` and `asy` govern bending in the
/// member's local x–y plane (displacement along local y, rotation about local
/// z); `iy` and `asz` govern bending in the local x–z plane. A shear area of
/// `None` means no shear deformation in that plane (Euler–Bernoulli); `Some(as)`
/// adds Timoshenko shear flexibility with `Φy = 12·E·Iz / (G·Asy·L²)` and
/// `Φz = 12·E·Iy / (G·Asz·L²)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section3 {
    /// Young's modulus `E`.
    pub e: f64,
    /// Shear modulus `G`.
    pub g: f64,
    /// Cross-section area `A` (axial stiffness `E·A/L`).
    pub a: f64,
    /// Second moment of area about local y (bending in the local x–z plane).
    pub iy: f64,
    /// Second moment of area about local z (bending in the local x–y plane).
    pub iz: f64,
    /// Torsion constant `J` (torsional stiffness `G·J/L`).
    pub j: f64,
    /// Shear area for shear along local y (x–y plane), or `None` for no shear deformation.
    pub asy: Option<f64>,
    /// Shear area for shear along local z (x–z plane), or `None` for no shear deformation.
    pub asz: Option<f64>,
}

/// A named linear combination of load cases: the combined load is
/// `Σ factor × (loads of case)`.
///
/// A case may appear more than once (the factors add up). A case name that has
/// no loads in the frame contributes nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadCombination {
    /// Name used to query [`Results3`].
    pub name: String,
    /// `(load case name, factor)` pairs.
    pub factors: Vec<(String, f64)>,
}

/// Per-member envelope of internal actions and deflection over all load
/// combinations, sampled at [`ENVELOPE_STATIONS`] equally spaced stations.
///
/// Every action pair is `[min, max]`. Internal actions at a station are those
/// the part of the member beyond the station exerts on the part from node `i`
/// to it, in local axes; so the axial force `n` is positive in tension, and at
/// node `i` the actions are the negated end forces of end 1 while at node `j`
/// they equal the end forces of end 2.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    /// Axial force (tension positive), `[min, max]`.
    pub n: [f64; 2],
    /// Shear force along local y, `[min, max]`.
    pub vy: [f64; 2],
    /// Shear force along local z, `[min, max]`.
    pub vz: [f64; 2],
    /// Torsional moment about local x, `[min, max]`.
    pub t: [f64; 2],
    /// Bending moment about local y, `[min, max]`.
    pub my: [f64; 2],
    /// Bending moment about local z, `[min, max]`.
    pub mz: [f64; 2],
    /// Largest magnitude of the transverse (local y–z) displacement of the member
    /// axis relative to the chord between its displaced ends.
    pub deflection: f64,
}

impl Envelope {
    const ZERO: Envelope = Envelope {
        n: [0.0; 2],
        vy: [0.0; 2],
        vz: [0.0; 2],
        t: [0.0; 2],
        my: [0.0; 2],
        mz: [0.0; 2],
        deflection: 0.0,
    };
}

/// A prismatic member between two nodes.
#[derive(Debug, Clone, Copy)]
struct Member {
    i: usize,
    j: usize,
    s: Section3,
    reference: [f64; 3],
}

/// Validated geometry of a member: length and local axes (rows x, y, z).
#[derive(Debug, Clone, Copy)]
struct Geometry {
    length: f64,
    r: Mat3,
}

/// A space frame: nodes with six degrees of freedom, prismatic Timoshenko
/// beam-column members, supports, nodal loads per load case, and load
/// combinations.
///
/// Build the model with [`add_node`](Frame3::add_node),
/// [`add_member`](Frame3::add_member), [`fix`](Frame3::fix),
/// [`load`](Frame3::load) and [`add_combination`](Frame3::add_combination), then
/// call [`analyze`](Frame3::analyze). Results are reported per combination;
/// load cases are only building blocks of combinations.
///
/// # Conventions
///
/// * Global axes are right-handed; rotations and moments follow the right-hand rule.
/// * A member's local x runs from node `i` to node `j`; local z is the unit
///   component of the member's reference vector orthogonal to x; local y = z × x.
///
/// # Contract
///
/// | Situation | Behaviour |
/// |---|---|
/// | node index out of range in `add_member`, `fix`, `load` | panic |
/// | `add_member(i, i, ..)` | panic |
/// | `fix` called again on a node | replaces all six flags |
/// | `load` called again on the same node and case | accumulates |
/// | coincident member nodes; `e, g, a, iy, iz, j` not finite and positive; a `Some` shear area not finite and positive; reference vector zero, non-finite or parallel to the member | `analyze()` returns `Err` naming the member |
/// | non-finite coordinate, load or combination factor; two combinations with the same name | `analyze()` returns `Err` |
/// | unstable or singular structure (rigid-body motion, mechanism, free node with no member) | `analyze()` returns `Err` (checked even when no combination is defined) |
/// | load case in no combination | ignored |
/// | combination naming a case that has no loads | that term contributes nothing |
/// | load on a restrained degree of freedom | absorbed by the support (reaction reduced by it) |
/// | reaction at a degree of freedom that is not restrained | exactly `0.0` |
///
/// Not modelled: member loads, releases, prescribed displacements, springs,
/// second-order effects and warping torsion.
#[derive(Debug, Clone, Default)]
pub struct Frame3 {
    nodes: Vec<[f64; 3]>,
    members: Vec<Member>,
    fixed: Vec<[bool; NDOF]>,
    /// `(case, node, load)` in order of the `load` calls.
    loads: Vec<(String, usize, [f64; NDOF])>,
    combos: Vec<LoadCombination>,
}

impl Frame3 {
    /// An empty space frame.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of nodes added so far.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of members added so far.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Adds a free node at global `(x, y, z)` and returns its index (0, 1, 2, …).
    ///
    /// Non-finite coordinates are rejected by [`analyze`](Frame3::analyze).
    pub fn add_node(&mut self, x: f64, y: f64, z: f64) -> usize {
        self.nodes.push([x, y, z]);
        self.fixed.push([false; NDOF]);
        self.nodes.len() - 1
    }

    /// Adds a prismatic member from node `i` to node `j` with section `s`, rigidly
    /// connected at both ends, and returns its index (0, 1, 2, …).
    ///
    /// `reference` orients the cross-section: local z is the unit component of
    /// `reference` orthogonal to the member axis (it need not be a unit vector,
    /// nor orthogonal to the axis), and local y = z × x.
    ///
    /// Invalid section values, coincident nodes and a reference vector parallel
    /// to the axis are reported by [`analyze`](Frame3::analyze), not here.
    ///
    /// # Panics
    ///
    /// If `i` or `j` is not an existing node, or `i == j`.
    pub fn add_member(&mut self, i: usize, j: usize, s: Section3, reference: [f64; 3]) -> usize {
        self.check_node(i);
        self.check_node(j);
        assert!(i != j, "member must connect two different nodes (got node {i} twice)");
        self.members.push(Member { i, j, s, reference });
        self.members.len() - 1
    }

    /// Sets the restraints of `node` in global axes, ordered `[ux, uy, uz, rx, ry, rz]`;
    /// `true` holds that degree of freedom at zero. The call replaces all six flags.
    ///
    /// # Panics
    ///
    /// If `node` does not exist.
    pub fn fix(&mut self, node: usize, restraints: [bool; 6]) {
        self.check_node(node);
        self.fixed[node] = restraints;
    }

    /// Adds a nodal load `[fx, fy, fz, mx, my, mz]` in global axes to load case
    /// `case`. Loads accumulate: repeated calls add up.
    ///
    /// # Panics
    ///
    /// If `node` does not exist.
    pub fn load(&mut self, node: usize, case: &str, f: [f64; 6]) {
        self.check_node(node);
        self.loads.push((case.to_string(), node, f));
    }

    /// Adds a load combination. Results are computed for every combination;
    /// two combinations with the same name make [`analyze`](Frame3::analyze) fail.
    pub fn add_combination(&mut self, combination: LoadCombination) {
        self.combos.push(combination);
    }

    fn check_node(&self, node: usize) {
        assert!(
            node < self.nodes.len(),
            "node index {node} out of range (the frame has {} nodes)",
            self.nodes.len()
        );
    }

    /// Solves the linear static problem for every load combination.
    ///
    /// The stiffness matrix is factorised once (dense Jacobi-scaled Cholesky) and
    /// each combination is solved as its own right-hand side. `analyze` does not
    /// modify the frame.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a message naming the culprit for the invalid inputs
    /// listed in the [type-level contract](Frame3), and when the structure is
    /// unstable or singular (a rigid-body motion or mechanism remains after the
    /// supports are applied, or a free node is not connected to any member).
    /// As in the 2D solver, stiffness contrasts beyond about 10¹⁰ along a pivot
    /// chain are indistinguishable from a mechanism and also reported.
    pub fn analyze(&self) -> Result<Results3, String> {
        let geoms = self.validate()?;
        let nn = self.nodes.len();
        let n = NDOF * nn;

        // Assemble the global stiffness matrix (dense, row-major).
        let locals: Vec<Mat12> = self
            .members
            .iter()
            .zip(&geoms)
            .map(|(m, g)| local_stiffness(&m.s, g.length))
            .collect();
        let mut k = vec![0.0; n * n];
        for ((mem, g), kl) in self.members.iter().zip(&geoms).zip(&locals) {
            let kg = global_stiffness(kl, &g.r);
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

        // Partition and factorise the free-free block once.
        let restrained: Vec<bool> = self.fixed.iter().flatten().copied().collect();
        let free: Vec<usize> = (0..n).filter(|&d| !restrained[d]).collect();
        let nf = free.len();
        let mut kff = Vec::with_capacity(nf * nf);
        for &r in &free {
            kff.extend(free.iter().map(|&c| k[r * n + c]));
        }
        let chol = Cholesky::factor(kff, nf).map_err(|e| explain(e, &free))?;

        let mut combos = Vec::with_capacity(self.combos.len());
        for combo in &self.combos {
            // Combined load vector.
            let mut f = vec![0.0; n];
            for (case, factor) in &combo.factors {
                for (_, node, l) in self.loads.iter().filter(|(c, _, _)| c == case) {
                    for (d, v) in l.iter().enumerate() {
                        f[NDOF * node + d] += factor * v;
                    }
                }
            }

            let mut u = vec![0.0; n];
            let ff: Vec<f64> = free.iter().map(|&d| f[d]).collect();
            let uf = chol.solve(&ff).map_err(|e| explain(e, &free))?;
            for (&d, v) in free.iter().zip(uf) {
                u[d] = v;
            }

            // Reactions K u − F on restrained DOFs: the force the support exerts.
            let mut react = vec![[0.0; NDOF]; nn];
            for (node, r) in react.iter_mut().enumerate() {
                for (d, rd) in r.iter_mut().enumerate() {
                    let row = NDOF * node + d;
                    if restrained[row] {
                        let ku: f64 = (0..n).map(|c| k[row * n + c] * u[c]).sum();
                        *rd = ku - f[row];
                    }
                }
            }

            // Local end displacements and end forces k · T · u.
            let mut end_forces = Vec::with_capacity(self.members.len());
            let mut local_disp = Vec::with_capacity(self.members.len());
            for ((mem, g), kl) in self.members.iter().zip(&geoms).zip(&locals) {
                let ug: Vec12 = member_dofs(mem).map(|d| u[d]);
                let ul = to_local(&g.r, &ug);
                end_forces.push(mat_vec(kl, &ul));
                local_disp.push(ul);
            }

            let disp: Vec<[f64; NDOF]> =
                u.chunks_exact(NDOF).map(|c| [c[0], c[1], c[2], c[3], c[4], c[5]]).collect();
            combos.push(ComboResults { name: combo.name.clone(), disp, react, end_forces, local_disp });
        }

        let envelopes = self
            .members
            .iter()
            .enumerate()
            .zip(&geoms)
            .map(|((m, mem), g)| envelope_of(m, &mem.s, g.length, &combos))
            .collect();

        let results = Results3 {
            nodes: nn,
            members: self.members.len(),
            combos: combos
                .into_iter()
                .map(|c| Combo { name: c.name, disp: c.disp, react: c.react, end_forces: c.end_forces })
                .collect(),
            envelopes,
        };
        if !results.is_finite() {
            return Err(OVERFLOW.to_string());
        }
        Ok(results)
    }

    /// Checks every input the solver relies on and returns each member's geometry.
    fn validate(&self) -> Result<Vec<Geometry>, String> {
        for (k, p) in self.nodes.iter().enumerate() {
            if !p.iter().all(|v| v.is_finite()) {
                return Err(format!("node {k} has non-finite coordinates {p:?}"));
            }
        }
        for (case, node, l) in &self.loads {
            if !l.iter().all(|v| v.is_finite()) {
                return Err(format!("load case {case:?}: the load on node {node} is not finite {l:?}"));
            }
        }
        for (c, combo) in self.combos.iter().enumerate() {
            if self.combos[..c].iter().any(|o| o.name == combo.name) {
                return Err(format!("two load combinations are named {:?}", combo.name));
            }
            for (case, factor) in &combo.factors {
                if !factor.is_finite() {
                    return Err(format!(
                        "combination {:?}: factor of case {case:?} is not finite ({factor})",
                        combo.name
                    ));
                }
            }
        }
        let mut geoms = Vec::with_capacity(self.members.len());
        for (m, mem) in self.members.iter().enumerate() {
            let s = &mem.s;
            for (name, v) in [("e", s.e), ("g", s.g), ("a", s.a), ("iy", s.iy), ("iz", s.iz), ("j", s.j)] {
                if !(v.is_finite() && v > 0.0) {
                    return Err(format!("member {m}: {name} must be finite and positive (got {v})"));
                }
            }
            for (name, v) in [("asy", s.asy), ("asz", s.asz)] {
                if let Some(v) = v {
                    if !(v.is_finite() && v > 0.0) {
                        return Err(format!(
                            "member {m}: shear area {name} must be finite and positive when given (got {v})"
                        ));
                    }
                }
            }
            let (pi, pj) = (self.nodes[mem.i], self.nodes[mem.j]);
            let d = [pj[0] - pi[0], pj[1] - pi[1], pj[2] - pi[2]];
            let length = norm(d);
            if length == 0.0 {
                return Err(format!(
                    "member {m} has zero length (nodes {} and {} coincide)",
                    mem.i, mem.j
                ));
            }
            if !length.is_finite() {
                return Err(format!("member {m} has a non-finite length"));
            }
            let x = scale(d, 1.0 / length);
            let rf = mem.reference;
            let rn = norm(rf);
            if !(rn.is_finite() && rn > 0.0) {
                return Err(format!("member {m}: reference vector {rf:?} must be finite and non-zero"));
            }
            let along = dot(rf, x);
            let perp = [rf[0] - along * x[0], rf[1] - along * x[1], rf[2] - along * x[2]];
            let pn = norm(perp);
            if pn <= PARALLEL_TOL * rn {
                return Err(format!(
                    "member {m}: reference vector {rf:?} is parallel to the member axis"
                ));
            }
            let z = scale(perp, 1.0 / pn);
            let y = cross(z, x);
            geoms.push(Geometry { length, r: [x, y, z] });
        }
        Ok(geoms)
    }
}

/// Everything solved for one combination, before it is packed into [`Results3`].
struct ComboResults {
    name: String,
    disp: Vec<[f64; NDOF]>,
    react: Vec<[f64; NDOF]>,
    end_forces: Vec<Vec12>,
    local_disp: Vec<Vec12>,
}

/// Results of one combination.
#[derive(Debug, Clone, PartialEq)]
struct Combo {
    name: String,
    disp: Vec<[f64; NDOF]>,
    react: Vec<[f64; NDOF]>,
    end_forces: Vec<Vec12>,
}

/// The outcome of [`Frame3::analyze`]: per combination, nodal displacements,
/// support reactions and member end forces; per member, the envelope over all
/// combinations.
///
/// # Conventions
///
/// * Displacements `[ux, uy, uz, rx, ry, rz]` and reactions
///   `[fx, fy, fz, mx, my, mz]` are in global axes. A reaction is the force the
///   support exerts on the structure; unrestrained DOFs report exactly `0.0`.
/// * Member end forces are the forces the nodes exert on the member, in the
///   member's local axes, ordered `[N1, Vy1, Vz1, T1, My1, Mz1, N2, Vy2, Vz2, T2, My2, Mz2]`
///   (end 1 at node `i`, end 2 at node `j`). A member in tension has `N1 < 0 < N2`.
/// * Envelopes: see [`Envelope`].
///
/// # Panics
///
/// The accessors panic on a combination name that was not added to the frame,
/// or on a node or member index that does not exist.
#[derive(Debug, Clone, PartialEq)]
pub struct Results3 {
    nodes: usize,
    members: usize,
    combos: Vec<Combo>,
    envelopes: Vec<Envelope>,
}

impl Results3 {
    /// Number of nodes in the analysed frame.
    pub fn node_count(&self) -> usize {
        self.nodes
    }

    /// Number of members in the analysed frame.
    pub fn member_count(&self) -> usize {
        self.members
    }

    /// Names of the combinations, in the order they were added.
    pub fn combinations(&self) -> Vec<&str> {
        self.combos.iter().map(|c| c.name.as_str()).collect()
    }

    /// Displacement of `node` under combination `combo`, global axes:
    /// `[ux, uy, uz, rx, ry, rz]`.
    ///
    /// # Panics
    ///
    /// If `combo` is unknown or `node` does not exist.
    pub fn displacement(&self, combo: &str, node: usize) -> [f64; 6] {
        self.check_node(node);
        self.combo(combo).disp[node]
    }

    /// Support reaction at `node` under combination `combo`, global axes:
    /// `[fx, fy, fz, mx, my, mz]` exerted by the support on the structure.
    ///
    /// # Panics
    ///
    /// If `combo` is unknown or `node` does not exist.
    pub fn reaction(&self, combo: &str, node: usize) -> [f64; 6] {
        self.check_node(node);
        self.combo(combo).react[node]
    }

    /// End forces of member `m` under combination `combo`: the forces the nodes
    /// exert on the member in its local axes,
    /// `[N1, Vy1, Vz1, T1, My1, Mz1, N2, Vy2, Vz2, T2, My2, Mz2]`.
    ///
    /// # Panics
    ///
    /// If `combo` is unknown or `m` does not exist.
    pub fn member_end_forces(&self, combo: &str, m: usize) -> [f64; 12] {
        self.check_member(m);
        self.combo(combo).end_forces[m]
    }

    /// Envelope of member `m` over all combinations (all zeros when the frame has
    /// no combination).
    ///
    /// # Panics
    ///
    /// If `m` does not exist.
    pub fn envelope(&self, m: usize) -> Envelope {
        self.check_member(m);
        self.envelopes[m]
    }

    fn combo(&self, name: &str) -> &Combo {
        self.combos
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("unknown load combination {name:?}"))
    }

    fn check_node(&self, node: usize) {
        assert!(node < self.nodes, "node index {node} out of range (the frame has {} nodes)", self.nodes);
    }

    fn check_member(&self, m: usize) {
        assert!(m < self.members, "member index {m} out of range (the frame has {} members)", self.members);
    }

    fn is_finite(&self) -> bool {
        self.combos.iter().all(|c| {
            c.disp.iter().chain(&c.react).flatten().all(|v| v.is_finite())
                && c.end_forces.iter().flatten().all(|v| v.is_finite())
        }) && self.envelopes.iter().all(|e| {
            [e.n, e.vy, e.vz, e.t, e.my, e.mz].iter().flatten().all(|v| v.is_finite())
                && e.deflection.is_finite()
        })
    }
}

/// Internal actions `[N, Vy, Vz, T, My, Mz]` at distance `x` from node `i`,
/// exerted by the part beyond the station on the part from node `i`, given the
/// end forces `f` (nodal loads only, so forces are constant and moments linear).
fn internal_actions(f: &Vec12, x: f64) -> [f64; 6] {
    // Equilibrium of the segment [0, x]: its loads are the end-1 forces at x = 0
    // and the internal actions at x. Moments about the station.
    [-f[0], -f[1], -f[2], -f[3], -f[4] - x * f[2], -f[5] + x * f[1]]
}

/// Transverse displacement at `x` of a Timoshenko beam with no span load,
/// relative to the chord, from end displacements `d1, d2` and end slopes of the
/// section `t1, t2` (`t = dv/dx` without shear) and shear parameter `phi`.
///
/// With no span load the shear force is constant and the bending moment linear:
/// the section rotation is `θ = t1 + b x + c x²` and the displacement
/// `v = d1 + t1 x + b x²/2 + c x³/3 + γ x`, with constant shear strain
/// `γ = −2·E·I·c / (G·As) = −c·Φ·L²/6`. `b` and `c` follow from `θ(L) = t2`,
/// `v(L) = d2`. This is exactly the field of the element stiffness.
fn transverse_field(d1: f64, t1: f64, d2: f64, t2: f64, phi: f64, l: f64, x: f64) -> f64 {
    // b L + c L² = t2 − t1;  b L²/2 + c L³ (1/3 − Φ/6) = d2 − d1 − t1 L.
    let r1 = t2 - t1;
    let r2 = d2 - d1 - t1 * l;
    let (a11, a12, a21, a22) = (l, l * l, l * l / 2.0, l * l * l * (1.0 / 3.0 - phi / 6.0));
    let det = a11 * a22 - a12 * a21; // = −L⁴ (1 + Φ) / 6, never zero
    let b = (r1 * a22 - a12 * r2) / det;
    let c = (a11 * r2 - a21 * r1) / det;
    let v = d1 + t1 * x + b * x * x / 2.0 + c * x * x * x / 3.0 - c * phi * l * l * x / 6.0;
    v - (d1 + (d2 - d1) * x / l)
}

/// Shear parameters `(Φy, Φz)` of a section on a member of length `l`.
fn shear_parameters(s: &Section3, l: f64) -> (f64, f64) {
    let phi = |i: f64, area: Option<f64>| area.map_or(0.0, |a| 12.0 * s.e * i / (s.g * a * l * l));
    (phi(s.iz, s.asy), phi(s.iy, s.asz))
}

/// Envelope of member `m` over all combinations.
fn envelope_of(m: usize, s: &Section3, l: f64, combos: &[ComboResults]) -> Envelope {
    if combos.is_empty() {
        return Envelope::ZERO;
    }
    let (phi_y, phi_z) = shear_parameters(s, l);
    let mut lo = [f64::INFINITY; 6];
    let mut hi = [f64::NEG_INFINITY; 6];
    let mut deflection = 0.0f64;
    for c in combos {
        let f = &c.end_forces[m];
        let d = &c.local_disp[m];
        for k in 0..ENVELOPE_STATIONS {
            let x = l * k as f64 / (ENVELOPE_STATIONS - 1) as f64;
            let act = internal_actions(f, x);
            for q in 0..6 {
                lo[q] = lo[q].min(act[q]);
                hi[q] = hi[q].max(act[q]);
            }
            // x–y plane: v with slope θz; x–z plane: w with slope −θy.
            let v = transverse_field(d[1], d[5], d[7], d[11], phi_y, l, x);
            let w = transverse_field(d[2], -d[4], d[8], -d[10], phi_z, l, x);
            deflection = deflection.max(v.hypot(w));
        }
    }
    Envelope {
        n: [lo[0], hi[0]],
        vy: [lo[1], hi[1]],
        vz: [lo[2], hi[2]],
        t: [lo[3], hi[3]],
        my: [lo[4], hi[4]],
        mz: [lo[5], hi[5]],
        deflection,
    }
}

/// Names the structural cause of a failed solve.
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
             (check the supports and the connectivity; stiffnesses that differ by more than \
             about 1e10 also look singular)",
            label(index)
        ),
        SolveError::NonFinite => OVERFLOW.to_string(),
    }
}

/// Global DOF numbers of a member's twelve end DOFs.
fn member_dofs(mem: &Member) -> [usize; 12] {
    let (i, j) = (NDOF * mem.i, NDOF * mem.j);
    std::array::from_fn(|k| if k < NDOF { i + k } else { j + k - NDOF })
}

/// Local stiffness of a prismatic Timoshenko beam-column for the DOFs
/// `(u1, v1, w1, θx1, θy1, θz1, u2, v2, w2, θx2, θy2, θz2)`.
fn local_stiffness(s: &Section3, l: f64) -> Mat12 {
    let (phi_y, phi_z) = shear_parameters(s, l);
    let mut k = [[0.0; 12]; 12];
    let ea = s.e * s.a / l;
    let gj = s.g * s.j / l;
    for (p, q, v) in [(0, 0, ea), (6, 6, ea), (0, 6, -ea), (3, 3, gj), (9, 9, gj), (3, 9, -gj)] {
        k[p][q] = v;
        k[q][p] = v;
    }
    // Bending block for (d1, t1, d2, t2) with slope sign `sg` (t = sg · dd/dx).
    let mut bend = |ei: f64, phi: f64, dofs: [usize; 4], sg: f64| {
        let c = ei / (l * l * l * (1.0 + phi));
        let b = [
            [12.0, 6.0 * l * sg, -12.0, 6.0 * l * sg],
            [6.0 * l * sg, (4.0 + phi) * l * l, -6.0 * l * sg, (2.0 - phi) * l * l],
            [-12.0, -6.0 * l * sg, 12.0, -6.0 * l * sg],
            [6.0 * l * sg, (2.0 - phi) * l * l, -6.0 * l * sg, (4.0 + phi) * l * l],
        ];
        for (r, &dr) in dofs.iter().enumerate() {
            for (cc, &dc) in dofs.iter().enumerate() {
                k[dr][dc] = c * b[r][cc];
            }
        }
    };
    bend(s.e * s.iz, phi_y, [1, 5, 7, 11], 1.0); // v, θz: θz = dv/dx
    bend(s.e * s.iy, phi_z, [2, 4, 8, 10], -1.0); // w, θy: θy = −dw/dx
    k
}

/// `T · u` for a 12-vector: each 3-block rotated into local axes by `R`.
fn to_local(r: &Mat3, u: &Vec12) -> Vec12 {
    let mut out = [0.0; 12];
    for b in 0..4 {
        for (row, rv) in r.iter().enumerate() {
            out[3 * b + row] = (0..3).map(|c| rv[c] * u[3 * b + c]).sum();
        }
    }
    out
}

/// Member stiffness in global axes, `Tᵀ k T`, symmetrised against round-off.
fn global_stiffness(kl: &Mat12, r: &Mat3) -> Mat12 {
    let mut t = [[0.0; 12]; 12];
    for b in 0..4 {
        for row in 0..3 {
            for c in 0..3 {
                t[3 * b + row][3 * b + c] = r[row][c];
            }
        }
    }
    // kt = k T, then kg = Tᵀ kt.
    let mut kt = [[0.0; 12]; 12];
    for (row, out) in kt.iter_mut().enumerate() {
        for (c, v) in out.iter_mut().enumerate() {
            *v = (0..12).map(|m| kl[row][m] * t[m][c]).sum();
        }
    }
    let mut kg = [[0.0; 12]; 12];
    for (row, out) in kg.iter_mut().enumerate() {
        for (c, v) in out.iter_mut().enumerate() {
            *v = (0..12).map(|m| t[m][row] * kt[m][c]).sum();
        }
    }
    let mut sym = [[0.0; 12]; 12];
    for (row, out) in sym.iter_mut().enumerate() {
        for (c, v) in out.iter_mut().enumerate() {
            *v = 0.5 * (kg[row][c] + kg[c][row]);
        }
    }
    sym
}

fn mat_vec(a: &Mat12, x: &Vec12) -> Vec12 {
    a.map(|row| row.iter().zip(x).map(|(m, v)| m * v).sum())
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[cfg(test)]
mod tests;
