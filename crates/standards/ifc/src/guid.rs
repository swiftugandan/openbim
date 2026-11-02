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
