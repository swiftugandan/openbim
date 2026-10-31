// External judge for T1.6 (bim-doc schema v1): ULID identity, Bool/List values,
// dependency graph and schema version. The CLI half lives in judges/T1.6/cli.py.
use bim_doc::{Document, EntityId, Op, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Closed-form ULID text encoding: 128 bits = 48-bit ms timestamp ‖ 80-bit randomness,
/// written as 26 Crockford base-32 digits, most significant first (ulid/spec).
fn ulid_text(ms: u64, rand: u128) -> String {
    let mut v = ((ms as u128) << 80) | (rand & ((1u128 << 80) - 1));
    let mut out = [0u8; 26];
    for i in (0..26).rev() {
        out[i] = CROCKFORD[(v & 31) as usize];
        v >>= 5;
    }
    String::from_utf8(out.to_vec()).unwrap()
}

fn ulid_fields(s: &str) -> (u64, u128) {
    let mut v: u128 = 0;
    for c in s.bytes() {
        v = (v << 5) | CROCKFORD.iter().position(|&a| a == c).unwrap() as u128;
    }
    ((v >> 80) as u64, v & ((1u128 << 80) - 1))
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

fn id(s: &str) -> EntityId {
    EntityId::derive(s)
}

fn create(doc: &mut Document, seed: &str, kind: &str) {
    doc.apply(Op::Create { id: id(seed), kind: kind.into() }).unwrap();
}

fn set(doc: &mut Document, seed: &str, key: &str, value: Value) {
    doc.apply(Op::Set { id: id(seed), key: key.into(), value }).unwrap();
}

fn r(seed: &str) -> Value {
    Value::Ref(id(seed))
}

fn sorted(mut v: Vec<EntityId>) -> Vec<EntityId> {
    v.sort();
    v.dedup();
    v
}

#[test]
fn schema_version_is_one() {
    assert_eq!(Document::SCHEMA_VERSION as u64, 1);
}

#[test]
fn ulids_parse_to_their_128_bit_value() {
    // The ulid/javascript README example: ulid(1469918176385) = "01ARYZ6S41TSV4RRFFQ69G5FAV".
    let example = "01ARYZ6S41TSV4RRFFQ69G5FAV";
    let (ms, rand) = ulid_fields(example);
    assert_eq!(ms, 1_469_918_176_385, "oracle self-check");
    assert_eq!(ulid_text(ms, rand), example, "oracle self-check");
    assert_eq!(EntityId::parse(example), EntityId::new_ulid(ms, rand));
    assert_ne!(EntityId::parse(example), EntityId::derive(example), "a ULID is used as-is, not hashed as a seed");

    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut seen = BTreeSet::new();
    for _ in 0..2000 {
        let ms = rng.next() & ((1 << 48) - 1);
        let rand = ((rng.next() as u128) << 64 | rng.next() as u128) & ((1u128 << 80) - 1);
        let text = ulid_text(ms, rand);
        let a = EntityId::new_ulid(ms, rand);
        assert_eq!(a, EntityId::new_ulid(ms, rand), "new_ulid must be deterministic");
        assert_eq!(EntityId::parse(&text), a, "parse({text}) != new_ulid({ms}, {rand})");
        assert!(seen.insert(a), "distinct (ms, rand) gave equal ids");
    }
    // Extremes, and time ordering: a later timestamp sorts later whatever the randomness.
    let max = "7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
    assert_eq!(EntityId::parse(max), EntityId::new_ulid((1 << 48) - 1, (1u128 << 80) - 1));
    assert_eq!(EntityId::parse("00000000000000000000000000"), EntityId::new_ulid(0, 0));
    assert!(EntityId::new_ulid(1000, (1u128 << 80) - 1) < EntityId::new_ulid(1001, 0));
    assert!(EntityId::new_ulid(1000, 5) < EntityId::new_ulid(1000, 6));
}

#[test]
fn non_ulid_strings_are_seeds() {
    for s in [
        "wall-1",
        "L1",
        "",
        "01ARZ3NDEKTSV4RRFFQ69G5FA",   // 25 chars
        "01ARZ3NDEKTSV4RRFFQ69G5FAVX", // 27 chars
        "01ARZ3NDEKTSV4RRFFQ69G5FAU",  // U is not a Crockford digit
        "01ARZ3NDEKTSV4RRFF-69G5FAV",  // punctuation
        "80000000000000000000000000",  // exceeds 128 bits: not a valid ULID
    ] {
        assert_eq!(EntityId::parse(s), EntityId::derive(s), "parse({s:?}) must derive from the seed");
    }
}

#[test]
fn bool_and_list_values_are_stored_and_hashed() {
    let mut doc = Document::new();
    create(&mut doc, "i", "Issue");
    let list = Value::List(vec![r("a"), Value::Num(1.0), Value::Text("x".into()), Value::Bool(false), Value::List(vec![r("b")])]);
    set(&mut doc, "i", "entities", list.clone());
    set(&mut doc, "i", "open", Value::Bool(true));
    let e = doc.get(id("i")).unwrap();
    assert_eq!(e.props.get("entities"), Some(&list));
    assert_eq!(e.props.get("open"), Some(&Value::Bool(true)));

    let hash_with = |v: Option<Value>| {
        let mut d = Document::new();
        create(&mut d, "e", "Wall");
        if let Some(v) = v {
            set(&mut d, "e", "k", v);
        }
        d.content_hash()
    };
    let variants = [
        None,
        Some(Value::Bool(true)),
        Some(Value::Bool(false)),
        Some(Value::Num(1.0)),
        Some(Value::Num(0.0)),
        Some(Value::Text("true".into())),
        Some(Value::List(vec![])),
        Some(Value::List(vec![Value::Num(1.0)])),
        Some(Value::List(vec![Value::Num(1.0), Value::Num(2.0)])),
        Some(Value::List(vec![Value::Num(2.0), Value::Num(1.0)])),
        Some(Value::List(vec![Value::List(vec![Value::Num(1.0), Value::Num(2.0)])])),
        Some(Value::List(vec![Value::List(vec![Value::Num(1.0)]), Value::Num(2.0)])),
        Some(Value::List(vec![Value::Bool(true)])),
        Some(Value::List(vec![r("a")])),
        Some(Value::List(vec![r("b")])),
    ];
    let hashes: Vec<u64> = variants.iter().cloned().map(hash_with).collect();
    for i in 0..hashes.len() {
        for j in 0..i {
            assert_ne!(hashes[i], hashes[j], "content_hash collides for {:?} and {:?}", variants[i], variants[j]);
        }
    }
    assert_eq!(hash_with(variants[9].clone()), hashes[9], "content_hash not deterministic");
}

#[test]
fn deps_and_dependents() {
    let mut doc = Document::new();
    for (s, k) in [("L", "Level"), ("T", "WallType"), ("w", "Wall"), ("d", "Opening"), ("i", "Issue"), ("lone", "Level")] {
        create(&mut doc, s, k);
    }
    set(&mut doc, "w", "level", r("L"));
    set(&mut doc, "w", "type", r("T"));
    set(&mut doc, "w", "also_level", r("L"));
    set(&mut doc, "w", "height", Value::Num(3.0));
    set(&mut doc, "d", "host", r("w"));
    set(&mut doc, "i", "entities", Value::List(vec![r("w"), Value::Text("note".into()), Value::List(vec![r("d"), r("ghost")])]));

    assert_eq!(doc.deps(id("w")), sorted(vec![id("L"), id("T")]), "deps(w): sorted, one entry per target");
    assert_eq!(doc.deps(id("d")), vec![id("w")]);
    assert_eq!(doc.deps(id("i")), sorted(vec![id("w"), id("d"), id("ghost")]), "Refs inside (nested) Lists are dependencies");
    assert!(doc.deps(id("L")).is_empty());
    assert!(doc.deps(id("nobody")).is_empty());

    assert_eq!(doc.dependents(id("L")), vec![id("w")]);
    assert_eq!(doc.dependents(id("w")), sorted(vec![id("d"), id("i")]));
    assert_eq!(doc.dependents(id("d")), vec![id("i")]);
    assert_eq!(doc.dependents(id("ghost")), vec![id("i")]);
    assert!(doc.dependents(id("lone")).is_empty());

    // Edges follow edits: replacing and deleting update both directions.
    set(&mut doc, "w", "type", Value::Num(0.0));
    assert_eq!(doc.deps(id("w")), vec![id("L")]);
    assert!(doc.dependents(id("T")).is_empty());
    doc.apply(Op::Delete { id: id("d") }).unwrap();
    assert_eq!(doc.dependents(id("w")), vec![id("i")]);
}

fn check_topo(doc: &Document, ids: &[EntityId], label: &str) {
    let order = match doc.topo_order() {
        Ok(o) => o,
        Err(_) => panic!("{label}: topo_order reported a cycle in an acyclic document"),
    };
    assert_eq!(order.len(), doc.len(), "{label}: topo_order must list every entity once");
    let pos: HashMap<EntityId, usize> = order.iter().enumerate().map(|(i, e)| (*e, i)).collect();
    assert_eq!(pos.len(), order.len(), "{label}: duplicate ids in topo_order");
    for e in ids {
        if doc.get(*e).is_none() {
            continue;
        }
        let pe = *pos.get(e).unwrap_or_else(|| panic!("{label}: live entity missing from topo_order"));
        for t in doc.deps(*e) {
            if let Some(pt) = pos.get(&t) {
                assert!(pt < &pe, "{label}: dependency listed after its dependent");
            }
        }
    }
}

#[test]
fn topo_order_puts_dependencies_first_and_detects_cycles() {
    let mut doc = Document::new();
    for s in ["a", "b", "c", "d"] {
        create(&mut doc, s, "Wall");
    }
    set(&mut doc, "a", "host", r("b"));
    set(&mut doc, "b", "refs", Value::List(vec![r("c"), r("missing")]));
    set(&mut doc, "d", "host", r("a"));
    let ids: Vec<EntityId> = ["a", "b", "c", "d"].iter().map(|s| id(s)).collect();
    check_topo(&doc, &ids, "chain");
    assert_eq!(doc.topo_order().ok().unwrap(), vec![id("c"), id("b"), id("a"), id("d")]);

    set(&mut doc, "c", "back", Value::List(vec![Value::List(vec![r("a")])]));
    assert!(doc.topo_order().is_err(), "a cycle through nested Lists must be reported");
    set(&mut doc, "c", "back", Value::Num(0.0));
    assert!(doc.topo_order().is_ok());
    set(&mut doc, "c", "self", r("c"));
    assert!(doc.topo_order().is_err(), "a self-reference is a cycle");
    doc.apply(Op::Delete { id: id("c") }).unwrap();
    check_topo(&doc, &ids, "after delete");
}

/// Brute-force cycle detection by DFS colouring over live-target edges.
fn has_cycle(edges: &BTreeMap<usize, Vec<usize>>, n: usize) -> bool {
    fn visit(v: usize, edges: &BTreeMap<usize, Vec<usize>>, colour: &mut [u8]) -> bool {
        colour[v] = 1;
        for &w in edges.get(&v).into_iter().flatten() {
            if colour[w] == 1 || (colour[w] == 0 && visit(w, edges, colour)) {
                return true;
            }
        }
        colour[v] = 2;
        false
    }
    let mut colour = vec![0u8; n];
    (0..n).any(|v| colour[v] == 0 && visit(v, edges, &mut colour))
}

#[test]
fn random_graphs_against_brute_force() {
    let mut rng = Rng(0x0DDB_A11C_AFE5_EED5);
    // Large DAG: entity k only references entities with a smaller k, in shuffled-id order.
    let n = 3000;
    let mut doc = Document::new();
    let seeds: Vec<String> = (0..n).map(|k| format!("n{k}")).collect();
    for s in &seeds {
        create(&mut doc, s, "Wall");
    }
    for k in 1..n {
        let a = rng.below(k as u64) as usize;
        let b = rng.below(k as u64) as usize;
        set(&mut doc, &seeds[k], "host", r(&seeds[a]));
        if k % 3 == 0 {
            set(&mut doc, &seeds[k], "list", Value::List(vec![Value::Num(1.0), r(&seeds[b]), r("nowhere")]));
        }
    }
    let ids: Vec<EntityId> = seeds.iter().map(|s| id(s)).collect();
    check_topo(&doc, &ids, "3000-node DAG");
    set(&mut doc, &seeds[0], "host", r(&seeds[n - 1]));
    // Node 0 → n-1 → … → 0 closes a cycle only if n-1 reaches 0; it always does (every k>0 reaches a smaller k).
    assert!(doc.topo_order().is_err(), "back edge closing a long cycle");

    // Small random graphs: Err exactly when brute force finds a cycle.
    let mut cyclic = 0;
    for case in 0..400 {
        let m = 2 + rng.below(7) as usize;
        let mut doc = Document::new();
        let names: Vec<String> = (0..m).map(|k| format!("g{case}_{k}")).collect();
        for s in &names {
            create(&mut doc, s, "Wall");
        }
        let mut edges: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (k, s) in names.iter().enumerate() {
            for key in ["p", "q"] {
                if rng.below(3) == 0 {
                    let t = rng.below(m as u64) as usize;
                    let v = if rng.below(2) == 0 { r(&names[t]) } else { Value::List(vec![r(&names[t])]) };
                    set(&mut doc, s, key, v);
                    edges.entry(k).or_default().push(t);
                }
            }
        }
        // Deleting a node removes its out-edges and makes edges into it dangling (ignored).
        let dead = rng.below(m as u64 + 3) as usize;
        if dead < m {
            doc.apply(Op::Delete { id: id(&names[dead]) }).unwrap();
            edges.remove(&dead);
            for v in edges.values_mut() {
                v.retain(|&t| t != dead);
            }
        }
        let expect = has_cycle(&edges, m);
        cyclic += expect as usize;
        let live: Vec<EntityId> = names.iter().map(|s| id(s)).collect();
        if expect {
            assert!(doc.topo_order().is_err(), "case {case}: cycle not detected");
        } else {
            check_topo(&doc, &live, &format!("case {case}"));
        }
    }
    assert!(cyclic > 50 && cyclic < 350, "oracle sanity: {cyclic} cyclic cases");
}
