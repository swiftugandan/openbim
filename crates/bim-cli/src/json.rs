//! A small, strict JSON (RFC 8259) reader and writer, enough for ops files, journal lines and
//! `--json` output. Hand-written so that `bim-cli` needs no JSON crate.
//!
//! The reader keeps object members in file order (duplicates included, so callers can reject
//! them), rejects trailing garbage, raw control characters in strings, lone surrogates and numbers
//! that overflow `f64`, and caps nesting depth so hostile input cannot exhaust the stack.

use std::fmt::Write as _;

/// Deepest array/object nesting [`parse`] accepts.
pub const MAX_DEPTH: usize = 128;

/// A parsed JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// A number; always finite.
    Num(f64),
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Json>),
    /// An object: members in file order, duplicate keys kept.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// A short name of the value's type, for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "a boolean",
            Json::Num(_) => "a number",
            Json::Str(_) => "a string",
            Json::Array(_) => "an array",
            Json::Object(_) => "an object",
        }
    }
}

/// Parses one JSON document (surrounding whitespace allowed, a leading UTF-8 byte order mark
/// ignored). The error names the byte offset of the problem.
pub fn parse(text: &str) -> Result<Json, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser { s: text.as_bytes(), i: 0, text };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("unexpected text after the JSON value"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    text: &'a str,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("invalid JSON at byte {}: {what}", self.i)
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.s.get(self.i) {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected `{}`", b as char)))
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("unexpected character"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'[') => {
                if depth >= MAX_DEPTH {
                    return Err(self.err(&format!("nesting deeper than {MAX_DEPTH}")));
                }
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(self.err("expected `,` or `]`")),
                    }
                }
            }
            Some(b'{') => {
                if depth >= MAX_DEPTH {
                    return Err(self.err(&format!("nesting deeper than {MAX_DEPTH}")));
                }
                self.i += 1;
                let mut members = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Json::Object(members));
                }
                loop {
                    self.ws();
                    if self.peek() != Some(b'"') {
                        return Err(self.err("expected a string key"));
                    }
                    let key = self.string()?;
                    self.ws();
                    self.expect(b':')?;
                    self.ws();
                    members.push((key, self.value(depth + 1)?));
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Object(members));
                        }
                        _ => return Err(self.err("expected `,` or `}`")),
                    }
                }
            }
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while let Some(b'0'..=b'9') = self.peek() {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err("expected a digit")),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if self.digits() == 0 {
                return Err(self.err("expected a digit after `.`"));
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.i += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.err("expected a digit in the exponent"));
            }
        }
        let word = &self.text[start..self.i];
        match word.parse::<f64>() {
            Ok(x) if x.is_finite() => Ok(Json::Num(x)),
            _ => Err(format!("invalid JSON at byte {start}: number `{word}` is not a finite 64-bit float")),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("truncated `\\u` escape"))?;
        let mut v = 0;
        for &d in digits {
            let n = (d as char).to_digit(16).ok_or_else(|| self.err("bad hex digit in `\\u` escape"))?;
            v = v * 16 + n;
        }
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            // Copy the run of plain characters in one go (the input is valid UTF-8).
            let start = self.i;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(&self.text[start..self.i]);
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = match hi {
                                0xD800..=0xDBFF => {
                                    if !self.s[self.i..].starts_with(b"\\u") {
                                        return Err(self.err("lone high surrogate in `\\u` escape"));
                                    }
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if !(0xDC00..=0xDFFF).contains(&lo) {
                                        return Err(self.err("high surrogate not followed by a low surrogate"));
                                    }
                                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                }
                                0xDC00..=0xDFFF => return Err(self.err("lone low surrogate in `\\u` escape")),
                                _ => hi,
                            };
                            out.push(char::from_u32(code).expect("a valid scalar value"));
                        }
                        _ => {
                            self.i -= 1;
                            return Err(self.err("unknown escape"));
                        }
                    }
                }
                Some(_) => return Err(self.err("raw control character in string")),
            }
        }
    }
}

