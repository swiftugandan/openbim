// External judge for T1.8 (bim-schema): KindRegistry, KindDef, PropType, Geometry, Term and SchemaError
// exactly as pinned in TASKS.md Details, against spec/schema.md (Entity kinds, Extension kinds) and
// spec/plugins.md (Trade packages). The CLI half lives in judges/T1.8/cli.py.
//
// No derives are pinned for KindRegistry/KindDef/PropType/Geometry/Term, so values are inspected by
// destructuring and `matches!` only; SchemaError is compared through Display only.
use bim_doc::{EntityId, Value};
use bim_schema::{Geometry, KindDef, KindRegistry, PropType, SchemaError, Term};
use std::path::{Path, PathBuf};

/// Every kind of the spec/schema.md "Entity kinds" table (including the direct-modelling kinds it
/// delegates to spec/sketch.md and the kernel-materialized ConflictDiagnostic), sorted.
const CORE: [&str; 34] = [
    "AreaLoad", "Beam", "Circuit", "Column", "ComponentDefinition", "ComponentInstance", "ConflictDiagnostic",
    "Constraint", "CurtainWall", "Dimension", "Furniture", "Guide", "Issue", "Level", "LoadCombination",
    "Material", "MepSystem", "Opening", "Point", "PointLoad", "Port", "Product", "Ramp", "Roof", "Scene",
    "Segment", "Shape", "Slab", "SlabOpening", "Space", "Stair", "Support", "Wall", "WallType",
];

/// The spec/schema.md example package, verbatim.
const FIRE: &str = r#"{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0",
 "kinds": {
   "fire:Sprinkler": {"props": {"level": "ref", "x": "num", "y": "num", "z": "num", "coverage_radius": "num"},
                      "geometry": {"box": {"centre": ["x", "y", "z"], "size": [0.1, 0.1, 0.1]}}},
   "fire:Main": {"props": {"x1": "num", "y1": "num", "z1": "num", "x2": "num", "y2": "num", "z2": "num", "diameter": "num"},
                 "geometry": {"segment": {"from": ["x1", "y1", "z1"], "to": ["x2", "y2", "z2"], "diameter": "diameter"}}}
 },
 "engine": "fire.wasm"}"#;

/// A second package: every prop type, number literals mixed with prop names (integers too), a kind
/// without geometry, and no engine.
const ACOUSTIC: &str = r#"{"package": "acoustic", "version": "2.1.0", "abi": "openbim:trade@0.1.0",
 "kinds": {
   "acoustic:Panel": {"props": {"label": "text", "active": "bool", "layers": "list", "host": "ref",
                                "x": "num", "y": "num", "w": "num"},
                      "geometry": {"box": {"centre": ["x", "y", 1.5], "size": [0.6, "w", 2]}}},
   "acoustic:Duct": {"props": {"len": "num"},
                     "geometry": {"segment": {"from": [0, 0, 0], "to": ["len", 0, -0.25], "diameter": 0.25}}},
   "acoustic:Note": {"props": {"text": "text"}}
 }}"#;

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh project directory with `plugins/<dir>/trade.json` = contents, for each entry.
fn project(tag: &str, packages: &[(&str, &str)]) -> Dir {
    let root = std::env::temp_dir().join(format!("judge-t1-8-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for (dir, json) in packages {
        let p = root.join("plugins").join(dir);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("trade.json"), json).unwrap();
    }
    Dir(root)
}

fn load(p: &Path) -> KindRegistry {
    match KindRegistry::load(p) {
        Ok(r) => r,
        Err(e) => panic!("KindRegistry::load({}) failed: {e}", p.display()),
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn prop(t: &Term, name: &str) -> bool {
    matches!(t, Term::Prop(s) if s == name)
}

fn num(t: &Term, x: f64) -> bool {
    matches!(t, Term::Num(v) if *v == x)
}

fn type_name(p: &PropType) -> &'static str {
    match p {
        PropType::Num => "num",
        PropType::Text => "text",
        PropType::Bool => "bool",
        PropType::Ref => "ref",
        PropType::List => "list",
    }
}

fn def<'a>(r: &'a KindRegistry, kind: &str) -> &'a KindDef {
    r.kind(kind).unwrap_or_else(|| panic!("kind {kind} is not in the registry"))
}

fn props_of(d: &KindDef) -> Vec<(String, &'static str)> {
    d.props.iter().map(|(k, t)| (k.clone(), type_name(t))).collect()
}

fn pairs(v: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    let mut out: Vec<_> = v.iter().map(|(k, t)| (k.to_string(), *t)).collect();
    out.sort();
    out
}

