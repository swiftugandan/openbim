//! `bim-query`: Datalog over the semantic graph (spec/mcp.md "Query language (Datalog)").
//!
//! * [`parse_program`] / [`parse_atom`] read the language: rules `head(X, Y) :- b1(X, Z), b2(Z, Y).`,
//!   facts `name("a", 1).`, negation `not p(X)`; variables start upper-case (`_` alone is an
//!   anonymous variable), constants are double-quoted strings (escapes `\"`, `\\`, `\n`, `\t`,
//!   `\r`) or numbers (`-2`, `3.5`, `1e-3`). `%` and `//` start a comment that runs to the end of
//!   the line. A predicate without arguments is written without parentheses.
//! * [`check`] rejects programs that cannot run: a rule defining a built-in relation, a predicate
//!   used with two arities (or a built-in with the wrong one), an unsafe rule (a head variable, or a
//!   variable of a negated atom, that no positive body atom binds; an anonymous variable in a
//!   head) and an unstratifiable program (a predicate that depends negatively on itself through
//!   recursion: a negative edge inside a strongly connected component of the predicate dependency
//!   graph).
//! * [`run`] evaluates the program bottom-up, one stratum (strongly connected component, in
//!   dependency order) at a time, to a fixpoint: semi-naive ([`Mode::SemiNaive`], each round joins
//!   at least one recursive body atom against the facts new in the previous round) or naive
//!   ([`Mode::Naive`], every round re-evaluates every rule over all facts; the reference mode).
//!   Both give the same rows.
//! * The rows answer the query atom: one value per argument, in order (constants included), only
//!   the facts that match its constants and repeated variables; sorted ([`Val`]'s order) and
//!   deduplicated.
//! * Values are strings, numbers and booleans ([`Val`]); numbers compare by numeric value (`3`
//!   matches `3.0`; `-0` is `0`). Entities are strings: their `_name`.
//! * The built-in (extensional) relations ([`BUILTINS`]) come from an [`Edb`]; [`DocumentEdb`]
//!   derives them from a document and the engines, each only when the program or query uses it.

#![forbid(unsafe_code)]

mod edb;
mod eval;
mod syntax;
mod tributary;

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

pub use edb::DocumentEdb;
pub use eval::{check, run, Checked};
pub use syntax::{parse_atom, parse_program, Atom, Literal, Program, Rule, Term};

/// The built-in relations and their arities.
pub const BUILTINS: &[(&str, usize)] = &[
    ("entity", 2),
    ("prop", 3),
    ("ref", 3),
    ("room", 2),
    ("adjacent", 2),
    ("bounds", 2),
    ("bears_on", 2),
    ("feeds", 2),
];

/// The arity of built-in relation `name`, if it is one.
pub fn builtin_arity(name: &str) -> Option<usize> {
    BUILTINS.iter().find(|(n, _)| *n == name).map(|&(_, a)| a)
}

/// A Datalog value. Numbers are normalized on construction ([`Val::num`]: `-0.0` → `0.0`, every
/// NaN → one NaN), so equality and hashing are numeric equality. Order: booleans (false < true),
/// then numbers (ascending), then strings (by code point).
#[derive(Clone, Debug)]
pub enum Val {
    /// A boolean (from a Bool prop).
    Bool(bool),
    /// A number; build with [`Val::num`].
    Num(f64),
    /// A string (Text props, entity names, string constants).
    Str(Rc<str>),
}

impl Val {
    /// A number value, normalized.
    pub fn num(x: f64) -> Val {
        Val::Num(if x == 0.0 {
            0.0
        } else if x.is_nan() {
            f64::NAN
        } else {
            x
        })
    }

    /// A string value.
    pub fn str(s: &str) -> Val {
        Val::Str(Rc::from(s))
    }

    fn rank(&self) -> u8 {
        match self {
            Val::Bool(_) => 0,
            Val::Num(_) => 1,
            Val::Str(_) => 2,
        }
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Val) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Val {}

impl Hash for Val {
    fn hash<H: Hasher>(&self, h: &mut H) {
        match self {
            Val::Bool(b) => (0u8, *b).hash(h),
            Val::Num(x) => (1u8, x.to_bits()).hash(h),
            Val::Str(s) => (2u8, &**s).hash(h),
        }
    }
}

impl PartialOrd for Val {
    fn partial_cmp(&self, other: &Val) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Val {
    fn cmp(&self, other: &Val) -> Ordering {
        match (self, other) {
            (Val::Bool(a), Val::Bool(b)) => a.cmp(b),
            (Val::Num(a), Val::Num(b)) => a.total_cmp(b),
            (Val::Str(a), Val::Str(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Bool(b) => write!(f, "{b}"),
            Val::Num(x) => write!(f, "{x:?}"),
            Val::Str(s) => f.write_str(s),
        }
    }
}

/// A tuple of a relation, or a result row.
pub type Row = Vec<Val>;

/// How [`run`] reaches the fixpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Semi-naive bottom-up evaluation (the default).
    #[default]
    SemiNaive,
    /// Naive bottom-up iteration (the reference mode).
    Naive,
}

/// Why a query did not produce rows.
#[derive(Debug, Clone)]
pub enum QueryError {
    /// The program or query atom is not a valid program: a syntax error, an arity clash, an unsafe
    /// rule, a rule for a built-in relation, or an unstratifiable program.
    Program(String),
    /// An engine failed while deriving a built-in relation.
    Engine(bim_plugin::EngineError),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::Program(m) => f.write_str(m),
            QueryError::Engine(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for QueryError {}

/// The source of the built-in relations.
pub trait Edb {
    /// Every tuple of built-in relation `name` (one of [`BUILTINS`]), in any order, possibly with
    /// repeats.
    fn relation(&self, name: &str) -> Result<Vec<Row>, QueryError>;
}

/// No built-in facts at all (every built-in relation is empty).
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyEdb;

impl Edb for EmptyEdb {
    fn relation(&self, _name: &str) -> Result<Vec<Row>, QueryError> {
        Ok(Vec::new())
    }
}

/// Parses `program` and `query`, checks them and evaluates: the sorted, deduplicated rows.
pub fn query(program: &str, query: &str, edb: &dyn Edb, mode: Mode) -> Result<Vec<Row>, QueryError> {
    let program = parse_program(program)?;
    let atom = parse_atom(query)?;
    let checked = check(&program, &atom)?;
    run(&checked, edb, mode)
}

#[cfg(test)]
mod tests;
