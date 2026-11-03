//! DXF output of a [`Drawing`] (spec/cli.md): AutoCAD 2013 ASCII (`AC1027`).
//!
//! Units are paper millimetres (`$INSUNITS` 4) at 1:100 in the frame of [`Frame::up`]: the SVG's
//! coordinates with y up. Coordinates are written to 0.0001 mm.
//!
//! * One layer per drawing layer of spec/cli.md (all six) with its lineweight (1/100 mm); HIDDEN
//!   uses the `DASHED` linetype (ISO 128 dashed line, dash 12·d and gap 3·d for d = 0.18 mm).
//!   Entities keep lineweight and linetype BYLAYER.
//! * CUT regions are closed LWPOLYLINEs, one per ring; for every cut entity one HATCH (user
//!   defined 45° lines, 1.5 mm apart, odd parity) whose boundary paths are exactly the rings of its
//!   cut region. The HATCH replaces the drawing's HATCH line items of that entity, which are its
//!   pattern (hatch lines of an entity without a cut region stay LINEs).
//! * Line sets are LINEs; a region on any other layer is closed LWPOLYLINEs.
//! * A dimension is the LINEs of its dimension line and ticks plus an MTEXT (bottom centre, 2.5 mm)
//!   holding the value as explicit text.
//! * Every entity carries XDATA of the application `OPENBIM` whose first string (code 1000) is the
//!   id of the entity shown. The id is written whole, even past the 255 bytes DXF allows an XDATA
//!   string, so that it stays exact (spec/cli.md).
//!
//! The file holds the structure AutoCAD expects of an R2013 drawing: HEADER, CLASSES, the nine
//! symbol tables, the model and paper space blocks, and the root dictionary with the layouts.
//! Handles are allocated sequentially and `$HANDSEED` is the next free one.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::paper::{dim_marks, num, Frame, TEXT};
use crate::{Drawing, Layer, Shape, P2, HATCH_SPACING, SCALE};

/// The application name of the XDATA carrying entity ids.
pub const APP: &str = "OPENBIM";
/// The linetype of HIDDEN.
pub const DASHED: &str = "DASHED";

/// Group-code/value writer with a sequential handle allocator.
struct Out {
    s: String,
    next: u32,
}

impl Out {
    fn handle(&mut self) -> u32 {
        let h = self.next;
        self.next += 1;
        h
    }
    /// One group: code and value, each on its own line.
    fn g(&mut self, code: u16, value: impl std::fmt::Display) {
        let _ = write!(self.s, "{code:>3}\n{value}\n");
    }
    fn h(&mut self, code: u16, handle: u32) {
        self.g(code, format_args!("{handle:X}"));
    }
    fn f(&mut self, code: u16, x: f64) {
        let t = num(x);
        // Floats keep a decimal point so readers never take them for integers.
        if t.contains('.') {
            self.g(code, t);
        } else {
            self.g(code, format_args!("{t}.0"));
        }
    }
    fn point(&mut self, code: u16, p: P2) {
        self.f(code, p[0]);
        self.f(code + 10, p[1]);
        self.f(code + 20, 0.0);
    }
    fn xy(&mut self, code: u16, p: P2) {
        self.f(code, p[0]);
        self.f(code + 10, p[1]);
    }
}

/// Handles of the fixed structure.
struct Fixed {
    t_vport: u32,
    t_ltype: u32,
    t_layer: u32,
    t_style: u32,
    t_view: u32,
    t_ucs: u32,
    t_appid: u32,
    t_dimstyle: u32,
    t_block_record: u32,
    br_model: u32,
    br_paper: u32,
    d_root: u32,
    d_group: u32,
    d_layout: u32,
    l_model: u32,
    l_paper: u32,
}

