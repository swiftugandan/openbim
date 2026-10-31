// Rule-set component whose check traps (wasm `unreachable`).
wit_bindgen::generate!({ world: "rule-set", path: "../../wit" });


struct Plugin;

impl Guest for Plugin {
    fn id() -> String {
        "judge-trap".into()
    }

    fn check(_entities: Vec<Entity>) -> Vec<Violation> {
        core::arch::wasm32::unreachable()
    }
}

export!(Plugin);
