//! MEP segment solids (spec/schema.md, "Derived geometry rules"): a `Segment` runs from the `x y z`
//! of its `from` Port to that of its `to` Port, as a prism of its cross-section.
//!
//! * `round`: a 16-gon prism with every vertex at radius `diameter / 2` from the axis (so the pipe
//!   is slightly thinner between the vertices than `diameter`).
//! * `rect`: a box with `width` horizontal and perpendicular to the axis and `height` vertical.
//!
//! Both are right prisms: the end caps are perpendicular to the axis, which therefore need not be
//! horizontal. The cross-section is laid out in the frame `(u, v)`: `u = axis × z` normalised (the
//! horizontal direction to the right of the axis; `x` for a vertical axis) and `v = axis × u`.
//! Seen from the `to` end, `u` then `v` run counter-clockwise, so `u × v = axis`. For a
//! horizontal axis `v` is exactly `−z`, `u` is exactly horizontal and the rectangle's `width`
//! runs along `u` and its `height` along `v`; for a sloped duct `v` is the in-section direction
//! closest to vertical, and for a vertical one `width` runs along `x` and `height` along `y`.
//!
//! The mesh has one vertex per corner of each end ring and no others (a cap is a fan of the ring),
//! so it is closed, consistently oriented (outward) and a 2-manifold by construction. Round
//! vertex `k` of a ring is at angle `2πk/16` from `u` towards `v`.
//!
//! A segment without a solid is simply not an element: a port that is missing or not a Port, a
//! coordinate or size that is missing or not finite, a non-positive size, an unknown `shape`, or
//! two ports at the same place. Nothing here needs the segment's `system` or hydraulics.

use bim_doc::{Document, Entity, Value};
use bim_geom::Point3;

use crate::{num, target, Mesh};

/// Sides of the polygon that a round segment's circle is cut into.
pub const ROUND_SIDES: usize = 16;

/// Shortest segment that has a solid (m).
const MIN_LENGTH: f64 = 1e-9;

/// A segment's cross-section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Section {
    /// A [`ROUND_SIDES`]-gon whose vertices are at radius `diameter / 2`.
    Round { diameter: f64 },
    /// A rectangle `width` across (horizontal) and `height` up.
    Rect { width: f64, height: f64 },
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// The closed, outward-oriented prism from `a` to `b` with cross-section `section`, or `None` when
/// the segment is degenerate: `|b − a| ≤ 1e-9`, a size that is not a positive finite number or a
/// non-finite coordinate.
pub fn segment_mesh(a: [f64; 3], b: [f64; 3], section: Section) -> Option<Mesh> {
    let sizes_ok = match section {
        Section::Round { diameter } => diameter.is_finite() && diameter > 0.0,
        Section::Rect { width, height } => width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0,
    };
    let d = sub(b, a);
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !sizes_ok || !length.is_finite() || !(length > MIN_LENGTH) {
        return None;
    }
    let axis = d.map(|x| x / length);
    // `axis × z` is horizontal and perpendicular to the axis; it vanishes for a vertical axis.
    let side = cross(axis, [0.0, 0.0, 1.0]);
    let side_len = (side[0] * side[0] + side[1] * side[1]).sqrt();
    let u = if side_len > 1e-12 { [side[0] / side_len, side[1] / side_len, 0.0] } else { [1.0, 0.0, 0.0] };
    let v = cross(axis, u);

    // The ring, counter-clockwise about the axis (from `u` towards `v`), as offsets from the axis.
    let at = |cu: f64, cv: f64| [cu * u[0] + cv * v[0], cu * u[1] + cv * v[1], cu * u[2] + cv * v[2]];
    let ring: Vec<[f64; 3]> = match section {
        Section::Round { diameter } => {
            let r = diameter / 2.0;
            (0..ROUND_SIDES)
                .map(|k| {
                    let theta = 2.0 * std::f64::consts::PI * k as f64 / ROUND_SIDES as f64;
                    at(r * theta.cos(), r * theta.sin())
                })
                .collect()
        }
        Section::Rect { width, height } => {
            let (hw, hh) = (width / 2.0, height / 2.0);
            vec![at(hw, hh), at(-hw, hh), at(-hw, -hh), at(hw, -hh)]
        }
    };
    let n = ring.len() as u32;
    let mut mesh = Mesh::default();
    for end in [a, b] {
        mesh.positions.extend(ring.iter().map(|r| Point3::new(end[0] + r[0], end[1] + r[1], end[2] + r[2])));
    }
    for k in 1..n - 1 {
        mesh.triangles.push([n, n + k, n + k + 1]); // the cap at `b`, facing along the axis
        mesh.triangles.push([0, k + 1, k]); // the cap at `a`, facing against it
    }
    for k in 0..n {
        let k1 = (k + 1) % n;
        mesh.triangles.push([k, k1, n + k1]);
        mesh.triangles.push([k, n + k1, n + k]);
    }
    Some(mesh)
}