/// The drawing as an AutoCAD 2013 DXF document.
pub fn to_dxf(drawing: &Drawing) -> String {
    let frame = Frame::of(drawing);
    let mut o = Out { s: String::new(), next: 1 };
    let fx = Fixed {
        t_vport: o.handle(),
        t_ltype: o.handle(),
        t_layer: o.handle(),
        t_style: o.handle(),
        t_view: o.handle(),
        t_ucs: o.handle(),
        t_appid: o.handle(),
        t_dimstyle: o.handle(),
        t_block_record: o.handle(),
        br_model: o.handle(),
        br_paper: o.handle(),
        d_root: o.handle(),
        d_group: o.handle(),
        d_layout: o.handle(),
        l_model: o.handle(),
        l_paper: o.handle(),
    };
    // Tables, blocks and objects go to `body` first so that `$HANDSEED` is known for the header.
    let mut body = Out { s: String::new(), next: o.next };
    tables(&mut body, &fx, &frame);
    blocks(&mut body, &fx);
    body.g(0, "SECTION");
    body.g(2, "ENTITIES");
    entities(&mut body, &fx, &frame, drawing);
    body.g(0, "ENDSEC");
    objects(&mut body, &fx);
    body.g(0, "EOF");

    o.g(0, "SECTION");
    o.g(2, "HEADER");
    o.g(9, "$ACADVER");
    o.g(1, "AC1027");
    o.g(9, "$DWGCODEPAGE");
    o.g(3, "ANSI_1252");
    o.g(9, "$INSBASE");
    o.point(10, [0.0, 0.0]);
    o.g(9, "$EXTMIN");
    o.point(10, [0.0, 0.0]);
    o.g(9, "$EXTMAX");
    o.point(10, [frame.width, frame.height]);
    o.g(9, "$LIMMIN");
    o.xy(10, [0.0, 0.0]);
    o.g(9, "$LIMMAX");
    o.xy(10, [frame.width, frame.height]);
    o.g(9, "$LTSCALE");
    o.f(40, 1.0);
    o.g(9, "$CLAYER");
    o.g(8, "0");
    o.g(9, "$HANDSEED");
    o.h(5, body.next);
    o.g(9, "$MEASUREMENT");
    o.g(70, 1);
    o.g(9, "$LWDISPLAY");
    o.g(290, 1);
    o.g(9, "$INSUNITS");
    o.g(70, 4);
    o.g(0, "ENDSEC");
    o.g(0, "SECTION");
    o.g(2, "CLASSES");
    o.g(0, "ENDSEC");
    o.s.push_str(&body.s);
    o.s
}

fn table_head(o: &mut Out, name: &str, handle: u32, count: usize) {
    o.g(0, "TABLE");
    o.g(2, name);
    o.h(5, handle);
    o.h(330, 0);
    o.g(100, "AcDbSymbolTable");
    o.g(70, count);
}

fn record_head(o: &mut Out, kind: &str, subclass: &str, owner: u32, name: &str) -> u32 {
    let h = o.handle();
    o.g(0, kind);
    o.h(if kind == "DIMSTYLE" { 105 } else { 5 }, h);
    o.h(330, owner);
    o.g(100, "AcDbSymbolTableRecord");
    o.g(100, subclass);
    o.g(2, name);
    o.g(70, 0);
    h
}

