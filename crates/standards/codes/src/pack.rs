//! The declarative pack format (spec/plugins.md "Declarative packs"): parsing and validation.
//!
//! ```json
//! {"pack": "ibc", "version": "2021", "rules": [
//!   {"id": "IBC-1020.2", "applies_to": "Space", "where": {"use": "corridor"}, "check": "min_width", "limit": 1.118},
//!   {"id": "IBC-1011.5.2-riser", "applies_to": "Stair", "check": "riser_max", "limit": 0.1778}
//! ]}
//! ```
//!
//! A pack is valid when
//!
//! * it is one JSON object with a non-empty string `pack`, a `rules` array and, if present, a string
//!   `version` (other keys are ignored);
//! * every rule is an object with a non-empty string `id` that no earlier rule has, a `check` of the
//!   closed vocabulary ([`Check`]), an `applies_to` kind the check measures (the table on
//!   [`Check::applies_to`]; a known check on any other kind is as malformed as an unknown check), a
//!   `limit` (one finite number; for `blondel` a pair `[lo, hi]` with `lo <= hi`) and, optionally, a
//!   `where` object.
//!
//! [`Pack::parse`] returns the first violation of these as its error text; front ends turn it into an
//! invalid-input failure (exit 2).

use std::collections::HashSet;

use serde_json::Value as Json;

/// The closed vocabulary of checks (spec/plugins.md); what each measures and when it is violated is
/// documented on the evaluator (`eval`) and in the crate docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    MinWidth,
    MinArea,
    MinClearHeight,
    RiserMax,
    RiserMin,
    TreadMin,
    Blondel,
    UValueMax,
    SlopeMax,
    EgressDistanceMax,
}

impl Check {
    /// Every check, in the order of the table in spec/plugins.md.
    pub const ALL: [Check; 10] = [
        Check::MinWidth,
        Check::MinArea,
        Check::MinClearHeight,
        Check::RiserMax,
        Check::RiserMin,
        Check::TreadMin,
        Check::Blondel,
        Check::UValueMax,
        Check::SlopeMax,
        Check::EgressDistanceMax,
    ];

    /// The check named `name` in a pack file (`"min_width"`, …), if it is one of the vocabulary.
    pub fn from_name(name: &str) -> Option<Check> {
        Check::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The name of the check in a pack file.
    pub fn name(self) -> &'static str {
        match self {
            Check::MinWidth => "min_width",
            Check::MinArea => "min_area",
            Check::MinClearHeight => "min_clear_height",
            Check::RiserMax => "riser_max",
            Check::RiserMin => "riser_min",
            Check::TreadMin => "tread_min",
            Check::Blondel => "blondel",
            Check::UValueMax => "u_value_max",
            Check::SlopeMax => "slope_max",
            Check::EgressDistanceMax => "egress_distance_max",
        }
    }

    /// The entity kinds the check measures (a rule's `applies_to` must be one of them): `Space` and
    /// `Opening` for `min_width`; `Space` for `min_area`, `min_clear_height` and
    /// `egress_distance_max`; `Stair` for `riser_max`, `riser_min`, `tread_min` and `blondel`;
    /// `Wall` for `u_value_max`; `Ramp` for `slope_max`.
    pub fn applies_to(self) -> &'static [&'static str] {
        match self {
            Check::MinWidth => &["Space", "Opening"],
            Check::MinArea | Check::MinClearHeight | Check::EgressDistanceMax => &["Space"],
            Check::RiserMax | Check::RiserMin | Check::TreadMin | Check::Blondel => &["Stair"],
            Check::UValueMax => &["Wall"],
            Check::SlopeMax => &["Ramp"],
        }
    }
}

/// A rule's `limit`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Limit {
    /// One bound (every check but `blondel`).
    Value(f64),
    /// `[lo, hi]` (`blondel`), `lo <= hi`.
    Range(f64, f64),
}

/// One rule of a pack.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// The rule's id, reported as `Violation.rule`; unique in its pack.
    pub id: String,
    /// The kind of entity the rule is about (one of [`Check::applies_to`] of its check).
    pub applies_to: &'static str,
    /// The `where` entries in file order: the rule applies to an entity when each listed prop equals
    /// the given JSON value. Empty when the rule has no `where`.
    pub where_: Vec<(String, Json)>,
    /// What to measure.
    pub check: Check,
    /// The bound(s) of the measured quantity.
    pub limit: Limit,
}

