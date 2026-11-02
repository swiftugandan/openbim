//! The materializer against `bim_crdt::Replica` on random histories, and the merge laws.
//!
//! Each peer is driven twice: through a `Replica` (the reference CRDT) and through an `OpLog` fed
//! with the very stamps the replica used. Merges are applied to both. At every step the log's
//! document must equal the replica's, and at the end every order of every subset of delivery must
//! give the same document.

use bim_crdt::Replica;
use bim_doc::{Document, EntityId, Value};
use bim_sync::{materialize_ops, same_state, OpLog, Stamp, StampedOp, SyncOp, SyncValue};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // PCG-style LCG; the high bits are the good ones.
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const NAMES: [&str; 6] = ["a", "b", "c", "d", "e", "f"];
const KINDS: [&str; 3] = ["Wall", "Slab", "Door"];
const KEYS: [&str; 4] = ["h", "w", "r", "label"];

fn value(rng: &mut Rng, depth: usize) -> SyncValue {
    match rng.below(if depth < 2 { 6 } else { 5 }) {
        0 => SyncValue::Num(f64::from(rng.below(5) as u32) * 0.5 - 1.0),
        1 => SyncValue::Text(rng.pick(&["x", "y z", ""]).to_string()),
        2 | 3 => SyncValue::Ref(rng.pick(&NAMES).to_string()),
        4 => SyncValue::Bool(rng.below(2) == 0),
        _ => SyncValue::List((0..rng.below(4)).map(|_| value(rng, depth + 1)).collect()),
    }
}

fn random_op(rng: &mut Rng) -> SyncOp {
    let name = rng.pick(&NAMES).to_string();
    match rng.below(10) {
        0..=2 => SyncOp::Create { name, kind: rng.pick(&KINDS).to_string() },
        3..=7 => SyncOp::Set { name, key: rng.pick(&KEYS).to_string(), value: value(rng, 0) },
        _ => SyncOp::Delete { name },
    }
}

struct Peer {
    id: u32,
    replica: Replica,
    log: OpLog,
}

impl Peer {
    fn edit(&mut self, op: SyncOp) -> StampedOp {
        let id = op.id();
        match &op {
            SyncOp::Create { kind, .. } => self.replica.create(id, kind.clone()),
            SyncOp::Set { key, value, .. } => self.replica.set(id, key.clone(), value.to_value()),
            SyncOp::Delete { .. } => self.replica.delete(id),
        }
        let stamped = StampedOp { stamp: Stamp { counter: self.replica.clock(), peer: self.id }, op };
        assert!(self.log.insert(stamped.clone()).unwrap());
        stamped
    }

    fn agrees(&self) -> bool {
        self.replica.clock() == self.log.clock() && same_state(&self.log.materialize().doc, &self.replica.document())
    }
}

fn run(seed: u64, steps: usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut peers: Vec<Peer> =
        [7_u32, 0x8000_0001, 3].iter().map(|&id| Peer { id, replica: Replica::new(id), log: OpLog::new() }).collect();
    let mut everything: Vec<StampedOp> = Vec::new();
    for step in 0..steps {
        let who = rng.below(peers.len());
        match rng.below(10) {
            0..=6 => {
                let op = random_op(&mut rng);
                everything.push(peers[who].edit(op));
            }
            _ => {
                let from = rng.below(peers.len());
                let source_replica = peers[from].replica.clone();
                let source_log = peers[from].log.clone();
                let target = &mut peers[who];
                target.replica.merge(&source_replica);
                if rng.below(2) == 0 {
                    // the whole log
                    target.log.insert_all(source_log.to_vec()).unwrap();
                } else {
                    // only what the target's version vector lacks: the delta protocol
                    let delta = source_log.missing_for(&target.log.vv());
                    target.log.insert_all(delta).unwrap();
                }
            }
        }
        assert!(peers[who].agrees(), "seed {seed}, step {step}: the log and the replica disagree");
    }
    // Everyone ends up with everything, whichever way it travelled.
    for _ in 0..2 {
        for to in 0..peers.len() {
            for from in 0..peers.len() {
                let (replica, log) = (peers[from].replica.clone(), peers[from].log.clone());
                peers[to].replica.merge(&replica);
                let delta = log.missing_for(&peers[to].log.vv());
                peers[to].log.insert_all(delta).unwrap();
            }
        }
    }
    let reference: Document = peers[0].replica.document();
    let mut all = OpLog::new();
    all.insert_all(everything.clone()).unwrap();
    for peer in &peers {
        assert!(peer.agrees(), "seed {seed}: converged peers disagree with their replicas");
        assert_eq!(peer.log.to_vec(), all.to_vec(), "seed {seed}: a peer is missing edits");
        assert!(same_state(&peer.replica.document(), &reference));
        // Replica::diagnostics predates Lists: it sees Refs that are the whole prop value only.
        let doc = peer.log.materialize().doc;
        let direct = doc
            .dangling_refs()
            .into_iter()
            .filter(|(id, key)| matches!(doc.get(*id).and_then(|e| e.props.get(key)), Some(Value::Ref(_))))
            .count();
        assert_eq!(peer.replica.diagnostics().len(), direct, "seed {seed}");
    }
    // Order-insensitive and idempotent: any delivery order, with repeats, gives the same document.
    for _ in 0..8 {
        let mut shuffled = everything.clone();
        for i in (1..shuffled.len()).rev() {
            let j = rng.below(i + 1);
            shuffled.swap(i, j);
        }
        let extra: Vec<StampedOp> = (0..shuffled.len() / 3).map(|_| rng.pick(&everything).clone()).collect();
        shuffled.extend(extra);
        let m = materialize_ops(shuffled.iter().map(|o| (o.stamp, &o.op)));
        assert!(same_state(&m.doc, &reference), "seed {seed}: the fold depends on delivery order");
        assert_eq!(m.doc.content_hash(), reference.content_hash());
        assert_eq!(m.names, all.materialize().names);
    }
}

