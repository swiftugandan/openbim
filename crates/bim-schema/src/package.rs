//! Reading trade packages: `<dir>/plugins/<package>/trade.json` (spec/schema.md, Extension kinds;
//! spec/plugins.md, Trade packages).
//!
//! [`load_all`] reads every package directory in name order and stops at the first defect, so a
//! valid package never masks a broken one. Every defect is reported against the package's own
//! `trade.json`, a path built from the *directory* name, so the file a user has to fix is always
//! named. [`parse`] holds the rules on the file's content and does no file access.

use std::collections::{BTreeMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::Path;

use crate::json::{self, Json};
use crate::{Geometry, KindDef, PackageInfo, PropType, SchemaError, Term, ABI, RESERVED_PACKAGES};

/// The manifest every package directory holds.
const MANIFEST: &str = "trade.json";

/// A package read from disk: its description and the kinds it declares.
#[derive(Debug)]
pub(crate) struct Package {
    pub(crate) info: PackageInfo,
    pub(crate) kinds: Vec<KindDef>,
}

/// Reads every package under `<dir>/plugins/`, in directory-name order.
///
/// A project without a `plugins` directory has no packages. Entries of `plugins` that are not
/// directories, and directories whose name starts with `.`, are not packages (a package name starts
/// with a letter) and are skipped. Every other directory must hold a valid `trade.json`.
pub(crate) fn load_all(dir: &Path) -> Result<Vec<Package>, SchemaError> {
    let plugins = dir.join("plugins");
    let fail = |file: &Path, reason: String| SchemaError::Package { file: file.to_path_buf(), reason };

    let listing = match fs::metadata(&plugins) {
        Ok(meta) if meta.is_dir() => fs::read_dir(&plugins).map_err(|e| fail(&plugins, e.to_string()))?,
        Ok(_) => return Err(fail(&plugins, "not a directory".to_string())),
        // No `plugins` directory (or `dir` is not even a directory): no packages.
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => return Ok(Vec::new()),
        Err(e) => return Err(fail(&plugins, e.to_string())),
    };
    let mut names: Vec<OsString> = Vec::new();
    for entry in listing {
        names.push(entry.map_err(|e| fail(&plugins, e.to_string()))?.file_name());
    }
    // `read_dir` order is arbitrary; the first defect reported must not depend on it.
    names.sort();

    let mut packages = Vec::new();
    for name in names {
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let package_dir = plugins.join(&name);
        let file = package_dir.join(MANIFEST);
        match fs::metadata(&package_dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => continue,
            // A dangling symlink or an unreadable entry: a package that cannot be read.
            Err(e) => return Err(fail(&file, e.to_string())),
        }
        let package = read(&name, &file).map_err(|reason| fail(&file, reason))?;
        packages.push(package);
    }
    Ok(packages)
}

/// Reads and checks the manifest `file` of the package directory called `dir_name`.
fn read(dir_name: &OsStr, file: &Path) -> Result<Package, String> {
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(format!("missing {MANIFEST}")),
        Err(e) => return Err(e.to_string()),
    };
    let text = String::from_utf8(bytes).map_err(|_| "not valid UTF-8".to_string())?;
    let dir_name = dir_name.to_str().ok_or("the package directory name is not valid UTF-8")?;
    parse(dir_name, &text)
}

