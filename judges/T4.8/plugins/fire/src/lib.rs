// Sample trade-engine component "fire" for the T4.8 judge. Its JSON output echoes exactly what the host
// passed (entities with parsed props, rooms, solids, in the order received) so the judge can check the
// host's inputs; its violations flag sprinklers whose coverage_radius is below 2.5 and carry the input
// counts in the message. A fire:Zone named "ERR" makes solve return err(..); one named "BADJSON" makes
// it return output whose json is not valid JSON.
wit_bindgen::generate!({ world: "trade-engine", path: "../../wit" });

use openbim::trade::types::{Metric, Violation};
use serde_json::{json, Value};

struct Engine;

impl Guest for Engine {
    fn name() -> String {
        "fire".into()
    }

    fn solve(entities: Vec<Entity>, rooms: Vec<Room>, solids: Vec<Solid>) -> Result<Output, String> {
        let message = format!("n={} rooms={} solids={}", entities.len(), rooms.len(), solids.len());
        let mut echo = Vec::new();
        let mut violations = Vec::new();
        let mut sprinklers = 0.0;
        let mut bad_json = false;
        for e in &entities {
            let props: Value = match serde_json::from_str(&e.props_json) {
                Ok(v) => v,
                Err(_) => json!({ "unparseable props-json": e.props_json }),
            };
            if e.kind == "fire:Zone" {
                match props.get("name").and_then(Value::as_str) {
                    Some("ERR") => return Err(format!("fire engine refused zone {}", e.id)),
                    Some("BADJSON") => bad_json = true,
                    _ => {}
                }
            }
            if e.kind == "fire:Sprinkler" {
                sprinklers += 1.0;
                if let Some(r) = props.get("coverage_radius").and_then(Value::as_f64) {
                    if r < 2.5 {
                        violations.push(Violation {
                            rule: "FIRE-COVER".into(),
                            entity: e.id.clone(),
                            message: message.clone(),
                            measured: r,
                            limit: 2.5,
                        });
                    }
                }
            }
            echo.push(json!({ "id": e.id, "kind": e.kind, "props": props }));
        }
        let rooms: Vec<Value> = rooms
            .iter()
            .map(|r| json!({ "id": r.id, "level": r.level, "area": r.area, "boundary": r.boundary }))
            .collect();
        let solids: Vec<Value> = solids
            .iter()
            .map(|s| json!({ "id": s.id, "volume": s.volume, "min": s.min, "max": s.max }))
            .collect();
        let doc = json!({ "engine": "fire", "entities": echo, "rooms": rooms, "solids": solids });
        Ok(Output {
            json: if bad_json { format!("{{\"engine\": \"fire\", {}", doc) } else { doc.to_string() },
            metrics: vec![Metric { key: "sprinklers".into(), value: sprinklers }],
            violations,
        })
    }
}

export!(Engine);
