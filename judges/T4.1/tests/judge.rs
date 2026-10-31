// External judge for T4.1 (bim-plugin): the traits, Violation and Registry of spec/plugins.md.
use bim_plugin::{EngineContext, EngineError, Registry, StandardRuleSet, TradeEngine, Violation};

struct Dummy(&'static str);

impl TradeEngine for Dummy {
    fn name(&self) -> &str {
        self.0
    }
    fn solve(&self, _ctx: &EngineContext) -> Result<serde_json::Value, EngineError> {
        Ok(serde_json::json!({ "engine": self.0 }))
    }
}

struct Rules(&'static str);

impl StandardRuleSet for Rules {
    fn id(&self) -> &str {
        self.0
    }
    fn check(&self, _ctx: &EngineContext) -> Vec<Violation> {
        vec![Violation {
            rule: format!("{}-1", self.0),
            entity: "W1".to_string(),
            message: "m".to_string(),
            measured: 2.0,
            limit: 1.0,
        }]
    }
}

fn send_sync<T: Send + Sync + ?Sized>() {}

#[test]
fn traits_are_object_safe_send_sync() {
    send_sync::<dyn TradeEngine>();
    send_sync::<dyn StandardRuleSet>();
    let e: Box<dyn TradeEngine> = Box::new(Dummy("x"));
    let r: Box<dyn StandardRuleSet> = Box::new(Rules("y"));
    assert_eq!(e.name(), "x");
    assert_eq!(r.id(), "y");
}

#[test]
fn violation_has_the_spec_fields() {
    let v = Violation { rule: "r".into(), entity: "e".into(), message: "m".into(), measured: 1.5, limit: 2.5 };
    let Violation { rule, entity, message, measured, limit } = v;
    assert_eq!((rule.as_str(), entity.as_str(), message.as_str(), measured, limit), ("r", "e", "m", 1.5, 2.5));
}

#[test]
fn registry_resolves_by_name_and_id() {
    let mut reg = Registry::new();
    assert!(reg.engine("alpha").is_none());
    assert!(reg.rule_set("pack-a").is_none());
    reg.register_engine(Box::new(Dummy("alpha")));
    reg.register_engine(Box::new(Dummy("beta")));
    reg.register_rule_set(Box::new(Rules("pack-a")));
    reg.register_rule_set(Box::new(Rules("pack-b")));
    assert_eq!(reg.engine("alpha").map(|e| e.name().to_string()).as_deref(), Some("alpha"));
    assert_eq!(reg.engine("beta").map(|e| e.name().to_string()).as_deref(), Some("beta"));
    assert_eq!(reg.rule_set("pack-b").map(|r| r.id().to_string()).as_deref(), Some("pack-b"));
    assert!(reg.engine("pack-a").is_none(), "engines and rule sets are separate namespaces");
    assert!(reg.rule_set("alpha").is_none(), "engines and rule sets are separate namespaces");
    assert!(reg.engine("gamma").is_none());
}
