// Trade-engine component whose solve traps (wasm `unreachable`).
wit_bindgen::generate!({ world: "trade-engine", path: "../../wit" });

struct Engine;

impl Guest for Engine {
    fn name() -> String {
        "fire".into()
    }

    fn solve(_entities: Vec<Entity>, _rooms: Vec<Room>, _solids: Vec<Solid>) -> Result<Output, String> {
        core::arch::wasm32::unreachable()
    }
}

export!(Engine);
