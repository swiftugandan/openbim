// Trade-engine component whose solve never returns; only the host's fuel bound can stop it.
wit_bindgen::generate!({ world: "trade-engine", path: "../../wit" });

struct Engine;

impl Guest for Engine {
    fn name() -> String {
        "fire".into()
    }

    fn solve(entities: Vec<Entity>, _rooms: Vec<Room>, _solids: Vec<Solid>) -> Result<Output, String> {
        let mut x: u64 = entities.len() as u64;
        loop {
            x = core::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            if x == 42 && core::hint::black_box(false) {
                break;
            }
        }
        Ok(Output { json: "{}".into(), metrics: Vec::new(), violations: Vec::new() })
    }
}

export!(Engine);
