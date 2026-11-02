//! The text op format: one op per line, shared by ops files (`bim apply`) and the `doc.oplog`
//! journal.
//!
//! ```text
//! create <seed> <kind>
//! set <seed> <key> num <f64>
//! set <seed> <key> text <word>
//! set <seed> <key> ref <seed>
//! delete <seed>
//! ```
//!
//! Words are separated by runs of whitespace, the number of words per form is exact, and the op
//! keyword and the value type are lowercase. Blank lines (and a leading UTF-8 byte order mark) are
//! skipped; there is no other syntax (no comments, no quoting). Numbers must be finite: `nan`,
//! `inf` and overflowing literals such as `1e400` are rejected, because a JSON number
//! (spec/schema.md) cannot carry them.
//!
//! An entity is named by its *seed*, which stands for the id [`EntityId::derive`] gives it
//! (see [`resolve_id`]). A [`LineOp`] keeps seeds exactly as written, so the journal can be
//! replayed to the identical document and `validate` can print the names the user chose.
//!
//! [`Display`](fmt::Display) renders the canonical form of a line: single spaces, numbers in
//! Rust's shortest round-trip `{:?}` form (`9` becomes `9.0`), and no line terminator.
//! Parsing the canonical form gives back the same op bit for bit.

use std::fmt;

use bim_doc::{DocError, EntityId, Op, Value};

const CREATE_FORM: &str = "create <seed> <kind>";
const SET_FORM: &str = "set <seed> <key> num <f64> | text <word> | ref <seed>";
const DELETE_FORM: &str = "delete <seed>";

/// The value written by a `set` line. A `Ref` keeps its target's seed as written.
#[derive(Debug, Clone, PartialEq)]
pub enum LineValue {
    /// `num <f64>`; always finite.
    Num(f64),
    /// `text <word>`: a single word, taken verbatim.
    Text(String),
    /// `ref <seed>`.
    Ref(String),
}

/// One op of the line format, with entities named by seed.
#[derive(Debug, Clone, PartialEq)]
pub enum LineOp {
    /// `create <seed> <kind>`
    Create {
        /// Seed of the new entity.
        seed: String,
        /// Its kind (any word; kinds are not validated yet).
        kind: String,
    },
    /// `set <seed> <key> <type> <value>`
    Set {
        /// Seed of the target entity.
        seed: String,
        /// Prop name.
        key: String,
        /// New value.
        value: LineValue,
    },
    /// `delete <seed>`
    Delete {
        /// Seed of the entity to remove.
        seed: String,
    },
}

/// The id an entity name stands for.
pub fn resolve_id(name: &str) -> EntityId {
    EntityId::derive(name)
}

impl LineOp {
    /// The seed of the entity this op acts on.
    pub fn seed(&self) -> &str {
        match self {
            LineOp::Create { seed, .. } | LineOp::Set { seed, .. } | LineOp::Delete { seed } => seed,
        }
    }

    /// The prop key this op writes, if it is a `set`.
    pub fn key(&self) -> Option<&str> {
        match self {
            LineOp::Set { key, .. } => Some(key),
            LineOp::Create { .. } | LineOp::Delete { .. } => None,
        }
    }

    /// Every entity name the line mentions: its target and, for a `ref` value, the referenced entity.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let referenced = match self {
            LineOp::Set { value: LineValue::Ref(target), .. } => Some(target.as_str()),
            _ => None,
        };
        std::iter::once(self.seed()).chain(referenced)
    }

    /// The document op this line stands for.
    pub fn to_op(&self) -> Op {
        match self {
            LineOp::Create { seed, kind } => Op::Create { id: resolve_id(seed), kind: kind.clone() },
            LineOp::Set { seed, key, value } => Op::Set {
                id: resolve_id(seed),
                key: key.clone(),
                value: match value {
                    LineValue::Num(x) => Value::Num(*x),
                    LineValue::Text(s) => Value::Text(s.clone()),
                    LineValue::Ref(target) => Value::Ref(resolve_id(target)),
                },
            },
            LineOp::Delete { seed } => Op::Delete { id: resolve_id(seed) },
        }
    }

    /// Words for the error `err` that [`bim_doc::Document::apply`] gave this op, naming the entity
    /// by its seed rather than by its 128-bit id.
    pub fn describe_error(&self, err: &DocError) -> String {
        let seed = self.seed();
        match err {
            DocError::NotFound(_) => format!("entity `{seed}` not found"),
            DocError::AlreadyExists(_) => format!("entity `{seed}` already exists"),
        }
    }
}

