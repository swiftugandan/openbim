//! `bim-draw`: 2D drawings of the compiled building (spec/cli.md, "Export, import, drawings").
//!
//! The drawing engine lives in [`bim_model::drawing`] (so that `bim_model::Session` can compute
//! drawings through `bim-incr` alongside the solids); this crate re-exports it unchanged —
//! [`View::parse`] reads a `--view` string, [`draw`] builds the [`Drawing`] of that view from the
//! document and its compiled [`bim_model::Model`], and [`svg::to_svg`] writes it as SVG — and adds
//! [`SvgQuery`], the memoized SVG rendering of an incremental session's view.

#![forbid(unsafe_code)]

use std::rc::Rc;

pub use bim_model::drawing::{draw, hatch_lines, inside, Drawing, Item, Layer, Shape, View, ViewError, ViewKind, P2, SCALE};

/// SVG output of a [`Drawing`].
pub mod svg {
    pub use bim_model::drawing::svg::to_svg;
}

/// The SVG of a registered view of a [`bim_model::Session`], as a `bim-incr` query over the
/// session's database: it re-renders only when the view's drawing
/// ([`bim_model::drawing::ViewQuery`]) changed.
pub struct SvgQuery;

impl bim_incr::Query for SvgQuery {
    type Key = String;
    type Value = Rc<String>;
    fn execute(db: &bim_incr::Db, view: &String) -> Rc<String> {
        match db.get::<bim_model::drawing::ViewQuery>(view).as_ref() {
            Some(drawing) => Rc::new(svg::to_svg(drawing)),
            None => Rc::new(String::new()),
        }
    }
}

/// The SVG of `view` drawn from the current state of `session`, memoized in the session's
/// database. `None` when the view string is invalid for the session's document.
pub fn session_svg(session: &bim_model::Session, view: &str) -> Option<String> {
    let db = session.db();
    db.get::<bim_model::drawing::ViewQuery>(&view.to_string()).as_ref().as_ref()?;
    Some(db.get::<SvgQuery>(&view.to_string()).as_ref().as_str().to_string())
}

#[cfg(test)]
mod tests;
