//! `bim-schema`: the kind registry (spec/schema.md, Entity kinds and Extension kinds).
//!
//! A [`KindRegistry`] knows every kind an entity may have:
//!
//! * the **core kinds**, [`CORE_KINDS`]: every row of the spec/schema.md kinds table, the direct-modelling
//!   kinds it delegates to spec/sketch.md and the kernel-materialized `ConflictDiagnostic`. Their props
//!   are not declared here and are never type-checked;
//! * the **extension kinds** that trade packages declare in `<dir>/plugins/<package>/trade.json`
//!   (spec/plugins.md, Trade packages): `<package>:<Kind>` with typed props and an optional geometry
//!   template.
//!
//! [`KindRegistry::load`] reads a project's packages and fails with a [`SchemaError::Package`] naming the
//! `trade.json` of the first package (in directory-name order) that is malformed or inconsistent.
//! [`KindRegistry::check_create`] and [`KindRegistry::check_set`] are the op validation of
//! spec/schema.md: a kind must be declared, and an extension entity may carry only its kind's declared
//! props, each with its declared type, plus reserved `_` props (which this crate never judges: who may
//! set them is the front end's rule). Every declared prop is optional.
//!
//! The registry is not part of the document: installing a package changes no entity and no hash.

#![forbid(unsafe_code)]

mod json;
mod package;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use bim_doc::Value;

/// The ABI a trade package must declare (spec/plugins.md, Trade packages).
pub const ABI: &str = "openbim:trade@0.1.0";

/// Names a trade package may not take: the built-in trades and the shipped pack ids
/// (spec/plugins.md, Trade packages).
pub const RESERVED_PACKAGES: [&str; 6] = ["arch", "struct", "mep", "ibc", "eurocode", "accessibility"];

/// The core kinds, sorted byte-wise: every row of the spec/schema.md kinds table (spec/sketch.md's
/// `Shape`, `ComponentDefinition`, `ComponentInstance`, `Guide` and `Scene` included, and the
/// kernel-materialized `ConflictDiagnostic`).
pub const CORE_KINDS: [&str; 34] = [
    "AreaLoad",
    "Beam",
    "Circuit",
    "Column",
    "ComponentDefinition",
    "ComponentInstance",
    "ConflictDiagnostic",
    "Constraint",
    "CurtainWall",
    "Dimension",
    "Furniture",
    "Guide",
    "Issue",
    "Level",
    "LoadCombination",
    "Material",
    "MepSystem",
    "Opening",
    "Point",
    "PointLoad",
    "Port",
    "Product",
    "Ramp",
    "Roof",
    "Scene",
    "Segment",
    "Shape",
    "Slab",
    "SlabOpening",
    "Space",
    "Stair",
    "Support",
    "Wall",
    "WallType",
];

/// The declared type of an extension prop: one of the Values of spec/schema.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropType {
    /// `num`: [`Value::Num`]
    Num,
    /// `text`: [`Value::Text`]
    Text,
    /// `bool`: [`Value::Bool`]
    Bool,
    /// `ref`: [`Value::Ref`]
    Ref,
    /// `list`: [`Value::List`], with any contents
    List,
}

impl PropType {
    /// The name `trade.json` uses for the type.
    pub fn name(self) -> &'static str {
        match self {
            PropType::Num => "num",
            PropType::Text => "text",
            PropType::Bool => "bool",
            PropType::Ref => "ref",
            PropType::List => "list",
        }
    }

    /// The type named `name` in a `trade.json`; `None` for any other word.
    pub fn parse(name: &str) -> Option<PropType> {
        Some(match name {
            "num" => PropType::Num,
            "text" => PropType::Text,
            "bool" => PropType::Bool,
            "ref" => PropType::Ref,
            "list" => PropType::List,
            _ => return None,
        })
    }

    /// The type of `value`: the one prop type that accepts it.
    pub fn of(value: &Value) -> PropType {
        match value {
            Value::Num(_) => PropType::Num,
            Value::Text(_) => PropType::Text,
            Value::Bool(_) => PropType::Bool,
            Value::Ref(_) => PropType::Ref,
            Value::List(_) => PropType::List,
        }
    }
}