fn n(x: f64) -> Value {
    Value::Num(x)
}
fn t(s: &str) -> Value {
    Value::Text(s.into())
}
fn r(s: &str) -> Value {
    Value::Ref(EntityId::derive(s))
}
fn kv(k: &str, v: Value) -> (String, Value) {
    (k.to_string(), v)
}

fn ok(res: Result<(), SchemaError>, what: &str) {
    if let Err(e) = res {
        panic!("{what}: rejected ({e}), but it is valid");
    }
}

fn rejected(res: Result<(), SchemaError>, what: &str) {
    match res {
        Ok(()) => panic!("{what}: accepted, but it breaks the declarations"),
        Err(e) => assert!(!e.to_string().trim().is_empty(), "{what}: SchemaError displays as an empty string"),
    }
}

#[test]
fn core_registry_lists_exactly_the_core_kinds() {
    let core = KindRegistry::core();
    assert_eq!(core.kinds(), names(&CORE), "core().kinds() must be the spec/schema.md kinds, sorted");
    for k in CORE {
        let d = def(&core, k);
        assert_eq!(d.name, k);
        assert!(d.package.is_none(), "core kind {k}: package must be None");
        assert!(d.props.is_empty(), "core kind {k}: props must be empty (core props are not declared)");
        assert!(d.geometry.is_none(), "core kind {k}: geometry must be None");
    }
    for k in ["fire:Sprinkler", "Bogus", "wall", "", "Wall2", "Wall "] {
        assert!(core.kind(k).is_none(), "core registry must not know {k:?}");
    }
}

#[test]
fn core_kinds_accept_any_props_and_unknown_kinds_are_rejected() {
    let core = KindRegistry::core();
    // Core kinds keep their current behaviour: their props are not type-checked (T1.6 style props).
    ok(core.check_create("Wall", &[kv("height", t("tall")), kv("exterior", Value::Bool(true)), kv("anything", n(1.0))]),
        "core Wall with free-form props");
    ok(core.check_create("Roof", &[kv("slope", n(0.3)), kv("covering", r("P1"))]), "core Roof");
    ok(core.check_create("Furniture", &[]), "core Furniture without props");
    ok(core.check_create("Product", &[kv("sku", t("X-1")), kv("anything", Value::List(vec![n(1.0)]))]), "core Product");
    ok(core.check_create("Shape", &[kv("vertices", Value::List(vec![]))]), "core Shape");
    ok(core.check_set("Level", "elevation", &t("not a number")), "set on a core Level (not type-checked)");
    ok(core.check_set("Segment", "new_key", &Value::Bool(false)), "set of a new key on a core Segment");
    rejected(core.check_create("Bogus", &[]), "create of an undeclared kind Bogus");
    rejected(core.check_create("fire:Sprinkler", &[]), "create of fire:Sprinkler with no package loaded");
    rejected(core.check_create("wall", &[]), "create of an undeclared kind wall (kinds are case-sensitive)");
}

#[test]
fn load_without_packages_is_core() {
    let none = project("none", &[]);
    assert_eq!(load(&none.0).kinds(), names(&CORE), "load() of a project without plugins/");
    std::fs::create_dir_all(none.0.join("plugins")).unwrap();
    assert_eq!(load(&none.0).kinds(), names(&CORE), "load() of a project with an empty plugins/");
}