/// Appends `s` as a JSON string literal. Every control character is escaped, so the result never
/// contains a line break; other characters (non-ASCII included) are written as they are.
pub fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            // C0, DEL, C1 and the Unicode line/paragraph separators: escaped so output is one
            // line under any reading of "line break".
            c if (c as u32) < 0x20 || ('\u{7f}'..='\u{9f}').contains(&c) || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Appends `x` as a JSON number in Rust's shortest round-trip form (`3.0`, `0.001`, `1e300`,
/// `-0.0`): parsing it gives back `x` bit for bit. Non-finite numbers have no JSON form and are
/// written as `null`.
pub fn write_num(out: &mut String, x: f64) {
    if x.is_finite() {
        let _ = write!(out, "{x:?}");
    } else {
        out.push_str("null");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> String {
        match parse(text) {
            Ok(Json::Str(s)) => s,
            other => panic!("{text}: {other:?}"),
        }
    }

    #[test]
    fn parses_every_value_kind() {
        assert_eq!(parse(" null ").unwrap(), Json::Null);
        assert_eq!(parse("true").unwrap(), Json::Bool(true));
        assert_eq!(parse("false").unwrap(), Json::Bool(false));
        assert_eq!(parse("-0").unwrap(), Json::Num(-0.0));
        assert_eq!(parse("1.5e3").unwrap(), Json::Num(1500.0));
        assert_eq!(parse("[1, [], {}]").unwrap(), Json::Array(vec![Json::Num(1.0), Json::Array(vec![]), Json::Object(vec![])]));
        assert_eq!(
            parse(r#"{"a": 1, "a": "x"}"#).unwrap(),
            Json::Object(vec![("a".into(), Json::Num(1.0)), ("a".into(), Json::Str("x".into()))])
        );
        assert_eq!(parse("\u{feff}[]").unwrap(), Json::Array(vec![]));
    }

    #[test]
    fn decodes_escapes_and_surrogate_pairs() {
        assert_eq!(s(r#""a\"b\\c\/d\b\f\n\r\t""#), "a\"b\\c/d\u{8}\u{c}\n\r\t");
        assert_eq!(s(r#""fa\u00e7ade""#), "façade");
        assert_eq!(s(r#""\ud83d\ude00""#), "😀");
        assert_eq!(s("\"façade 😀\""), "façade 😀");
    }

    #[test]
    fn rejects_invalid_documents() {
        for bad in [
            "", " ", "nul", "True", "[1,]", "[1 2]", "{\"a\":1,}", "{a:1}", "{\"a\" 1}", "01", "1.", ".5", "+1", "1e", "-",
            "0x10", "NaN", "Infinity", "1e400", "-1e400", "\"abc", "\"a\nb\"", "\"\\x\"", "\"\\u12\"", "\"\\ud800\"",
            "\"\\ud800\\u0041\"", "\"\\udc00\"", "[1] x", "{} {}", "'a'", "[", "{",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should be rejected");
        }
        let deep = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
        assert!(parse(&deep).unwrap_err().contains("nesting"));
        let ok = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert!(parse(&ok).is_ok());
        assert!(parse(&"[".repeat(100_000)).is_err(), "no stack overflow");
    }

    #[test]
    fn strings_round_trip_on_one_line() {
        for text in ["", "plain", "q\"b\\s", "line\nbreak\r\t", "\u{0}\u{1f}\u{7f}\u{85}\u{2028}", "ünï €😀"] {
            let mut out = String::new();
            write_str(&mut out, text);
            assert!(!out.contains('\n') && !out.contains('\r') && !out.contains('\u{2028}'), "{out}");
            assert_eq!(s(&out), text);
        }
    }

    #[test]
    fn numbers_round_trip_bit_for_bit() {
        for x in [0.0, -0.0, 3.0, 0.1 + 0.2, 1e300, 5e-324, -1e-3, f64::MAX, f64::MIN_POSITIVE, 1e21, 123456789012345680.0] {
            let mut out = String::new();
            write_num(&mut out, x);
            match parse(&out) {
                Ok(Json::Num(y)) => assert_eq!(x.to_bits(), y.to_bits(), "{out}"),
                other => panic!("{out}: {other:?}"),
            }
        }
        let mut out = String::new();
        write_num(&mut out, f64::NAN);
        assert_eq!(out, "null");
    }
}
