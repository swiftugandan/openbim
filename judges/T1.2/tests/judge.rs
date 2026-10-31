// External judge for T1.2 (bim-crdt): semantic cases plus 10,000 random
// multi-peer edit/merge sequences that must converge without panics.
use bim_crdt::{ConflictDiagnostic, Replica};
use bim_doc::{EntityId, Value};

fn id(s: &str) -> EntityId {
    EntityId::derive(s)
}

fn merged(a: &Replica, b: &Replica) -> Replica {
    let mut m = a.clone();
    m.merge(b);
    m
}

fn state(r: &Replica) -> (u64, Vec<ConflictDiagnostic>) {
    (r.document().content_hash(), r.diagnostics())
}

#[test]
fn last_writer_wins_after_sync() {
    let mut p1 = Replica::new(1);
    p1.create(id("w"), "Wall".into());
    p1.set(id("w"), "t".into(), Value::Num(1.0));
    let mut p2 = Replica::new(2);
    p2.merge(&p1);
    p2.set(id("w"), "t".into(), Value::Num(2.0));
    p1.merge(&p2);
    let doc = p1.document();
    assert_eq!(doc.get(id("w")).unwrap().props.get("t"), Some(&Value::Num(2.0)));
    assert_eq!(state(&p1), state(&p2));
}

#[test]
fn delete_under_a_reference_becomes_a_diagnostic() {
    let mut p1 = Replica::new(1);
    p1.create(id("wall"), "Wall".into());
    p1.create(id("door"), "Door".into());
    p1.set(id("door"), "host".into(), Value::Ref(id("wall")));
    let mut p2 = Replica::new(2);
    p2.merge(&p1);
    p2.delete(id("wall"));
    p1.merge(&p2);
    assert!(p1.document().get(id("wall")).is_none());
    assert_eq!(
        p1.diagnostics(),
        vec![ConflictDiagnostic { entity: id("door"), key: "host".into(), target: id("wall") }]
    );
    // Re-creating with a higher stamp revives the entity and clears the conflict.
    p1.create(id("wall"), "Wall".into());
    p2.merge(&p1);
    assert!(p2.document().get(id("wall")).is_some());
    assert!(p2.diagnostics().is_empty());
}

#[test]
fn props_older_than_a_tombstone_stay_dead_after_revival() {
    let mut p = Replica::new(1);
    p.create(id("s"), "Slab".into());
    p.set(id("s"), "depth".into(), Value::Num(0.3));
    p.delete(id("s"));
    p.create(id("s"), "Slab".into());
    assert!(p.document().get(id("s")).unwrap().props.is_empty());
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_edit(r: &mut Replica, rng: &mut Rng) {
    let seeds = ["a", "b", "c", "d"];
    let target = id(seeds[rng.below(4) as usize]);
    match rng.below(5) {
        0 => r.create(target, ["Wall", "Level"][rng.below(2) as usize].into()),
        1 => r.delete(target),
        2 => r.set(target, "ref".into(), Value::Ref(id(seeds[rng.below(4) as usize]))),
        3 => r.set(target, "x".into(), Value::Num(rng.below(100) as f64)),
        _ => r.set(target, "name".into(), Value::Text(format!("n{}", rng.below(3)))),
    }
}

#[test]
fn ten_thousand_random_sequences_converge() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for case in 0..10_000 {
        let mut peers: Vec<Replica> = (1..=3).map(Replica::new).collect();
        for _ in 0..rng.below(24) + 4 {
            let p = rng.below(3) as usize;
            if rng.below(4) == 0 {
                // Partial sync between two peers while the third stays offline.
                let q = rng.below(3) as usize;
                let other = peers[q].clone();
                peers[p].merge(&other);
            } else {
                random_edit(&mut peers[p], &mut rng);
            }
        }
        let (a, b, c) = (&peers[0], &peers[1], &peers[2]);
        let left = merged(&merged(a, b), c);
        let right = merged(c, &merged(b, a));
        let mixed = merged(&merged(b, c), a);
        assert_eq!(state(&left), state(&right), "case {case}: not commutative/associative");
        assert_eq!(state(&left), state(&mixed), "case {case}: not associative");
        assert_eq!(state(&merged(&left, &left)), state(&left), "case {case}: not idempotent");
        assert_eq!(state(&merged(&left, a)), state(&left), "case {case}: merge not absorbing");
        let doc = left.document();
        let dangling = doc.dangling_refs().len();
        assert_eq!(dangling, left.diagnostics().len(), "case {case}: dangling ref without diagnostic");
    }
}