/// Checks the text of a `trade.json` found in the package directory `dir_name` and returns the
/// package it declares. The error is the reason, without the file name.
///
/// The rules (spec/schema.md, Extension kinds; spec/plugins.md, Trade packages): the document is a
/// JSON object without repeated keys; `package` matches `[a-z][a-z0-9-]*`, equals the directory
/// name and is not [`RESERVED_PACKAGES`]; `version` is a non-empty string; `abi` is [`ABI`];
/// `engine`, when present, is a non-empty string; `kinds` is an object whose keys
/// are `<package>:<Kind>` (Kind matching `[A-Z][A-Za-z0-9]*`), each declaring only `props` (names
/// not starting with `_`, types `num`, `text`, `bool`, `ref` or `list`) and `geometry` (a `box` or
/// a `segment` whose entries are numbers or declared prop names). Unknown top-level members are
/// ignored; an unknown member of a kind, of a geometry template or of `geometry` is an error.
pub(crate) fn parse(dir_name: &str, text: &str) -> Result<Package, String> {
    let root = json::parse(text).map_err(|e| format!("malformed JSON: {e}"))?;
    let root = Members::new(&root, "the top level")?;

    let package = string(root.require("package")?, "`package`")?;
    if !is_package_name(package) {
        return Err(format!("package name `{package}` must match [a-z][a-z0-9-]*"));
    }
    if package != dir_name {
        return Err(format!("package `{package}` does not match its directory name `{dir_name}`"));
    }
    if RESERVED_PACKAGES.contains(&package) {
        return Err(format!("package `{package}` is reserved (a built-in trade or a shipped pack)"));
    }
    let version = string(root.require("version")?, "`version`")?;
    if version.is_empty() {
        return Err("`version` must not be empty".to_string());
    }
    let abi = string(root.require("abi")?, "`abi`")?;
    if abi != ABI {
        return Err(format!("unsupported abi `{abi}` (expected `{ABI}`)"));
    }
    let engine = root.get("engine").map(engine).transpose()?;

    let declared = Members::new(root.require("kinds")?, "`kinds`")?;
    let mut kinds = Vec::new();
    for (name, body) in declared.members {
        check_kind_name(name, package)?;
        kinds.push(kind_def(name, package, body).map_err(|e| format!("kind `{name}`: {e}"))?);
    }
    let mut names: Vec<String> = kinds.iter().map(|kind| kind.name.clone()).collect();
    names.sort();
    let info = PackageInfo { name: package.to_string(), version: version.to_string(), abi: abi.to_string(), engine, kinds: names };
    Ok(Package { info, kinds })
}

/// `engine`: the component file of the package, a non-empty string. Only its type is checked here:
/// whether the file exists, is a valid component, or lies inside the package directory matters only
/// when the engine is invoked (spec/plugins.md: "a missing or invalid `engine` file exits 2 only when
/// that engine is invoked"), so that is for whoever invokes it.
fn engine(value: &Json) -> Result<String, String> {
    let path = string(value, "`engine`")?;
    if path.is_empty() {
        return Err("`engine` must not be empty".to_string());
    }
    Ok(path.to_string())
}

/// A kind is named `<package>:<Kind>`.
fn check_kind_name(name: &str, package: &str) -> Result<(), String> {
    let Some((prefix, kind)) = name.split_once(':') else {
        return Err(format!("kind `{name}` must be named `{package}:<Kind>`"));
    };
    if prefix != package {
        return Err(format!("kind `{name}` is outside the namespace of package `{package}` (`{package}:<Kind>`)"));
    }
    if !is_kind_name(kind) {
        return Err(format!("kind `{name}`: `{kind}` must match [A-Z][A-Za-z0-9]*"));
    }
    Ok(())
}

/// One entry of `kinds`. `props` is read before `geometry` whatever their order in the file, since
/// geometry entries name props.
fn kind_def(name: &str, package: &str, body: &Json) -> Result<KindDef, String> {
    let body = Members::new(body, "the declaration")?;
    body.only(&["props", "geometry"])?;
    let props = match body.get("props") {
        Some(props) => props_of(props)?,
        None => BTreeMap::new(),
    };
    let geometry = body.get("geometry").map(|geometry| geometry_of(geometry, &props)).transpose()?;
    Ok(KindDef { name: name.to_string(), package: Some(package.to_string()), props, geometry })
}

fn props_of(value: &Json) -> Result<BTreeMap<String, PropType>, String> {
    let declared = Members::new(value, "`props`")?;
    let mut props = BTreeMap::new();
    for (key, ty) in declared.members {
        if key.is_empty() {
            return Err("a prop name must not be empty".to_string());
        }
        if key.starts_with('_') {
            return Err(format!("prop `{key}`: names starting with `_` are reserved"));
        }
        let ty = string(ty, &format!("the type of prop `{key}`"))?;
        let ty = PropType::parse(ty)
            .ok_or_else(|| format!("prop `{key}` has unknown type `{ty}` (expected num, text, bool, ref or list)"))?;
        props.insert(key.clone(), ty);
    }
    Ok(props)
}