/// A parsed, validated pack.
#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    /// The pack's id (`"ibc"`, …); the id of the rule set a pack file becomes.
    pub pack: String,
    /// The pack's `version` text, if it has one.
    pub version: Option<String>,
    /// The rules, in file order.
    pub rules: Vec<Rule>,
}

impl Pack {
    /// Parses and validates a pack file's text (a leading UTF-8 byte-order mark is skipped). The
    /// error text says what is wrong and, for a rule, which one.
    pub fn parse(text: &str) -> Result<Pack, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let json: Json = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        let object = json.as_object().ok_or("a pack must be a JSON object")?;
        let pack = match object.get("pack") {
            Some(Json::String(id)) if !id.is_empty() => id.clone(),
            _ => return Err("`pack` must be a non-empty string".to_string()),
        };
        let version = match object.get("version") {
            None => None,
            Some(Json::String(v)) => Some(v.clone()),
            Some(_) => return Err("`version` must be a string".to_string()),
        };
        let Some(Json::Array(items)) = object.get("rules") else {
            return Err("`rules` must be an array".to_string());
        };
        let mut rules = Vec::with_capacity(items.len());
        let mut seen = HashSet::new();
        for (index, item) in items.iter().enumerate() {
            let rule = parse_rule(index, item)?;
            if !seen.insert(rule.id.clone()) {
                return Err(format!("rule `{}` appears twice (rule ids must be unique)", rule.id));
            }
            rules.push(rule);
        }
        Ok(Pack { pack, version, rules })
    }
}

fn parse_rule(index: usize, json: &Json) -> Result<Rule, String> {
    let object = json.as_object().ok_or_else(|| format!("rule #{} is not an object", index + 1))?;
    let id = match object.get("id") {
        Some(Json::String(id)) if !id.is_empty() => id.clone(),
        _ => return Err(format!("rule #{}: `id` must be a non-empty string", index + 1)),
    };
    let at = format!("rule `{id}`");
    let check_name = match object.get("check") {
        Some(Json::String(name)) => name.as_str(),
        _ => return Err(format!("{at}: `check` must be a string")),
    };
    let check = Check::from_name(check_name).ok_or_else(|| {
        let known: Vec<&str> = Check::ALL.iter().map(|c| c.name()).collect();
        format!("{at}: unknown check `{check_name}` (the checks are {})", known.join(", "))
    })?;
    let kind = match object.get("applies_to") {
        Some(Json::String(kind)) => kind.as_str(),
        _ => return Err(format!("{at}: `applies_to` must be a string")),
    };
    let applies_to = check.applies_to().iter().copied().find(|k| *k == kind).ok_or_else(|| {
        format!("{at}: `{check_name}` measures {}, not `{kind}`", check.applies_to().join(" or "))
    })?;
    let where_ = match object.get("where") {
        None => Vec::new(),
        Some(Json::Object(entries)) => entries.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Some(_) => return Err(format!("{at}: `where` must be an object")),
    };
    let limit = parse_limit(check, object.get("limit")).map_err(|e| format!("{at}: {e}"))?;
    Ok(Rule { id, applies_to, where_, check, limit })
}