#[test]
fn the_log_materializes_exactly_like_a_replica_on_random_histories() {
    for seed in 0..60 {
        run(seed, 250);
    }
}

#[test]
fn long_histories_stay_equivalent() {
    run(9_999, 2_500);
}

#[test]
fn merging_by_delta_is_commutative_associative_and_idempotent() {
    let mut rng = Rng(77);
    for round in 0..30 {
        let mut logs: Vec<OpLog> = Vec::new();
        for (i, peer) in [1_u32, 2, 3].into_iter().enumerate() {
            let mut log = OpLog::new();
            let mut ops = Vec::new();
            for _ in 0..(10 + rng.below(30)) {
                ops.push(random_op(&mut rng));
            }
            // Different clocks per peer, so counters collide across peers (as concurrent edits do).
            let start = (i as u64) * 3;
            let stamped: Vec<StampedOp> = ops
                .into_iter()
                .enumerate()
                .map(|(k, op)| StampedOp { stamp: Stamp { counter: start + k as u64 + 1, peer }, op })
                .collect();
            log.insert_all(stamped).unwrap();
            logs.push(log);
        }
        let join = |a: &OpLog, b: &OpLog| {
            let mut out = a.clone();
            out.insert_all(b.missing_for(&a.vv())).unwrap();
            out
        };
        let (a, b, c) = (&logs[0], &logs[1], &logs[2]);
        let (ab, ba) = (join(a, b), join(b, a));
        assert_eq!(ab.to_vec(), ba.to_vec(), "round {round}: commutative");
        assert_eq!(join(&join(a, b), c).to_vec(), join(a, &join(b, c)).to_vec(), "round {round}: associative");
        assert_eq!(join(&ab, &ab).to_vec(), ab.to_vec(), "round {round}: idempotent");
        assert_eq!(join(&ab, a).to_vec(), ab.to_vec(), "round {round}: absorbs its inputs");
        let (x, y) = (ab.materialize().doc, ba.materialize().doc);
        assert!(same_state(&x, &y));
        assert_eq!(x.content_hash(), y.content_hash());
    }
}

#[test]
fn a_replicas_diagnostics_are_the_materialized_documents_dangling_refs() {
    // The scenario of the sync judge, at the library level: a delete under a reference.
    let mut a = Peer { id: 1, replica: Replica::new(1), log: OpLog::new() };
    let mut b = Peer { id: 2, replica: Replica::new(2), log: OpLog::new() };
    a.edit(SyncOp::Create { name: "L1".into(), kind: "Level".into() });
    a.edit(SyncOp::Create { name: "W1".into(), kind: "Wall".into() });
    a.edit(SyncOp::Set { name: "W1".into(), key: "level".into(), value: SyncValue::Ref("L1".into()) });
    b.replica.merge(&a.replica);
    b.log.insert_all(a.log.to_vec()).unwrap();
    a.edit(SyncOp::Delete { name: "L1".into() });
    b.edit(SyncOp::Create { name: "W2".into(), kind: "Wall".into() });
    b.edit(SyncOp::Set { name: "W2".into(), key: "level".into(), value: SyncValue::Ref("L1".into()) });
    let (ra, rb) = (a.replica.clone(), b.replica.clone());
    a.replica.merge(&rb);
    b.replica.merge(&ra);
    a.log.insert_all(b.log.to_vec()).unwrap();
    b.log.insert_all(a.log.to_vec()).unwrap();
    assert!(a.agrees() && b.agrees());
    let doc = a.log.materialize().doc;
    assert!(doc.get(EntityId::derive("L1")).is_none());
    assert_eq!(doc.dangling_refs().len(), 2);
    assert_eq!(a.replica.diagnostics(), b.replica.diagnostics());
    assert_eq!(doc.get(EntityId::derive("W2")).unwrap().props["level"], Value::Ref(EntityId::derive("L1")));
}
