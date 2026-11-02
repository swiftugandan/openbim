//! The text op format: one op per line, shared by ops files (`bim apply`) and the `doc.oplog`
//! journal. Also the JSON ops format of spec/cli.md, which maps onto the same [`LineOp`]s.
//!
//! ```text
//! create <seed> <kind>
//! set <seed> <key> num <f64>
//! set <seed> <key> text <word>
//! set <seed> <key> ref <seed>
//! delete <seed>
//! {"op": "set", "id": "<id>", "key": "<key>", "value": <JSON value>}
//! ```
//!
//! Words are separated by runs of whitespace, the number of words per form is exact, and the op
//! keyword and the value type are lowercase. Blank lines (and a leading UTF-8 byte order mark) are
//! skipped; there is no other syntax (no comments, no quoting). Numbers must be finite: `nan`,
//! `inf` and overflowing literals such as `1e400` are rejected, because a JSON number
//! (spec/schema.md) cannot carry them.
//!
//! A line whose first non-blank character is `{` is one op written as a JSON object, exactly like
//! an entry of a JSON ops file's `ops` array (see [`parse_json_ops`]) except that a `create` there
//! takes no `props`. That form carries what words cannot: texts, ids, keys and kinds that are empty
//! or contain whitespace, and Bool and List values. The journal writes it only for such ops.
//!
//! An entity is named by the string it was written with — a ULID or a seed — which stands for the
//! id [`EntityId::parse`] gives it (see [`resolve_id`]). A [`LineOp`] keeps names exactly as
//! written, so the journal can be replayed to the identical document and output can show the
//! names the user chose.
//!
//! [`Display`](fmt::Display) renders the canonical form of a line: single spaces, numbers in
//! Rust's shortest round-trip `{:?}` form (`9` becomes `9.0`), and no line terminator; or the
//! compact JSON object form when the op cannot be written in words. Parsing the canonical form
//! gives back the same op bit for bit.

use std::fmt;

use bim_doc::{DocError, Document, EntityId, Op, Value};

use crate::json::{self, Json};

const CREATE_FORM: &str = "create <seed> <kind>";
const SET_FORM: &str = "set <seed> <key> num <f64> | text <word> | ref <seed>";
const DELETE_FORM: &str = "delete <seed>";

/// Reserved prop holding the name an entity was created with (spec/schema.md).
pub const NAME_PROP: &str = "_name";
/// Reserved prop holding the schema version an entity was created under (spec/schema.md).
pub const SCHEMA_PROP: &str = "_schema";

/// A value written by a `set`. A `Ref` keeps its target's name as written.
#[derive(Debug, Clone, PartialEq)]
pub enum LineValue {
    /// `num <f64>`; always finite.
    Num(f64),
    /// `text <word>`, or any JSON string.
    Text(String),
    /// `ref <seed>`, or `{"ref": "<id>"}`.
    Ref(String),
    /// JSON `true` / `false`.
    Bool(bool),
    /// A JSON array.
    List(Vec<LineValue>),
}

/// One op, with entities named as written.
#[derive(Debug, Clone, PartialEq)]
pub enum LineOp {
    /// `create <seed> <kind>`
    Create {
        /// Name of the new entity.
        seed: String,
        /// Its kind (kinds are not validated yet).
        kind: String,
    },
    /// `set <seed> <key> <type> <value>`
    Set {
        /// Name of the target entity.
        seed: String,
        /// Prop name.
        key: String,
        /// New value.
        value: LineValue,
    },
    /// `delete <seed>`
    Delete {
        /// Name of the entity to remove.
        seed: String,
    },
}

/// The id an entity name stands for: a ULID as-is, any other string derived as a seed.
pub fn resolve_id(name: &str) -> EntityId {
    EntityId::parse(name)
}

