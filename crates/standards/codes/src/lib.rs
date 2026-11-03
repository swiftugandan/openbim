//! `bim-codes`: declarative code-rule packs as [`StandardRuleSet`]s (spec/plugins.md "Declarative
//! packs").
//!
//! A pack is a JSON file of rules (see [`Pack`]); each rule says which entities it is about
//! (`applies_to` and `where`), what to measure (`check`, one of the closed vocabulary [`Check`])
//! and the `limit`. A [`PackRuleSet`] evaluates one pack against the document of an
//! [`EngineContext`] and returns one [`Violation`] per entity that breaks a rule:
//!
//! | `check` | measured quantity | violation when |
//! |---|---|---|
//! | `min_width` | `Space`: shorter side of the minimum-area bounding rectangle of its room polygon; `Opening`: `width` | measured < limit |
//! | `min_area` | `Space`: room area | measured < limit |
//! | `min_clear_height` | `Space`: floor to underside of the slab above | measured < limit |
//! | `riser_max` / `riser_min` | `Stair`: (top − base elevation) / `riser_count` | > / < limit |
//! | `tread_min` | `Stair`: `tread` | measured < limit |
//! | `blondel` | `Stair`: 2 · riser + tread; `limit` is `[lo, hi]` | outside `[lo, hi]` (the reported limit is the bound crossed) |
//! | `u_value_max` | `Wall`: 1 / (0.13 + Σ tᵢ/λᵢ + 0.04) over its WallType's layers | measured > limit |
//! | `slope_max` | `Ramp`: (top − base elevation) / `length` | measured > limit |
//! | `egress_distance_max` | `Space`: longest shortest path inside the room polygon from a vertex to the nearest door | measured > limit |
//!
//! Comparisons are strict and exact. A quantity that cannot be measured (missing or invalid props,
//! a Space in no room, a room with no door) is no violation; props are effective props (an entity
//! takes the props it lacks from its `product`); a Space's room is the `arch` trade's room
//! (`bim_arch`) on the Space's level that contains its `x y`. The private `eval` module documents
//! each check's definition and the matching rules of `where` in full.
//!
//! # Packs are data
//!
//! A pack is read when a command runs, never compiled in: the rules of a pack file given by path
//! are parsed by [`PackRuleSet::from_path`] on every run, and the three shipped packs
//! ([`SHIPPED`], `packs/ibc.json`, `packs/eurocode.json`, `packs/accessibility.json`) are re-read
//! from this crate's `packs/` directory by every [`try_check`](StandardRuleSet::try_check), so
//! editing a pack file changes the next result with no rebuild. A copy of each shipped pack is
//! embedded in the library as a fallback for a binary that runs where the source tree is not. An
//! unreadable path, malformed JSON, an unknown `check`, a known `check` on a kind it does not
//! measure, or a limit of the wrong shape is [`EngineError::Input`] (exit 2 in `bim`), reported
//! before any result.

#![forbid(unsafe_code)]

mod eval;
mod geom;
mod pack;
mod rooms;
#[cfg(test)]
mod testdoc;

use std::path::Path;

use bim_plugin::{EngineContext, EngineError, StandardRuleSet, Violation};

pub use pack::{Check, Limit, Pack, Rule};

/// The ids of the shipped packs, in this order.
pub const SHIPPED: [&str; 3] = ["ibc", "eurocode", "accessibility"];

/// Per shipped pack: its id, the path of its file in this crate's source tree (re-read on every
/// run) and a copy embedded at build time (used when that file cannot be read).
pub(crate) const SHIPPED_PACKS: [(&str, &str, &str); 3] = [
    ("ibc", concat!(env!("CARGO_MANIFEST_DIR"), "/packs/ibc.json"), include_str!("../packs/ibc.json")),
    ("eurocode", concat!(env!("CARGO_MANIFEST_DIR"), "/packs/eurocode.json"), include_str!("../packs/eurocode.json")),
    (
        "accessibility",
        concat!(env!("CARGO_MANIFEST_DIR"), "/packs/accessibility.json"),
        include_str!("../packs/accessibility.json"),
    ),
];

/// A declarative pack as a rule set.
pub struct PackRuleSet {
    id: String,
    source: Source,
}

enum Source {
    /// A shipped pack: read again from `path` by every `try_check`, falling back to `embedded`
    /// when the file cannot be read.
    Shipped { path: &'static str, embedded: &'static str },
    /// A pack parsed and validated when the rule set was made.
    Parsed(Pack),
}

