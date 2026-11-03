//! glTF 2.0 writer (spec/cli.md, `bim export --format gltf`): one JSON document with the buffer
//! embedded as a base64 `data:` URI, one mesh node per element carrying `extras.id` (the entity's
//! name), metres, glTF's y-up frame: glTF (x, y, z) = model (x, z, −y). Each node is translated
//! to its element's bounding-box centre and its float32 positions are relative to that, so
//! precision does not depend on the distance from the origin.

use std::fmt::Write as _;

use crate::Element;

/// Model frame (z up) to glTF frame (y up).
fn y_up(x: f64, y: f64, z: f64) -> [f64; 3] {
    [x, z, -y]
}

/// The glTF document for `elements`, as a string.
pub fn to_gltf(elements: &[Element]) -> String {
    let mut bin: Vec<u8> = Vec::new();
    let (mut views, mut accessors, mut meshes, mut nodes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for el in elements {
        let Some((lo, hi)) = el.mesh.bounds() else { continue };
        if el.mesh.triangles.is_empty() {
            continue;
        }
        let c = [(lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0, (lo.z + hi.z) / 2.0];
        let local: Vec<[f32; 3]> = el
            .mesh
            .positions
            .iter()
            .map(|p| y_up(p.x - c[0], p.y - c[1], p.z - c[2]).map(|v| v as f32))
            .collect();
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for p in &local {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        // Positions.
        let pos_off = bin.len();
        for p in &local {
            for v in p {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        let pos_view = views.len();
        views.push(format!(
            "{{\"buffer\":0,\"byteOffset\":{pos_off},\"byteLength\":{},\"target\":34962}}",
            bin.len() - pos_off
        ));
        let pos_acc = accessors.len();
        accessors.push(format!(
            "{{\"bufferView\":{pos_view},\"componentType\":5126,\"count\":{},\"type\":\"VEC3\",\"min\":[{},{},{}],\"max\":[{},{},{}]}}",
            local.len(),
            f(mn[0]), f(mn[1]), f(mn[2]), f(mx[0]), f(mx[1]), f(mx[2])
        ));
        // Indices.
        let idx_off = bin.len();
        let (mut imin, mut imax) = (u32::MAX, 0u32);
        for t in &el.mesh.triangles {
            for &i in t {
                imin = imin.min(i);
                imax = imax.max(i);
                bin.extend_from_slice(&i.to_le_bytes());
            }
        }
        let idx_view = views.len();
        views.push(format!(
            "{{\"buffer\":0,\"byteOffset\":{idx_off},\"byteLength\":{},\"target\":34963}}",
            bin.len() - idx_off
        ));
        let idx_acc = accessors.len();
        accessors.push(format!(
            "{{\"bufferView\":{idx_view},\"componentType\":5125,\"count\":{},\"type\":\"SCALAR\",\"min\":[{imin}],\"max\":[{imax}]}}",
            el.mesh.triangles.len() * 3
        ));
        let mut name = String::new();
        write_str(&mut name, &el.name);
        let mesh_index = meshes.len();
        meshes.push(format!(
            "{{\"name\":{name},\"primitives\":[{{\"attributes\":{{\"POSITION\":{pos_acc}}},\"indices\":{idx_acc},\"mode\":4}}]}}"
        ));
        let t = y_up(c[0], c[1], c[2]);
        nodes.push(format!(
            "{{\"name\":{name},\"mesh\":{mesh_index},\"translation\":[{},{},{}],\"extras\":{{\"id\":{name},\"kind\":\"{}\"}}}}",
            n(t[0]), n(t[1]), n(t[2]), el.kind
        ));
    }
    let mut out = String::from("{\"asset\":{\"version\":\"2.0\",\"generator\":\"bim (openbim)\"},\"scene\":0,\"scenes\":[{");
    if !nodes.is_empty() {
        let roots: Vec<String> = (0..nodes.len()).map(|i| i.to_string()).collect();
        let _ = write!(out, "\"nodes\":[{}]", roots.join(","));
    }
    out.push('}');
    out.push(']');
    if !nodes.is_empty() {
        let _ = write!(
            out,
            ",\"nodes\":[{}],\"meshes\":[{}],\"accessors\":[{}],\"bufferViews\":[{}],\"buffers\":[{{\"byteLength\":{},\"uri\":\"data:application/octet-stream;base64,{}\"}}]",
            nodes.join(","),
            meshes.join(","),
            accessors.join(","),
            views.join(","),
            bin.len(),
            base64(&bin)
        );
    }
    out.push_str("}\n");
    out
}

/// A float32 written so that parsing it back as float32 gives the same value.
fn f(x: f32) -> String {
    n(x as f64)
}

fn n(x: f64) -> String {
    if x.is_finite() {
        format!("{x:?}")
    } else {
        "0.0".to_string()
    }
}

/// Appends `s` as a JSON string literal.
fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let v = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(A[(v >> 18) as usize & 63] as char);
        out.push(A[(v >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { A[(v >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[v as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn empty_model_is_valid_json_shape() {
        let s = to_gltf(&[]);
        assert!(s.contains("\"version\":\"2.0\""));
        assert!(!s.contains("buffers"));
    }
}