/// One coordinate, size or diameter entry of a geometry template: the value of a declared prop of
/// the entity, or a number literal (metres). A solid exists only when every named prop is present.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    /// The name of a prop declared by the same kind.
    Prop(String),
    /// A literal.
    Num(f64),
}

/// A geometry template of an extension kind (spec/schema.md, Derived geometry rules).
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// An axis-aligned cuboid centred at `centre` with edge lengths `size` along x, y, z.
    Box {
        /// The centre (absolute x, y, z).
        centre: [Term; 3],
        /// The edge lengths along x, y, z.
        size: [Term; 3],
    },
    /// A round pipe meshed like an MEP segment, from `from` to `to`, of the given `diameter`.
    Segment {
        /// The start point.
        from: [Term; 3],
        /// The end point.
        to: [Term; 3],
        /// The outer diameter.
        diameter: Term,
    },
}

/// A kind of entity.
#[derive(Debug, Clone, PartialEq)]
pub struct KindDef {
    /// The kind's name: `Wall`, or `fire:Sprinkler` for an extension kind.
    pub name: String,
    /// The package that declares it; `None` for a core kind.
    pub package: Option<String>,
    /// The declared props and their types. Empty for a core kind: core props are not declared.
    pub props: BTreeMap<String, PropType>,
    /// The solid the kind's entities have; `None` when it has none (always, for a core kind).
    pub geometry: Option<Geometry>,
}

/// A loaded trade package, as `bim plugin list` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageInfo {
    /// The package name, equal to its directory name under `plugins/`.
    pub name: String,
    /// The package's own version, as declared.
    pub version: String,
    /// The ABI, always [`ABI`].
    pub abi: String,
    /// The component file named by `engine`, relative to the package directory; `None` if undeclared.
    /// Whether the file exists matters only when the engine is invoked.
    pub engine: Option<String>,
    /// The kinds the package declares, sorted.
    pub kinds: Vec<String>,
}

/// Every kind a project may use: the core kinds plus those of its trade packages.
#[derive(Debug, Clone)]
pub struct KindRegistry {
    kinds: BTreeMap<String, KindDef>,
    packages: Vec<PackageInfo>,
}

impl KindRegistry {
    /// The core kinds only ([`CORE_KINDS`]).
    pub fn core() -> KindRegistry {
        let kinds = CORE_KINDS
            .iter()
            .map(|&name| {
                let def = KindDef { name: name.to_string(), package: None, props: BTreeMap::new(), geometry: None };
                (name.to_string(), def)
            })
            .collect();
        KindRegistry { kinds, packages: Vec::new() }
    }

    /// The core kinds plus the kinds of every package in `<dir>/plugins/*/trade.json`.
    ///
    /// A project without a `plugins` directory has no packages. Every package directory is read: the
    /// first defect (in directory-name order) fails the whole load with a [`SchemaError::Package`]
    /// naming that directory's `trade.json`, so a valid package never hides a broken one.
    pub fn load(dir: &Path) -> Result<KindRegistry, SchemaError> {
        let mut registry = KindRegistry::core();
        for package in package::load_all(dir)? {
            // Kind names carry their package's name, which equals its (unique) directory name: they
            // cannot collide with each other or with a core name (core names have no `:`).
            for kind in package.kinds {
                registry.kinds.insert(kind.name.clone(), kind);
            }
            registry.packages.push(package.info);
        }
        registry.packages.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(registry)
    }

    /// The kind called exactly `name` (case-sensitive, no trimming); `None` if undeclared.
    pub fn kind(&self, name: &str) -> Option<&KindDef> {
        self.kinds.get(name)
    }

    /// Every kind's name, sorted byte-wise.
    pub fn kinds(&self) -> Vec<String> {
        self.kinds.keys().cloned().collect()
    }

    /// The loaded packages, sorted by name. Empty for [`KindRegistry::core`].
    pub fn packages(&self) -> &[PackageInfo] {
        &self.packages
    }

    /// Checks a create of an entity of `kind` carrying `props` (the props it is created with, in
    /// order): the kind must be declared, and for an extension kind every prop must pass
    /// [`KindRegistry::check_set`]. `check_create(kind, &[])` checks the kind alone. Every pair is
    /// checked, so a key repeated with different types fails.
    pub fn check_create(&self, kind: &str, props: &[(String, Value)]) -> Result<(), SchemaError> {
        let def = self.declared(kind)?;
        props.iter().try_for_each(|(key, value)| def.check_prop(key, value))
    }

