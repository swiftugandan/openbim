//! IFC GlobalId conversion: 22 characters of the IFC base-64 alphabet `0-9A-Za-z_$` encode 128
//! bits, the first character the top 2 bits and each further one the next 6 bits (the encoding of
//! `ifcopenshell.guid.compress` / `expand`).

const ALPHABET: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";

fn digit(c: u8) -> Option<u32> {
    ALPHABET.iter().position(|&a| a == c).map(|p| p as u32)
}

/// The 22-character GlobalId of a 128-bit value.
pub fn compress(value: u128) -> String {
    let mut out = String::with_capacity(22);
    out.push(ALPHABET[(value >> 126) as usize] as char);
    for k in (0..21).rev() {
        out.push(ALPHABET[((value >> (6 * k)) & 0x3f) as usize] as char);
    }
    out
}

/// The 128-bit value of a 22-character GlobalId; `None` for a malformed one (wrong length,
/// characters outside the alphabet, first character above `3`).
pub fn expand(guid: &str) -> Option<u128> {
    let b = guid.as_bytes();
    if b.len() != 22 {
        return None;
    }
    let first = digit(b[0])?;
    if first > 3 {
        return None;
    }
    let mut v = first as u128;
    for &c in &b[1..] {
        v = (v << 6) | digit(c)? as u128;
    }
    Some(v)
}

/// The GlobalId of a 32-hex-digit UUID (dashes allowed), as `ifcopenshell.guid.compress`.
pub fn compress_hex(uuid: &str) -> Option<String> {
    let hex: String = uuid.chars().filter(|&c| c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok().map(compress)
}

/// The 32 lower-case hex digits of a GlobalId, as `ifcopenshell.guid.expand`.
pub fn expand_hex(guid: &str) -> Option<String> {
    expand(guid).map(|v| format!("{v:032x}"))
}

/// The UUID text of a 128-bit value: 32 lower-case hex digits in the dashed 8-4-4-4-12 form (that
/// of Python's `str(uuid.UUID(...))`), the spelling ifcJSON uses for a `globalId`.
pub fn to_uuid(value: u128) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (value >> 96) as u32,
        (value >> 80) as u16,
        (value >> 64) as u16,
        (value >> 48) as u16,
        value as u64 & 0xffff_ffff_ffff,
    )
}

/// The 128-bit value of a UUID: 32 hex digits of either case, plain or in the dashed 8-4-4-4-12
/// form. `None` for anything else (braces, a `urn:uuid:` prefix, wrong lengths or characters).
pub fn parse_uuid(s: &str) -> Option<u128> {
    const DASHES: [usize; 4] = [8, 13, 18, 23];
    let b = s.as_bytes();
    if b.len() != 32 && b.len() != 36 {
        return None;
    }
    let dashed = b.len() == 36;
    if dashed && DASHES.iter().any(|&k| b[k] != b'-') {
        return None;
    }
    let mut value: u128 = 0;
    for (k, &c) in b.iter().enumerate() {
        if dashed && DASHES.contains(&k) {
            continue;
        }
        value = (value << 4) | u128::from((c as char).to_digit(16)?);
    }
    Some(value)
}

/// A GlobalId in either spelling ifcJSON allows: the 22-character IFC base-64 form ([`expand`]) or
/// a UUID ([`parse_uuid`]). Both name the same 128 bits, so `parse_global_id(&compress(v))` and
/// `parse_global_id(&to_uuid(v))` are both `Some(v)`.
pub fn parse_global_id(s: &str) -> Option<u128> {
    if s.len() == 22 {
        expand(s)
    } else {
        parse_uuid(s)
    }
}

/// A deterministic generator of distinct GlobalIds (a SplitMix64-style sequence from a seed),
/// for writers that need fresh ids without a random number source.
#[derive(Debug, Clone)]
pub struct GuidGenerator {
    state: u64,
}

impl GuidGenerator {
    pub fn new(seed: u64) -> GuidGenerator {
        GuidGenerator { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The next 128-bit value, shaped as a version-4 UUID.
    pub fn next_value(&mut self) -> u128 {
        let v = ((self.next_u64() as u128) << 64) | self.next_u64() as u128;
        (v & !(0xF000u128 << 64) & !(0xC000u128 << 48)) | (0x4000u128 << 64) | (0x8000u128 << 48)
    }

    /// The next GlobalId.
    pub fn next_guid(&mut self) -> String {
        compress(self.next_value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uuid_and_the_base64_spelling_name_the_same_value() {
        for v in [0u128, 1, u128::MAX, 1 << 127, (1 << 126) - 1, 0x0123_4567_89ab_cdef_0011_2233_4455_6677] {
            let (g, u) = (compress(v), to_uuid(v));
            assert_eq!((g.len(), u.len()), (22, 36), "{v:#x}");
            assert_eq!(parse_global_id(&g), Some(v), "{g}");
            assert_eq!(parse_global_id(&u), Some(v), "{u}");
            assert_eq!(parse_global_id(&u.to_uppercase()), Some(v));
            assert_eq!(parse_global_id(&u.replace('-', "")), Some(v));
        }
        assert_eq!(to_uuid(0x0123_4567_89ab_cdef_0011_2233_4455_6677), "01234567-89ab-cdef-0011-223344556677");
    }

    #[test]
    fn a_global_id_from_ifcopenshell_has_the_uuid_python_prints() {
        // ifcopenshell.guid.expand("0qhSLGUAa32dZHUwxM$WqP") and str(uuid.UUID(...)).
        let v = expand("0qhSLGUAa32dZHUwxM$WqP").unwrap();
        assert_eq!(to_uuid(v), "34adc550-78a9-030a-78d1-7baed6fe0d19");
        assert_eq!(parse_global_id("34adc550-78a9-030a-78d1-7baed6fe0d19"), Some(v));
    }

    #[test]
    fn malformed_global_ids_are_none() {
        for s in [
            "",
            "xyz",
            "34adc550-78a9-030a-78d1-7baed6fe0d1",
            "34adc550_78a9_030a_78d1_7baed6fe0d19",
            "34adc550-78a9-030a-78d1-7baed6fe0d1g",
            "{34adc550-78a9-030a-78d1-7baed6fe0d19}",
            "0qhSLGUAa32dZHUwxM$Wq",
            "4qhSLGUAa32dZHUwxM$WqP",
            "34adc5507-8a9-030a-78d1-7baed6fe0d19",
        ] {
            assert_eq!(parse_global_id(s), None, "{s:?}");
        }
    }
}
