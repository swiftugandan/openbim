//! PDF output of a [`Drawing`] (spec/cli.md): PDF 1.7, one page.
//!
//! The page is the drawing's sheet ([`Frame`]) in points (1 mm = 72/25.4 pt), y up, so its
//! coordinates are the SVG's with y up, scaled to points; they are written to 0.0001 pt with an
//! identity CTM. Every drawing layer of spec/cli.md is an optional content group named as the
//! layer (all six, listed in the catalog's `/OCProperties`); the strokes of a layer are drawn
//! inside `/OC /<LAYER> BDC … EMC`, the key resolving through the page's `/Resources/Properties`,
//! with the layer's line width in points, round caps and joins, and HIDDEN dashed (ISO 128: dash
//! 12·d, gap 3·d). Regions are stroked closed subpaths (`m l … h`), line sets open two-point
//! subpaths, dimensions their line and ticks plus the value as Helvetica text (`Tj`, centred on
//! the SVG's text anchor). The content stream is uncompressed.

use std::fmt::Write;

use crate::paper::{dim_marks, num, Frame, TEXT};
use crate::{Drawing, Layer, Shape, P2};

/// Points per paper millimetre.
pub const PT: f64 = 72.0 / 25.4;

/// The drawing as a PDF 1.7 document.
pub fn to_pdf(drawing: &Drawing) -> Vec<u8> {
    let frame = Frame::of(drawing);
    let pt = |p: P2| -> String {
        let q = frame.up(p);
        format!("{} {}", num(q[0] * PT), num(q[1] * PT))
    };
    // Paper points of the SVG's y-down frame (dimension marks), mirrored to y up.
    let pt_down = |q: P2| -> String { format!("{} {}", num(q[0] * PT), num((frame.height - q[1]) * PT)) };

    let mut c = String::new();
    for layer in Layer::ALL {
        let items: Vec<_> = drawing.items.iter().filter(|it| it.layer == layer).collect();
        if items.is_empty() {
            continue;
        }
        let _ = writeln!(c, "/OC /{} BDC", layer.name());
        let _ = writeln!(c, "q 0 G 0 g 1 J 1 j {} w", num(layer.width() * PT));
        if layer.dashed() {
            let d = layer.width();
            let _ = writeln!(c, "[{} {}] 0 d", num(12.0 * d * PT), num(3.0 * d * PT));
        }
        for it in items {
            match &it.shape {
                Shape::Region(rings) => {
                    let mut any = false;
                    for ring in rings.iter().filter(|r| !r.is_empty()) {
                        for (i, &p) in ring.iter().enumerate() {
                            let _ = writeln!(c, "{} {}", pt(p), if i == 0 { "m" } else { "l" });
                        }
                        c.push_str("h\n");
                        any = true;
                    }
                    if any {
                        c.push_str("S\n");
                    }
                }
                Shape::Lines(segs) => {
                    for seg in segs {
                        let _ = writeln!(c, "{} m {} l", pt(seg[0]), pt(seg[1]));
                    }
                    if !segs.is_empty() {
                        c.push_str("S\n");
                    }
                }
                Shape::Dimension { a, b, text } => {
                    let marks = dim_marks(frame.down(*a), frame.down(*b));
                    for [p, q] in &marks.segments {
                        let _ = writeln!(c, "{} m {} l", pt_down(*p), pt_down(*q));
                    }
                    c.push_str("S\n");
                    let size = TEXT * PT;
                    let at = marks.text_at;
                    let x = at[0] * PT - text_width(text) * size / 2.0;
                    let y = (frame.height - at[1]) * PT;
                    let _ = writeln!(c, "BT /F1 {} Tf {} {} Td ({}) Tj ET", num(size), num(x), num(y), pdf_string(text));
                }
            }
        }
        c.push_str("Q\nEMC\n");
    }

    // Objects: 1 catalog, 2 pages, 3 page, 4 font, 5.. the layer OCGs, then the content stream.
    let ocg = |i: usize| 5 + i;
    let content = ocg(Layer::ALL.len());
    let refs = (0..Layer::ALL.len()).map(|i| format!("{} 0 R", ocg(i))).collect::<Vec<_>>().join(" ");
    let props = Layer::ALL.iter().enumerate().map(|(i, l)| format!("/{} {} 0 R", l.name(), ocg(i))).collect::<Vec<_>>().join(" ");
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [{refs}] /D << /Name (Layers) /Order [{refs}] /ON [{refs}] >> >> >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 4 0 R >> /Properties << {props} >> >> /Contents {content} 0 R >>",
            num(frame.width * PT),
            num(frame.height * PT)
        )
        .into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
    ];
    for layer in Layer::ALL {
        objs.push(format!("<< /Type /OCG /Name ({}) >>", layer.name()).into_bytes());
    }
    let mut stream = format!("<< /Length {} >>\nstream\n", c.len()).into_bytes();
    stream.extend_from_slice(c.as_bytes());
    stream.extend_from_slice(b"endstream");
    objs.push(stream);

    let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let mut tail = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for off in offsets {
        let _ = write!(tail, "{off:010} 00000 n \n");
    }
    let _ = write!(tail, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1);
    out.extend_from_slice(tail.as_bytes());
    out
}