fn tables(o: &mut Out, fx: &Fixed, frame: &Frame) {
    o.g(0, "SECTION");
    o.g(2, "TABLES");

    table_head(o, "VPORT", fx.t_vport, 1);
    record_head(o, "VPORT", "AcDbViewportTableRecord", fx.t_vport, "*Active");
    o.xy(10, [0.0, 0.0]);
    o.xy(11, [1.0, 1.0]);
    o.xy(12, [frame.width / 2.0, frame.height / 2.0]);
    o.xy(13, [0.0, 0.0]);
    o.xy(14, [0.5, 0.5]);
    o.xy(15, [0.5, 0.5]);
    o.f(16, 0.0);
    o.f(26, 0.0);
    o.f(36, 1.0);
    o.point(17, [0.0, 0.0]);
    o.f(40, frame.height);
    o.f(41, if frame.height > 0.0 { frame.width / frame.height } else { 1.0 });
    o.f(42, 50.0);
    o.f(43, 0.0);
    o.f(44, 0.0);
    o.f(50, 0.0);
    o.f(51, 0.0);
    for (code, v) in [(71, 0), (72, 1000), (73, 1), (74, 3), (75, 0), (76, 0), (77, 0), (78, 0), (281, 0), (65, 1)] {
        o.g(code, v);
    }
    o.f(146, 0.0);
    o.g(0, "ENDTAB");

    table_head(o, "LTYPE", fx.t_ltype, 4);
    for name in ["ByBlock", "ByLayer", "Continuous"] {
        record_head(o, "LTYPE", "AcDbLinetypeTableRecord", fx.t_ltype, name);
        o.g(3, if name == "Continuous" { "Solid line" } else { "" });
        o.g(72, 65);
        o.g(73, 0);
        o.f(40, 0.0);
    }
    // ISO 128 dashed line for the HIDDEN lineweight d: dash 12·d, gap 3·d.
    let d = Layer::Hidden.width();
    record_head(o, "LTYPE", "AcDbLinetypeTableRecord", fx.t_ltype, DASHED);
    o.g(3, "ISO dashed __ __ __");
    o.g(72, 65);
    o.g(73, 2);
    o.f(40, 15.0 * d);
    o.f(49, 12.0 * d);
    o.g(74, 0);
    o.f(49, -3.0 * d);
    o.g(74, 0);
    o.g(0, "ENDTAB");

    table_head(o, "LAYER", fx.t_layer, 1 + Layer::ALL.len());
    record_head(o, "LAYER", "AcDbLayerTableRecord", fx.t_layer, "0");
    o.g(62, 7);
    o.g(6, "Continuous");
    o.g(370, -3);
    for layer in Layer::ALL {
        record_head(o, "LAYER", "AcDbLayerTableRecord", fx.t_layer, layer.name());
        o.g(62, 7);
        o.g(6, if layer.dashed() { DASHED } else { "Continuous" });
        o.g(370, lineweight(layer));
    }
    o.g(0, "ENDTAB");

    table_head(o, "STYLE", fx.t_style, 1);
    record_head(o, "STYLE", "AcDbTextStyleTableRecord", fx.t_style, "Standard");
    o.f(40, 0.0);
    o.f(41, 1.0);
    o.f(50, 0.0);
    o.g(71, 0);
    o.f(42, TEXT);
    o.g(3, "txt");
    o.g(4, "");
    o.g(0, "ENDTAB");

    table_head(o, "VIEW", fx.t_view, 0);
    o.g(0, "ENDTAB");
    table_head(o, "UCS", fx.t_ucs, 0);
    o.g(0, "ENDTAB");

    table_head(o, "APPID", fx.t_appid, 2);
    for name in ["ACAD", APP] {
        record_head(o, "APPID", "AcDbRegAppTableRecord", fx.t_appid, name);
    }
    o.g(0, "ENDTAB");

    table_head(o, "DIMSTYLE", fx.t_dimstyle, 1);
    o.g(100, "AcDbDimStyleTable");
    record_head(o, "DIMSTYLE", "AcDbDimStyleTableRecord", fx.t_dimstyle, "Standard");
    o.g(0, "ENDTAB");

    table_head(o, "BLOCK_RECORD", fx.t_block_record, 2);
    for (h, name, layout) in [(fx.br_model, "*Model_Space", fx.l_model), (fx.br_paper, "*Paper_Space", fx.l_paper)] {
        o.g(0, "BLOCK_RECORD");
        o.h(5, h);
        o.h(330, fx.t_block_record);
        o.g(100, "AcDbSymbolTableRecord");
        o.g(100, "AcDbBlockTableRecord");
        o.g(2, name);
        o.h(340, layout);
        o.g(70, 0);
        o.g(280, 1);
        o.g(281, 0);
    }
    o.g(0, "ENDTAB");
    o.g(0, "ENDSEC");
}