/// A closed set of templates: exactly one of `box` and `segment`.
fn geometry_of(value: &Json, props: &BTreeMap<String, PropType>) -> Result<Geometry, String> {
    let template = Members::new(value, "`geometry`")?;
    let [(name, body)] = template.members else {
        return Err(format!(
            "`geometry` must have exactly one member (`box` or `segment`), found {}",
            template.members.len()
        ));
    };
    match name.as_str() {
        "box" => {
            let body = Members::new(body, "`geometry.box`")?;
            body.only(&["centre", "size"])?;
            Ok(Geometry::Box {
                centre: terms(body.require("centre")?, "`geometry.box.centre`", props)?,
                size: terms(body.require("size")?, "`geometry.box.size`", props)?,
            })
        }
        "segment" => {
            let body = Members::new(body, "`geometry.segment`")?;
            body.only(&["from", "to", "diameter"])?;
            Ok(Geometry::Segment {
                from: terms(body.require("from")?, "`geometry.segment.from`", props)?,
                to: terms(body.require("to")?, "`geometry.segment.to`", props)?,
                diameter: term(body.require("diameter")?, "`geometry.segment.diameter`", props)?,
            })
        }
        other => Err(format!("unknown geometry template `{other}` (expected `box` or `segment`)")),
    }
}

/// Three entries, each a number or a prop of the kind.
fn terms(value: &Json, what: &str, props: &BTreeMap<String, PropType>) -> Result<[Term; 3], String> {
    let Json::Array(items) = value else {
        return Err(format!("{what} must be an array of 3 entries, found {}", value.type_name()));
    };
    let [x, y, z] = items.as_slice() else {
        return Err(format!("{what} must have exactly 3 entries, found {}", items.len()));
    };
    Ok([term(x, what, props)?, term(y, what, props)?, term(z, what, props)?])
}

fn term(value: &Json, what: &str, props: &BTreeMap<String, PropType>) -> Result<Term, String> {
    match value {
        Json::Num(x) => Ok(Term::Num(*x)),
        Json::Str(name) if props.contains_key(name) => Ok(Term::Prop(name.clone())),
        Json::Str(name) => Err(format!("{what}: entry `{name}` is neither a declared prop nor a number")),
        other => Err(format!("{what}: an entry must be a prop name or a number, found {}", other.type_name())),
    }
}

fn string<'a>(value: &'a Json, what: &str) -> Result<&'a str, String> {
    match value {
        Json::Str(s) => Ok(s),
        other => Err(format!("{what} must be a string, found {}", other.type_name())),
    }
}

/// `[a-z][a-z0-9-]*`
fn is_package_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `[A-Z][A-Za-z0-9]*`
fn is_kind_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_uppercase()) && bytes.all(|b| b.is_ascii_alphanumeric())
}

/// The members of a JSON object, in file order, known to have unique keys.
struct Members<'a> {
    what: &'a str,
    members: &'a [(String, Json)],
}

impl<'a> Members<'a> {
    /// `value` as an object; a repeated key is an error (JSON leaves its meaning open).
    fn new(value: &'a Json, what: &'a str) -> Result<Self, String> {
        let Json::Object(members) = value else {
            return Err(format!("{what} must be an object, found {}", value.type_name()));
        };
        let mut seen = HashSet::new();
        for (key, _) in members {
            if !seen.insert(key.as_str()) {
                return Err(format!("duplicate member `{key}` in {what}"));
            }
        }
        Ok(Members { what, members })
    }

