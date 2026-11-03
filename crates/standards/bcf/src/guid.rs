//! BCF GUIDs: the lowercase `8-4-4-4-12` hex form the BCF 3.0 schemas require.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Whether `s` is exactly a BCF 3.0 `Guid`: `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`.
pub fn is_guid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

/// The BCF form of a GUID written in either case (and with surrounding whitespace): lowercase.
/// `None` if `s` is not an `8-4-4-4-12` hex UUID.
pub fn normalize_guid(s: &str) -> Option<String> {
    let lowered = s.trim().to_ascii_lowercase();
    is_guid(&lowered).then_some(lowered)
}

/// The version-4 (random) UUID with the bits of `v`: the version and variant bits are set, the
/// other 122 are `v`'s. Deterministic.
pub fn guid_from_u128(v: u128) -> String {
    let v = (v & !(0xFu128 << 76)) | (0x4u128 << 76);
    let v = (v & !(0x3u128 << 62)) | (0x2u128 << 62);
    let hex = format!("{v:032x}");
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// A fresh random version-4 UUID. Draws on the per-process random keys the standard library seeds
/// its hash maps with (operating-system entropy), mixed with a counter, the clock and the process
/// id, so GUIDs made in quick succession, in one process or many, differ.
pub fn new_guid() -> String {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let call = CALLS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let half = |salt: u8| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(call);
        hasher.write_u32(std::process::id());
        hasher.write_u128(nanos);
        hasher.write_u8(salt);
        u128::from(hasher.finish())
    };
    let high = half(0);
    let low = half(1);
    guid_from_u128((high << 64) | low)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn constructed_guids_match_the_schema_pattern() {
        for v in [0u128, 1, u128::MAX, 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef, 1 << 76, 1 << 63] {
            let g = guid_from_u128(v);
            assert!(is_guid(&g), "{g}");
            assert_eq!(g.as_bytes().get(14), Some(&b'4'), "version nibble of {g}");
            assert!(matches!(g.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b')), "variant nibble of {g}");
        }
        assert_eq!(guid_from_u128(0), "00000000-0000-4000-8000-000000000000");
        assert_eq!(guid_from_u128(u128::MAX), "ffffffff-ffff-4fff-bfff-ffffffffffff");
    }

    #[test]
    fn new_guids_are_valid_and_distinct() {
        let guids: HashSet<String> = (0..2000).map(|_| new_guid()).collect();
        assert_eq!(guids.len(), 2000);
        assert!(guids.iter().all(|g| is_guid(g)));
    }

    #[test]
    fn normalize_lowercases_and_validates() {
        assert_eq!(normalize_guid("ABCDEF01-2345-6789-ABCD-EF0123456789").as_deref(), Some("abcdef01-2345-6789-abcd-ef0123456789"));
        assert_eq!(normalize_guid("  abcdef01-2345-6789-abcd-ef0123456789\n").as_deref(), Some("abcdef01-2345-6789-abcd-ef0123456789"));
        for bad in [
            "",
            "abcdef01-2345-6789-abcd-ef012345678",
            "abcdef01-2345-6789-abcd-ef01234567890",
            "abcdef01-2345-6789-abcd-ef012345678g",
            "abcdef0123456789abcdef0123456789",
            "abcdef01_2345-6789-abcd-ef0123456789",
            "{abcdef01-2345-6789-abcd-ef0123456789}",
            "ábcdef01-2345-6789-abcd-ef0123456789",
        ] {
            assert_eq!(normalize_guid(bad), None, "{bad:?}");
        }
        assert!(!is_guid("ABCDEF01-2345-6789-ABCD-EF0123456789"), "the schema pattern is lowercase");
    }
}