impl LineValue {
    /// The document value: Ref names resolved to ids.
    pub fn to_value(&self) -> Value {
        match self {
            LineValue::Num(x) => Value::Num(*x),
            LineValue::Text(s) => Value::Text(s.clone()),
            LineValue::Ref(target) => Value::Ref(resolve_id(target)),
            LineValue::Bool(b) => Value::Bool(*b),
            LineValue::List(items) => Value::List(items.iter().map(LineValue::to_value).collect()),
        }
    }

    /// Every Ref target name in the value, at any depth, in order.
    fn ref_names<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            LineValue::Ref(target) => out.push(target),
            LineValue::List(items) => items.iter().for_each(|item| item.ref_names(out)),
            LineValue::Num(_) | LineValue::Text(_) | LineValue::Bool(_) => {}
        }
    }

    /// The value from its JSON encoding (spec/schema.md): number → Num, string → Text,
    /// `true`/`false` → Bool, array → List, `{"ref": "<id>"}` → Ref. `null` and any other object
    /// are invalid.
    pub fn from_json(v: &Json) -> Result<LineValue, String> {
        Ok(match v {
            Json::Num(x) => LineValue::Num(*x),
            Json::Str(s) => LineValue::Text(s.clone()),
            Json::Bool(b) => LineValue::Bool(*b),
            Json::Array(items) => LineValue::List(items.iter().map(LineValue::from_json).collect::<Result<_, _>>()?),
            Json::Object(members) => match members.as_slice() {
                [(key, Json::Str(target))] if key == "ref" => LineValue::Ref(target.clone()),
                _ => return Err("an object value must be exactly {\"ref\": \"<id>\"}".to_string()),
            },
            Json::Null => return Err("null is not a value".to_string()),
        })
    }

    /// Appends the JSON encoding of the value, naming Ref targets as written.
    pub fn write_json(&self, out: &mut String) {
        match self {
            LineValue::Num(x) => json::write_num(out, *x),
            LineValue::Text(s) => json::write_str(out, s),
            LineValue::Ref(target) => {
                out.push_str("{\"ref\":");
                json::write_str(out, target);
                out.push('}');
            }
            LineValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            LineValue::List(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
        }
    }

    /// True when the value has a word form (`num`, `text` or `ref` with a one-word payload).
    fn has_word_form(&self) -> bool {
        match self {
            LineValue::Num(x) => x.is_finite(),
            LineValue::Text(s) | LineValue::Ref(s) => is_word(s),
            LineValue::Bool(_) | LineValue::List(_) => false,
        }
    }
}

/// True when `s` can stand as one word of a line: non-empty, without whitespace.
fn is_word(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(char::is_whitespace)
}

impl LineOp {
    /// The name of the entity this op acts on.
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

