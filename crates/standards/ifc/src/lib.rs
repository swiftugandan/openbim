//! ISO 10303-21 ("STEP physical file", Part 21) reading and writing, plus typed access to the
//! IFC4 / IFC2X3 entities used by the IFC import/export.
//!
//! * [`StepFile::read`] is a streaming lexer/parser over any [`std::io::BufRead`]: header
//!   section, one or more `DATA` sections, simple instances `#1=IFCWALL(...);` and complex
//!   instances `#1=(A(...)B(...));`, every Part 21 value form (`$`, `*`, integers, reals,
//!   strings with all escape directives, binaries, enumerations, references, nested lists and
//!   typed values), comments and arbitrary whitespace between tokens. Malformed input (truncated
//!   records, missing `ENDSEC;` / `END-ISO-10303-21;`, unterminated strings or comments,
//!   unbalanced parentheses, duplicate instance ids, ...) is reported as a [`StepError`]; the
//!   reader never panics.
//! * [`StepFile::write`] emits a conforming file that keeps every instance id, the header
//!   entries and every value bit-exactly (reals in shortest round-trip form).
//! * [`schema`] holds attribute tables (names in order, supertypes) for the IFC4 and IFC2X3
//!   entities the import/export needs; [`Entity`] is a typed view over an instance with
//!   by-name attribute access, and [`StepFile::add_entity`] builds instances by attribute name.
//! * [`guid`] converts IFC GlobalIds (22-character base-64) to and from 128-bit values.

mod error;
pub mod export;
pub mod guid;
mod model;
mod reader;
pub mod schema;
mod strings;
mod typed;
mod writer;

pub use error::{ErrorKind, StepError};
pub use model::{HeaderEntry, Instance, Param, StepFile};
pub use schema::{EntityDef, Schema};
pub use strings::{decode_string, encode_string};
pub use typed::Entity;
pub use writer::format_real;

#[cfg(test)]
mod tests;
