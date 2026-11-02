//! Streaming Part 21 lexer and parser.

use crate::error::{ErrorKind, StepError};
use crate::model::{HeaderEntry, Instance, Param, StepFile};
use crate::strings::decode_string;
use std::io::{self, BufRead};

/// Maximum nesting of parentheses inside one record.
const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Keyword(String),
    InstanceName(u64),
    Integer(i64),
    Real(f64),
    Str(String),
    Binary(String),
    Enum(String),
    Dollar,
    Star,
    LParen,
    RParen,
    Comma,
    Semi,
    Eq,
    Eof,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Keyword(k) => format!("keyword {k}"),
            Tok::InstanceName(id) => format!("#{id}"),
            Tok::Integer(i) => format!("integer {i}"),
            Tok::Real(r) => format!("real {r}"),
            Tok::Str(_) => "string".into(),
            Tok::Binary(_) => "binary".into(),
            Tok::Enum(e) => format!(".{e}."),
            Tok::Dollar => "'$'".into(),
            Tok::Star => "'*'".into(),
            Tok::LParen => "'('".into(),
            Tok::RParen => "')'".into(),
            Tok::Comma => "','".into(),
            Tok::Semi => "';'".into(),
            Tok::Eq => "'='".into(),
            Tok::Eof => "end of input".into(),
        }
    }
}

struct Lexer<R: BufRead> {
    r: R,
    line: usize,
    scratch: Vec<u8>,
}

impl<R: BufRead> Lexer<R> {
    fn new(r: R) -> Self {
        Lexer { r, line: 1, scratch: Vec::with_capacity(256) }
    }

    fn err(&self, kind: ErrorKind, msg: impl Into<String>) -> StepError {
        StepError::new(kind, self.line, msg)
    }