impl PackRuleSet {
    /// The shipped pack `id` (one of [`SHIPPED`]); `None` for any other id. Nothing is read until
    /// [`try_check`](StandardRuleSet::try_check).
    pub fn shipped(id: &str) -> Option<PackRuleSet> {
        let &(id, path, embedded) = SHIPPED_PACKS.iter().find(|(pack, _, _)| *pack == id)?;
        Some(PackRuleSet { id: id.to_string(), source: Source::Shipped { path, embedded } })
    }

    /// The pack in the file at `path`, read and validated now. An unreadable file, text that is
    /// not JSON, and an invalid pack are [`EngineError::Input`], the message naming the path.
    pub fn from_path(path: &Path) -> Result<PackRuleSet, EngineError> {
        let origin = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|e| EngineError::Input(format!("{origin}: {e}")))?;
        PackRuleSet::from_bytes(&bytes, &origin)
    }

    /// Like [`from_path`](PackRuleSet::from_path), from the file's bytes; `origin` names it in
    /// messages. Text that is not UTF-8 is [`EngineError::Input`].
    pub fn from_bytes(bytes: &[u8], origin: &str) -> Result<PackRuleSet, EngineError> {
        let text = std::str::from_utf8(bytes).map_err(|e| EngineError::Input(format!("{origin}: not UTF-8 text: {e}")))?;
        let pack = Pack::parse(text).map_err(|e| EngineError::Input(format!("{origin}: {e}")))?;
        Ok(PackRuleSet { id: pack.pack.clone(), source: Source::Parsed(pack) })
    }

    /// The pack in `text`, validated now; an invalid pack is [`EngineError::Input`].
    pub fn from_json(text: &str) -> Result<PackRuleSet, EngineError> {
        PackRuleSet::from_bytes(text.as_bytes(), "pack")
    }
}

impl StandardRuleSet for PackRuleSet {
    /// The shipped pack's id, else the `pack` of the file.
    fn id(&self) -> &str {
        &self.id
    }

    /// Like [`try_check`](StandardRuleSet::try_check), with a pack that cannot be loaded giving no
    /// violations; front ends call `try_check`.
    fn check(&self, ctx: &EngineContext<'_>) -> Vec<Violation> {
        self.try_check(ctx).unwrap_or_default()
    }

    /// Every violation of the pack, rules in pack order. A shipped pack is read from its file
    /// first (the embedded copy only if the file cannot be read), so an edit takes effect at once
    /// and a malformed edit is [`EngineError::Input`].
    fn try_check(&self, ctx: &EngineContext<'_>) -> Result<Vec<Violation>, EngineError> {
        match &self.source {
            Source::Parsed(pack) => Ok(eval::run(pack, ctx)),
            Source::Shipped { path, embedded } => {
                let on_disk = std::fs::read_to_string(path);
                let pack = Pack::parse(on_disk.as_deref().unwrap_or(embedded))
                    .map_err(|e| EngineError::Input(format!("{path}: {e}")))?;
                if pack.pack != self.id {
                    return Err(EngineError::Input(format!("{path}: `pack` is `{}`, expected `{}`", pack.pack, self.id)));
                }
                Ok(eval::run(&pack, ctx))
            }
        }
    }
}