fn blocks(o: &mut Out, fx: &Fixed) {
    o.g(0, "SECTION");
    o.g(2, "BLOCKS");
    for (owner, name) in [(fx.br_model, "*Model_Space"), (fx.br_paper, "*Paper_Space")] {
        let h = o.handle();
        o.g(0, "BLOCK");
        o.h(5, h);
        o.h(330, owner);
        o.g(100, "AcDbEntity");
        o.g(8, "0");
        o.g(100, "AcDbBlockBegin");
        o.g(2, name);
        o.g(70, 0);
        o.point(10, [0.0, 0.0]);
        o.g(3, name);
        o.g(1, "");
        let h = o.handle();
        o.g(0, "ENDBLK");
        o.h(5, h);
        o.h(330, owner);
        o.g(100, "AcDbEntity");
        o.g(8, "0");
        o.g(100, "AcDbBlockEnd");
    }
    o.g(0, "ENDSEC");
}

/// The common head of a model space entity.
fn entity_head(o: &mut Out, fx: &Fixed, kind: &str, layer: Layer, subclass: &str) {
    let h = o.handle();
    o.g(0, kind);
    o.h(5, h);
    o.h(330, fx.br_model);
    o.g(100, "AcDbEntity");
    o.g(8, layer.name());
    o.g(100, subclass);
}

/// The XDATA naming the entity shown (always the last groups of an entity).
fn xdata(o: &mut Out, entity: &str) {
    o.g(1001, APP);
    o.g(1000, string(entity));
}

fn line(o: &mut Out, fx: &Fixed, layer: Layer, entity: &str, a: P2, b: P2) {
    entity_head(o, fx, "LINE", layer, "AcDbLine");
    o.point(10, a);
    o.point(11, b);
    xdata(o, entity);
}

fn closed_polyline(o: &mut Out, fx: &Fixed, layer: Layer, entity: &str, ring: &[P2]) {
    entity_head(o, fx, "LWPOLYLINE", layer, "AcDbPolyline");
    o.g(90, ring.len());
    o.g(70, 1);
    for &p in ring {
        o.xy(10, p);
    }
    xdata(o, entity);
}

fn entities(o: &mut Out, fx: &Fixed, frame: &Frame, drawing: &Drawing) {
    let up = |p: P2| frame.up(p);
    // The cut region of every cut entity, for its HATCH: rings in item order.
    let mut cut: BTreeMap<&str, Vec<Vec<P2>>> = BTreeMap::new();
    for it in &drawing.items {
        if let (Layer::Cut, Shape::Region(rings)) = (it.layer, &it.shape) {
            let rings = rings.iter().filter(|r| r.len() >= 3).map(|r| r.iter().map(|&p| up(p)).collect());
            cut.entry(it.entity.as_str()).or_default().extend(rings);
        }
    }
    for layer in Layer::ALL {
        if layer == Layer::Hatch {
            for (entity, rings) in &cut {
                if !rings.is_empty() {
                    hatch(o, fx, frame, entity, rings);
                }
            }
        }
        for it in drawing.items.iter().filter(|it| it.layer == layer) {
            match &it.shape {
                Shape::Region(rings) => {
                    for ring in rings.iter().filter(|r| r.len() >= 2) {
                        let ring: Vec<P2> = ring.iter().map(|&p| up(p)).collect();
                        closed_polyline(o, fx, layer, &it.entity, &ring);
                    }
                }
                // The HATCH entity of a cut entity carries its hatch pattern.
                Shape::Lines(_) if layer == Layer::Hatch && cut.get(it.entity.as_str()).is_some_and(|r| !r.is_empty()) => {}
                Shape::Lines(segs) => {
                    for seg in segs {
                        line(o, fx, layer, &it.entity, up(seg[0]), up(seg[1]));
                    }
                }
                Shape::Dimension { a, b, text } => {
                    // Dimension marks are built in the SVG's y-down frame and mirrored.
                    let flip = |p: P2| [p[0], frame.height - p[1]];
                    let marks = dim_marks(frame.down(*a), frame.down(*b));
                    for [p, q] in &marks.segments {
                        line(o, fx, layer, &it.entity, flip(*p), flip(*q));
                    }
                    entity_head(o, fx, "MTEXT", layer, "AcDbMText");
                    o.point(10, flip(marks.text_at));
                    o.f(40, TEXT);
                    o.f(41, 0.0);
                    o.g(71, 8); // bottom centre
                    o.g(72, 1); // left to right
                    o.g(1, mtext(text));
                    o.g(7, "Standard");
                    xdata(o, &it.entity);
                }
            }
        }
    }
}

