//! `bim-plugin`: the plugin boundary of spec/plugins.md.
//!
//! * [`TradeEngine`] — a trade (`arch`, `struct`, `mep`, …): a pure projection of the document into a
//!   JSON result.
//! * [`StandardRuleSet`] — a standard (`ibc`, `eurocode`, …): checks the document and returns
//!   [`Violation`]s.
//! * [`EngineContext`] — what both see: read-only access to the [`Document`] and memoized derived
//!   data (the compiled [`bim_model::Model`]: solids, walls, slabs, …).
//! * [`Registry`] — maps trade names to engines and rule-set ids to rule sets (two separate
//!   namespaces). Front ends (the CLI, the MCP server) resolve trades and standards only through it.
//!   This crate ships no built-in engines or rule sets: the front end registers them, so the engines
//!   can depend on this crate without a cycle.
//! * [`wasm::WasmRuleSet`] — the host for WebAssembly components (`wasm32-wasip2`) of the
//!   `rule-set` WIT world (`wit/plugin.wit`), run under wasmtime with no filesystem, environment or
//!   network, linear memory capped at [`wasm::MEMORY_LIMIT`] and fuel plus a wall-clock deadline so
//!   a non-terminating `check` stops within 10 s.

#![forbid(unsafe_code)]

pub mod wasm;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::OnceLock;

use bim_doc::{Document, Entity, EntityId, Value};
use bim_model::Model;

pub use wasm::WasmRuleSet;

/// One rule broken by one entity (spec/plugins.md).
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    /// The rule's id, e.g. `"IBC-1020.2"`.
    pub rule: String,
    /// The offending entity's `_name` (or `"*"` for a model-wide finding).
    pub entity: String,
    /// A human-readable explanation.
    pub message: String,
    /// The measured quantity.
    pub measured: f64,
    /// The limit it was checked against.
    pub limit: f64,
}

/// Why an engine or rule set could not produce a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// The input is unusable: an invalid model for this engine, a file that is not a valid plugin
    /// component, a plugin that does not implement the expected world. Front ends exit 2.
    Input(String),
    /// The engine ran and reported a failure of its own (a trade engine's `err(message)`). Front
    /// ends exit 1.
    Failed(String),
    /// The plugin crashed: a trap, fuel or time exhaustion, a memory failure. Front ends exit 3.
    Plugin(String),
}

impl EngineError {
    /// The message, without the variant.
    pub fn message(&self) -> &str {
        match self {
            EngineError::Input(m) | EngineError::Failed(m) | EngineError::Plugin(m) => m,
        }
    }