    /// Every entity name the op mentions: its target, then each Ref target in its value (Refs
    /// nested in Lists included).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let mut names = vec![self.seed()];
        if let LineOp::Set { value, .. } = self {
            value.ref_names(&mut names);
        }
        names.into_iter()
    }

    /// The document ops this op stands for. A `create` also sets the reserved props
    /// (spec/schema.md): `_name`, the name as written, and `_schema`, the schema version.
    pub fn to_ops(&self) -> Vec<Op> {
        match self {
            LineOp::Create { seed, kind } => {
                let id = resolve_id(seed);
                vec![
                    Op::Create { id, kind: kind.clone() },
                    Op::Set { id, key: NAME_PROP.to_string(), value: Value::Text(seed.clone()) },
                    Op::Set {
                        id,
                        key: SCHEMA_PROP.to_string(),
                        value: Value::Num(f64::from(Document::SCHEMA_VERSION)),
                    },
                ]
            }
            LineOp::Set { seed, key, value } => {
                vec![Op::Set { id: resolve_id(seed), key: key.clone(), value: value.to_value() }]
            }
            LineOp::Delete { seed } => vec![Op::Delete { id: resolve_id(seed) }],
        }
    }

    /// Applies the op to `doc`, all of its document ops or none.
    pub fn apply_to(&self, doc: &mut Document) -> Result<(), DocError> {
        // Only the first document op can fail (a create's reserved sets target the new entity).
        for op in self.to_ops() {
            doc.apply(op)?;
        }
        Ok(())
    }

    /// Words for the error `err` that [`bim_doc::Document::apply`] gave this op, naming the entity
    /// by the name it was written with rather than by its 128-bit id.
    pub fn describe_error(&self, err: &DocError) -> String {
        let seed = self.seed();
        match err {
            DocError::NotFound(_) => format!("entity `{seed}` not found"),
            DocError::AlreadyExists(_) => format!("entity `{seed}` already exists"),
        }
    }

    /// True when the op has a word form (see the module docs).
    fn has_word_form(&self) -> bool {
        match self {
            LineOp::Create { seed, kind } => is_word(seed) && is_word(kind),
            LineOp::Set { seed, key, value } => is_word(seed) && is_word(key) && value.has_word_form(),
            LineOp::Delete { seed } => is_word(seed),
        }
    }

    /// The op as one compact JSON object, the shape of a JSON ops file entry.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        let field = |out: &mut String, name: &str, value: &str| {
            out.push_str(",\"");
            out.push_str(name);
            out.push_str("\":");
            json::write_str(out, value);
        };
        match self {
            LineOp::Create { seed, kind } => {
                out.push_str("{\"op\":\"create\"");
                field(&mut out, "id", seed);
                field(&mut out, "kind", kind);
            }
            LineOp::Set { seed, key, value } => {
                out.push_str("{\"op\":\"set\"");
                field(&mut out, "id", seed);
                field(&mut out, "key", key);
                out.push_str(",\"value\":");
                value.write_json(&mut out);
            }
            LineOp::Delete { seed } => {
                out.push_str("{\"op\":\"delete\"");
                field(&mut out, "id", seed);
            }
        }
        out.push('}');
        out
    }
}

impl fmt::Display for LineValue {
    /// The word form (`num 3.5`, `text north`, `ref lvl`) when there is one, else the JSON encoding.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineValue::Num(x) if x.is_finite() => write!(f, "num {x:?}"),
            LineValue::Text(word) if is_word(word) => write!(f, "text {word}"),
            LineValue::Ref(seed) if is_word(seed) => write!(f, "ref {seed}"),
            other => {
                let mut out = String::new();
                other.write_json(&mut out);
                f.write_str(&out)
            }
        }
    }
}

impl fmt::Display for LineOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.has_word_form() {
            return f.write_str(&self.to_json());
        }
        match self {
            LineOp::Create { seed, kind } => write!(f, "create {seed} {kind}"),
            LineOp::Set { seed, key, value } => write!(f, "set {seed} {key} {value}"),
            LineOp::Delete { seed } => write!(f, "delete {seed}"),
        }
    }
}

/// Parses one line. `Ok(None)` for a blank line, `Err(reason)` for an invalid one.
pub fn parse_line(line: &str) -> Result<Option<LineOp>, String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('{') {
        let value = json::parse(trimmed)?;
        let ops = op_from_json(&value, false)?;
        let [op] = <[LineOp; 1]>::try_from(ops).expect("a create without props is one op");
        return Ok(Some(op));
    }
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