#[test]
fn load_reads_the_declared_kinds_props_and_geometry() {
    let p = project("ok", &[("fire", FIRE), ("acoustic", ACOUSTIC)]);
    let reg = load(&p.0);
    let mut want = names(&CORE);
    want.extend(names(&["acoustic:Duct", "acoustic:Note", "acoustic:Panel", "fire:Main", "fire:Sprinkler"]));
    want.sort();
    assert_eq!(reg.kinds(), want, "kinds() = core kinds plus every package kind, sorted");
    assert!(reg.kind("Wall").is_some_and(|d| d.package.is_none()), "core kinds stay in a loaded registry");

    let s = def(&reg, "fire:Sprinkler");
    assert_eq!(s.name, "fire:Sprinkler");
    assert_eq!(s.package.as_deref(), Some("fire"));
    assert_eq!(props_of(s), pairs(&[("level", "ref"), ("x", "num"), ("y", "num"), ("z", "num"), ("coverage_radius", "num")]));
    match &s.geometry {
        Some(Geometry::Box { centre, size }) => {
            assert!(prop(&centre[0], "x") && prop(&centre[1], "y") && prop(&centre[2], "z"), "fire:Sprinkler box centre");
            assert!(size.iter().all(|e| num(e, 0.1)), "fire:Sprinkler box size [0.1, 0.1, 0.1]");
        }
        _ => panic!("fire:Sprinkler geometry must be Some(Geometry::Box {{ .. }})"),
    }

    let m = def(&reg, "fire:Main");
    assert_eq!(m.package.as_deref(), Some("fire"));
    assert_eq!(m.props.len(), 7);
    assert!(m.props.values().all(|t| matches!(t, PropType::Num)), "fire:Main props are all num");
    match &m.geometry {
        Some(Geometry::Segment { from, to, diameter }) => {
            assert!(prop(&from[0], "x1") && prop(&from[1], "y1") && prop(&from[2], "z1"), "fire:Main segment from");
            assert!(prop(&to[0], "x2") && prop(&to[1], "y2") && prop(&to[2], "z2"), "fire:Main segment to");
            assert!(prop(diameter, "diameter"), "fire:Main segment diameter");
        }
        _ => panic!("fire:Main geometry must be Some(Geometry::Segment {{ .. }})"),
    }

    let a = def(&reg, "acoustic:Panel");
    assert_eq!(a.package.as_deref(), Some("acoustic"));
    assert_eq!(props_of(a), pairs(&[("label", "text"), ("active", "bool"), ("layers", "list"), ("host", "ref"),
        ("x", "num"), ("y", "num"), ("w", "num")]));
    match &a.geometry {
        Some(Geometry::Box { centre, size }) => {
            assert!(prop(&centre[0], "x") && prop(&centre[1], "y") && num(&centre[2], 1.5), "acoustic:Panel centre");
            assert!(num(&size[0], 0.6) && prop(&size[1], "w") && num(&size[2], 2.0), "acoustic:Panel size (integer literal 2)");
        }
        _ => panic!("acoustic:Panel geometry must be a Box"),
    }
    match &def(&reg, "acoustic:Duct").geometry {
        Some(Geometry::Segment { from, to, diameter }) => {
            assert!(from.iter().all(|e| num(e, 0.0)), "acoustic:Duct from [0, 0, 0]");
            assert!(prop(&to[0], "len") && num(&to[1], 0.0) && num(&to[2], -0.25), "acoustic:Duct to");
            assert!(num(diameter, 0.25), "acoustic:Duct diameter literal");
        }
        _ => panic!("acoustic:Duct geometry must be a Segment"),
    }
    let note = def(&reg, "acoustic:Note");
    assert!(note.geometry.is_none(), "a kind without geometry has geometry None");
    assert_eq!(props_of(note), pairs(&[("text", "text")]));
    assert!(reg.kind("fire:Hose").is_none() && reg.kind("acoustic:Main").is_none(), "only declared kinds exist");
}

#[test]
fn extension_props_are_type_checked() {
    let p = project("types", &[("fire", FIRE), ("acoustic", ACOUSTIC)]);
    let reg = load(&p.0);
    let list = Value::List(vec![n(1.0), t("a"), Value::List(vec![])]);
    // Every declared prop is optional; present props must have their declared type.
    ok(reg.check_create("fire:Sprinkler", &[]), "fire:Sprinkler without props");
    ok(reg.check_create("fire:Sprinkler", &[kv("x", n(1.0)), kv("level", r("L1"))]), "fire:Sprinkler with a subset");
    ok(reg.check_create("acoustic:Panel", &[kv("label", t("p")), kv("active", Value::Bool(false)), kv("layers", list.clone()),
        kv("host", r("W1")), kv("x", n(-2.5)), kv("y", n(0.0)), kv("w", n(1e-3))]), "acoustic:Panel with every prop");
    ok(reg.check_create("acoustic:Panel", &[kv("layers", Value::List(vec![]))]), "an empty List for a list prop");

    let wrong: [(&str, Value); 10] = [
        ("x", t("1.0")), ("x", Value::Bool(true)), ("x", r("L1")), ("x", Value::List(vec![n(1.0)])),
        ("level", t("L1")), ("level", n(1.0)), ("coverage_radius", Value::List(vec![])),
        ("z", Value::Bool(false)), ("y", t("")), ("level", Value::List(vec![r("L1")])),
    ];
    for (k, v) in &wrong {
        rejected(reg.check_create("fire:Sprinkler", &[kv("x", n(0.0)), kv(k, v.clone())]),
            &format!("create fire:Sprinkler with {k} of the wrong type"));
        rejected(reg.check_set("fire:Sprinkler", k, v), &format!("set fire:Sprinkler.{k} to the wrong type"));
    }
    for (k, v) in [("label", n(1.0)), ("active", n(1.0)), ("active", t("true")), ("layers", t("a,b")), ("host", t("W1")),
        ("w", Value::Bool(true))] {
        rejected(reg.check_create("acoustic:Panel", &[kv(k, v.clone())]), &format!("create acoustic:Panel with {k} mistyped"));
        rejected(reg.check_set("acoustic:Panel", k, &v), &format!("set acoustic:Panel.{k} mistyped"));
    }
    // An extension entity may carry only declared props.
    rejected(reg.check_create("fire:Sprinkler", &[kv("x", n(1.0)), kv("colour", t("red"))]), "undeclared prop colour on create");
    rejected(reg.check_set("fire:Sprinkler", "colour", &t("red")), "undeclared prop colour on set");
    rejected(reg.check_set("fire:Main", "x", &n(1.0)), "fire:Main has x1, not x");
    rejected(reg.check_set("acoustic:Note", "label", &t("a")), "acoustic:Note declares only text");
    ok(reg.check_set("fire:Sprinkler", "coverage_radius", &n(2.5)), "set of a declared num prop");
    ok(reg.check_set("fire:Sprinkler", "level", &r("L2")), "set of a declared ref prop");
    ok(reg.check_set("acoustic:Panel", "layers", &list), "set of a declared list prop");
    ok(reg.check_set("acoustic:Panel", "active", &Value::Bool(true)), "set of a declared bool prop");
    ok(reg.check_set("acoustic:Note", "text", &t("hello")), "set of a declared text prop");
    // Kinds still need a declaration; core kinds stay unchecked in a loaded registry.
    rejected(reg.check_create("fire:Hose", &[]), "fire:Hose is not declared by the fire package");
    rejected(reg.check_create("struct:Brace", &[]), "struct:Brace has no package");
    ok(reg.check_create("Wall", &[kv("x", t("not checked"))]), "core Wall in a loaded registry");
}