/// Every shipped pack as a rule set, in the order of [`SHIPPED`]: what a front end registers.
pub fn shipped() -> Vec<Box<dyn StandardRuleSet>> {
    SHIPPED.iter().filter_map(|id| PackRuleSet::shipped(id)).map(|r| Box::new(r) as Box<dyn StandardRuleSet>).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdoc::{base_doc, create, num, reference, text};
    use bim_doc::Document;

    fn doc_with_narrow_door() -> Document {
        let mut doc = base_doc();
        create(&mut doc, "w0", "Wall", vec![]);
        create(&mut doc, "d", "Opening", vec![("host", reference("w0")), ("kind", text("door")), ("width", num(0.7))]);
        create(&mut doc, "win", "Opening", vec![("host", reference("w0")), ("kind", text("window")), ("width", num(0.5))]);
        doc
    }

    #[test]
    fn the_shipped_rule_sets_are_registered_by_id() {
        let all = shipped();
        assert_eq!(all.iter().map(|r| r.id()).collect::<Vec<_>>(), SHIPPED);
        assert!(PackRuleSet::shipped("ibc").is_some());
        assert!(PackRuleSet::shipped("IBC").is_none());
        assert!(PackRuleSet::shipped("other").is_none());
        assert_eq!(SHIPPED_PACKS.map(|p| p.0), SHIPPED);
    }

    #[test]
    fn shipped_packs_check_a_document() {
        let doc = doc_with_narrow_door();
        let ctx = EngineContext::new(&doc);
        // the door is under 0.813 and 0.85; the narrow window is no door
        let ibc = PackRuleSet::shipped("ibc").unwrap().try_check(&ctx).unwrap();
        let found: Vec<_> = ibc.iter().map(|v| (v.rule.as_str(), v.entity.as_str(), v.measured, v.limit)).collect();
        assert_eq!(found, [("IBC-1010.1.1", "d", 0.7, 0.813)]);
        let acc = PackRuleSet::shipped("accessibility").unwrap().check(&ctx);
        assert_eq!(acc.iter().map(|v| (v.rule.as_str(), v.entity.as_str())).collect::<Vec<_>>(), [("ACC-door-clear-width", "d")]);
        assert!(PackRuleSet::shipped("eurocode").unwrap().try_check(&ctx).unwrap().is_empty());
    }

    #[test]
    fn a_pack_from_text_is_validated_at_once_and_named_by_its_pack() {
        let set = PackRuleSet::from_json(
            r#"{"pack": "mine", "rules": [{"id": "R", "applies_to": "Opening", "check": "min_width", "limit": 0.6}]}"#,
        )
        .unwrap();
        assert_eq!(set.id(), "mine");
        let doc = doc_with_narrow_door();
        let found = set.try_check(&EngineContext::new(&doc)).unwrap();
        assert_eq!(found.iter().map(|v| v.entity.as_str()).collect::<Vec<_>>(), ["win"]);
        for bad in [
            r#"{"pack": "broken", "rules": ["#,
            r#"{"pack": "p", "rules": [{"id": "R", "applies_to": "Space", "check": "max_banana", "limit": 1}]}"#,
            "null",
        ] {
            let err = PackRuleSet::from_json(bad).err().expect(bad);
            assert!(matches!(err, EngineError::Input(_)), "{bad}: {err:?}");
            assert_eq!(err.exit_code(), 2);
        }
    }

    #[test]
    fn a_pack_file_is_read_each_time_it_is_loaded() {
        let dir = std::env::temp_dir().join(format!("bim-codes-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.json");
        let doc = doc_with_narrow_door();
        let ctx = EngineContext::new(&doc);
        let rule = |limit: f64| {
            format!(
                r#"{{"pack": "p", "rules": [{{"id": "R", "applies_to": "Opening", "where": {{"kind": "door"}}, "check": "min_width", "limit": {limit}}}]}}"#
            )
        };
        std::fs::write(&path, rule(0.8)).unwrap();
        assert_eq!(PackRuleSet::from_path(&path).unwrap().try_check(&ctx).unwrap().len(), 1);
        std::fs::write(&path, rule(0.6)).unwrap();
        assert!(PackRuleSet::from_path(&path).unwrap().try_check(&ctx).unwrap().is_empty());
        std::fs::write(&path, r#"{"pack": "p", "rules": ["#).unwrap();
        let err = PackRuleSet::from_path(&path).err().unwrap();
        assert!(matches!(&err, EngineError::Input(m) if m.contains("p.json")), "{err:?}");
        let err = PackRuleSet::from_path(&dir.join("missing.json")).err().unwrap();
        assert!(matches!(&err, EngineError::Input(m) if m.contains("missing.json")), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shipped_pack_is_reread_from_its_file_and_falls_back_to_the_embedded_copy() {
        let doc = doc_with_narrow_door();
        let ctx = EngineContext::new(&doc);
        let embedded = include_str!("../packs/ibc.json");
        let dir = std::env::temp_dir().join(format!("bim-codes-shipped-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path: &'static str = Box::leak(dir.join("ibc.json").display().to_string().into_boxed_str());
        let set = PackRuleSet { id: "ibc".to_string(), source: Source::Shipped { path, embedded } };
        // no file: the embedded copy
        assert_eq!(set.try_check(&ctx).unwrap().len(), 1);
        // an edited file wins, with no rebuild or reload of the rule set
        std::fs::write(path, embedded.replace("0.813", "0.6")).unwrap();
        assert!(set.try_check(&ctx).unwrap().is_empty());
        // a malformed edit is invalid input, and so is a file for another pack
        std::fs::write(path, "{").unwrap();
        assert!(matches!(set.try_check(&ctx), Err(EngineError::Input(m)) if m.contains("ibc.json")));
        assert!(set.check(&ctx).is_empty());
        std::fs::write(path, embedded.replace(r#""pack": "ibc""#, r#""pack": "other""#)).unwrap();
        assert!(matches!(set.try_check(&ctx), Err(EngineError::Input(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
