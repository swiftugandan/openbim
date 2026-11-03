//! The paper frame shared by every drawing writer ([`crate::svg`], [`crate::dxf`],
//! [`crate::pdf`]): paper millimetres at 1:100, origin at the drawing's lower-left extent minus a
//! margin, and the paper geometry of a dimension (line, ticks, text anchor).
//!
//! The SVG works y down ([`Frame::down`]); DXF and PDF work y up ([`Frame::up`]), which is the
//! SVG's y mirrored inside the sheet (`y_up = height − y_down`), so all formats carry the same
//! coordinates up to the y flip.

use crate::{Drawing, Shape, P2, SCALE};

/// Margin around the drawing, paper mm.
pub(crate) const MARGIN: f64 = 10.0;
/// Dimension text height, paper mm.
pub(crate) const TEXT: f64 = 2.5;
/// Tick half-length and text offset of a dimension, paper mm.
pub(crate) const TICK: f64 = 1.0;

/// The sheet of a drawing: its extent in model metres and its size in paper mm.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    lo: P2,
    hi: P2,
    /// Sheet width, paper mm.
    pub(crate) width: f64,
    /// Sheet height, paper mm.
    pub(crate) height: f64,
}

impl Frame {
    /// The frame of `drawing`: its bounding box plus [`MARGIN`] (an empty drawing is a
    /// `2·MARGIN` square).
    pub(crate) fn of(drawing: &Drawing) -> Frame {
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        let mut grow = |p: P2| {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        };
        for it in &drawing.items {
            match &it.shape {
                Shape::Region(rings) => rings.iter().flatten().for_each(|&p| grow(p)),
                Shape::Lines(segs) => segs.iter().flatten().for_each(|&p| grow(p)),
                Shape::Dimension { a, b, .. } => {
                    grow(*a);
                    grow(*b);
                }
            }
        }
        if !(lo[0].is_finite() && hi[0].is_finite()) {
            lo = [0.0, 0.0];
            hi = [0.0, 0.0];
        }
        Frame { lo, hi, width: (hi[0] - lo[0]) * SCALE + 2.0 * MARGIN, height: (hi[1] - lo[1]) * SCALE + 2.0 * MARGIN }
    }

    /// A model point on paper, y down (SVG).
    pub(crate) fn down(&self, p: P2) -> P2 {
        [(p[0] - self.lo[0]) * SCALE + MARGIN, (self.hi[1] - p[1]) * SCALE + MARGIN]
    }

    /// A model point on paper, y up (DXF, PDF).
    pub(crate) fn up(&self, p: P2) -> P2 {
        [(p[0] - self.lo[0]) * SCALE + MARGIN, (p[1] - self.lo[1]) * SCALE + MARGIN]
    }
}

/// The paper geometry of a dimension, in the y-down paper frame of [`Frame::down`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DimMarks {
    /// The dimension line (absent when both ends coincide) and the two 45° ticks.
    pub(crate) segments: Vec<[P2; 2]>,
    /// The text anchor: the middle of the baseline, one [`TICK`] above the line on paper.
    pub(crate) text_at: P2,
}

/// The marks of a dimension between paper points `a` and `b` (y down).
pub(crate) fn dim_marks(a: P2, b: P2) -> DimMarks {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    // Unit direction along the line and its normal pointing up on paper.
    let (ux, uy) = if len > 1e-9 { (dx / len, dy / len) } else { (1.0, 0.0) };
    let (mut nx, mut ny) = (uy, -ux);
    if ny > 0.0 || (ny == 0.0 && nx < 0.0) {
        nx = -nx;
        ny = -ny;
    }
    let mut segments = Vec::new();
    if len > 1e-9 {
        segments.push([a, b]);
    }
    // 45° architectural ticks at both ends.
    let (tx, ty) = ((ux + nx) * TICK * std::f64::consts::FRAC_1_SQRT_2, (uy + ny) * TICK * std::f64::consts::FRAC_1_SQRT_2);
    for p in [a, b] {
        segments.push([[p[0] - tx, p[1] - ty], [p[0] + tx, p[1] + ty]]);
    }
    DimMarks { segments, text_at: [(a[0] + b[0]) / 2.0 + nx * TICK, (a[1] + b[1]) / 2.0 + ny * TICK] }
}

/// A number with at most four decimals, without trailing zeros (never in exponent notation).
pub(crate) fn num(x: f64) -> String {
    let mut t = format!("{:.4}", x);
    if t.contains('.') {
        while t.ends_with('0') {
            t.pop();
        }
        if t.ends_with('.') {
            t.pop();
        }
    }
    if t == "-0" {
        t = "0".into();
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Item, Layer};

    #[test]
    fn numbers_are_short_and_exact_to_a_tenth_of_a_micron() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(0.25), "0.25");
        assert_eq!(num(-0.00001), "0");
        assert_eq!(num(12.34567), "12.3457");
        assert_eq!(num(1e20), "100000000000000000000");
    }

    #[test]
    fn up_is_down_mirrored_in_the_sheet() {
        let d = Drawing {
            view: "v".into(),
            items: vec![Item { layer: Layer::Cut, entity: "W".into(), shape: Shape::Lines(vec![[[1.0, 2.0], [3.0, 7.0]]]) }],
        };
        let f = Frame::of(&d);
        assert_eq!((f.width, f.height), (40.0, 70.0));
        for p in [[1.0, 2.0], [3.0, 7.0], [2.5, 4.0]] {
            let (u, w) = (f.up(p), f.down(p));
            assert!((u[0] - w[0]).abs() < 1e-12 && (u[1] - (f.height - w[1])).abs() < 1e-9);
        }
        assert_eq!(f.up([1.0, 2.0]), [MARGIN, MARGIN]);
        let e = Frame::of(&Drawing::default());
        assert_eq!((e.width, e.height), (2.0 * MARGIN, 2.0 * MARGIN));
    }
}
