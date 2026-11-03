//! Lexer and parser of the Datalog language.

use crate::{QueryError, Val};

/// An argument of an atom.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    /// A named variable (starts upper-case, or with `_` followed by more characters).
    Var(String),
    /// The anonymous variable `_`: every occurrence is a fresh variable.
    Wildcard,
    /// A string or number constant.
    Const(Val),
}

/// `pred(arg, …)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Atom {
    /// The predicate name.
    pub pred: String,
    /// The arguments.
    pub args: Vec<Term>,
}

/// A body literal: an atom, possibly negated.
#[derive(Debug, Clone, PartialEq)]
pub struct Literal {
    /// `not atom`.
    pub negated: bool,
    /// The atom.
    pub atom: Atom,
}

/// `head :- body.` (a fact has an empty body).
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// The head atom.
    pub head: Atom,
    /// The body literals, in source order.
    pub body: Vec<Literal>,
    /// The 1-based source line of the head.
    pub line: usize,
}

/// A parsed program.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    /// Rules and facts, in source order.
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Var(String),
    Str(String),
    Num(f64),
    LParen,
    RParen,
    Comma,
    Dot,
    If,
    Not,
    Eof,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Var(s) => format!("variable `{s}`"),
            Tok::Str(s) => format!("string {s:?}"),
            Tok::Num(x) => format!("number {x}"),
            Tok::LParen => "`(`".into(),
            Tok::RParen => "`)`".into(),
            Tok::Comma => "`,`".into(),
            Tok::Dot => "`.`".into(),
            Tok::If => "`:-`".into(),
            Tok::Not => "`not`".into(),
            Tok::Eof => "end of input".into(),
        }
    }
}

fn err(what: &str, line: usize, col: usize, msg: impl std::fmt::Display) -> QueryError {
    QueryError::Program(format!("{what}:{line}:{col}: syntax error: {msg}"))
}

struct Cursor {
    chars: Vec<char>,
    i: usize,
    line: usize,
    col: usize,
}