/// The ops of one JSON op object (an entry of a JSON ops file's `ops` array): a `create` with
/// `props` becomes the create followed by one `set` per prop, in file order. `allow_props` says
/// whether `create` may carry `props` (not in a single line, which holds exactly one op).
///
/// Every member must be known and of the right type, and none may repeat.
pub fn op_from_json(v: &Json, allow_props: bool) -> Result<Vec<LineOp>, String> {
    let Json::Object(members) = v else {
        return Err(format!("an op must be an object, found {}", v.type_name()));
    };
    let mut fields = Fields::new(members)?;
    let op = fields.string("op")?;
    let ops = match op.as_str() {
        "create" => {
            let seed = fields.string("id")?;
            let kind = fields.string("kind")?;
            if kind.is_empty() {
                return Err("`kind` must not be empty".to_string());
            }
            let mut ops = vec![LineOp::Create { seed: seed.clone(), kind }];
            if let Some(props) = fields.take("props") {
                if !allow_props {
                    return Err("a one-line `create` takes no `props` (write them as `set` ops)".to_string());
                }
                let Json::Object(props) = props else {
                    return Err(format!("`props` must be an object, found {}", props.type_name()));
                };
                check_unique(props, "props")?;
                for (key, value) in props {
                    let value = LineValue::from_json(value).map_err(|e| format!("prop `{key}`: {e}"))?;
                    ops.push(LineOp::Set { seed: seed.clone(), key: key.clone(), value });
                }
            }
            ops
        }
        "set" => {
            let seed = fields.string("id")?;
            let key = fields.string("key")?;
            let value = fields.take("value").ok_or("missing member `value`")?;
            let value = LineValue::from_json(value).map_err(|e| format!("`value`: {e}"))?;
            vec![LineOp::Set { seed, key, value }]
        }
        "delete" => vec![LineOp::Delete { seed: fields.string("id")? }],
        other => return Err(format!("unknown op `{other}` (expected create, set or delete)")),
    };
    fields.finish()?;
    Ok(ops)
}

/// Parses a JSON ops file (spec/cli.md): `{"schema": 1, "ops": [<op>, …]}`. Each op comes with
/// its index in `ops`. Nothing is checked against a document here.
pub fn parse_json_ops(text: &str) -> Result<Vec<(usize, LineOp)>, String> {
    let root = json::parse(text)?;
    let Json::Object(members) = &root else {
        return Err(format!("a JSON ops file must be an object, found {}", root.type_name()));
    };
    let mut fields = Fields::new(members)?;
    let schema = fields.take("schema").ok_or("missing member `schema`")?;
    match schema {
        Json::Num(x) if *x == f64::from(Document::SCHEMA_VERSION) => {}
        Json::Num(x) => {
            return Err(format!("unsupported schema version {x} (expected {})", Document::SCHEMA_VERSION));
        }
        other => return Err(format!("`schema` must be a number, found {}", other.type_name())),
    }
    let ops = fields.take("ops").ok_or("missing member `ops`")?;
    fields.finish()?;
    let Json::Array(ops) = ops else {
        return Err(format!("`ops` must be an array, found {}", ops.type_name()));
    };
    let mut out = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        for op in op_from_json(op, true).map_err(|e| format!("ops[{index}]: {e}"))? {
            out.push((index, op));
        }
    }
    Ok(out)
}

fn check_unique(members: &[(String, Json)], what: &str) -> Result<(), String> {
    for (i, (key, _)) in members.iter().enumerate() {
        if members[..i].iter().any(|(k, _)| k == key) {
            return Err(format!("duplicate member `{key}` in {what}"));
        }
    }
    Ok(())
}

/// The members of a JSON object, taken one by one; [`Fields::finish`] rejects any left over.
struct Fields<'a>(Vec<(&'a str, &'a Json)>);

impl<'a> Fields<'a> {
    fn new(members: &'a [(String, Json)]) -> Result<Self, String> {
        check_unique(members, "object")?;
        Ok(Fields(members.iter().map(|(k, v)| (k.as_str(), v)).collect()))
    }