impl fmt::Display for LineValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineValue::Num(x) => write!(f, "num {x:?}"),
            LineValue::Text(word) => write!(f, "text {word}"),
            LineValue::Ref(seed) => write!(f, "ref {seed}"),
        }
    }
}

impl fmt::Display for LineOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineOp::Create { seed, kind } => write!(f, "create {seed} {kind}"),
            LineOp::Set { seed, key, value } => write!(f, "set {seed} {key} {value}"),
            LineOp::Delete { seed } => write!(f, "delete {seed}"),
        }
    }
}

/// Parses one line. `Ok(None)` for a blank line, `Err(reason)` for an invalid one.
pub fn parse_line(line: &str) -> Result<Option<LineOp>, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some(&keyword) = words.first() else {
        return Ok(None);
    };
    let op = match (keyword, words.as_slice()) {
        ("create", &[_, seed, kind]) => LineOp::Create { seed: seed.to_string(), kind: kind.to_string() },
        ("create", _) => return Err(arity("create", CREATE_FORM, words.len())),
        ("set", &[_, seed, key, ty, word]) => LineOp::Set {
            seed: seed.to_string(),
            key: key.to_string(),
            value: parse_value(ty, word)?,
        },
        ("set", _) => return Err(arity("set", SET_FORM, words.len())),
        ("delete", &[_, seed]) => LineOp::Delete { seed: seed.to_string() },
        ("delete", _) => return Err(arity("delete", DELETE_FORM, words.len())),
        (other, _) => return Err(format!("unknown op `{other}` (expected create, set or delete)")),
    };
    Ok(Some(op))
}

fn arity(op: &str, form: &str, found: usize) -> String {
    format!("`{op}` takes the form `{form}`, found {found} word(s)")
}

fn parse_value(ty: &str, word: &str) -> Result<LineValue, String> {
    match ty {
        "num" => match word.parse::<f64>() {
            Ok(x) if x.is_finite() => Ok(LineValue::Num(x)),
            Ok(_) => Err(format!("number `{word}` is not finite")),
            Err(_) => Err(format!("`{word}` is not a number")),
        },
        "text" => Ok(LineValue::Text(word.to_string())),
        "ref" => Ok(LineValue::Ref(word.to_string())),
        other => Err(format!("unknown value type `{other}` (expected num, text or ref)")),
    }
}

/// An op with its 1-based line number in the text it was parsed from.
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    /// 1-based line number; blank lines count.
    pub line: usize,
    /// The op on that line.
    pub op: LineOp,
}

/// The first invalid line of a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line number; blank lines count.
    pub line: usize,
    /// What is wrong with the line.
    pub reason: String,
}