/// A HATCH of 45° lines filling the rings (odd parity) on HATCH.
fn hatch(o: &mut Out, fx: &Fixed, frame: &Frame, entity: &str, rings: &[Vec<P2>]) {
    entity_head(o, fx, "HATCH", Layer::Hatch, "AcDbHatch");
    o.point(10, [0.0, 0.0]);
    o.f(210, 0.0);
    o.f(220, 0.0);
    o.f(230, 1.0);
    o.g(2, "_USER");
    o.g(70, 0); // pattern fill
    o.g(71, 0); // not associative
    o.g(91, rings.len());
    for (i, ring) in rings.iter().enumerate() {
        o.g(92, if i == 0 { 2 | 1 } else { 2 }); // polyline path (the first one external)
        o.g(72, 0);
        o.g(73, 1);
        o.g(93, ring.len());
        for &p in ring {
            o.xy(10, p);
        }
        o.g(97, 0);
    }
    let spacing = HATCH_SPACING * SCALE;
    let off = spacing * std::f64::consts::FRAC_1_SQRT_2;
    // The drawing's hatch lines pass through the model origin (`hatch_lines`); the pattern's base
    // point is that line family's crossing of the x axis nearest the sheet origin, so the HATCH
    // renders the very lines of the SVG and PDF.
    let origin = frame.up([0.0, 0.0]);
    let base = (origin[0] - origin[1]).rem_euclid(spacing * std::f64::consts::SQRT_2);
    o.g(75, 0); // odd parity
    o.g(76, 0); // user defined
    o.f(52, 45.0);
    o.f(41, spacing);
    o.g(77, 0);
    o.g(78, 1);
    // One pattern line: 53 angle, 43/44 base point, 45/46 offset (not code + 10 pairs).
    o.f(53, 45.0);
    // Base point and offset keep ten decimals so that the line spacing is 1.5 mm, not 1.5001.
    o.g(43, format_args!("{base:.10}"));
    o.f(44, 0.0);
    o.g(45, format_args!("{:.10}", -off));
    o.g(46, format_args!("{:.10}", off));
    o.g(79, 0);
    o.g(98, 0);
    xdata(o, entity);
}

fn objects(o: &mut Out, fx: &Fixed) {
    o.g(0, "SECTION");
    o.g(2, "OBJECTS");
    o.g(0, "DICTIONARY");
    o.h(5, fx.d_root);
    o.h(330, 0);
    o.g(100, "AcDbDictionary");
    o.g(281, 1);
    o.g(3, "ACAD_GROUP");
    o.h(350, fx.d_group);
    o.g(3, "ACAD_LAYOUT");
    o.h(350, fx.d_layout);
    o.g(0, "DICTIONARY");
    o.h(5, fx.d_group);
    o.h(330, fx.d_root);
    o.g(100, "AcDbDictionary");
    o.g(281, 1);
    o.g(0, "DICTIONARY");
    o.h(5, fx.d_layout);
    o.h(330, fx.d_root);
    o.g(100, "AcDbDictionary");
    o.g(281, 1);
    o.g(3, "Model");
    o.h(350, fx.l_model);
    o.g(3, "Layout1");
    o.h(350, fx.l_paper);
    for (h, name, model, br) in [(fx.l_model, "Model", true, fx.br_model), (fx.l_paper, "Layout1", false, fx.br_paper)] {
        o.g(0, "LAYOUT");
        o.h(5, h);
        o.h(330, fx.d_layout);
        o.g(100, "AcDbPlotSettings");
        o.g(1, "");
        o.g(4, "A3");
        o.g(6, "");
        for (code, v) in [(40, 7.5), (41, 20.0), (42, 7.5), (43, 20.0), (44, 420.0), (45, 297.0), (46, 0.0), (47, 0.0), (48, 0.0), (49, 0.0), (140, 0.0), (141, 0.0), (142, 1.0), (143, 1.0)] {
            o.f(code, v);
        }
        o.g(70, if model { 1024 } else { 0 });
        o.g(72, 1);
        o.g(73, 0);
        o.g(74, 5);
        o.g(7, "");
        o.g(75, 16);
        o.g(76, 0);
        o.g(77, 2);
        o.g(78, 300);
        o.f(147, 1.0);
        o.f(148, 0.0);
        o.f(149, 0.0);
        o.g(100, "AcDbLayout");
        o.g(1, name);
        o.g(70, 1);
        o.g(71, if model { 0 } else { 1 });
        o.xy(10, [0.0, 0.0]);
        o.xy(11, [420.0, 297.0]);
        o.point(12, [0.0, 0.0]);
        // Empty extents.
        o.g(14, "1e+20");
        o.g(24, "1e+20");
        o.g(34, "1e+20");
        o.g(15, "-1e+20");
        o.g(25, "-1e+20");
        o.g(35, "-1e+20");
        o.f(146, 0.0);
        o.point(13, [0.0, 0.0]);
        o.point(16, [1.0, 0.0]);
        o.point(17, [0.0, 1.0]);
        o.g(76, 1);
        o.h(330, br);
    }
    o.g(0, "ENDSEC");
}