/// The position of the Port that the Ref prop `key` of `e` refers to.
fn port_position(doc: &Document, e: &Entity, key: &str) -> Option<[f64; 3]> {
    let port = target(doc, e, key, "Port")?;
    Some([num(doc, port, "x")?, num(doc, port, "y")?, num(doc, port, "z")?])
}

/// The solid of the `Segment` entity `e`, if it has one.
pub(crate) fn segment_element(doc: &Document, e: &Entity) -> Option<Mesh> {
    let (a, b) = (port_position(doc, e, "from")?, port_position(doc, e, "to")?);
    let section = match e.props.get("shape") {
        Some(Value::Text(s)) if s == "round" => Section::Round { diameter: num(doc, e, "diameter")? },
        Some(Value::Text(s)) if s == "rect" => Section::Rect { width: num(doc, e, "width")?, height: num(doc, e, "height")? },
        _ => return None,
    };
    segment_mesh(a, b, section)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(m: &Mesh, i: usize) -> [f64; 3] {
        [m.positions[i].x, m.positions[i].y, m.positions[i].z]
    }

    fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    /// The 16-gon's area for circumradius `r`.
    fn a16(r: f64) -> f64 {
        8.0 * (std::f64::consts::PI / 8.0).sin() * r * r
    }

    /// (distance along the axis from `a`, distance from the axis line) of every vertex.
    fn along_and_radial(m: &Mesh, a: [f64; 3], b: [f64; 3]) -> Vec<(f64, f64)> {
        let d = sub(b, a);
        let len = dot(d, d).sqrt();
        let ax = d.map(|x| x / len);
        (0..m.positions.len())
            .map(|i| {
                let rel = sub(p(m, i), a);
                let t = dot(rel, ax);
                let radial = [rel[0] - t * ax[0], rel[1] - t * ax[1], rel[2] - t * ax[2]];
                (t, dot(radial, radial).sqrt())
            })
            .collect()
    }

    const AXES: [([f64; 3], [f64; 3]); 7] = [
        ([2.0, -1.0, 1.5], [2.0, 1.0, 1.5]),   // +y
        ([1.0, 0.18, 0.5], [3.0, 0.18, 0.5]),  // +x
        ([5.0, 3.0, 1.0], [2.0, 3.0, 1.0]),    // -x
        ([0.0, 0.0, 0.0], [3.0, 4.0, 0.0]),    // horizontal diagonal
        ([1.0, 2.0, 3.0], [1.0, 2.0, 7.0]),    // +z
        ([1.0, 2.0, 7.0], [1.0, 2.0, 3.0]),    // -z
        ([-1.0, 2.0, 0.5], [2.0, 0.0, 3.0]),   // sloped
    ];

    #[test]
    fn a_round_segment_is_a_closed_16_gon_prism_with_vertices_at_the_radius() {
        for (a, b) in AXES {
            let d = 0.1;
            let m = segment_mesh(a, b, Section::Round { diameter: d }).unwrap();
            let len = dot(sub(b, a), sub(b, a)).sqrt();
            assert!(m.is_closed(), "{a:?} -> {b:?}");
            assert!(m.to_solid().is_closed_manifold(), "{a:?} -> {b:?}");
            assert_eq!((m.positions.len(), m.triangles.len()), (32, 60), "{a:?} -> {b:?}");
            let rt = along_and_radial(&m, a, b);
            for (i, &(t, r)) in rt.iter().enumerate() {
                assert!((r - d / 2.0).abs() < 1e-12, "vertex {i}: radius {r}");
                let want = if i < 16 { 0.0 } else { len };
                assert!((t - want).abs() < 1e-12, "vertex {i}: {t} vs {want}");
            }
            // Positive: outward-oriented (an inward mesh has the negated volume).
            let want = a16(d / 2.0) * len;
            assert!((m.volume() - want).abs() < 1e-12 * want, "{a:?} -> {b:?}: {} vs {want}", m.volume());
            // The ring is a regular 16-gon: neighbouring vertices are one side apart.
            let side = 2.0 * (d / 2.0) * (std::f64::consts::PI / 16.0).sin();
            for k in 0..16 {
                let e = sub(p(&m, k), p(&m, (k + 1) % 16));
                assert!((dot(e, e).sqrt() - side).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn a_round_segment_starts_on_the_horizontal_side_of_its_axis() {
        // Vertex 0 lies in the horizontal plane through the axis, on the right of the direction.
        let m = segment_mesh([1.0, 0.18, 0.5], [3.0, 0.18, 0.5], Section::Round { diameter: 0.1 }).unwrap();
        let v0 = p(&m, 0);
        assert!((v0[0] - 1.0).abs() < 1e-15 && (v0[1] - 0.13).abs() < 1e-15 && (v0[2] - 0.5).abs() < 1e-15, "{v0:?}");
        // The prism is thinner than the circle only between vertices: the apothem is R cos(π/16).
        let m = segment_mesh([0.0; 3], [1.0, 0.0, 0.0], Section::Round { diameter: 2.0 }).unwrap();
        let zs: Vec<f64> = (0..16).map(|k| p(&m, k)[2]).collect();
        assert!((zs.iter().cloned().fold(f64::MIN, f64::max) - 1.0).abs() < 1e-12, "a vertex is at the top");
    }

    #[test]
    fn a_rect_segment_is_a_box_with_horizontal_width_and_vertical_height() {
        for (a, b) in AXES {
            let (w, h) = (0.4, 0.3);
            let m = segment_mesh(a, b, Section::Rect { width: w, height: h }).unwrap();
            let len = dot(sub(b, a), sub(b, a)).sqrt();
            assert!(m.is_closed() && m.to_solid().is_closed_manifold(), "{a:?} -> {b:?}");
            assert_eq!((m.positions.len(), m.triangles.len()), (8, 12));
            assert!((m.volume() - w * h * len).abs() < 1e-12, "{a:?} -> {b:?}: {}", m.volume());
            let ax = sub(b, a).map(|x| x / len);
            let horizontal = ax[2].abs() < 1e-12;
            let vertical = ax[0].abs() < 1e-12 && ax[1].abs() < 1e-12;
            let ptp = |f: &dyn Fn(usize) -> f64| {
                let vals: Vec<f64> = (0..8).map(f).collect();
                vals.iter().cloned().fold(f64::MIN, f64::max) - vals.iter().cloned().fold(f64::MAX, f64::min)
            };
            if horizontal {
                // `width` across the axis in the horizontal plane, `height` straight up.
                let across = [ax[1], -ax[0], 0.0];
                assert!((ptp(&|i| dot(p(&m, i), across)) - w).abs() < 1e-12, "{a:?} -> {b:?}");
                assert!((ptp(&|i| p(&m, i)[2]) - h).abs() < 1e-12, "{a:?} -> {b:?}");
                for i in 0..8 {
                    assert!((p(&m, i)[2] - a[2]).abs() - h / 2.0 < 1e-12);
                }
            }
            if vertical {
                assert!((ptp(&|i| p(&m, i)[0]) - w).abs() < 1e-12 && (ptp(&|i| p(&m, i)[1]) - h).abs() < 1e-12);
            }
            // The caps are perpendicular to the axis whatever the slope.
            for (t, _) in along_and_radial(&m, a, b) {
                assert!(t.abs() < 1e-12 || (t - len).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn horizontal_rect_ducts_have_exact_coordinates() {
        let m = segment_mesh([4.0, -1.0, 1.0], [4.0, 1.0, 1.0], Section::Rect { width: 0.4, height: 0.3 }).unwrap();
        for i in 0..8 {
            let q = p(&m, i);
            assert!([3.8, 4.2].contains(&q[0]) && [-1.0, 1.0].contains(&q[1]) && [0.85, 1.15].contains(&q[2]), "{q:?}");
        }
    }

    #[test]
    fn degenerate_segments_have_no_solid() {
        let ok = Section::Round { diameter: 0.1 };
        assert!(segment_mesh([0.0; 3], [1.0, 0.0, 0.0], ok).is_some());
        assert!(segment_mesh([1.0, 2.0, 3.0], [1.0, 2.0, 3.0], ok).is_none(), "zero length");
        assert!(segment_mesh([0.0; 3], [5e-10, 0.0, 0.0], ok).is_none(), "shorter than a nanometre");
        assert!(segment_mesh([0.0; 3], [2e-9, 0.0, 0.0], ok).is_some());
        for bad in [0.0, -0.1, f64::NAN, f64::INFINITY] {
            assert!(segment_mesh([0.0; 3], [1.0, 0.0, 0.0], Section::Round { diameter: bad }).is_none(), "{bad}");
            assert!(segment_mesh([0.0; 3], [1.0, 0.0, 0.0], Section::Rect { width: bad, height: 0.1 }).is_none(), "{bad}");
            assert!(segment_mesh([0.0; 3], [1.0, 0.0, 0.0], Section::Rect { width: 0.1, height: bad }).is_none(), "{bad}");
        }
        assert!(segment_mesh([0.0, f64::NAN, 0.0], [1.0, 0.0, 0.0], ok).is_none());
        assert!(segment_mesh([0.0; 3], [f64::INFINITY, 0.0, 0.0], ok).is_none());
    }
}
