//! Curtain walls, stairs and ramps (spec/schema.md). Each solid is built in a local frame with x
//! along the wall or the run, y to its left and z up from the datum level (a curtain wall's level,
//! a stair's or ramp's base), then placed: `world = origin + x·u + y·left`, `z + elevation`, with
//! `u` the unit direction in plan and `left` its 90° counter-clockwise turn. That is a rotation,
//! so triangles stay outward-oriented.
//!
//! * **Curtain wall.** Grid lines at `0, u_spacing, 2·u_spacing, …` and at the length `L` along
//!   the wall, at `0, v_spacing, …` and at `height` up it ([`grid`]: the last bay takes the
//!   remainder). A mullion of `mullion_width` × `mullion_depth` (centred on the centreline) sits
//!   on every grid line: the vertical ones from the level up by `height`, the horizontal ones
//!   along the full length, so the outermost ones stick out of the wall rectangle by half their
//!   width. A panel of `panel_thickness` (centred) fills each bay between the mullion faces; in a
//!   bay not wider than a mullion there is none. The solid is the union of these boxes
//!   ([`box_union`]), which keeps the pieces from overlapping in the mesh.
//! * **Stair.** `riser_count` blocks, block `k` spanning `[(k−1)·tread, k·tread]` along the run,
//!   `width` across it (centred on the run line) and from the base level up by `k` risers, where a
//!   riser is `(top − base) / riser_count` high; the stepped union of the blocks ([`box_union`]).
//! * **Ramp.** The wedge over the `length` × `width` rectangle starting at (x, y), centred on the
//!   run line, whose underside is at the base level and whose top rises linearly from the base
//!   level at (x, y) to the top level at the far end.
//!
//! Props that are missing or not finite, a non-positive size, a `riser_count` that is not a whole
//! number of at least 1, a stair or ramp whose top is not above its base, or a size beyond the
//! limits below leave the element without quantities or solid. A curtain wall whose pieces cannot
//! be joined into a manifold solid (see [`box_union`]: only with mullions of about 1e-7) keeps its
//! quantities but has no mesh.

use bim_doc::{Document, Entity};
use bim_geom::Point3;

use crate::mesh::{box_union, Aabb};
use crate::{elevation, entity_name, num, CurtainWallGeom, Mesh, StairGeom};

/// Most panels (bays along the wall × bays up it) of a curtain wall that is built: the mesh has
/// some 30 triangles per panel.
const MAX_PANELS: usize = 1 << 16;

/// Most risers of a stair that is built: the stepped solid is meshed cell by cell, so it has
/// about `2·n²` triangles.
const MAX_RISERS: u64 = 1000;

/// The number of bays when a length `total` is cut every `spacing`: `⌈total / spacing − 1e-9⌉`,
/// at least 1 (a spacing beyond the length gives one bay, and a last bay shorter than a billionth
/// of the spacing is not created). Clamped to one more than [`MAX_PANELS`], which every caller
/// rejects, so that no ratio, however large, overflows the cast or the grid.
fn bays(total: f64, spacing: f64) -> usize {
    (total / spacing - 1e-9).ceil().max(1.0).min((MAX_PANELS + 1) as f64) as usize
}

/// The grid lines along a length `total` cut every `spacing`: `k·spacing` for each bay but the
/// last (computed per line, not accumulated), then `total`; so `bays + 1` increasing values from 0
/// to `total` whose last bay takes the remainder. `total` and `spacing` are finite and positive.
pub(crate) fn grid(total: f64, spacing: f64) -> Vec<f64> {
    (0..bays(total, spacing)).map(|k| (k as f64 * spacing).min(total)).chain([total]).collect()
}

/// A local frame in the model: x along the unit vector `u` from `origin`, y along its left, z up
/// from `z0`.
struct Frame {
    origin: [f64; 2],
    u: [f64; 2],
    z0: f64,
}

impl Frame {
    /// The mesh moved from the local frame into the model; `None` if a coordinate overflows.
    fn place(&self, mut mesh: Mesh) -> Option<Mesh> {
        let left = [-self.u[1], self.u[0]];
        for p in &mut mesh.positions {
            let x = self.origin[0] + (self.u[0] * p.x + left[0] * p.y);
            let y = self.origin[1] + (self.u[1] * p.x + left[1] * p.y);
            *p = Point3::new(x, y, p.z + self.z0);
            if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
                return None;
            }
        }
        Some(mesh)
    }
}

