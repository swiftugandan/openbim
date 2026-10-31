// Trade-engine component installed as the "fire" package's engine whose name() is not the package name.
wit_bindgen::generate!({ world: "trade-engine", path: "../../wit" });

struct Engine;

impl Guest for Engine {
    fn name() -> String {
        "sprinkle".into()
    }

    fn solve(_entities: Vec<Entity>, _rooms: Vec<Room>, _solids: Vec<Solid>) -> Result<Output, String> {
        Ok(Output { json: "{}".into(), metrics: Vec::new(), violations: Vec::new() })
    }
}

export!(Engine);
