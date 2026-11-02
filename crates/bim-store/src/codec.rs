//! Hashes and canonical text encodings. Everything here is hand-written (FNV-1a plus the SplitMix64
//! finaliser, the recipe `bim-doc` uses for content hashes), so ids and encodings are identical
//! across runs, platforms and Rust versions: they are stored in files and compared across peers.

use std::fmt::Write as _;

use bim_doc::{EntityId, Value};

const FNV64_OFFSET: u64 = 0xcbf29ce484222325;
const FNV64_PRIME: u64 = 0x100000001b3;

/// SplitMix64-style finaliser: a bijection on `u64` with good avalanche.
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// FNV-1a 64 over `bytes`, finished with the SplitMix64 mixer.
///
/// Used for op ids and for the journal mark; stable forever, because stores written today are
/// read by later versions.
pub fn bytes_hash(bytes: &[u8]) -> u64 {
    let mut h = FNV64_OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV64_PRIME);
    }
    mix64(h)
}

/// The id of an op: [`bytes_hash`] of its canonical text, as 16 lowercase hex digits.
///
/// Ops are content-addressed: the same canonical text always has the same id, whichever journal or
/// peer it came from, so storing an op twice stores it once.
pub fn op_id(canonical: &str) -> String {
    format!("{:016x}", bytes_hash(canonical.as_bytes()))
}

/// The key the store files an entity under: its 128-bit id as 32 lowercase hex digits.
pub fn entity_key(id: EntityId) -> String {
    format!("{:032x}", id.0)
}

/// Appends the canonical text of `value`, the form state and snapshot rows hold it in.
///
/// The encoding is injective up to what [`bim_doc::Document::content_hash`] also ignores: `-0.0`
/// encodes as `0.0`, so two values encode alike exactly when the document hash treats them alike.
/// Numbers use Rust's shortest round-trip form, texts are JSON strings, a Ref is
/// `{"ref":"<32 hex digits>"}`, a Bool is `true` / `false` and a List is `[…]` with `,` between
/// items. It is not meant to be parsed back; diffs only compare it for equality.
pub fn encode_value(value: &Value, out: &mut String) {
    enum Step<'a> {
        Value(&'a Value),
        Raw(&'static str),
    }
    // An explicit stack: a List may be nested arbitrarily deep.
    let mut stack = vec![Step::Value(value)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Raw(text) => out.push_str(text),
            Step::Value(Value::Num(x)) => {
                if *x == 0.0 {
                    out.push_str("0.0");
                } else {
                    let _ = write!(out, "{x:?}");
                }
            }
            Step::Value(Value::Text(s)) => push_text(out, s),
            Step::Value(Value::Bool(b)) => out.push_str(if *b { "true" } else { "false" }),
            Step::Value(Value::Ref(id)) => {
                let _ = write!(out, "{{\"ref\":\"{:032x}\"}}", id.0);
            }
            Step::Value(Value::List(items)) => {
                out.push('[');
                stack.push(Step::Raw("]"));
                for (i, item) in items.iter().enumerate().rev() {
                    stack.push(Step::Value(item));
                    if i > 0 {
                        stack.push(Step::Raw(","));
                    }
                }
            }
        }
    }
}

/// `s` as a JSON string literal: `"`, `\` and control characters escaped.
fn push_text(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(value: &Value) -> String {
        let mut out = String::new();
        encode_value(value, &mut out);
        out
    }

    #[test]
    fn op_ids_are_sixteen_lowercase_hex_digits_and_deterministic() {
        let id = op_id("create wall Wall");
        assert_eq!(id.len(), 16);
        assert!(id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{id}");
        assert_eq!(id, op_id("create wall Wall"));
        assert_ne!(id, op_id("create wall Wall "));
        assert_ne!(id, op_id("create wall Slab"));
        assert_ne!(op_id(""), op_id("\n"));
    }

    #[test]
    fn the_hash_is_frozen() {
        // Stores on disk hold these ids: the function must never change.
        assert_eq!(bytes_hash(b""), mix64(FNV64_OFFSET));
        assert_eq!(op_id("create wall Wall"), format!("{:016x}", bytes_hash(b"create wall Wall")));
        // FNV-1a("a") = 0xaf63dc4c8601ec8c, a published test vector; the mixer is applied on top.
        let mut h = FNV64_OFFSET;
        h ^= u64::from(b'a');
        h = h.wrapping_mul(FNV64_PRIME);
        assert_eq!(h, 0xaf63dc4c8601ec8c);
        assert_eq!(bytes_hash(b"a"), mix64(0xaf63dc4c8601ec8c));
    }

    #[test]
    fn entity_keys_are_32_hex_digits() {
        assert_eq!(entity_key(EntityId(0)), "0".repeat(32));
        assert_eq!(entity_key(EntityId(u128::MAX)), "f".repeat(32));
        assert_eq!(entity_key(EntityId(0xabc)), format!("{}abc", "0".repeat(29)));
    }

    #[test]
    fn negative_zero_encodes_like_zero_and_other_numbers_round_trip() {
        assert_eq!(encoded(&Value::Num(-0.0)), encoded(&Value::Num(0.0)));
        assert_eq!(encoded(&Value::Num(0.0)), "0.0");
        assert_eq!(encoded(&Value::Num(3.0)), "3.0");
        assert_eq!(encoded(&Value::Num(-1.5)), "-1.5");
        assert_ne!(encoded(&Value::Num(0.1)), encoded(&Value::Num(0.1 + f64::EPSILON)));
        assert_eq!(encoded(&Value::Num(1e300)), "1e300");
    }

    #[test]
    fn values_of_different_types_never_encode_alike() {
        let id = EntityId(7);
        let all = [
            Value::Num(1.0),
            Value::Text("1.0".into()),
            Value::Bool(true),
            Value::Text("true".into()),
            Value::Ref(id),
            Value::Text(format!("{{\"ref\":\"{:032x}\"}}", id.0)),
            Value::List(vec![]),
            Value::List(vec![Value::Num(1.0)]),
            Value::List(vec![Value::Num(1.0), Value::Num(2.0)]),
            Value::List(vec![Value::List(vec![Value::Num(1.0)]), Value::Num(2.0)]),
            Value::List(vec![Value::Num(1.0), Value::List(vec![Value::Num(2.0)])]),
            Value::Text("[1.0]".into()),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(encoded(a), encoded(b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn texts_are_escaped_and_lists_nest() {
        assert_eq!(encoded(&Value::Text("a \"b\" \\ \n\t\r\u{1}é".into())), "\"a \\\"b\\\" \\\\ \\n\\t\\r\\u0001é\"");
        let value = Value::List(vec![
            Value::Num(1.5),
            Value::Bool(false),
            Value::List(vec![Value::Text("x".into()), Value::List(vec![])]),
            Value::Ref(EntityId(255)),
        ]);
        assert_eq!(encoded(&value), format!("[1.5,false,[\"x\",[]],{{\"ref\":\"{:032x}\"}}]", 255));
    }

    #[test]
    fn a_very_deep_list_does_not_overflow_the_stack() {
        let mut value = Value::Num(1.0);
        for _ in 0..50_000 {
            value = Value::List(vec![value]);
        }
        let text = encoded(&value);
        assert_eq!(text.len(), 100_000 + 3);
        // Dropping the value recursively would overflow; unwind it by hand.
        let mut current = value;
        while let Value::List(mut items) = current {
            current = items.pop().unwrap();
        }
    }
}