    fn take(&mut self, name: &str) -> Option<&'a Json> {
        let at = self.0.iter().position(|(k, _)| *k == name)?;
        Some(self.0.remove(at).1)
    }

    fn string(&mut self, name: &str) -> Result<String, String> {
        match self.take(name) {
            Some(Json::Str(s)) => Ok(s.clone()),
            Some(other) => Err(format!("`{name}` must be a string, found {}", other.type_name())),
            None => Err(format!("missing member `{name}`")),
        }
    }

    fn finish(self) -> Result<(), String> {
        match self.0.first() {
            Some((k, _)) => Err(format!("unknown member `{k}`")),
            None => Ok(()),
        }
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
        assert!(reject("set a k json 1").contains("unknown value type `json`"));
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
    fn to_ops_resolves_names_and_sets_reserved_props_at_create() {
        let wall = EntityId::derive("wall");
        assert_eq!(
            parse("create wall Wall").to_ops(),
            vec![
                Op::Create { id: wall, kind: "Wall".into() },
                Op::Set { id: wall, key: "_name".into(), value: Value::Text("wall".into()) },
                Op::Set { id: wall, key: "_schema".into(), value: Value::Num(1.0) },
            ]
        );
        assert_eq!(
            parse("set wall host ref lvl").to_ops(),
            vec![Op::Set { id: wall, key: "host".into(), value: Value::Ref(EntityId::derive("lvl")) }]
        );
        assert_eq!(
            parse("set wall h num 2").to_ops(),
            vec![Op::Set { id: wall, key: "h".into(), value: Value::Num(2.0) }]
        );
        assert_eq!(
            parse("set wall n text north").to_ops(),
            vec![Op::Set { id: wall, key: "n".into(), value: Value::Text("north".into()) }]
        );
        assert_eq!(parse("delete wall").to_ops(), vec![Op::Delete { id: wall }]);
        // A ULID names itself; any other string is a seed.
        let ulid = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        assert_eq!(parse(&format!("delete {ulid}")).to_ops(), vec![Op::Delete { id: EntityId::parse(ulid) }]);
        assert_ne!(EntityId::parse(ulid), EntityId::derive(ulid));
    }

    #[test]
    fn ops_without_a_word_form_are_written_as_json_lines_and_read_back() {
        let ops = [
            set("w", "name", LineValue::Text("north wall, façade".into())),
            set("w", "name", LineValue::Text(String::new())),
            set("w", "tags", LineValue::List(vec![
                LineValue::Ref("L 1".into()),
                LineValue::Text("a\nb\"c".into()),
                LineValue::List(vec![LineValue::Num(1.0), LineValue::Bool(false)]),
                LineValue::List(vec![]),
            ])),
            set("w", "open", LineValue::Bool(true)),
            set("w x", "k", LineValue::Num(-0.0)),
            set("w", "a key", LineValue::Num(1.0)),
            set("w", "host", LineValue::Ref("".into())),
            create("", "Wall"),
            create("w", "Curtain Wall"),
            LineOp::Delete { seed: "two words".into() },
        ];
        for op in ops {
            let line = op.to_string();
            assert!(line.starts_with('{') && !line.contains('\n'), "{line}");
            assert_eq!(parse(&line), op, "{line}");
        }
        assert_eq!(
            set("w", "open", LineValue::Bool(true)).to_string(),
            r#"{"op":"set","id":"w","key":"open","value":true}"#
        );
        assert_eq!(create("w", "Wall").to_string(), "create w Wall", "word-form ops keep the T1.5 form");
    }

    #[test]
    fn json_lines_are_checked_like_json_ops() {
        assert!(reject(r#"{"op":"create","id":"a","kind":"W","props":{}}"#).contains("takes no `props`"));
        assert!(reject(r#"{"op":"rename","id":"a"}"#).contains("unknown op `rename`"));
        assert!(reject(r#"{"op":"set","id":"a","key":"k","value":null}"#).contains("null"));
        assert!(reject(r#"{"op":"delete","id":"a","extra":1}"#).contains("unknown member `extra`"));
        assert!(reject(r#"{"op":"delete","id":"a""#).contains("invalid JSON"));
    }

    #[test]
    fn json_ops_files_expand_create_props_in_order() {
        let text = r#"{"schema": 1, "ops": [
            {"op": "create", "id": "L1", "kind": "Level", "props": {"elevation": 0, "on": true}},
            {"op": "set", "id": "L1", "key": "refs", "value": [{"ref": "X"}, [1.5]]},
            {"op": "create", "id": "w", "kind": "Wall"},
            {"op": "delete", "id": "w"}]}"#;
        let ops = parse_json_ops(text).unwrap();
        assert_eq!(
            ops,
            vec![
                (0, create("L1", "Level")),
                (0, set("L1", "elevation", LineValue::Num(0.0))),
                (0, set("L1", "on", LineValue::Bool(true))),
                (1, set("L1", "refs", LineValue::List(vec![LineValue::Ref("X".into()), LineValue::List(vec![LineValue::Num(1.5)])]))),
                (2, create("w", "Wall")),
                (3, LineOp::Delete { seed: "w".into() }),
            ]
        );
        assert_eq!(ops[3].1.names().collect::<Vec<_>>(), ["L1", "X"]);
        assert_eq!(parse_json_ops(r#"{"ops": [], "schema": 1.0}"#), Ok(vec![]));
    }

    #[test]
    fn invalid_json_ops_files_are_rejected() {
        let cases = [
            ("[]", "must be an object"),
            (r#"{"schema": 2, "ops": []}"#, "unsupported schema version 2"),
            (r#"{"schema": "1", "ops": []}"#, "`schema` must be a number"),
            (r#"{"ops": []}"#, "missing member `schema`"),
            (r#"{"schema": 1}"#, "missing member `ops`"),
            (r#"{"schema": 1, "ops": {}}"#, "`ops` must be an array"),
            (r#"{"schema": 1, "ops": [], "x": 1}"#, "unknown member `x`"),
            (r#"{"schema": 1, "schema": 1, "ops": []}"#, "duplicate member `schema`"),
            (r#"{"schema": 1, "ops": [3]}"#, "ops[0]: an op must be an object"),
            (r#"{"schema": 1, "ops": [{"id": "a"}]}"#, "missing member `op`"),
            (r#"{"schema": 1, "ops": [{"op": "create", "id": "a"}]}"#, "missing member `kind`"),
            (r#"{"schema": 1, "ops": [{"op": "create", "id": "a", "kind": ""}]}"#, "`kind` must not be empty"),
            (r#"{"schema": 1, "ops": [{"op": "create", "id": 1, "kind": "W"}]}"#, "`id` must be a string"),
            (r#"{"schema": 1, "ops": [{"op": "create", "id": "a", "kind": "W", "props": []}]}"#, "`props` must be an object"),
            (r#"{"schema": 1, "ops": [{"op": "create", "id": "a", "kind": "W", "props": {"k": 1, "k": 2}}]}"#, "duplicate member `k`"),
            (r#"{"schema": 1, "ops": [{"op": "create", "id": "a", "kind": "W", "props": {"k": [null]}}]}"#, "null"),
            (r#"{"schema": 1, "ops": [{"op": "set", "id": "a", "key": "k"}]}"#, "missing member `value`"),
            (r#"{"schema": 1, "ops": [{"op": "set", "id": "a", "key": "k", "value": {"ref": 1}}]}"#, "{\"ref\""),
            (r#"{"schema": 1, "ops": [{"op": "set", "id": "a", "key": "k", "value": {"ref": "x", "y": 1}}]}"#, "{\"ref\""),
            (r#"{"schema": 1, "ops": [{"op": "set", "id": "a", "key": "k", "value": 1e999}]}"#, "not a finite"),
            (r#"{"schema": 1, "ops": [{"op": "Delete", "id": "a"}]}"#, "unknown op `Delete`"),
            (r#"{"schema": 1, "ops": []} x"#, "invalid JSON"),
        ];
        for (text, want) in cases {
            let err = parse_json_ops(text).unwrap_err();
            assert!(err.contains(want), "{text}: {err}");
        }
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
        let nested = set("w", "k", LineValue::List(vec![LineValue::Ref("a".into()), LineValue::List(vec![LineValue::Ref("b".into())])]));
        assert_eq!(nested.names().collect::<Vec<_>>(), ["w", "a", "b"]);
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