/// Parses a whole ops file or journal (`\n` or `\r\n` line ends), skipping blank lines. A leading
/// UTF-8 byte order mark is an encoding marker, not part of the first line. Stops at the first
/// invalid line.
pub fn parse_text(text: &str) -> Result<Vec<Located>, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut ops = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        match parse_line(raw) {
            Ok(Some(op)) => ops.push(Located { line, op }),
            Ok(None) => {}
            Err(reason) => return Err(ParseError { line, reason }),
        }
    }
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> LineOp {
        parse_line(line).unwrap().unwrap_or_else(|| panic!("`{line}` parsed as blank"))
    }

    fn reject(line: &str) -> String {
        match parse_line(line) {
            Err(reason) => reason,
            Ok(op) => panic!("`{line}` should be invalid, parsed as {op:?}"),
        }
    }

    fn create(seed: &str, kind: &str) -> LineOp {
        LineOp::Create { seed: seed.into(), kind: kind.into() }
    }

    fn set(seed: &str, key: &str, value: LineValue) -> LineOp {
        LineOp::Set { seed: seed.into(), key: key.into(), value }
    }

    #[test]
    fn parses_each_form() {
        assert_eq!(parse("create wall Wall"), create("wall", "Wall"));
        assert_eq!(parse("set wall height num 3.5"), set("wall", "height", LineValue::Num(3.5)));
        assert_eq!(parse("set wall name text north"), set("wall", "name", LineValue::Text("north".into())));
        assert_eq!(parse("set wall host ref lvl"), set("wall", "host", LineValue::Ref("lvl".into())));
        assert_eq!(parse("delete wall"), LineOp::Delete { seed: "wall".into() });
    }

    #[test]
    fn blank_and_whitespace_only_lines_are_none() {
        for line in ["", " ", "\t", "  \t  ", "\r", "\u{a0}"] {
            assert_eq!(parse_line(line), Ok(None), "{line:?}");
        }
    }

    #[test]
    fn tolerates_extra_whitespace_tabs_and_crlf() {
        assert_eq!(parse("  create \t wall   Wall  "), create("wall", "Wall"));
        assert_eq!(parse("set\twall\theight\tnum\t3.5\r"), set("wall", "height", LineValue::Num(3.5)));
    }

    #[test]
    fn arity_is_exact() {
        for line in [
            "create",
            "create wall",
            "create wall Wall extra",
            "set",
            "set wall",
            "set wall name",
            "set wall name text",
            "set wall name text north south",
            "delete",
            "delete wall extra",
        ] {
            let reason = reject(line);
            assert!(reason.contains("word(s)"), "{line}: {reason}");
        }
    }

    #[test]
    fn keywords_are_case_sensitive_and_closed() {
        assert!(reject("bogus op here").contains("unknown op `bogus`"));
        assert!(reject("CREATE a b").contains("unknown op `CREATE`"));
        assert!(reject("Set a k num 1").contains("unknown op `Set`"));
        assert!(reject("# a comment").contains("unknown op `#`"));
        assert!(reject("set a k NUM 1").contains("unknown value type `NUM`"));
        assert!(reject("set a k bool true").contains("unknown value type `bool`"));
        assert!(reject("set a k list 1").contains("unknown value type `list`"));
    }

    #[test]
    fn accepts_the_number_forms_f64_parsing_accepts() {
        let cases: [(&str, f64); 9] = [
            ("3.5", 3.5),
            ("+3", 3.0),
            ("-2", -2.0),
            (".5", 0.5),
            ("5.", 5.0),
            ("1e5", 1e5),
            ("1E-3", 1e-3),
            ("-0", -0.0),
            ("100000000000000000000", 1e20),
        ];
        for (word, want) in cases {
            match parse(&format!("set a k num {word}")) {
                LineOp::Set { value: LineValue::Num(got), .. } => {
                    assert_eq!(got.to_bits(), want.to_bits(), "{word}");
                }
                other => panic!("{word}: {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_bad_and_non_finite_numbers() {
        for word in ["3.5abc", "abc", "0x10", "1_0", "1,5", "--1", "1e", "e5", "."] {
            let reason = reject(&format!("set a k num {word}"));
            assert!(reason.contains("is not a number"), "{word}: {reason}");
        }
        for word in ["nan", "NaN", "inf", "-inf", "+inf", "infinity", "-Infinity", "1e400", "-1e400"] {
            let reason = reject(&format!("set a k num {word}"));
            assert!(reason.contains("is not finite"), "{word}: {reason}");
        }
    }

    #[test]
    fn text_and_ref_words_are_verbatim() {
        assert_eq!(
            parse("set a k text 3.5"),
            set("a", "k", LineValue::Text("3.5".into())),
            "text is never reinterpreted"
        );
        assert_eq!(parse("set a k text nan"), set("a", "k", LineValue::Text("nan".into())));
        assert_eq!(parse("set a k ref Ünï-€"), set("a", "k", LineValue::Ref("Ünï-€".into())));
        // Reserved keys are a matter for `bim apply`, not for the syntax (the journal holds kernel-written ones).
        assert_eq!(parse("set a _name text a"), set("a", "_name", LineValue::Text("a".into())));
    }

    #[test]
    fn display_is_canonical() {
        assert_eq!(parse("  create   wall\tWall ").to_string(), "create wall Wall");
        assert_eq!(parse("set wall height num 9").to_string(), "set wall height num 9.0");
        assert_eq!(parse("set wall height num +3.50").to_string(), "set wall height num 3.5");
        assert_eq!(parse("set wall h num -0").to_string(), "set wall h num -0.0");
        assert_eq!(parse("set wall h num 1e300").to_string(), "set wall h num 1e300");
        assert_eq!(parse("set wall n text north").to_string(), "set wall n text north");
        assert_eq!(parse("set wall host ref lvl").to_string(), "set wall host ref lvl");
        assert_eq!(parse("delete  wall").to_string(), "delete wall");
    }

    #[test]
    fn display_round_trips_every_form() {
        let ops = [
            create("w", "Wall"),
            create("ünï", "fire:Sprinkler"),
            set("w", "k", LineValue::Text("x".into())),
            set("w", "k", LineValue::Ref("ünï".into())),
            LineOp::Delete { seed: "w".into() },
        ];
        for op in ops {
            assert_eq!(parse(&op.to_string()), op);
        }
    }

    #[test]
    fn display_round_trips_f64_bit_for_bit() {
        let mut values = vec![
            0.1 + 0.2,
            1e300,
            5e-324,
            -0.0,
            0.0,
            9.0,
            3.5,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            f64::EPSILON,
            1e21,
            1e-7,
            123456789012345680.0,
        ];
        // A deterministic spread of bit patterns (xorshift64*); non-finite ones are not representable.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..20_000 {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let x = f64::from_bits(state.wrapping_mul(0x2545_f491_4f6c_dd1d));
            if x.is_finite() {
                values.push(x);
            }
        }
        for x in values {
            let line = set("w", "k", LineValue::Num(x)).to_string();
            match parse(&line) {
                LineOp::Set { value: LineValue::Num(y), .. } => {
                    assert_eq!(x.to_bits(), y.to_bits(), "{line}");
                }
                other => panic!("{line}: {other:?}"),
            }
        }
    }

    #[test]
    fn to_op_derives_ids_from_seeds() {
        assert_eq!(
            parse("create wall Wall").to_op(),
            Op::Create { id: EntityId::derive("wall"), kind: "Wall".into() }
        );
        assert_eq!(
            parse("set wall host ref lvl").to_op(),
            Op::Set { id: EntityId::derive("wall"), key: "host".into(), value: Value::Ref(EntityId::derive("lvl")) }
        );
        assert_eq!(
            parse("set wall h num 2").to_op(),
            Op::Set { id: EntityId::derive("wall"), key: "h".into(), value: Value::Num(2.0) }
        );
        assert_eq!(
            parse("set wall n text north").to_op(),
            Op::Set { id: EntityId::derive("wall"), key: "n".into(), value: Value::Text("north".into()) }
        );
        assert_eq!(parse("delete wall").to_op(), Op::Delete { id: EntityId::derive("wall") });
    }

    #[test]
    fn accessors() {
        assert_eq!(parse("create wall Wall").seed(), "wall");
        assert_eq!(parse("delete wall").seed(), "wall");
        assert_eq!(parse("set wall height num 1").key(), Some("height"));
        assert_eq!(parse("create wall Wall").key(), None);
        assert_eq!(parse("delete wall").key(), None);
        assert_eq!(parse("set wall host ref lvl").names().collect::<Vec<_>>(), ["wall", "lvl"]);
        assert_eq!(parse("set wall h num 1").names().collect::<Vec<_>>(), ["wall"]);
        assert_eq!(parse("create wall Wall").names().collect::<Vec<_>>(), ["wall"]);
    }

    #[test]
    fn describes_doc_errors_by_seed() {
        let id = EntityId::derive("ghost");
        let op = parse("set ghost h num 1");
        assert_eq!(op.describe_error(&DocError::NotFound(id)), "entity `ghost` not found");
        assert_eq!(
            parse("create ghost Wall").describe_error(&DocError::AlreadyExists(id)),
            "entity `ghost` already exists"
        );
    }

    #[test]
    fn parse_text_skips_blanks_and_numbers_lines_from_one() {
        let text = "create a Wall\n\n   \nset a h num 1\r\ndelete a";
        let ops = parse_text(text).unwrap();
        let lines: Vec<usize> = ops.iter().map(|l| l.line).collect();
        assert_eq!(lines, [1, 4, 5]);
        assert_eq!(ops[1].op, set("a", "h", LineValue::Num(1.0)));
        assert_eq!(parse_text(""), Ok(vec![]));
        assert_eq!(parse_text("\n\n  \n"), Ok(vec![]));
    }

    #[test]
    fn parse_text_ignores_a_leading_byte_order_mark() {
        let ops = parse_text("\u{feff}create a Wall\nset a h num 1\n").unwrap();
        assert_eq!(ops.iter().map(|l| l.line).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(ops[0].op, create("a", "Wall"));
        assert_eq!(parse_text("\u{feff}"), Ok(vec![]));
        // Only at the very start: anywhere else it is an ordinary (and here invalid) character.
        assert_eq!(parse_text("create a Wall\n\u{feff}create b Wall\n").unwrap_err().line, 2);
    }

    #[test]
    fn parse_text_reports_the_first_bad_line() {
        let err = parse_text("create a Wall\n\nbogus op here\nalso bad\n").unwrap_err();
        assert_eq!(err.line, 3);
        assert!(err.reason.contains("unknown op `bogus`"), "{}", err.reason);
    }
}