/// The quantities and the solid of a `CurtainWall`.
pub(crate) fn curtain_wall(doc: &Document, e: &Entity) -> Option<(CurtainWallGeom, Mesh)> {
    let (x1, y1, x2, y2) = (num(doc, e, "x1")?, num(doc, e, "y1")?, num(doc, e, "x2")?, num(doc, e, "y2")?);
    let (h, u_spacing, v_spacing) = (num(doc, e, "height")?, num(doc, e, "u_spacing")?, num(doc, e, "v_spacing")?);
    let (w, d, pt) = (num(doc, e, "mullion_width")?, num(doc, e, "mullion_depth")?, num(doc, e, "panel_thickness")?);
    let z0 = elevation(doc, e, "level")?;
    let len = (x2 - x1).hypot(y2 - y1);
    if !(len > 1e-9 && len.is_finite()) || ![h, u_spacing, v_spacing, w, d, pt].iter().all(|&x| x > 0.0) {
        return None;
    }
    let (nu, nv) = (bays(len, u_spacing), bays(h, v_spacing));
    if nu as u64 * nv as u64 > MAX_PANELS as u64 {
        return None;
    }
    let (us, vs) = (grid(len, u_spacing), grid(h, v_spacing));

    let mut boxes = Vec::with_capacity(nu + 1 + nv + 1 + nu * nv);
    for &s in &us {
        boxes.push(Aabb { lo: [s - w / 2.0, -d / 2.0, 0.0], hi: [s + w / 2.0, d / 2.0, h] });
    }
    for &t in &vs {
        boxes.push(Aabb { lo: [0.0, -d / 2.0, t - w / 2.0], hi: [len, d / 2.0, t + w / 2.0] });
    }
    for a in us.windows(2) {
        for c in vs.windows(2) {
            // Between the mullion faces; empty (and ignored) in a bay no wider than a mullion.
            boxes.push(Aabb { lo: [a[0] + w / 2.0, -pt / 2.0, c[0] + w / 2.0], hi: [a[1] - w / 2.0, pt / 2.0, c[1] - w / 2.0] });
        }
    }
    let (volume, local) = box_union(&boxes);
    let mesh = Frame { origin: [x1, y1], u: [(x2 - x1) / len, (y2 - y1) / len], z0 }.place(local)?;
    let geom = CurtainWallGeom {
        id: e.id,
        name: entity_name(e),
        panels: nu as u64 * nv as u64,
        mullion_length: (nu + 1) as f64 * h + (nv + 1) as f64 * len,
        volume,
    };
    Some((geom, mesh))
}

/// The quantities and the solid of a `Stair`.
pub(crate) fn stair(doc: &Document, e: &Entity) -> Option<(StairGeom, Mesh)> {
    let (x, y, angle) = (num(doc, e, "x")?, num(doc, e, "y")?, num(doc, e, "angle")?);
    let (width, tread, count) = (num(doc, e, "width")?, num(doc, e, "tread")?, num(doc, e, "riser_count")?);
    let (zb, zt) = (elevation(doc, e, "base")?, elevation(doc, e, "top")?);
    if !(width > 0.0 && tread > 0.0 && zt > zb) || count.fract() != 0.0 || !(1.0..=MAX_RISERS as f64).contains(&count) {
        return None;
    }
    let risers = count as u64;
    let riser = (zt - zb) / count;
    // Each coordinate from its own integer, so the faces two blocks share are bit-identical.
    let boxes: Vec<Aabb> = (1..=risers)
        .map(|k| Aabb { lo: [(k - 1) as f64 * tread, -width / 2.0, 0.0], hi: [k as f64 * tread, width / 2.0, k as f64 * riser] })
        .collect();
    let (volume, local) = box_union(&boxes);
    let mesh = Frame { origin: [x, y], u: [angle.cos(), angle.sin()], z0: zb }.place(local)?;
    Some((StairGeom { id: e.id, name: entity_name(e), riser, tread, risers, volume }, mesh))
}

/// The solid of a `Ramp`.
pub(crate) fn ramp(doc: &Document, e: &Entity) -> Option<Mesh> {
    let (x, y, angle) = (num(doc, e, "x")?, num(doc, e, "y")?, num(doc, e, "angle")?);
    let (width, length) = (num(doc, e, "width")?, num(doc, e, "length")?);
    let (zb, zt) = (elevation(doc, e, "base")?, elevation(doc, e, "top")?);
    let rise = zt - zb;
    if !(width > 0.0 && length > 0.0 && rise > 0.0) {
        return None;
    }
    let hw = width / 2.0;
    let positions = [
        Point3::new(0.0, -hw, 0.0),
        Point3::new(length, -hw, 0.0),
        Point3::new(length, -hw, rise),
        Point3::new(0.0, hw, 0.0),
        Point3::new(length, hw, 0.0),
        Point3::new(length, hw, rise),
    ];
    let triangles = vec![
        [0, 1, 2], [3, 5, 4], // the sides, at −y and +y
        [0, 4, 1], [0, 3, 4], // the underside
        [1, 4, 5], [1, 5, 2], // the high end
        [0, 2, 5], [0, 5, 3], // the slope
    ];
    Frame { origin: [x, y], u: [angle.cos(), angle.sin()], z0: zb }.place(Mesh { positions: positions.to_vec(), triangles })
}
