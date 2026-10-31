// External judge for T4.8 (bim-plugin API): `Metric`, the default `TradeEngine::metrics` and
// `Registry::engine_names` of spec/plugins.md ("Added by T4.8"). The T4.1 API is unchanged.
use bim_plugin::{EngineContext, EngineError, Metric, Registry, StandardRuleSet, TradeEngine, Violation};
use serde_json::{json, Value};

/// An engine that keeps the default `metrics`.
struct Plain(&'static str);

impl TradeEngine for Plain {
    fn name(&self) -> &str {
        self.0
    }
    fn solve(&self, _ctx: &EngineContext) -> Result<Value, EngineError> {
        Ok(json!({ "engine": self.0 }))
    }
}

/// An engine that reports one metric computed from its result.
struct Counting;

impl TradeEngine for Counting {
    fn name(&self) -> &str {
        "count"
    }
    fn solve(&self, _ctx: &EngineContext) -> Result<Value, EngineError> {
        Ok(json!({ "items": [] }))
    }
    fn metrics(&self, result: &Value) -> Vec<Metric> {
        let n = result.get("items").and_then(Value::as_array).map_or(0, Vec::len);
        vec![Metric { key: "items".to_string(), value: n as f64 }]
    }
}

struct Rules(&'static str);

impl StandardRuleSet for Rules {
    fn id(&self) -> &str {
        self.0
    }
    fn check(&self, _ctx: &EngineContext) -> Vec<Violation> {
        Vec::new()
    }
}

#[test]
fn metric_has_the_spec_fields() {
    let m = Metric { key: "total_area".to_string(), value: 12.5 };
    let Metric { key, value } = m;
    assert_eq!((key.as_str(), value), ("total_area", 12.5));
}

#[test]
fn default_metrics_are_empty_and_overrides_are_used() {
    let plain: Box<dyn TradeEngine> = Box::new(Plain("x"));
    assert!(plain.metrics(&json!({ "rooms": [{ "area": 3.0 }] })).is_empty(), "default metrics() returns no metrics");
    assert!(plain.metrics(&Value::Null).is_empty());
    let counting: Box<dyn TradeEngine> = Box::new(Counting);
    let m = counting.metrics(&json!({ "items": [1, 2, 3] }));
    assert_eq!(m.len(), 1);
    assert_eq!((m[0].key.as_str(), m[0].value), ("items", 3.0));
}

#[test]
fn engine_names_lists_every_registered_engine_sorted() {
    let mut reg = Registry::new();
    assert!(reg.engine_names().is_empty(), "a new Registry has no engines");
    for name in ["struct", "fire", "arch", "zeta-2", "mep", "acoustic"] {
        reg.register_engine(Box::new(Plain(name)));
    }
    reg.register_engine(Box::new(Counting));
    reg.register_rule_set(Box::new(Rules("ibc")));
    reg.register_rule_set(Box::new(Rules("beta-pack")));
    assert_eq!(
        reg.engine_names(),
        vec!["acoustic", "arch", "count", "fire", "mep", "struct", "zeta-2"],
        "engine_names(): every registered engine name, sorted (rule sets are a separate namespace)"
    );
    assert_eq!(reg.engine("fire").map(|e| e.name().to_string()).as_deref(), Some("fire"));
    assert_eq!(reg.rule_set("ibc").map(|r| r.id().to_string()).as_deref(), Some("ibc"));
}
