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

use super::paper::{dim_marks, num, Frame, TEXT};
use super::{Drawing, Layer, Shape};

/// The drawing as an SVG document.
pub fn to_svg(drawing: &Drawing) -> String {
    let frame = Frame::of(drawing);
    let (width, height) = (frame.width, frame.height);
    let paper = |p| frame.down(p);

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
                    let marks = dim_marks(paper(*a), paper(*b));
                    let mut d = String::new();
                    for [p, q] in &marks.segments {
                        let _ = write!(d, "M{} {} L{} {} ", num(p[0]), num(p[1]), num(q[0]), num(q[1]));
                    }
                    let _ = writeln!(s, "<path data-entity=\"{ent}\" {common} d=\"{}\"/>", d.trim_end());
                    let m = marks.text_at;
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
    fn xml_text_is_escaped() {
        assert_eq!(escape("a<\"&"), "a&lt;&quot;&amp;");
    }
}
