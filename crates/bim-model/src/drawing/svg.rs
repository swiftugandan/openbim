//! SVG output of a [`Drawing`] (spec/cli.md).
//!
//! User units are paper millimetres (`width`/`height` in `mm` equal to the `viewBox` size), y
//! down, model up is up on paper, scale 1:100, origin at the drawing's lower-left extent plus a
//! margin. Coordinates are written to 0.0001 mm. One `<g id="<LAYER>">` per layer of spec/cli.md
//! (all six, possibly empty); inside, every element carries its own `data-entity` and
//! `stroke-width` (the layer's lineweight in mm), HIDDEN elements a `stroke-dasharray`. Regions are
//! `<path>`s of closed subpaths (`M … Z`, holes as extra subpaths, `fill-rule="evenodd"`), line
//! sets `<path>`s of open two-point subpaths, dimensions a `<path>` (dimension line and ticks) plus
//! a `<text>` with the value.

use std::fmt::Write;

use super::{Drawing, Layer, Shape, P2, SCALE};

/// Margin around the drawing, paper mm.
const MARGIN: f64 = 10.0;
/// Dimension text height, paper mm.
const TEXT: f64 = 2.5;
/// Tick half-length and text offset of a dimension, paper mm.
const TICK: f64 = 1.0;

/// The drawing as an SVG document.
pub fn to_svg(drawing: &Drawing) -> String {
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
    let width = (hi[0] - lo[0]) * SCALE + 2.0 * MARGIN;
    let height = (hi[1] - lo[1]) * SCALE + 2.0 * MARGIN;
    let paper = |p: P2| -> P2 { [(p[0] - lo[0]) * SCALE + MARGIN, (hi[1] - p[1]) * SCALE + MARGIN] };

    let mut s = String::new();
    let _ = writeln!(s, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\" data-view=\"{v}\">",
        w = num(width),
        h = num(height),
        v = escape(&drawing.view)
    );
    for layer in Layer::ALL {
        let _ = writeln!(
            s,
            "<g id=\"{}\" fill=\"none\" stroke=\"#000\" stroke-linecap=\"round\" stroke-linejoin=\"round\">",
            layer.name()
        );
        let dash = if layer.dashed() { " stroke-dasharray=\"2.16 0.54\"" } else { "" };
        let common = format!("stroke-width=\"{}\"{dash}", num(layer.width()));
        for it in drawing.items.iter().filter(|it| it.layer == layer) {
            let ent = escape(&it.entity);
            match &it.shape {
                Shape::Region(rings) => {
                    let mut d = String::new();
                    for ring in rings {
                        for (i, &p) in ring.iter().enumerate() {
                            let q = paper(p);
                            let _ = write!(d, "{}{} {} ", if i == 0 { "M" } else { "L" }, num(q[0]), num(q[1]));
                        }
                        d.push_str("Z ");
                    }
                    let _ = writeln!(s, "<path data-entity=\"{ent}\" {common} fill-rule=\"evenodd\" d=\"{}\"/>", d.trim_end());
                }
                Shape::Lines(segs) => {
                    let mut d = String::new();
                    for seg in segs {
                        let (a, b) = (paper(seg[0]), paper(seg[1]));
                        let _ = write!(d, "M{} {} L{} {} ", num(a[0]), num(a[1]), num(b[0]), num(b[1]));
                    }
                    let _ = writeln!(s, "<path data-entity=\"{ent}\" {common} d=\"{}\"/>", d.trim_end());
                }
                Shape::Dimension { a, b, text } => {
                    let (a, b) = (paper(*a), paper(*b));
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let len = dx.hypot(dy);
                    // Unit direction along the line and its normal pointing up on paper.
                    let (ux, uy) = if len > 1e-9 { (dx / len, dy / len) } else { (1.0, 0.0) };
                    let (mut nx, mut ny) = (uy, -ux);
                    if ny > 0.0 || (ny == 0.0 && nx < 0.0) {
                        nx = -nx;
                        ny = -ny;
                    }
                    let mut d = String::new();
                    if len > 1e-9 {
                        let _ = write!(d, "M{} {} L{} {} ", num(a[0]), num(a[1]), num(b[0]), num(b[1]));
                    }
                    // 45° architectural ticks at both ends.
                    let (tx, ty) = ((ux + nx) * TICK * std::f64::consts::FRAC_1_SQRT_2, (uy + ny) * TICK * std::f64::consts::FRAC_1_SQRT_2);
                    for p in [a, b] {
                        let _ = write!(d, "M{} {} L{} {} ", num(p[0] - tx), num(p[1] - ty), num(p[0] + tx), num(p[1] + ty));
                    }
                    let _ = writeln!(s, "<path data-entity=\"{ent}\" {common} d=\"{}\"/>", d.trim_end());
                    let m = [(a[0] + b[0]) / 2.0 + nx * TICK, (a[1] + b[1]) / 2.0 + ny * TICK];
                    let _ = writeln!(
                        s,
                        "<text data-entity=\"{ent}\" x=\"{}\" y=\"{}\" font-size=\"{}\" font-family=\"sans-serif\" text-anchor=\"middle\" fill=\"#000\" stroke=\"none\">{}</text>",
                        num(m[0]),
                        num(m[1]),
                        num(TEXT),
                        escape(text)
                    );
                }
            }
        }
        s.push_str("</g>\n");
    }
    s.push_str("</svg>\n");
    s
}

/// A number with at most four decimals, without trailing zeros.
fn num(x: f64) -> String {
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

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_short_and_exact_to_a_tenth_of_a_micron() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(0.25), "0.25");
        assert_eq!(num(-0.00001), "0");
        assert_eq!(num(12.34567), "12.3457");
        assert_eq!(escape("a<\"&"), "a&lt;&quot;&amp;");
    }
}