    /// The `bim` exit code for this error (spec/cli.md, spec/plugins.md): `Input` → 2,
    /// `Failed` → 1, `Plugin` → 3.
    pub fn exit_code(&self) -> u8 {
        match self {
            EngineError::Input(_) => 2,
            EngineError::Failed(_) => 1,
            EngineError::Plugin(_) => 3,
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for EngineError {}

/// A trade engine (spec/plugins.md): a pure projection of the document. Never mutates it.
pub trait TradeEngine: Send + Sync {
    /// The trade's name: `"arch"`, `"struct"`, `"mep"`, …
    fn name(&self) -> &str;
    /// The trade's result for the document in `ctx`.
    fn solve(&self, ctx: &EngineContext<'_>) -> Result<serde_json::Value, EngineError>;
}

/// A standard's rule set (spec/plugins.md).
pub trait StandardRuleSet: Send + Sync {
    /// The rule set's id: `"ibc"`, `"eurocode"`, …
    fn id(&self) -> &str;
    /// Every violation of the rule set in the document in `ctx`.
    fn check(&self, ctx: &EngineContext<'_>) -> Vec<Violation>;
    /// Like [`check`](StandardRuleSet::check), for rule sets that can fail (a plugin component
    /// that traps); front ends call this one. The default never fails.
    fn try_check(&self, ctx: &EngineContext<'_>) -> Result<Vec<Violation>, EngineError> {
        Ok(self.check(ctx))
    }
}

/// What an engine or rule set sees: the document, read-only, and derived data computed at most
/// once per context.
pub struct EngineContext<'a> {
    doc: &'a Document,
    names: HashMap<EntityId, String>,
    model: OnceLock<Model>,
}

impl<'a> EngineContext<'a> {
    /// A context over `doc`.
    pub fn new(doc: &'a Document) -> EngineContext<'a> {
        EngineContext { doc, names: HashMap::new(), model: OnceLock::new() }
    }

    /// Adds fallback names for entities without a `_name` prop (e.g. the names a journal wrote
    /// them with, which also name deleted Ref targets).
    pub fn with_names(mut self, names: HashMap<EntityId, String>) -> EngineContext<'a> {
        self.names = names;
        self
    }

    /// The document.
    pub fn document(&self) -> &'a Document {
        self.doc
    }

    /// The compiled building geometry ([`Model::compile`]), computed on first use.
    pub fn model(&self) -> &Model {
        self.model.get_or_init(|| Model::compile(self.doc))
    }

    /// The name of `id`: its `_name` prop if it is a live entity that has one, else its fallback
    /// name, else the id itself.
    pub fn name(&self, id: EntityId) -> String {
        if let Some(Value::Text(name)) = self.doc.get(id).and_then(|e| e.props.get("_name")) {
            return name.clone();
        }
        self.names.get(&id).cloned().unwrap_or_else(|| id.to_string())
    }

    /// Every live entity with its name, sorted by name (byte-wise).
    pub fn entities(&self) -> Vec<(String, &'a Entity)> {
        let mut out: Vec<(String, &Entity)> = self.doc.entities().map(|e| (self.name(e.id), e)).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
        out
    }

    /// `value` in the JSON encoding of spec/schema.md: Num → number (non-finite → null), Text →
    /// string, Bool → bool, List → array, Ref → `{"ref": "<target name>"}`.
    pub fn value_json(&self, value: &Value) -> serde_json::Value {
        match value {
            Value::Num(x) => serde_json::Number::from_f64(*x).map_or(serde_json::Value::Null, serde_json::Value::Number),
            Value::Text(s) => serde_json::Value::String(s.clone()),
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Ref(target) => {
                let mut m = serde_json::Map::new();
                m.insert("ref".to_string(), serde_json::Value::String(self.name(*target)));
                serde_json::Value::Object(m)
            }
            Value::List(items) => serde_json::Value::Array(items.iter().map(|v| self.value_json(v)).collect()),
        }
    }

    /// Every prop of `entity` (reserved `_` ones included), in key order, as one JSON object.
    pub fn props_json(&self, entity: &Entity) -> serde_json::Value {
        props_object(self, &entity.props)
    }
}

fn props_object(ctx: &EngineContext<'_>, props: &BTreeMap<String, Value>) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for (key, value) in props {
        m.insert(key.clone(), ctx.value_json(value));
    }
    serde_json::Value::Object(m)
}

/// Trade engines by name and rule sets by id, in separate namespaces.
#[derive(Default)]
pub struct Registry {
    engines: HashMap<String, Box<dyn TradeEngine>>,
    rule_sets: HashMap<String, Box<dyn StandardRuleSet>>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Registers `engine` under its [`name`](TradeEngine::name), replacing any engine of that name.
    pub fn register_engine(&mut self, engine: Box<dyn TradeEngine>) {
        self.engines.insert(engine.name().to_string(), engine);
    }

    /// Registers `rule_set` under its [`id`](StandardRuleSet::id), replacing any rule set of that id.
    pub fn register_rule_set(&mut self, rule_set: Box<dyn StandardRuleSet>) {
        self.rule_sets.insert(rule_set.id().to_string(), rule_set);
    }

    /// The engine named `name`.
    pub fn engine(&self, name: &str) -> Option<&dyn TradeEngine> {
        self.engines.get(name).map(|e| e.as_ref())
    }

    /// The rule set with id `id`.
    pub fn rule_set(&self, id: &str) -> Option<&dyn StandardRuleSet> {
        self.rule_sets.get(id).map(|r| r.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bim_doc::Op;

    struct E(&'static str);
    impl TradeEngine for E {
        fn name(&self) -> &str {
            self.0
        }
        fn solve(&self, ctx: &EngineContext<'_>) -> Result<serde_json::Value, EngineError> {
            Ok(serde_json::json!({ "n": ctx.document().len() }))
        }
    }
    struct R(&'static str);
    impl StandardRuleSet for R {
        fn id(&self) -> &str {
            self.0
        }
        fn check(&self, _: &EngineContext<'_>) -> Vec<Violation> {
            Vec::new()
        }
    }

    #[test]
    fn registry_namespaces_are_separate_and_last_registration_wins() {
        let mut reg = Registry::new();
        reg.register_engine(Box::new(E("same")));
        assert!(reg.rule_set("same").is_none());
        reg.register_rule_set(Box::new(R("same")));
        assert_eq!(reg.engine("same").unwrap().name(), "same");
        assert_eq!(reg.rule_set("same").unwrap().id(), "same");
        let doc = Document::new();
        let ctx = EngineContext::new(&doc);
        assert_eq!(reg.engine("same").unwrap().solve(&ctx).unwrap(), serde_json::json!({"n": 0}));
        assert_eq!(reg.rule_set("same").unwrap().try_check(&ctx).unwrap(), Vec::new());
    }

    #[test]
    fn exit_codes() {
        assert_eq!(EngineError::Input("x".into()).exit_code(), 2);
        assert_eq!(EngineError::Failed("x".into()).exit_code(), 1);
        assert_eq!(EngineError::Plugin("x".into()).exit_code(), 3);
        assert_eq!(EngineError::Plugin("boom".into()).to_string(), "boom");
    }

    #[test]
    fn props_json_encodes_refs_by_name_and_lists_as_arrays() {
        let (l, w, gone) = (EntityId::derive("L1"), EntityId::derive("W1"), EntityId::derive("G"));
        let mut doc = Document::new();
        let set = |id, key: &str, value| Op::Set { id, key: key.to_string(), value };
        doc.apply(Op::Create { id: l, kind: "Level".into() }).unwrap();
        doc.apply(set(l, "_name", Value::Text("L1".into()))).unwrap();
        doc.apply(Op::Create { id: w, kind: "Wall".into() }).unwrap();
        doc.apply(set(w, "_name", Value::Text("W1".into()))).unwrap();
        doc.apply(set(w, "level", Value::Ref(l))).unwrap();
        doc.apply(set(w, "h", Value::Num(3.5))).unwrap();
        doc.apply(set(w, "xs", Value::List(vec![Value::Num(1.0), Value::Text("a".into()), Value::Bool(true)]))).unwrap();
        doc.apply(set(w, "lost", Value::Ref(gone))).unwrap();
        let ctx = EngineContext::new(&doc).with_names(HashMap::from([(gone, "G".to_string())]));
        let names: Vec<String> = ctx.entities().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["L1", "W1"]);
        let wall = doc.get(w).unwrap();
        assert_eq!(
            ctx.props_json(wall),
            serde_json::json!({"_name": "W1", "h": 3.5, "level": {"ref": "L1"}, "lost": {"ref": "G"}, "xs": [1.0, "a", true]})
        );
    }
}
