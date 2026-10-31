// Sample trade-engine component "fire" for the T6.3 judge. It computes two metrics from the entities the
// host passes: `sprinklers` (number of fire:Sprinkler entities) and `coverage_total` (sum of their
// coverage_radius). It returns them in `output.metrics` (which feed TradeEngine::metrics and MCP
// `trades.fire`) and also writes them into `output.json`, so the judge can read what the engine computed
// through `bim solve --trade fire --json`.
wit_bindgen::generate!({ world: "trade-engine", path: "../../wit" });

use openbim::trade::types::Metric;
use serde_json::{json, Value};

struct Engine;

impl Guest for Engine {
    fn name() -> String {
        "fire".into()
    }

    fn solve(entities: Vec<Entity>, _rooms: Vec<Room>, _solids: Vec<Solid>) -> Result<Output, String> {
        let (mut count, mut coverage) = (0.0, 0.0);
        for e in entities.iter().filter(|e| e.kind == "fire:Sprinkler") {
            count += 1.0;
            let props: Value = serde_json::from_str(&e.props_json).map_err(|err| format!("props-json of {}: {err}", e.id))?;
            coverage += props.get("coverage_radius").and_then(Value::as_f64).unwrap_or(0.0);
        }
        Ok(Output {
            json: json!({ "engine": "fire", "metrics": { "sprinklers": count, "coverage_total": coverage } }).to_string(),
            metrics: vec![
                Metric { key: "sprinklers".into(), value: count },
                Metric { key: "coverage_total".into(), value: coverage },
            ],
            violations: Vec::new(),
        })
    }
}

export!(Engine);