/// A package with one defect: `(dir name, trade.json)`.
fn broken_packages() -> Vec<(&'static str, &'static str, String)> {
    let good = |pkg: &str, abi: &str, kinds: &str| {
        format!(r#"{{"package": "{pkg}", "version": "0.1.0", "abi": "{abi}", "kinds": {kinds}}}"#)
    };
    let ok_kinds = r#"{"fire:Sprinkler": {"props": {"x": "num"}}}"#;
    vec![
        ("malformed JSON", "fire", r#"{"package": "fire", "version": "0.1.0", "abi": "openbim:trade@0.1.0", "kinds": {"#.to_string()),
        ("directory name differs from package", "fire2", good("fire", "openbim:trade@0.1.0", ok_kinds)),
        ("package is the built-in trade mep", "mep", good("mep", "openbim:trade@0.1.0", r#"{"mep:Hanger": {"props": {"x": "num"}}}"#)),
        ("package is the built-in trade arch", "arch", good("arch", "openbim:trade@0.1.0", r#"{"arch:Thing": {"props": {}}}"#)),
        ("kind outside the package namespace", "fire", good("fire", "openbim:trade@0.1.0", r#"{"fire:Sprinkler": {"props": {}}, "acoustic:Panel": {"props": {}}}"#)),
        ("kind without a namespace", "fire", good("fire", "openbim:trade@0.1.0", r#"{"Sprinkler": {"props": {}}}"#)),
        ("wrong abi", "fire", good("fire", "openbim:trade@0.2.0", ok_kinds)),
        ("unknown prop type", "fire", good("fire", "openbim:trade@0.1.0", r#"{"fire:Sprinkler": {"props": {"x": "number"}}}"#)),
        ("unknown geometry template", "fire", good("fire", "openbim:trade@0.1.0",
            r#"{"fire:Sprinkler": {"props": {"x": "num"}, "geometry": {"sphere": {"centre": ["x", 0, 0], "radius": 1}}}}"#)),
    ]
}

#[test]
fn malformed_or_inconsistent_packages_fail_to_load() {
    for (i, (why, dir, json)) in broken_packages().into_iter().enumerate() {
        // Alongside a valid package, so a loader that skips bad packages fails too.
        let p = project(&format!("bad{i}"), &[(dir, json.as_str()), ("acoustic", ACOUSTIC)]);
        match KindRegistry::load(&p.0) {
            Ok(_) => panic!("KindRegistry::load accepted a package with a defect: {why}"),
            Err(e) => assert!(!e.to_string().trim().is_empty(), "{why}: SchemaError displays as an empty string"),
        }
    }
    // SchemaError is a std error.
    fn is_error<E: std::error::Error>() {}
    is_error::<SchemaError>();
}