    fn get(&self, key: &str) -> Option<&'a Json> {
        self.members.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn require(&self, key: &str) -> Result<&'a Json, String> {
        self.get(key).ok_or_else(|| format!("{} has no member `{key}`", self.what))
    }

    /// An error for the first member not in `allowed`.
    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        match self.members.iter().find(|(key, _)| !allowed.contains(&key.as_str())) {
            Some((key, _)) => {
                let expected: Vec<String> = allowed.iter().map(|name| format!("`{name}`")).collect();
                Err(format!("unknown member `{key}` in {} (expected {})", self.what, expected.join(", ")))
            }
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid package document with `kinds` (JSON text) for the package `fire`.
    fn doc(kinds: &str) -> String {
        format!(r#"{{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0", "kinds": {kinds}}}"#)
    }

    fn defect(dir: &str, text: &str) -> String {
        match parse(dir, text) {
            Err(reason) => reason,
            Ok(package) => panic!("accepted: {text}\n{package:?}"),
        }
    }

    fn kind_of(kinds: &str) -> KindDef {
        let package = parse("fire", &doc(kinds)).unwrap_or_else(|e| panic!("{kinds}: {e}"));
        package.kinds.into_iter().next().expect("one kind")
    }

    fn prop(name: &str) -> Term {
        Term::Prop(name.to_string())
    }

    #[test]
    fn parses_the_spec_example() {
        let text = r#"{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0",
         "kinds": {
           "fire:Sprinkler": {"props": {"level": "ref", "x": "num", "y": "num", "z": "num", "coverage_radius": "num"},
                              "geometry": {"box": {"centre": ["x", "y", "z"], "size": [0.1, 0.1, 0.1]}}},
           "fire:Main": {"props": {"x1": "num", "y1": "num", "z1": "num", "x2": "num", "y2": "num", "z2": "num", "diameter": "num"},
                         "geometry": {"segment": {"from": ["x1", "y1", "z1"], "to": ["x2", "y2", "z2"], "diameter": "diameter"}}}
         },
         "engine": "fire.wasm"}"#;
        let package = parse("fire", text).unwrap();
        assert_eq!(package.info.name, "fire");
        assert_eq!(package.info.version, "0.1.0");
        assert_eq!(package.info.abi, "openbim:trade@0.1.0");
        assert_eq!(package.info.engine.as_deref(), Some("fire.wasm"));
        assert_eq!(package.info.kinds, ["fire:Main", "fire:Sprinkler"], "sorted by name");
        let sprinkler = package.kinds.iter().find(|kind| kind.name == "fire:Sprinkler").unwrap();
        assert_eq!(sprinkler.package.as_deref(), Some("fire"));
        assert_eq!(sprinkler.props.len(), 5);
        assert_eq!(sprinkler.props["level"], PropType::Ref);
        assert_eq!(
            sprinkler.geometry,
            Some(Geometry::Box {
                centre: [prop("x"), prop("y"), prop("z")],
                size: [Term::Num(0.1), Term::Num(0.1), Term::Num(0.1)],
            })
        );
        let main = package.kinds.iter().find(|kind| kind.name == "fire:Main").unwrap();
        assert_eq!(
            main.geometry,
            Some(Geometry::Segment {
                from: [prop("x1"), prop("y1"), prop("z1")],
                to: [prop("x2"), prop("y2"), prop("z2")],
                diameter: prop("diameter"),
            })
        );
    }

    #[test]
    fn optional_parts_may_be_absent() {
        let package = parse("fire", &doc("{}")).unwrap();
        assert!(package.kinds.is_empty() && package.info.kinds.is_empty() && package.info.engine.is_none());
        let kind = kind_of(r#"{"fire:Note": {}}"#);
        assert!(kind.props.is_empty() && kind.geometry.is_none(), "props and geometry are optional");
        let kind = kind_of(r#"{"fire:Note": {"props": {}}}"#);
        assert!(kind.props.is_empty());
    }

    #[test]
    fn literals_and_prop_terms_mix_and_integers_become_floats() {
        let kind = kind_of(
            r#"{"fire:Duct": {"props": {"len": "num"},
                 "geometry": {"segment": {"from": [0, 0, 0], "to": ["len", -0.25, 1e1], "diameter": 2}}}}"#,
        );
        assert_eq!(
            kind.geometry,
            Some(Geometry::Segment {
                from: [Term::Num(0.0), Term::Num(0.0), Term::Num(0.0)],
                to: [prop("len"), Term::Num(-0.25), Term::Num(10.0)],
                diameter: Term::Num(2.0),
            })
        );
    }

    #[test]
    fn geometry_may_come_before_props_in_the_file() {
        let kind = kind_of(r#"{"fire:S": {"geometry": {"box": {"centre": ["x", 0, 0], "size": [1, 1, 1]}}, "props": {"x": "num"}}}"#);
        assert!(matches!(kind.geometry, Some(Geometry::Box { ref centre, .. }) if centre[0] == prop("x")));
    }

    #[test]
    fn every_prop_type_is_known() {
        let kind = kind_of(r#"{"fire:T": {"props": {"a": "num", "b": "text", "c": "bool", "d": "ref", "e": "list"}}}"#);
        let types: Vec<PropType> = kind.props.values().copied().collect();
        assert_eq!(types, [PropType::Num, PropType::Text, PropType::Bool, PropType::Ref, PropType::List]);
    }

    #[test]
    fn rejects_malformed_json_and_the_wrong_top_level() {
        assert!(defect("fire", r#"{"package": "fire", "kinds": {"#).starts_with("malformed JSON"));
        assert!(defect("fire", "").starts_with("malformed JSON"));
        assert!(defect("fire", "[]").contains("must be an object"));
        assert!(defect("fire", r#"{"package": "fire", "package": "fire"}"#).contains("duplicate member `package`"));
        let kinds = r#"{"fire:A": {}, "fire:A": {}}"#;
        assert!(defect("fire", &doc(kinds)).contains("duplicate member `fire:A`"));
        let kinds = r#"{"fire:A": {"props": {"x": "num", "x": "text"}}}"#;
        assert!(defect("fire", &doc(kinds)).contains("duplicate member `x`"));
    }

    #[test]
    fn rejects_inconsistent_package_headers() {
        let mep = r#"{"package": "mep", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {"mep:H": {}}}"#;
        assert!(defect("fire2", &doc("{}")).contains("does not match its directory name `fire2`"));
        assert!(defect("mep", mep).contains("reserved"));
        for name in ["arch", "struct", "mep", "ibc", "eurocode", "accessibility"] {
            let text = format!(r#"{{"package": "{name}", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {{}}}}"#);
            assert!(defect(name, &text).contains("reserved"), "{name}");
        }
        for name in ["Fire", "fire_x", "1fire", "", "fire x", "fïre"] {
            let text = format!(r#"{{"package": "{name}", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {{}}}}"#);
            assert!(defect(name, &text).contains("must match [a-z][a-z0-9-]*"), "{name:?}");
        }
        assert!(parse("a-b-2", r#"{"package": "a-b-2", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {}}"#).is_ok());
        for abi in ["openbim:trade@0.2.0", "openbim:plugin@0.1.0", "", "OPENBIM:TRADE@0.1.0"] {
            let text = format!(r#"{{"package": "fire", "version": "1", "abi": "{abi}", "kinds": {{}}}}"#);
            assert!(defect("fire", &text).contains("unsupported abi"), "{abi}");
        }
        assert!(defect("fire", r#"{"package": "fire", "version": "", "abi": "openbim:trade@0.1.0", "kinds": {}}"#).contains("must not be empty"));
    }

    #[test]
    fn rejects_missing_and_mistyped_members() {
        let cases = [
            (r#"{"version": "1", "abi": "openbim:trade@0.1.0", "kinds": {}}"#, "no member `package`"),
            (r#"{"package": "fire", "abi": "openbim:trade@0.1.0", "kinds": {}}"#, "no member `version`"),
            (r#"{"package": "fire", "version": "1", "kinds": {}}"#, "no member `abi`"),
            (r#"{"package": "fire", "version": "1", "abi": "openbim:trade@0.1.0"}"#, "no member `kinds`"),
            (r#"{"package": 1, "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {}}"#, "`package` must be a string"),
            (r#"{"package": "fire", "version": 1, "abi": "openbim:trade@0.1.0", "kinds": {}}"#, "`version` must be a string"),
            (r#"{"package": "fire", "version": "1", "abi": null, "kinds": {}}"#, "`abi` must be a string"),
            (r#"{"package": "fire", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": []}"#, "`kinds` must be an object"),
            (r#"{"package": "fire", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {}, "engine": null}"#, "`engine` must be a string"),
            (r#"{"package": "fire", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {}, "engine": 3}"#, "`engine` must be a string"),
        ];
        for (text, reason) in cases {
            let got = defect("fire", text);
            assert!(got.contains(reason), "{text}: {got}");
        }
        // Unknown top-level members are ignored.
        let text = r#"{"package": "fire", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {}, "description": "x", "n": 1}"#;
        assert!(parse("fire", text).is_ok());
    }

    #[test]
    fn the_engine_is_a_non_empty_string_whose_file_is_not_looked_at() {
        let with = |engine: &str| {
            parse(
                "fire",
                &format!(r#"{{"package": "fire", "version": "1", "abi": "openbim:trade@0.1.0", "kinds": {{}}, "engine": "{engine}"}}"#),
            )
            .map(|package| package.info.engine)
        };
        // Nothing about the file is checked at load: not that it exists, nor where it is.
        for engine in ["fire.wasm", "bin/fire.wasm", "./fire.wasm", "missing/nowhere.wasm"] {
            assert_eq!(with(engine), Ok(Some(engine.to_string())), "{engine}");
        }
        assert_eq!(with("").unwrap_err(), "`engine` must not be empty");
    }

    #[test]
    fn rejects_kinds_outside_the_namespace() {
        let cases = [
            (r#"{"fire:Sprinkler": {}, "acoustic:Panel": {}}"#, "outside the namespace"),
            (r#"{"Sprinkler": {}}"#, "must be named `fire:<Kind>`"),
            (r#"{"": {}}"#, "must be named `fire:<Kind>`"),
            (r#"{"fire:": {}}"#, "must match [A-Z][A-Za-z0-9]*"),
            (r#"{"fire:sprinkler": {}}"#, "must match [A-Z][A-Za-z0-9]*"),
            (r#"{"fire:Spr-inkler": {}}"#, "must match [A-Z][A-Za-z0-9]*"),
            (r#"{"fire:Spr:inkler": {}}"#, "must match [A-Z][A-Za-z0-9]*"),
            (r#"{"fire:Spr inkler": {}}"#, "must match [A-Z][A-Za-z0-9]*"),
            (r#"{"fire:Ärger": {}}"#, "must match [A-Z][A-Za-z0-9]*"),
            (r#"{"Fire:Sprinkler": {}}"#, "outside the namespace"),
            (r#"{"fire2:Sprinkler": {}}"#, "outside the namespace"),
        ];
        for (kinds, reason) in cases {
            let got = defect("fire", &doc(kinds));
            assert!(got.contains(reason), "{kinds}: {got}");
        }
        assert!(parse("fire", &doc(r#"{"fire:A1b2": {}, "fire:X": {}}"#)).is_ok());
    }

    #[test]
    fn rejects_bad_kind_declarations() {
        let cases = [
            (r#"{"fire:A": []}"#, "must be an object"),
            (r#"{"fire:A": {"prop": {}}}"#, "unknown member `prop`"),
            (r#"{"fire:A": {"props": [], "geometry": null}}"#, "`props` must be an object"),
            (r#"{"fire:A": {"props": {"x": "number"}}}"#, "unknown type `number`"),
            (r#"{"fire:A": {"props": {"x": "float"}}}"#, "unknown type `float`"),
            (r#"{"fire:A": {"props": {"x": "Num"}}}"#, "unknown type `Num`"),
            (r#"{"fire:A": {"props": {"x": 1}}}"#, "the type of prop `x` must be a string"),
            (r#"{"fire:A": {"props": {"": "num"}}}"#, "must not be empty"),
            (r#"{"fire:A": {"props": {"_name": "text"}}}"#, "reserved"),
            (r#"{"fire:A": {"props": {"_": "text"}}}"#, "reserved"),
            (r#"{"fire:A": {"geometry": null}}"#, "`geometry` must be an object"),
            (r#"{"fire:A": {"geometry": {}}}"#, "exactly one member"),
            (r#"{"fire:A": {"geometry": {"box": {"centre": [0,0,0], "size": [1,1,1]}, "segment": {}}}}"#, "exactly one member"),
        ];
        for (kinds, reason) in cases {
            let got = defect("fire", &doc(kinds));
            assert!(got.contains(reason), "{kinds}: {got}");
            assert!(got.contains("kind `fire:A`") || got.contains("fire"), "{kinds}: {got}");
        }
    }

    #[test]
    fn rejects_bad_geometry() {
        let with = |geometry: &str| format!(r#"{{"fire:A": {{"props": {{"x": "num", "d": "num"}}, "geometry": {geometry}}}}}"#);
        let cases = [
            (r#"{"sphere": {"centre": ["x", 0, 0], "radius": 1}}"#, "unknown geometry template `sphere`"),
            (r#"{"cylinder": {}}"#, "unknown geometry template `cylinder`"),
            (r#"{"box": {"centre": [0, 0, 0]}}"#, "no member `size`"),
            (r#"{"box": {"size": [1, 1, 1]}}"#, "no member `centre`"),
            (r#"{"box": {"centre": [0, 0, 0], "size": [1, 1, 1], "angle": 0}}"#, "unknown member `angle`"),
            (r#"{"box": {"centre": [0, 0], "size": [1, 1, 1]}}"#, "exactly 3 entries, found 2"),
            (r#"{"box": {"centre": [0, 0, 0, 0], "size": [1, 1, 1]}}"#, "exactly 3 entries, found 4"),
            (r#"{"box": {"centre": "x", "size": [1, 1, 1]}}"#, "must be an array of 3 entries"),
            (r#"{"box": {"centre": ["q", 0, 0], "size": [1, 1, 1]}}"#, "entry `q` is neither a declared prop nor a number"),
            (r#"{"box": {"centre": ["_name", 0, 0], "size": [1, 1, 1]}}"#, "neither a declared prop nor a number"),
            (r#"{"box": {"centre": [true, 0, 0], "size": [1, 1, 1]}}"#, "a prop name or a number, found a boolean"),
            (r#"{"box": {"centre": [null, 0, 0], "size": [1, 1, 1]}}"#, "found null"),
            (r#"{"box": {"centre": [[1], 0, 0], "size": [1, 1, 1]}}"#, "found an array"),
            (r#"{"box": []}"#, "`geometry.box` must be an object"),
            (r#"{"segment": {"from": [0,0,0], "to": [1,1,1]}}"#, "no member `diameter`"),
            (r#"{"segment": {"from": [0,0,0], "to": [1,1,1], "diameter": "nope"}}"#, "`nope` is neither a declared prop nor a number"),
            (r#"{"segment": {"from": [0,0,0], "to": [1,1,1], "diameter": ["d"]}}"#, "found an array"),
            (r#"{"segment": {"from": [0,0,0], "to": [1,1,1], "diameter": 1, "x": 0}}"#, "unknown member `x`"),
        ];
        for (geometry, reason) in cases {
            let got = defect("fire", &doc(&with(geometry)));
            assert!(got.contains(reason), "{geometry}: {got}");
        }
    }

    #[test]
    fn mutated_manifests_are_accepted_or_rejected_but_never_panic() {
        let original = r#"{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0",
         "kinds": {"fire:Sprinkler": {"props": {"level": "ref", "x": "num", "z": "num"},
                                      "geometry": {"box": {"centre": ["x", 0, "z"], "size": [0.1, 1e-3, 2]}}},
                   "fire:Main": {"props": {"d": "num"}, "geometry": {"segment": {"from": [0, 0, 0], "to": [1, 1, 1], "diameter": "d"}}}},
         "engine": "bin/fire.wasm"}"#;
        assert!(parse("fire", original).is_ok());
        let alphabet: Vec<char> = "{}[]\",:.-+eE0189 \\/_ntfulxyzA\u{e9}\u{0}\u{7f}".chars().collect();
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut accepted = 0;
        for _ in 0..4000 {
            let mut chars: Vec<char> = original.chars().collect();
            for _ in 0..=(next() % 4) {
                if chars.is_empty() {
                    break;
                }
                let at = (next() as usize) % chars.len();
                match next() % 4 {
                    0 => {
                        chars.remove(at);
                    }
                    1 => chars.insert(at, alphabet[(next() as usize) % alphabet.len()]),
                    2 => chars[at] = alphabet[(next() as usize) % alphabet.len()],
                    _ => chars.truncate(at.max(1)),
                }
            }
            let text: String = chars.into_iter().collect();
            accepted += usize::from(parse("fire", &text).is_ok());
        }
        assert!(accepted > 0, "some mutations (whitespace, literals) stay valid");
    }

    #[test]
    fn json_nesting_cannot_overflow_the_stack() {
        let deep = format!(r#"{{"package": "fire", "kinds": {}{}}}"#, "[".repeat(10_000), "]".repeat(10_000));
        assert!(defect("fire", &deep).starts_with("malformed JSON"));
    }
}