    /// Checks a set of `key` to `value` on an entity of `kind`: the kind must be declared and, for an
    /// extension kind, `key` must be one of its declared props (or a reserved `_` key) and `value`
    /// must have the declared type. Core kinds accept any key and value.
    pub fn check_set(&self, kind: &str, key: &str, value: &Value) -> Result<(), SchemaError> {
        self.declared(kind)?.check_prop(key, value)
    }

    fn declared(&self, kind: &str) -> Result<&KindDef, SchemaError> {
        self.kind(kind).ok_or_else(|| SchemaError::UnknownKind(kind.to_string()))
    }
}

impl KindDef {
    /// The per-prop rule of an extension kind; core kinds and `_` props pass.
    fn check_prop(&self, key: &str, value: &Value) -> Result<(), SchemaError> {
        if self.package.is_none() || key.starts_with('_') {
            return Ok(());
        }
        match self.props.get(key) {
            None => Err(SchemaError::UndeclaredProp { kind: self.name.clone(), key: key.to_string() }),
            Some(&expected) => match PropType::of(value) {
                found if found == expected => Ok(()),
                found => Err(SchemaError::WrongType { kind: self.name.clone(), key: key.to_string(), expected, found }),
            },
        }
    }
}

/// Why a kind registry could not be loaded, or why an op breaks its rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// A trade package is malformed or inconsistent (or `plugins` itself is unreadable).
    Package {
        /// The file to fix: `<dir>/plugins/<directory>/trade.json`, built from the package
        /// directory's name (not from the `package` the file declares); `<dir>/plugins` itself when
        /// that cannot be listed.
        file: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// A create of a kind that is neither a core kind nor declared by a loaded package.
    UnknownKind(String),
    /// A create or set of a prop an extension kind does not declare.
    UndeclaredProp {
        /// The extension kind.
        kind: String,
        /// The prop.
        key: String,
    },
    /// A create or set of a declared prop with a value of another type.
    WrongType {
        /// The extension kind.
        kind: String,
        /// The prop.
        key: String,
        /// The declared type.
        expected: PropType,
        /// The type of the value given.
        found: PropType,
    },
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::Package { file, reason } => write!(f, "{}: {reason}", file.display()),
            SchemaError::UnknownKind(kind) => {
                write!(f, "unknown kind `{kind}` (neither a core kind nor declared by a loaded package)")
            }
            SchemaError::UndeclaredProp { kind, key } => write!(f, "kind `{kind}` declares no prop `{key}`"),
            SchemaError::WrongType { kind, key, expected, found } => {
                write!(f, "prop `{key}` of `{kind}` must be {}, got {}", expected.name(), found.name())
            }
        }
    }
}

impl std::error::Error for SchemaError {}

