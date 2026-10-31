// Rule-set component whose check never returns; only the host's fuel bound can stop it.
wit_bindgen::generate!({ world: "rule-set", path: "../../wit" });


struct Plugin;

impl Guest for Plugin {
    fn id() -> String {
        "judge-spin".into()
    }

    fn check(entities: Vec<Entity>) -> Vec<Violation> {
        let mut x: u64 = entities.len() as u64;
        loop {
            x = core::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            if x == 42 && core::hint::black_box(false) {
                break;
            }
        }
        Vec::new()
    }
}

export!(Plugin);