    fn peek(&mut self) -> Result<Option<u8>, StepError> {
        loop {
            match self.r.fill_buf() {
                Ok(buf) => return Ok(buf.first().copied()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(StepError::io(e, self.line)),
            }
        }
    }

    fn bump(&mut self) -> Result<Option<u8>, StepError> {
        let b = self.peek()?;
        if let Some(b) = b {
            self.r.consume(1);
            if b == b'\n' {
                self.line += 1;
            }
        }
        Ok(b)
    }

    /// Consumes bytes while `pred` holds, appending them to `self.scratch`.
    fn take_while(&mut self, pred: impl Fn(u8) -> bool) -> Result<(), StepError> {
        loop {
            let buf = match self.r.fill_buf() {
                Ok(buf) => buf,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(StepError::io(e, self.line)),
            };
            if buf.is_empty() {
                return Ok(());
            }
            let n = buf.iter().position(|&b| !pred(b)).unwrap_or(buf.len());
            self.line += buf[..n].iter().filter(|&&b| b == b'\n').count();
            self.scratch.extend_from_slice(&buf[..n]);
            let done = n < buf.len();
            self.r.consume(n);
            if done {
                return Ok(());
            }
        }
    }

    fn skip_space_and_comments(&mut self) -> Result<(), StepError> {
        loop {
            match self.peek()? {
                Some(b) if b <= b' ' || b == 0x7f => {
                    self.bump()?;
                }
                Some(b'/') => {
                    self.bump()?;
                    if self.bump()? != Some(b'*') {
                        return Err(self.err(ErrorKind::InvalidToken, "'/' that does not start a comment"));
                    }
                    let start = self.line;
                    let mut star = false;
                    loop {
                        match self.bump()? {
                            None => {
                                return Err(StepError::new(
                                    ErrorKind::UnterminatedComment,
                                    start,
                                    "comment not closed before end of input",
                                ))
                            }
                            Some(b'/') if star => break,
                            Some(b) => star = b == b'*',
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next(&mut self) -> Result<Tok, StepError> {
        self.skip_space_and_comments()?;
        let b = match self.peek()? {
            None => return Ok(Tok::Eof),
            Some(b) => b,
        };
        match b {
            b'(' | b')' | b',' | b';' | b'=' | b'$' | b'*' => {
                self.bump()?;
                Ok(match b {
                    b'(' => Tok::LParen,
                    b')' => Tok::RParen,
                    b',' => Tok::Comma,
                    b';' => Tok::Semi,
                    b'=' => Tok::Eq,
                    b'$' => Tok::Dollar,
                    _ => Tok::Star,
                })
            }
            b'#' => {
                self.bump()?;
                self.scratch.clear();
                self.take_while(|c| c.is_ascii_digit())?;
                if self.scratch.is_empty() {
                    return Err(self.err(ErrorKind::InvalidToken, "'#' not followed by an instance number"));
                }
                let s = std::str::from_utf8(&self.scratch).unwrap_or("");
                s.parse::<u64>()
                    .map(Tok::InstanceName)
                    .map_err(|_| self.err(ErrorKind::InvalidNumber, format!("instance number #{s} out of range")))
            }
            b'0'..=b'9' | b'+' | b'-' => self.number(),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'!' => {
                self.scratch.clear();
                self.bump()?;
                self.scratch.push(b);
                self.take_while(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')?;
                let mut s = String::from_utf8_lossy(&self.scratch).into_owned();
                s.make_ascii_uppercase();
                Ok(Tok::Keyword(s))
            }
            b'.' => {
                self.bump()?;
                self.scratch.clear();
                self.take_while(|c| c.is_ascii_alphanumeric() || c == b'_')?;
                if self.scratch.is_empty() || self.bump()? != Some(b'.') {
                    return Err(self.err(ErrorKind::InvalidToken, "malformed enumeration value"));
                }
                let mut s = String::from_utf8_lossy(&self.scratch).into_owned();
                s.make_ascii_uppercase();
                Ok(Tok::Enum(s))
            }
            b'\'' => self.string(),
            b'"' => self.binary(),
            _ => Err(self.err(ErrorKind::InvalidToken, format!("unexpected character {:?}", b as char))),
        }
    }

    fn number(&mut self) -> Result<Tok, StepError> {
        self.scratch.clear();
        if let Some(sign @ (b'+' | b'-')) = self.peek()? {
            self.bump()?;
            self.scratch.push(sign);
        }
        let digits_start = self.scratch.len();
        self.take_while(|c| c.is_ascii_digit())?;
        if self.scratch.len() == digits_start {
            return Err(self.err(ErrorKind::InvalidToken, "sign not followed by a digit"));
        }
        let mut real = false;
        if self.peek()? == Some(b'.') {
            real = true;
            self.bump()?;
            self.scratch.push(b'.');
            let frac_start = self.scratch.len();
            self.take_while(|c| c.is_ascii_digit())?;
            if self.scratch.len() == frac_start {
                self.scratch.push(b'0');
            }
        }
        if let Some(e @ (b'E' | b'e')) = self.peek()? {
            real = true;
            self.bump()?;
            self.scratch.push(e);
            if let Some(sign @ (b'+' | b'-')) = self.peek()? {
                self.bump()?;
                self.scratch.push(sign);
            }
            let exp_start = self.scratch.len();
            self.take_while(|c| c.is_ascii_digit())?;
            if self.scratch.len() == exp_start {
                return Err(self.err(ErrorKind::InvalidNumber, "exponent without digits"));
            }
        }
        let s = std::str::from_utf8(&self.scratch).unwrap_or("");
        if real {
            s.parse::<f64>()
                .map(Tok::Real)
                .map_err(|_| self.err(ErrorKind::InvalidNumber, format!("invalid real {s}")))
        } else {
            s.parse::<i64>()
                .map(Tok::Integer)
                .map_err(|_| self.err(ErrorKind::InvalidNumber, format!("integer {s} out of range")))
        }
    }

    /// Scans a string literal and decodes it. The lexical scan keeps `''`, `\\` and `\S\c` (whose
    /// `c` may be an apostrophe) intact. Raw line breaks inside the literal are kept as characters
    /// of the value (as ifcopenshell reads them).
    fn string(&mut self) -> Result<Tok, StepError> {
        let start = self.line;
        self.bump()?; // opening apostrophe
        self.scratch.clear();
        let unterminated =
            || StepError::new(ErrorKind::UnterminatedString, start, "string literal not closed before end of input");
        loop {
            self.take_while(|c| c != b'\'' && c != b'\\')?;
            match self.bump()? {
                None => return Err(unterminated()),
                Some(b'\'') => {
                    if self.peek()? == Some(b'\'') {
                        self.bump()?;
                        self.scratch.extend_from_slice(b"''");
                    } else {
                        break;
                    }
                }
                Some(_) => {
                    // backslash
                    self.scratch.push(b'\\');
                    match self.peek()? {
                        Some(b'\\') => {
                            self.bump()?;
                            self.scratch.push(b'\\');
                        }
                        Some(b'S') => {
                            self.bump()?;
                            self.scratch.push(b'S');
                            if self.peek()? == Some(b'\\') {
                                self.bump()?;
                                self.scratch.push(b'\\');
                                match self.bump()? {
                                    None => return Err(unterminated()),
                                    Some(c) => self.scratch.push(c),
                                }
                            }
                        }
                        None => return Err(unterminated()),
                        _ => {}
                    }
                }
            }
        }
        decode_string(&self.scratch)
            .map(Tok::Str)
            .map_err(|m| StepError::new(ErrorKind::InvalidEscape, start, m))
    }

    fn binary(&mut self) -> Result<Tok, StepError> {
        self.bump()?;
        self.scratch.clear();
        self.take_while(|c| c.is_ascii_hexdigit())?;
        if self.bump()? != Some(b'"') {
            return Err(self.err(ErrorKind::InvalidBinary, "binary literal not closed by '\"' or contains non-hex digits"));
        }
        let digits = &self.scratch;
        let unused = match digits.first() {
            Some(d @ b'0'..=b'3') => (d - b'0') as usize,
            _ => return Err(self.err(ErrorKind::InvalidBinary, "binary literal must start with 0..3 unused bits")),
        };
        if digits.len() == 1 && unused != 0 {
            return Err(self.err(ErrorKind::InvalidBinary, "binary literal without digits has unused bits"));
        }
        let mut bits = String::with_capacity(digits.len() * 4);
        for &d in &digits[1..] {
            let v = (d as char).to_digit(16).unwrap_or(0);
            for k in (0..4).rev() {
                bits.push(if (v >> k) & 1 == 1 { '1' } else { '0' });
            }
        }
        Ok(Tok::Binary(bits[unused..].to_string()))
    }
}

struct Parser<R: BufRead> {
    lex: Lexer<R>,
}

impl<R: BufRead> Parser<R> {
    fn unexpected(&self, tok: &Tok, wanted: &str) -> StepError {
        let kind = if *tok == Tok::Eof { ErrorKind::UnexpectedEof } else { ErrorKind::UnexpectedToken };
        self.lex.err(kind, format!("expected {wanted}, found {}", tok.describe()))
    }

    fn expect(&mut self, want: Tok, wanted: &str) -> Result<(), StepError> {
        let t = self.lex.next()?;
        if t == want {
            Ok(())
        } else {
            Err(self.unexpected(&t, wanted))
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), StepError> {
        match self.lex.next()? {
            Tok::Keyword(k) if k == kw => Ok(()),
            t => Err(self.unexpected(&t, kw)),
        }
    }

    /// Parses the parameters after an opening parenthesis, through the closing one.
    fn params(&mut self, depth: usize) -> Result<Vec<Param>, StepError> {
        if depth > MAX_DEPTH {
            return Err(self.lex.err(ErrorKind::TooDeep, "parameter lists nested too deeply"));
        }
        let mut out = Vec::new();
        let mut t = self.lex.next()?;
        if t == Tok::RParen {
            return Ok(out);
        }
        loop {
            out.push(self.param(t, depth)?);
            match self.lex.next()? {
                Tok::Comma => t = self.lex.next()?,
                Tok::RParen => return Ok(out),
                other => return Err(self.unexpected(&other, "',' or ')'")),
            }
        }
    }

    fn param(&mut self, t: Tok, depth: usize) -> Result<Param, StepError> {
        Ok(match t {
            Tok::Dollar => Param::Unset,
            Tok::Star => Param::Derived,
            Tok::Integer(i) => Param::Integer(i),
            Tok::Real(r) => Param::Real(r),
            Tok::Str(s) => Param::String(s),
            Tok::Binary(b) => Param::Binary(b),
            Tok::Enum(e) => Param::Enum(e),
            Tok::InstanceName(id) => Param::Ref(id),
            Tok::LParen => Param::List(self.params(depth + 1)?),
            Tok::Keyword(name) => {
                self.expect(Tok::LParen, "'(' after a type name")?;
                if depth + 1 > MAX_DEPTH {
                    return Err(self.lex.err(ErrorKind::TooDeep, "parameter lists nested too deeply"));
                }
                let t = self.lex.next()?;
                let inner = self.param(t, depth + 1)?;
                self.expect(Tok::RParen, "')' closing a typed value")?;
                Param::Typed(name, Box::new(inner))
            }
            other => return Err(self.unexpected(&other, "a parameter")),
        })
    }

    fn header(&mut self) -> Result<Vec<HeaderEntry>, StepError> {
        match self.lex.next()? {
            Tok::Keyword(k) if k == "ISO-10303-21" => {}
            t => return Err(self.unexpected(&t, "ISO-10303-21 (not a STEP physical file)")),
        }
        self.expect(Tok::Semi, "';'")?;
        self.expect_keyword("HEADER")?;
        self.expect(Tok::Semi, "';'")?;
        let mut entries = Vec::new();
        loop {
            match self.lex.next()? {
                Tok::Keyword(k) if k == "ENDSEC" => {
                    self.expect(Tok::Semi, "';' after ENDSEC")?;
                    return Ok(entries);
                }
                Tok::Keyword(k) => {
                    self.expect(Tok::LParen, "'('")?;
                    let params = self.params(1)?;
                    self.expect(Tok::Semi, "';' ending a header entry")?;
                    entries.push(HeaderEntry { entity: k, params });
                }
                t => return Err(self.unexpected(&t, "a header entry or ENDSEC")),
            }
        }
    }

    fn data(&mut self, file: &mut StepFile) -> Result<(), StepError> {
        loop {
            match self.lex.next()? {
                Tok::Keyword(k) if k == "ENDSEC" => {
                    return self.expect(Tok::Semi, "';' after ENDSEC");
                }
                Tok::InstanceName(id) => {
                    let line = self.lex.line;
                    self.expect(Tok::Eq, "'=' after the instance name")?;
                    let inst = match self.lex.next()? {
                        Tok::Keyword(entity) => {
                            self.expect(Tok::LParen, "'(' after the entity name")?;
                            let params = self.params(1)?;
                            Instance { id, entity, params }
                        }
                        Tok::LParen => {
                            let mut parts = Vec::new();
                            loop {
                                match self.lex.next()? {
                                    Tok::RParen if !parts.is_empty() => break,
                                    Tok::Keyword(name) => {
                                        self.expect(Tok::LParen, "'(' after the partial entity name")?;
                                        let ps = self.params(2)?;
                                        parts.push(Param::Typed(name, Box::new(Param::List(ps))));
                                    }
                                    t => return Err(self.unexpected(&t, "a partial entity of a complex instance")),
                                }
                            }
                            Instance { id, entity: String::new(), params: parts }
                        }
                        t => return Err(self.unexpected(&t, "an entity name")),
                    };
                    self.expect(Tok::Semi, "';' ending the instance")?;
                    if file.get(id).is_some() {
                        return Err(StepError::new(ErrorKind::DuplicateId, line, format!("duplicate instance #{id}")));
                    }
                    file.push_unchecked(inst);
                }
                t => return Err(self.unexpected(&t, "an instance or ENDSEC")),
            }
        }
    }

    /// Skips a section this reader does not interpret (ANCHOR, REFERENCE, SIGNATURE).
    fn skip_section(&mut self) -> Result<(), StepError> {
        loop {
            match self.lex.next()? {
                Tok::Keyword(k) if k == "ENDSEC" => return self.expect(Tok::Semi, "';' after ENDSEC"),
                Tok::Eof => return Err(self.unexpected(&Tok::Eof, "ENDSEC")),
                _ => {}
            }
        }
    }

    fn file(&mut self) -> Result<StepFile, StepError> {
        let header = self.header()?;
        let mut file = StepFile::new(header);
        loop {
            match self.lex.next()? {
                Tok::Keyword(k) if k == "DATA" => {
                    match self.lex.next()? {
                        Tok::Semi => {}
                        Tok::LParen => {
                            self.params(1)?;
                            self.expect(Tok::Semi, "';' after DATA(...)")?;
                        }
                        t => return Err(self.unexpected(&t, "';' after DATA")),
                    }
                    self.data(&mut file)?;
                }
                Tok::Keyword(k) if k == "ANCHOR" || k == "REFERENCE" || k == "SIGNATURE" => {
                    self.expect(Tok::Semi, "';'")?;
                    self.skip_section()?;
                }
                Tok::Keyword(k) if k == "END-ISO-10303-21" => {
                    self.expect(Tok::Semi, "';' after END-ISO-10303-21")?;
                    return Ok(file);
                }
                t => return Err(self.unexpected(&t, "DATA or END-ISO-10303-21")),
            }
        }
    }
}

impl StepFile {
    /// Reads a STEP physical file from a buffered reader, streaming over its bytes.
    pub fn read<R: BufRead>(r: R) -> Result<StepFile, StepError> {
        Parser { lex: Lexer::new(r) }.file()
    }

    /// Reads a STEP physical file from a string.
    pub fn parse_str(s: &str) -> Result<StepFile, StepError> {
        StepFile::read(s.as_bytes())
    }
}