#[cfg(test)]
mod tests {
    use super::*;
    use bim_doc::EntityId;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh project directory removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("bim-schema-unit-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        /// Installs `text` as `plugins/<dir>/trade.json`.
        fn install(&self, dir: &str, text: &str) -> &TempDir {
            let package = self.0.join("plugins").join(dir);
            fs::create_dir_all(&package).unwrap();
            fs::write(package.join("trade.json"), text).unwrap();
            self
        }

        fn load(&self) -> Result<KindRegistry, SchemaError> {
            KindRegistry::load(&self.0)
        }

        fn load_error(&self) -> String {
            match self.load() {
                Err(e) => e.to_string(),
                Ok(_) => panic!("load accepted {}", self.0.display()),
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const FIRE: &str = r#"{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0",
     "kinds": {
       "fire:Sprinkler": {"props": {"level": "ref", "x": "num", "y": "num", "z": "num", "coverage_radius": "num"},
                          "geometry": {"box": {"centre": ["x", "y", "z"], "size": [0.1, 0.1, 0.1]}}},
       "fire:Zone": {"props": {"name": "text", "active": "bool", "rooms": "list"}}
     },
     "engine": "fire.wasm"}"#;

    const ACOUSTIC: &str = r#"{"package": "acoustic", "version": "2.1.0", "abi": "openbim:trade@0.1.0",
     "kinds": {"acoustic:Panel": {"props": {"nrc": "num"}}}}"#;

    fn num(x: f64) -> Value {
        Value::Num(x)
    }

    fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    fn pair(key: &str, value: Value) -> (String, Value) {
        (key.to_string(), value)
    }

    #[test]
    fn core_kinds_are_the_34_spec_kinds_sorted_and_unchecked() {
        assert_eq!(CORE_KINDS.len(), 34);
        assert!(CORE_KINDS.windows(2).all(|w| w[0] < w[1]), "sorted and distinct");
        let core = KindRegistry::core();
        assert_eq!(core.kinds(), CORE_KINDS);
        assert!(core.packages().is_empty());
        for name in CORE_KINDS {
            let def = core.kind(name).unwrap();
            assert_eq!(def.name, name);
            assert!(def.package.is_none() && def.props.is_empty() && def.geometry.is_none(), "{name}");
        }
        for name in ["Roof", "Furniture", "Product", "Shape", "Scene", "ConflictDiagnostic"] {
            assert!(core.kind(name).is_some(), "{name}");
        }
        for name in ["", "wall", "WALL", "Wall ", " Wall", "Wall2", "fire:Sprinkler", "W"] {
            assert!(core.kind(name).is_none(), "{name:?}");
        }
    }

    #[test]
    fn core_kinds_accept_any_props_and_reserved_keys_are_not_judged_here() {
        let core = KindRegistry::core();
        let props = [pair("height", text("tall")), pair("height", num(1.0)), pair("_name", num(1.0))];
        assert_eq!(core.check_create("Wall", &props), Ok(()));
        assert_eq!(core.check_set("Level", "elevation", &Value::Bool(true)), Ok(()));
        assert_eq!(core.check_create("ConflictDiagnostic", &[]), Ok(()));
        assert_eq!(core.check_create("Bogus", &[]), Err(SchemaError::UnknownKind("Bogus".to_string())));
        assert_eq!(core.check_set("wall", "h", &num(1.0)), Err(SchemaError::UnknownKind("wall".to_string())));
        assert_eq!(core.check_create("", &[]), Err(SchemaError::UnknownKind(String::new())));
    }

    #[test]
    fn extension_props_are_checked_against_their_declarations() {
        let tmp = TempDir::new();
        tmp.install("fire", FIRE).install("acoustic", ACOUSTIC);
        let reg = tmp.load().unwrap();
        let target = Value::Ref(EntityId::derive("L1"));
        assert_eq!(reg.check_create("fire:Sprinkler", &[]), Ok(()));
        let valid = [pair("level", target.clone()), pair("x", num(1.5)), pair("coverage_radius", num(2.5))];
        assert_eq!(reg.check_create("fire:Sprinkler", &valid), Ok(()));
        assert_eq!(reg.check_set("fire:Zone", "rooms", &Value::List(vec![])), Ok(()));
        assert_eq!(reg.check_set("fire:Zone", "rooms", &Value::List(vec![target.clone(), num(1.0)])), Ok(()));
        assert_eq!(reg.check_set("fire:Zone", "name", &text("north")), Ok(()));
        assert_eq!(reg.check_set("fire:Zone", "active", &Value::Bool(false)), Ok(()));
        assert_eq!(reg.check_set("fire:Sprinkler", "_stamp", &text("reserved keys are not judged")), Ok(()));

        let wrong_type = |kind: &str, key: &str, expected, found| SchemaError::WrongType {
            kind: kind.to_string(),
            key: key.to_string(),
            expected,
            found,
        };
        assert_eq!(
            reg.check_set("fire:Sprinkler", "x", &text("1")),
            Err(wrong_type("fire:Sprinkler", "x", PropType::Num, PropType::Text))
        );
        assert_eq!(
            reg.check_set("fire:Sprinkler", "level", &num(0.0)),
            Err(wrong_type("fire:Sprinkler", "level", PropType::Ref, PropType::Num))
        );
        assert_eq!(
            reg.check_set("fire:Zone", "rooms", &target),
            Err(wrong_type("fire:Zone", "rooms", PropType::List, PropType::Ref))
        );
        assert_eq!(
            reg.check_set("fire:Zone", "active", &num(1.0)),
            Err(wrong_type("fire:Zone", "active", PropType::Bool, PropType::Num))
        );
        // Every pair counts, a repeated key included; the first failure is reported.
        let repeated = [pair("x", num(1.0)), pair("x", text("1"))];
        assert_eq!(
            reg.check_create("fire:Sprinkler", &repeated),
            Err(wrong_type("fire:Sprinkler", "x", PropType::Num, PropType::Text))
        );
        let undeclared = SchemaError::UndeclaredProp { kind: "fire:Sprinkler".to_string(), key: "colour".to_string() };
        assert_eq!(reg.check_set("fire:Sprinkler", "colour", &text("red")), Err(undeclared.clone()));
        assert_eq!(reg.check_create("fire:Sprinkler", &[pair("x", num(1.0)), pair("colour", text("red"))]), Err(undeclared));
        assert!(reg.check_set("fire:Zone", "x", &num(1.0)).is_err(), "props belong to their own kind");
        assert!(reg.check_create("fire:Hose", &[]).is_err());
        assert!(reg.check_create("acoustic:Sprinkler", &[]).is_err());
        assert_eq!(reg.check_create("acoustic:Panel", &[pair("nrc", num(0.9))]), Ok(()));
        // Core kinds stay unchecked next to packages.
        assert_eq!(reg.check_create("Wall", &[pair("x", text("free-form"))]), Ok(()));
    }

    #[test]
    fn load_lists_packages_sorted_with_their_kinds() {
        let tmp = TempDir::new();
        assert_eq!(tmp.load().unwrap().kinds(), CORE_KINDS, "no plugins directory");
        fs::create_dir(tmp.0.join("plugins")).unwrap();
        assert_eq!(tmp.load().unwrap().kinds(), CORE_KINDS, "an empty plugins directory");
        tmp.install("fire", FIRE).install("acoustic", ACOUSTIC);
        let reg = tmp.load().unwrap();
        let mut want: Vec<String> = CORE_KINDS.iter().map(|k| k.to_string()).collect();
        want.extend(["acoustic:Panel", "fire:Sprinkler", "fire:Zone"].map(String::from));
        want.sort();
        assert_eq!(reg.kinds(), want);
        let names: Vec<&str> = reg.packages().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["acoustic", "fire"]);
        assert_eq!(reg.packages()[0].engine, None);
        assert_eq!(reg.packages()[1].engine.as_deref(), Some("fire.wasm"));
        assert_eq!(reg.packages()[1].kinds, ["fire:Sprinkler", "fire:Zone"]);
        assert_eq!(reg.packages()[0].version, "2.1.0");
        let def = reg.kind("fire:Sprinkler").unwrap();
        assert_eq!(def.package.as_deref(), Some("fire"));
        assert_eq!(def.props["coverage_radius"], PropType::Num);
        assert!(matches!(def.geometry, Some(Geometry::Box { .. })));
        assert!(reg.kind("fire:Zone").unwrap().geometry.is_none());
    }