/// The DXF lineweight of a layer, 1/100 mm.
pub fn lineweight(layer: Layer) -> i32 {
    (layer.width() * 100.0).round() as i32
}

/// A string value: one line, without control characters.
fn string(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// MTEXT content: `\`, `{` and `}` escaped, one line.
fn mtext(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in string(s).chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Item;

    fn sample() -> Drawing {
        let sq = |x: f64, y: f64, s: f64| vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]];
        Drawing {
            view: "plan:L1@1.2".into(),
            items: vec![
                Item { layer: Layer::Hatch, entity: "W".into(), shape: Shape::Lines(vec![[[0.1, 0.1], [0.3, 0.3]]]) },
                Item { layer: Layer::Hidden, entity: "S".into(), shape: Shape::Lines(vec![[[-1.0, -1.0], [5.0, -1.0]]]) },
                Item { layer: Layer::Cut, entity: "W".into(), shape: Shape::Region(vec![sq(0.0, 0.0, 4.0), sq(1.0, 1.0, 1.0)]) },
                Item { layer: Layer::Dimension, entity: "D".into(), shape: Shape::Dimension { a: [0.0, 0.0], b: [4.0, 0.0], text: "4000".into() } },
            ],
        }
    }

    /// The (code, value) pairs of a DXF text.
    fn groups(s: &str) -> Vec<(i32, &str)> {
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len() % 2, 0);
        lines.chunks(2).map(|c| (c[0].trim().parse().unwrap(), c[1])).collect()
    }

    #[test]
    fn handles_are_unique_and_below_the_seed() {
        let s = to_dxf(&sample());
        let g = groups(&s);
        let mut handles: Vec<u32> = g.iter().filter(|(c, _)| *c == 5 || *c == 105).map(|(_, v)| u32::from_str_radix(v, 16).unwrap()).collect();
        let seed = handles.remove(0); // $HANDSEED comes first
        let n = handles.len();
        handles.sort();
        handles.dedup();
        assert_eq!(handles.len(), n, "duplicate handles");
        assert!(handles.iter().all(|&h| h > 0 && h < seed));
        assert_eq!(seed, handles.last().unwrap() + 1);
        assert_eq!(g.last(), Some(&(0, "EOF")));
        assert!(s.contains("  1\nAC1027\n"));
    }

    #[test]
    fn every_entity_names_its_entity_and_holes_are_hatch_paths() {
        let s = to_dxf(&sample());
        let g = groups(&s);
        let start = g.iter().position(|&(c, v)| c == 2 && v == "ENTITIES").unwrap();
        let end = start + g[start..].iter().position(|&(c, v)| c == 0 && v == "ENDSEC").unwrap();
        let mut ents: Vec<Vec<(i32, &str)>> = Vec::new();
        for &gv in &g[start + 1..end] {
            if gv.0 == 0 {
                ents.push(Vec::new());
            }
            ents.last_mut().unwrap().push(gv);
        }
        let kinds: Vec<&str> = ents.iter().map(|e| e[0].1).collect();
        assert_eq!(kinds, ["HATCH", "LINE", "LWPOLYLINE", "LWPOLYLINE", "LINE", "LINE", "LINE", "MTEXT"]);
        for e in &ents {
            let n = e.len();
            assert_eq!(e[n - 2], (1001, APP));
            assert!(["W", "S", "D"].contains(&e[n - 1].1));
            assert!(!e.iter().any(|&(c, _)| c == 370), "per-entity lineweight");
        }
        assert!(ents[0].contains(&(91, "2")) && ents[0].contains(&(1000, "W")));
        assert!(ents[7].contains(&(1, "4000")));
        assert!(s.contains("  2\nHIDDEN\n 70\n0\n 62\n7\n  6\nDASHED\n370\n18\n"));
        assert!(s.contains("  2\nCUT\n 70\n0\n 62\n7\n  6\nContinuous\n370\n50\n"));
    }

    #[test]
    fn the_hatch_pattern_line_uses_its_own_group_codes() {
        let s = to_dxf(&sample());
        let g = groups(&s);
        let at = g.iter().position(|&(c, v)| c == 0 && v == "HATCH").unwrap();
        let end = at + 1 + g[at + 1..].iter().position(|&(c, _)| c == 0).unwrap();
        let h = &g[at..end];
        let p = h.iter().position(|&(c, _)| c == 78).unwrap();
        assert_eq!(h[p].1, "1");
        let codes: Vec<i32> = h[p + 1..p + 7].iter().map(|&(c, _)| c).collect();
        assert_eq!(codes, [53, 43, 44, 45, 46, 79]);
        let v: Vec<f64> = h[p + 1..p + 6].iter().map(|&(_, v)| v.parse().unwrap()).collect();
        let off = HATCH_SPACING * SCALE * std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(v[0], 45.0);
        assert!((v[3] + off).abs() < 1e-9 && (v[4] - off).abs() < 1e-9);
        // The base point is on the line through the model origin, as the drawing's hatch lines are.
        let period = HATCH_SPACING * SCALE * std::f64::consts::SQRT_2;
        let o = Frame::of(&sample()).up([0.0, 0.0]);
        let k = (v[1] - v[2] - (o[0] - o[1])) / period;
        assert!((k - k.round()).abs() < 1e-6 && (0.0..period).contains(&v[1]) && v[2] == 0.0);
        let mut shifted = sample();
        for it in &mut shifted.items {
            if let Shape::Lines(segs) = &mut it.shape {
                segs.iter_mut().flatten().for_each(|p| p[0] -= 0.37);
            }
        }
        let o = Frame::of(&shifted).up([0.0, 0.0]);
        let s = to_dxf(&shifted);
        let g = groups(&s);
        let at = g.iter().position(|&(c, v)| c == 0 && v == "HATCH").unwrap();
        let b: f64 = g[at..].iter().find(|&&(c, _)| c == 43).unwrap().1.parse().unwrap();
        let k = (b - (o[0] - o[1])) / period;
        assert!((k - k.round()).abs() < 1e-6 && b > 0.0);
        assert_eq!(h[p + 6].1, "0");
        assert_eq!(h[p + 7], (98, "0"));
        assert!(!h.iter().any(|&(c, _)| c == 55 || c == 63 || c == 65), "no code + 10 pattern groups");
    }

    #[test]
    fn xdata_ids_are_written_whole_even_past_255_bytes() {
        let id = "é".repeat(200); // 400 bytes
        let s = to_dxf(&Drawing {
            view: "v".into(),
            items: vec![Item { layer: Layer::Projection, entity: id.clone(), shape: Shape::Lines(vec![[[0.0, 0.0], [1.0, 1.0]]]) }],
        });
        assert!(groups(&s).contains(&(1000, id.as_str())));
    }

    #[test]
    fn an_empty_drawing_is_still_a_drawing() {
        let s = to_dxf(&Drawing::default());
        assert!(s.contains("ENTITIES\n  0\nENDSEC\n") && s.contains("OPENBIM") && s.ends_with("  0\nEOF\n"));
        assert_eq!(mtext("a{b}\\c\nd"), "a\\{b\\}\\\\c d");
    }
}