impl Cursor {
    fn at(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).copied()
    }

    fn bump(&mut self, n: usize) {
        for _ in 0..n {
            if self.chars[self.i] == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.i += 1;
        }
    }
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn lex(text: &str, what: &str) -> Result<Vec<(Tok, usize, usize)>, QueryError> {
    let mut cur = Cursor { chars: text.chars().collect(), i: 0, line: 1, col: 1 };
    let mut out = Vec::new();
    while let Some(c) = cur.at(0) {
        let (l0, c0) = (cur.line, cur.col);
        if c.is_whitespace() {
            cur.bump(1);
            continue;
        }
        if c == '%' || (c == '/' && cur.at(1) == Some('/')) {
            while cur.at(0).is_some_and(|d| d != '\n') {
                cur.bump(1);
            }
            continue;
        }
        let single = match c {
            '(' => Some(Tok::LParen),
            ')' => Some(Tok::RParen),
            ',' => Some(Tok::Comma),
            '.' => Some(Tok::Dot),
            _ => None,
        };
        let tok = if let Some(t) = single {
            cur.bump(1);
            t
        } else if c == ':' && cur.at(1) == Some('-') {
            cur.bump(2);
            Tok::If
        } else if c == '"' {
            cur.bump(1);
            let mut s = String::new();
            loop {
                match cur.at(0) {
                    None | Some('\n') => return Err(err(what, l0, c0, "unterminated string")),
                    Some('"') => {
                        cur.bump(1);
                        break;
                    }
                    Some('\\') => {
                        let r = match cur.at(1) {
                            Some('"') => '"',
                            Some('\\') => '\\',
                            Some('n') => '\n',
                            Some('t') => '\t',
                            Some('r') => '\r',
                            _ => return Err(err(what, cur.line, cur.col, "unknown escape in string")),
                        };
                        s.push(r);
                        cur.bump(2);
                    }
                    Some(d) => {
                        s.push(d);
                        cur.bump(1);
                    }
                }
            }
            Tok::Str(s)
        } else if c.is_ascii_digit() || (c == '-' && cur.at(1).is_some_and(|d| d.is_ascii_digit())) {
            let digit = |k: usize| cur.at(k).is_some_and(|d| d.is_ascii_digit());
            let mut j = 1;
            while digit(j) {
                j += 1;
            }
            // A `.` is part of the number only when a digit follows (`3.5`, not the `3.` that
            // ends a clause).
            if cur.at(j) == Some('.') && digit(j + 1) {
                j += 1;
                while digit(j) {
                    j += 1;
                }
            }
            if matches!(cur.at(j), Some('e' | 'E')) {
                let mut k = j + 1;
                if matches!(cur.at(k), Some('+' | '-')) {
                    k += 1;
                }
                if digit(k) {
                    while digit(k) {
                        k += 1;
                    }
                    j = k;
                }
            }
            if cur.at(j).is_some_and(ident_char) {
                return Err(err(what, l0, c0, "malformed number"));
            }
            let text: String = cur.chars[cur.i..cur.i + j].iter().collect();
            let x: f64 = text.parse().map_err(|_| err(what, l0, c0, format!("malformed number `{text}`")))?;
            if !x.is_finite() {
                return Err(err(what, l0, c0, format!("number `{text}` is out of range")));
            }
            cur.bump(j);
            Tok::Num(x)
        } else if c.is_alphabetic() || c == '_' {
            let mut j = 1;
            while cur.at(j).is_some_and(ident_char) {
                j += 1;
            }
            let word: String = cur.chars[cur.i..cur.i + j].iter().collect();
            cur.bump(j);
            if c.is_uppercase() || c == '_' {
                Tok::Var(word)
            } else if word == "not" {
                Tok::Not
            } else if c.is_lowercase() {
                Tok::Ident(word)
            } else {
                return Err(err(what, l0, c0, format!("`{word}`: a name starts with a lower-case letter, a variable with an upper-case one")));
            }
        } else {
            return Err(err(what, l0, c0, format!("unexpected character `{c}`")));
        };
        out.push((tok, l0, c0));
    }
    out.push((Tok::Eof, cur.line, cur.col));
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<(Tok, usize, usize)>,
    pos: usize,
    what: &'a str,
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }

    fn line(&self) -> usize {
        self.toks[self.pos].1
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn fail<T>(&self, expected: &str) -> Result<T, QueryError> {
        let (tok, line, col) = &self.toks[self.pos];
        Err(err(self.what, *line, *col, format!("expected {expected}, found {}", tok.describe())))
    }

    fn expect(&mut self, tok: Tok, expected: &str) -> Result<(), QueryError> {
        if *self.peek() == tok {
            self.next();
            Ok(())
        } else {
            self.fail(expected)
        }
    }

    fn atom(&mut self) -> Result<Atom, QueryError> {
        let pred = match self.peek() {
            Tok::Ident(name) => name.clone(),
            _ => return self.fail("a predicate name"),
        };
        self.next();
        let mut args = Vec::new();
        if *self.peek() == Tok::LParen {
            self.next();
            if *self.peek() != Tok::RParen {
                loop {
                    args.push(self.term()?);
                    match self.peek() {
                        Tok::Comma => {
                            self.next();
                        }
                        Tok::RParen => break,
                        _ => return self.fail("`,` or `)`"),
                    }
                }
            }
            self.expect(Tok::RParen, "`)`")?;
        }
        Ok(Atom { pred, args })
    }

    fn term(&mut self) -> Result<Term, QueryError> {
        let t = match self.peek() {
            Tok::Var(v) if v == "_" => Term::Wildcard,
            Tok::Var(v) => Term::Var(v.clone()),
            Tok::Str(s) => Term::Const(Val::str(s)),
            Tok::Num(x) => Term::Const(Val::num(*x)),
            _ => return self.fail("a variable or a constant"),
        };
        self.next();
        Ok(t)
    }

    fn literal(&mut self) -> Result<Literal, QueryError> {
        let negated = *self.peek() == Tok::Not;
        if negated {
            self.next();
        }
        Ok(Literal { negated, atom: self.atom()? })
    }

    fn rule(&mut self) -> Result<Rule, QueryError> {
        let line = self.line();
        let head = self.atom()?;
        let mut body = Vec::new();
        if *self.peek() == Tok::If {
            self.next();
            loop {
                body.push(self.literal()?);
                match self.peek() {
                    Tok::Comma => {
                        self.next();
                    }
                    Tok::Dot => break,
                    _ => return self.fail("`,` or `.`"),
                }
            }
        }
        self.expect(Tok::Dot, if body.is_empty() { "`:-` or `.`" } else { "`.`" })?;
        Ok(Rule { head, body, line })
    }
}

/// Parses a program (rules and facts).
pub fn parse_program(text: &str) -> Result<Program, QueryError> {
    let mut p = Parser { toks: lex(text, "program")?, pos: 0, what: "program" };
    let mut rules = Vec::new();
    while *p.peek() != Tok::Eof {
        rules.push(p.rule()?);
    }
    Ok(Program { rules })
}

/// Parses a query atom, e.g. `path("a", X)`; a trailing `.` is allowed.
pub fn parse_atom(text: &str) -> Result<Atom, QueryError> {
    let mut p = Parser { toks: lex(text, "query")?, pos: 0, what: "query" };
    let atom = p.atom()?;
    if *p.peek() == Tok::Dot {
        p.next();
    }
    if *p.peek() != Tok::Eof {
        return p.fail("end of the query atom");
    }
    Ok(atom)
}