    #[test]
    fn a_broken_package_names_its_directorys_trade_json_even_next_to_valid_ones() {
        let mep = FIRE.replace("fire", "mep");
        let cases = [
            ("fire", r#"{"package": "fire", "kinds": {"#, "malformed JSON"),
            ("fire2", FIRE, "does not match its directory name"),
            ("mep", mep.as_str(), "reserved"),
        ];
        for (dir, text, reason) in cases {
            let tmp = TempDir::new();
            // `acoustic` sorts before and `zzz` after every case's directory.
            tmp.install("acoustic", ACOUSTIC).install(dir, text).install("zzz", &ACOUSTIC.replace("acoustic", "zzz"));
            let message = tmp.load_error();
            assert!(message.contains(&format!("plugins/{dir}/trade.json: ")), "{message}");
            assert!(message.contains(reason), "{dir}: {message}");
        }
    }

    #[test]
    fn the_first_defect_in_directory_order_is_reported() {
        let tmp = TempDir::new();
        tmp.install("b", "{").install("a", "[]").install("c", "}");
        assert!(tmp.load_error().contains("plugins/a/trade.json: "), "{}", tmp.load_error());
    }

    #[test]
    fn directories_without_a_manifest_and_unreadable_entries() {
        let tmp = TempDir::new();
        tmp.install("acoustic", ACOUSTIC);
        // Stray files and dot directories are not packages.
        fs::write(tmp.0.join("plugins").join("README.md"), "x").unwrap();
        fs::create_dir(tmp.0.join("plugins").join(".git")).unwrap();
        assert_eq!(tmp.load().unwrap().packages().len(), 1);
        // A package directory without trade.json is a broken package.
        fs::create_dir(tmp.0.join("plugins").join("fire")).unwrap();
        let message = tmp.load_error();
        assert!(message.ends_with("plugins/fire/trade.json: missing trade.json"), "{message}");
        // So is one whose trade.json is not a file, or not UTF-8.
        let manifest = tmp.0.join("plugins").join("fire").join("trade.json");
        fs::create_dir(&manifest).unwrap();
        assert!(tmp.load_error().contains("plugins/fire/trade.json: "));
        fs::remove_dir(&manifest).unwrap();
        fs::write(&manifest, [0xff, 0xfe]).unwrap();
        assert!(tmp.load_error().ends_with("plugins/fire/trade.json: not valid UTF-8"));
    }

    #[test]
    fn plugins_that_is_not_a_directory_is_an_error() {
        let tmp = TempDir::new();
        fs::write(tmp.0.join("plugins"), "x").unwrap();
        let message = tmp.load_error();
        assert!(message.ends_with("plugins: not a directory"), "{message}");
        // A project path that is itself a file has no plugins.
        let file = tmp.0.join("plugins");
        assert_eq!(KindRegistry::load(&file).unwrap().kinds(), CORE_KINDS);
        assert_eq!(KindRegistry::load(&tmp.0.join("missing")).unwrap().kinds(), CORE_KINDS);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_package_directories_are_followed_and_dangling_ones_fail() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new();
        let store = TempDir::new();
        store.install("shared", ACOUSTIC);
        // The directory name is the package name, so the link is called `acoustic`.
        fs::create_dir_all(tmp.0.join("plugins")).unwrap();
        symlink(store.0.join("plugins").join("shared"), tmp.0.join("plugins").join("acoustic")).unwrap();
        assert_eq!(tmp.load().unwrap().kind("acoustic:Panel").map(|k| k.name.as_str()), Some("acoustic:Panel"));
        symlink(tmp.0.join("nowhere"), tmp.0.join("plugins").join("gone")).unwrap();
        assert!(tmp.load_error().contains("plugins/gone/trade.json: "));
    }

    #[test]
    fn errors_display_what_is_wrong() {
        let reg = KindRegistry::core();
        assert_eq!(
            reg.check_create("Bogus", &[]).unwrap_err().to_string(),
            "unknown kind `Bogus` (neither a core kind nor declared by a loaded package)"
        );
        let tmp = TempDir::new();
        tmp.install("fire", FIRE);
        let reg = tmp.load().unwrap();
        assert_eq!(
            reg.check_set("fire:Sprinkler", "colour", &text("red")).unwrap_err().to_string(),
            "kind `fire:Sprinkler` declares no prop `colour`"
        );
        assert_eq!(
            reg.check_set("fire:Sprinkler", "x", &text("1")).unwrap_err().to_string(),
            "prop `x` of `fire:Sprinkler` must be num, got text"
        );
        fn is_error<E: std::error::Error>() {}
        is_error::<SchemaError>();
    }

    #[test]
    fn prop_types_round_trip_through_their_names() {
        for ty in [PropType::Num, PropType::Text, PropType::Bool, PropType::Ref, PropType::List] {
            assert_eq!(PropType::parse(ty.name()), Some(ty));
        }
        assert_eq!(PropType::parse("number"), None);
        assert_eq!(PropType::parse("Num"), None);
        let values = [num(1.0), text("a"), Value::Bool(true), Value::Ref(EntityId::derive("a")), Value::List(vec![])];
        let types: Vec<PropType> = values.iter().map(PropType::of).collect();
        assert_eq!(types, [PropType::Num, PropType::Text, PropType::Bool, PropType::Ref, PropType::List]);
    }
}
