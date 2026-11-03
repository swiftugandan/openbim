//! `bim-bcf`: BCF 3.0 (BIM Collaboration Format) issue tracking.
//!
//! A `.bcfzip` is a ZIP archive holding `bcf.version`, an optional `extensions.xml` and one folder
//! per topic: `<topic guid>/markup.bcf` plus one `<viewpoint guid>.bcfv` per viewpoint. This crate
//! reads and writes that container with no dependency beyond a DEFLATE codec:
//!
//! * [`zip`]: the ZIP container (central-directory driven reader, deterministic writer);
//! * [`xml`]: a small XML reader (the subset BCF needs) and an indenting writer;
//! * [`model`]: [`Topic`], [`Comment`], [`Viewpoint`] and [`Camera`];
//! * [`markup`]: the XML shapes of `markup.bcf`, `*.bcfv`, `bcf.version` and `extensions.xml`
//!   (element order follows the buildingSMART XSDs);
//! * [`archive`]: [`read_bcfzip`] and [`write_bcfzip`] over whole archives;
//! * [`mirror`]: how a topic is mirrored into the document as an `Issue` entity and back;
//! * [`datetime`] and [`guid`]: the `xs:dateTime` and GUID helpers BCF needs.
//!
//! **Components** are identified by IFC GlobalIds derived exactly as the IFC4 export does
//! (`bim_ifc::export::entity_guid`: the 22-character form of the entity's 128-bit id), so a
//! viewpoint's `Component IfcGuid` and a document entity convert into each other without a table.
//!
//! **Reading is strict, writing is schema-valid.** Anything that is not a BCF 3.0 archive, or whose
//! topics are malformed, is a [`BcfError`]; nothing panics on hostile input. Every file written
//! validates against the BCF 3.0 XSDs. Text is stored and written exactly (never trimmed or
//! re-escaped), dates keep their lexical form (so an offset such as `+01:00` survives), and
//! GUIDs are written in the lowercase form the XSD requires.
//!
//! What a round trip keeps: for every topic its Guid, title, type, status, creation date and
//! author, description, labels and comments (Guid, date, author, text, viewpoint link), and for
//! every viewpoint its Guid, selected components and camera. Other topic data (priority, due date,
//! assignee, stage, modification stamps, related topics, document references, snippets), snapshots,
//! lines, clipping planes, bitmaps and the visibility and colouring of components are not kept.

#![forbid(unsafe_code)]

pub mod archive;
pub mod datetime;
pub mod guid;
pub mod markup;
pub mod mirror;
pub mod model;
pub mod xml;
pub mod zip;

use std::fmt;

pub use archive::{read_bcfzip, write_bcfzip};
pub use model::{Camera, Comment, Projection, Topic, Viewpoint};

/// Why a `.bcfzip` could not be read or written: a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BcfError(pub String);

impl BcfError {
    /// An error with this message.
    pub fn new(message: impl Into<String>) -> BcfError {
        BcfError(message.into())
    }
}

impl fmt::Display for BcfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BcfError {}
