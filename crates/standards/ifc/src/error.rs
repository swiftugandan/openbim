use std::fmt;
use std::io;

/// What went wrong while reading a STEP file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The underlying reader failed.
    Io,
    /// The input ended before the file was complete (inside a record, a section or a token).
    UnexpectedEof,
    /// A string literal is not closed before the end of the input.
    UnterminatedString,
    /// A comment is not closed before the end of the input.
    UnterminatedComment,
    /// A byte sequence that is not a Part 21 token.
    InvalidToken,
    /// A token that is not allowed at this point of the grammar (incl. unbalanced parentheses).
    UnexpectedToken,
    /// A malformed escape directive inside a string.
    InvalidEscape,
    /// A malformed binary literal.
    InvalidBinary,
    /// An integer or real that cannot be represented.
    InvalidNumber,
    /// Two instances with the same id.
    DuplicateId,
    /// Lists nested deeper than the reader accepts.
    TooDeep,
    /// A schema-level problem (unknown entity or attribute when building instances).
    Schema,
}

/// An error while reading (or building) a STEP file, with the 1-based line it was detected on
/// (0 when not applicable).
#[derive(Debug)]
pub struct StepError {
    pub kind: ErrorKind,
    pub line: usize,
    pub message: String,
    source: Option<io::Error>,
}

impl StepError {
    pub fn new(kind: ErrorKind, line: usize, message: impl Into<String>) -> StepError {
        StepError { kind, line, message: message.into(), source: None }
    }

    pub(crate) fn io(e: io::Error, line: usize) -> StepError {
        StepError { kind: ErrorKind::Io, line, message: e.to_string(), source: Some(e) }
    }
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "line {}: {}", self.line, self.message)
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for StepError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|e| e as &(dyn std::error::Error + 'static))
    }
}

impl From<io::Error> for StepError {
    fn from(e: io::Error) -> StepError {
        StepError::io(e, 0)
    }
}