fn parse_limit(check: Check, json: Option<&Json>) -> Result<Limit, String> {
    let finite = |j: &Json| j.as_f64().filter(|x| x.is_finite());
    if check == Check::Blondel {
        let bounds = match json {
            Some(Json::Array(items)) if items.len() == 2 => finite(&items[0]).zip(finite(&items[1])),
            _ => None,
        };
        return match bounds {
            Some((lo, hi)) if lo <= hi => Ok(Limit::Range(lo, hi)),
            _ => Err("`limit` of `blondel` must be [lo, hi]: two finite numbers with lo <= hi".to_string()),
        };
    }
    match json.and_then(finite) {
        Some(x) => Ok(Limit::Value(x)),
        None => Err(format!("`limit` of `{}` must be a finite number", check.name())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The (applies_to, check, limit) triples spec/plugins.md lists per shipped pack.
    fn spec_rules(pack: &str) -> Vec<(&'static str, Check, Limit)> {
        match pack {
            "ibc" => vec![
                ("Space", Check::MinWidth, Limit::Value(1.118)),
                ("Opening", Check::MinWidth, Limit::Value(0.813)),
                ("Stair", Check::RiserMax, Limit::Value(0.1778)),
                ("Stair", Check::TreadMin, Limit::Value(0.2794)),
            ],
            "eurocode" => {
                vec![("Stair", Check::Blondel, Limit::Range(0.60, 0.65)), ("Wall", Check::UValueMax, Limit::Value(0.35))]
            }
            "accessibility" => vec![
                ("Opening", Check::MinWidth, Limit::Value(0.85)),
                ("Space", Check::MinWidth, Limit::Value(1.5)),
                ("Ramp", Check::SlopeMax, Limit::Value(1.0 / 12.0)),
            ],
            other => panic!("no spec rules for {other}"),
        }
    }

    #[test]
    fn the_shipped_packs_are_the_spec_rules() {
        for (id, path, embedded) in crate::SHIPPED_PACKS {
            let pack = Pack::parse(embedded).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(pack.pack, id);
            let got: Vec<_> = pack.rules.iter().map(|r| (r.applies_to, r.check, r.limit)).collect();
            assert_eq!(got, spec_rules(id), "{id}");
            // The file the shipped rule set re-reads on every run is the embedded fallback.
            let on_disk = std::fs::read_to_string(path).unwrap();
            assert_eq!(Pack::parse(&on_disk).unwrap(), pack, "{id}");
        }
        // The ramp limit is exactly 1:12 and the door/corridor rules are filtered.
        let acc = Pack::parse(crate::SHIPPED_PACKS[2].2).unwrap();
        assert_eq!(acc.rules[2].limit, Limit::Value(1.0 / 12.0));
        assert_eq!(acc.rules[0].where_, vec![("kind".to_string(), Json::from("door"))]);
        assert_eq!(acc.rules[1].where_, vec![("use".to_string(), Json::from("corridor"))]);
    }

    #[test]
    fn the_vocabulary_is_closed_and_every_check_has_a_kind() {
        let names: Vec<&str> = Check::ALL.iter().map(|c| c.name()).collect();
        let spec = ["min_width", "min_area", "min_clear_height", "riser_max", "riser_min", "tread_min"];
        assert_eq!(names[..6], spec);
        assert_eq!(names[6..], ["blondel", "u_value_max", "slope_max", "egress_distance_max"]);
        for check in Check::ALL {
            assert_eq!(Check::from_name(check.name()), Some(check));
            assert!(!check.applies_to().is_empty());
        }
        assert_eq!(Check::from_name("max_banana"), None);
        assert_eq!(Check::from_name("MIN_WIDTH"), None);
        assert_eq!(Check::from_name(""), None);
    }

    fn pack_with(rule: &str) -> String {
        format!(r#"{{"pack": "p", "rules": [{rule}]}}"#)
    }

    #[test]
    fn a_minimal_pack_parses_and_unknown_keys_are_ignored() {
        let pack = Pack::parse(r#"{"pack": "p", "rules": [], "note": [1, 2]}"#).unwrap();
        assert_eq!((pack.pack.as_str(), pack.version, pack.rules.len()), ("p", None, 0));
        let pack = Pack::parse("\u{feff}{\"pack\": \"p\", \"version\": \"3\", \"rules\": []}").unwrap();
        assert_eq!(pack.version.as_deref(), Some("3"));
        let rule = r#"{"id": "r", "applies_to": "Wall", "check": "u_value_max", "limit": 1, "clause": "x", "where": {"a": 1, "b": [true]}}"#;
        let pack = Pack::parse(&pack_with(rule)).unwrap();
        let r = &pack.rules[0];
        assert_eq!((r.id.as_str(), r.applies_to, r.check, r.limit), ("r", "Wall", Check::UValueMax, Limit::Value(1.0)));
        assert_eq!(r.where_.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn malformed_packs_are_errors() {
        let ok = r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": 0.18}"#;
        assert!(Pack::parse(&pack_with(ok)).is_ok());
        let bad = [
            r#"{"pack": "broken", "rules": ["#.to_string(),
            "".to_string(),
            "[]".to_string(),
            "42".to_string(),
            r#"{"rules": []}"#.to_string(),
            r#"{"pack": "", "rules": []}"#.to_string(),
            r#"{"pack": 7, "rules": []}"#.to_string(),
            r#"{"pack": "p"}"#.to_string(),
            r#"{"pack": "p", "rules": {}}"#.to_string(),
            r#"{"pack": "p", "version": 3, "rules": []}"#.to_string(),
            pack_with("7"),
            pack_with(r#"{"applies_to": "Stair", "check": "riser_max", "limit": 0.18}"#),
            pack_with(r#"{"id": "", "applies_to": "Stair", "check": "riser_max", "limit": 0.18}"#),
            pack_with(r#"{"id": "r", "check": "riser_max", "limit": 0.18}"#),
            pack_with(r#"{"id": "r", "applies_to": 3, "check": "riser_max", "limit": 0.18}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "limit": 0.18}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": 3, "limit": 0.18}"#),
            // an unknown check, spelled several ways
            pack_with(r#"{"id": "r", "applies_to": "Space", "check": "max_banana", "limit": 1.0}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "Riser_Max", "limit": 0.18}"#),
            // a known check on a kind it does not measure
            pack_with(r#"{"id": "r", "applies_to": "Wall", "check": "riser_max", "limit": 0.18}"#),
            pack_with(r#"{"id": "r", "applies_to": "Walls", "check": "u_value_max", "limit": 0.35}"#),
            pack_with(r#"{"id": "r", "applies_to": "Opening", "check": "min_area", "limit": 1}"#),
            // limits
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max"}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": "0.18"}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": true}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": null}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": [0.1, 0.2]}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": 1e999}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "blondel", "limit": 0.62}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "blondel", "limit": [0.6]}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "blondel", "limit": [0.6, 0.62, 0.65]}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "blondel", "limit": [0.65, 0.6]}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "blondel", "limit": [0.6, "0.65"]}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "blondel", "limit": [0.6, null]}"#),
            // where
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": 0.18, "where": "x"}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": 0.18, "where": null}"#),
            pack_with(r#"{"id": "r", "applies_to": "Stair", "check": "riser_max", "limit": 0.18, "where": [["a", 1]]}"#),
            // duplicate rule ids
            pack_with(&format!("{ok}, {ok}")),
        ];
        for text in &bad {
            let err = Pack::parse(text).expect_err(text);
            assert!(!err.is_empty(), "{text}");
        }
    }

    #[test]
    fn errors_name_the_rule_and_the_problem() {
        let e = Pack::parse(&pack_with(r#"{"id": "J-bad", "applies_to": "Space", "check": "max_banana", "limit": 1.0}"#))
            .unwrap_err();
        assert!(e.contains("J-bad") && e.contains("max_banana") && e.contains("min_width"), "{e}");
        let e = Pack::parse(&pack_with(r#"{"id": "W", "applies_to": "Wall", "check": "riser_max", "limit": 0.18}"#)).unwrap_err();
        assert!(e.contains("W") && e.contains("Stair") && e.contains("Wall"), "{e}");
    }

    #[test]
    fn limits_are_read_exactly() {
        let rule = r#"{"id": "r", "applies_to": "Ramp", "check": "slope_max", "limit": 0.08333333333333333}"#;
        assert_eq!(Pack::parse(&pack_with(rule)).unwrap().rules[0].limit, Limit::Value(1.0 / 12.0));
        let rule = r#"{"id": "r", "applies_to": "Stair", "check": "blondel", "limit": [0.60, 0.65]}"#;
        assert_eq!(Pack::parse(&pack_with(rule)).unwrap().rules[0].limit, Limit::Range(0.6, 0.65));
        let rule = r#"{"id": "r", "applies_to": "Stair", "check": "blondel", "limit": [1, 1]}"#;
        assert_eq!(Pack::parse(&pack_with(rule)).unwrap().rules[0].limit, Limit::Range(1.0, 1.0));
    }
}
