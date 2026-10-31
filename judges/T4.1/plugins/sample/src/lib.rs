// Sample rule-set component: walls taller than 3.0 m violate "H-MAX". It also checks that the host
// passes every live entity (count goes into the message) and encodes props per spec/schema.md.
wit_bindgen::generate!({ world: "rule-set", path: "../../wit" });

use serde_json::Value;

struct Plugin;

fn v(rule: &str, entity: &str, message: String, measured: f64, limit: f64) -> Violation {
    Violation { rule: rule.into(), entity: entity.into(), message, measured, limit }
}

impl Guest for Plugin {
    fn id() -> String {
        "judge-sample".into()
    }

    fn check(entities: Vec<Entity>) -> Vec<Violation> {
        let n = entities.len();
        let mut out = Vec::new();
        for e in &entities {
            let props: Value = match serde_json::from_str(&e.props_json) {
                Ok(Value::Object(m)) => Value::Object(m),
                _ => {
                    out.push(v("BAD-PROPS-JSON", &e.id, e.props_json.clone(), 0.0, 0.0));
                    continue;
                }
            };
            match e.kind.as_str() {
                "Wall" => {
                    if props.get("level") != Some(&serde_json::json!({"ref": "L1"})) {
                        out.push(v("BAD-REF", &e.id, format!("{:?}", props.get("level")), 0.0, 0.0));
                    }
                    match props.get("height").and_then(Value::as_f64) {
                        Some(h) if h > 3.0 => out.push(v("H-MAX", &e.id, format!("n={n}"), h, 3.0)),
                        Some(_) => {}
                        None => out.push(v("BAD-NUM", &e.id, format!("{:?}", props.get("height")), 0.0, 0.0)),
                    }
                }
                "WallType" => {
                    let ok = matches!(props.get("layer_thickness"), Some(Value::Array(a))
                        if a.len() == 1 && a[0].as_f64() == Some(0.2))
                        && matches!(props.get("layer_material"), Some(Value::Array(a))
                        if a.len() == 1 && a[0].as_str() == Some("concrete"));
                    if !ok {
                        out.push(v("BAD-LIST", &e.id, props.to_string(), 0.0, 0.0));
                    }
                }
                _ => {}
            }
        }
        out
    }
}

export!(Plugin);
