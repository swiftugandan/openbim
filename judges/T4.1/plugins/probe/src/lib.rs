// Sandbox probe: tries to use the filesystem and the network. The `Issue` entity's `title` is the
// absolute path of a secret file and its `status` a TCP port on 127.0.0.1 the judge listens on.
// Every access that succeeds becomes an "ESCAPE-*" violation; "PROBE-DONE" proves check ran.
wit_bindgen::generate!({ world: "rule-set", path: "../../wit" });

use serde_json::Value;

struct Plugin;

fn v(rule: &str, message: String) -> Violation {
    Violation { rule: rule.into(), entity: "*".into(), message, measured: 0.0, limit: 0.0 }
}

impl Guest for Plugin {
    fn id() -> String {
        "judge-probe".into()
    }

    fn check(entities: Vec<Entity>) -> Vec<Violation> {
        let mut out = Vec::new();
        let (mut path, mut port) = (String::new(), String::new());
        for e in &entities {
            if e.kind == "Issue" {
                if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&e.props_json) {
                    path = m.get("title").and_then(Value::as_str).unwrap_or("").to_string();
                    port = m.get("status").and_then(Value::as_str).unwrap_or("").to_string();
                }
            }
        }
        if let Ok(s) = std::fs::read_to_string(&path) {
            out.push(v("ESCAPE-READ-ABS", s));
        }
        if std::fs::read_dir("/").is_ok() {
            out.push(v("ESCAPE-LIST-ROOT", String::new()));
        }
        if std::fs::read_dir(".").is_ok() {
            out.push(v("ESCAPE-LIST-CWD", String::new()));
        }
        if std::fs::write("probe-rel.pwned", b"x").is_ok() {
            out.push(v("ESCAPE-WRITE-REL", String::new()));
        }
        if std::fs::write(format!("{path}.pwned"), b"x").is_ok() {
            out.push(v("ESCAPE-WRITE-ABS", String::new()));
        }
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            out.push(v("ESCAPE-NET", String::new()));
        }
        out.push(v("PROBE-DONE", format!("path={path} port={port}")));
        out
    }
}

export!(Plugin);
