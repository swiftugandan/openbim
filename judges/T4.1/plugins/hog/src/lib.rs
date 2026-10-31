// Rule-set component that touches 1 GiB of linear memory; a memory-bounded host must stop it.
wit_bindgen::generate!({ world: "rule-set", path: "../../wit" });


struct Plugin;

impl Guest for Plugin {
    fn id() -> String {
        "judge-hog".into()
    }

    fn check(_entities: Vec<Entity>) -> Vec<Violation> {
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        for i in 0..16u8 {
            // 16 × 64 MiB = 1 GiB, every page written.
            chunks.push(vec![i.wrapping_add(1); 64 << 20]);
        }
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        vec![Violation {
            rule: "HOG-SURVIVED".into(),
            entity: "*".into(),
            message: format!("allocated {total} bytes"),
            measured: total as f64,
            limit: 0.0,
        }]
    }
}

export!(Plugin);