/// The advance width of `s` in Helvetica, in text space units per unit font size.
fn text_width(s: &str) -> f64 {
    s.chars()
        .map(|ch| match ch {
            ' ' | '.' | ',' | '/' | ':' | ';' | '!' | 'I' | 'f' | 't' => 278,
            'i' | 'j' | 'l' => 222,
            '-' | '(' | ')' | 'r' => 333,
            'm' | 'M' => 833,
            'W' => 944,
            'w' => 722,
            _ => 556,
        })
        .sum::<u32>() as f64
        / 1000.0
}

/// A PDF literal string body in WinAnsi: `(`, `)` and `\` escaped, characters outside Latin-1
/// printable as `?`.
fn pdf_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            ' '..='~' => out.push(ch),
            '\u{a0}'..='\u{ff}' => {
                let _ = write!(out, "\\{:03o}", ch as u32);
            }
            _ => out.push('?'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Item;

    fn sample() -> Drawing {
        Drawing {
            view: "plan:L1@1.2".into(),
            items: vec![
                Item { layer: Layer::Cut, entity: "W".into(), shape: Shape::Region(vec![vec![[0.0, 0.0], [4.0, 0.0], [4.0, 1.0]]]) },
                Item { layer: Layer::Hidden, entity: "S".into(), shape: Shape::Lines(vec![[[0.0, 2.0], [4.0, 2.0]]]) },
                Item { layer: Layer::Dimension, entity: "D".into(), shape: Shape::Dimension { a: [0.0, 0.0], b: [4.0, 0.0], text: "4000".into() } },
            ],
        }
    }

    #[test]
    fn xref_offsets_point_at_their_objects() {
        let pdf = to_pdf(&sample());
        assert!(pdf.starts_with(b"%PDF-1.7\n"));
        let text = String::from_utf8_lossy(&pdf).into_owned();
        let sx: usize = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(pdf[sx..].starts_with(b"xref\n0 12\n"));
        let tail = std::str::from_utf8(&pdf[sx..]).unwrap();
        let entries: Vec<&str> = tail.lines().skip(3).take(11).collect();
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(e.len(), 19, "{e:?}"); // 20 bytes with the end of line
            let off: usize = e[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{} 0 obj\n", i + 1).as_bytes()), "object {}", i + 1);
        }
        let len: usize = text.split("/Length ").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
        let start = text.find("stream\n").unwrap() + 7;
        assert_eq!(&text[start + len..start + len + 9], "endstream");
    }

    #[test]
    fn strokes_sit_in_their_layer_with_the_layer_width_in_points() {
        let pdf = String::from_utf8_lossy(&to_pdf(&sample())).into_owned();
        assert!(pdf.contains("/OC /CUT BDC\nq 0 G 0 g 1 J 1 j 1.4173 w\n28.3465 28.3465 m\n"));
        assert!(pdf.contains("/OC /HIDDEN BDC\nq 0 G 0 g 1 J 1 j 0.5102 w\n[6.1228 1.5307] 0 d\n"));
        assert!(pdf.contains("(4000) Tj ET\nQ\nEMC\n"));
        for l in Layer::ALL {
            assert!(pdf.contains(&format!("/{0} ", l.name())) && pdf.contains(&format!("/Name ({}) ", l.name())));
        }
        assert_eq!(pdf.matches(" BDC").count(), pdf.matches("EMC").count());
        assert_eq!(pdf_string("a(b)\\é€"), "a\\(b\\)\\\\\\351?");
    }

    #[test]
    fn an_empty_drawing_is_one_blank_page() {
        let pdf = String::from_utf8_lossy(&to_pdf(&Drawing::default())).into_owned();
        assert!(pdf.contains("/MediaBox [0 0 56.6929 56.6929]") && pdf.contains("/Length 0 >>"));
    }
}
