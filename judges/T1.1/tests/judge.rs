// External judge for T1.1 (bim-doc). Asserts the spec in TASKS.md.
use bim_doc::{Document, EntityId, Op, Value};
use std::collections::HashSet;

fn id(s: &str) -> EntityId {
    EntityId::derive(s)
}

fn create(doc: &mut Document, seed: &str, kind: &str) {
    doc.apply(Op::Create { id: id(seed), kind: kind.into() }).unwrap();
}

fn set(doc: &mut Document, seed: &str, key: &str, value: Value) {
    doc.apply(Op::Set { id: id(seed), key: key.into(), value }).unwrap();
}

#[test]
fn ids_are_deterministic_and_distinct() {
    assert_eq!(id("wall-1"), id("wall-1"));
    let ids: HashSet<EntityId> = (0..10_000).map(|i| id(&format!("e{i}"))).collect();
    assert_eq!(ids.len(), 10_000);
}

#[test]
fn apply_create_set_get() {
    let mut doc = Document::new();
    create(&mut doc, "w", "Wall");
    set(&mut doc, "w", "height", Value::Num(3.0));
    let e = doc.get(id("w")).expect("entity exists");
    assert_eq!(e.kind, "Wall");
    assert_eq!(e.props.get("height"), Some(&Value::Num(3.0)));
    assert_eq!(doc.len(), 1);
}

#[test]
fn invalid_ops_are_rejected_and_not_logged() {
    let mut doc = Document::new();
    assert!(doc.apply(Op::Set { id: id("x"), key: "k".into(), value: Value::Num(1.0) }).is_err());
    assert!(doc.apply(Op::Delete { id: id("x") }).is_err());
    create(&mut doc, "x", "Level");
    assert!(doc.apply(Op::Create { id: id("x"), kind: "Level".into() }).is_err());
    assert_eq!(doc.log(), &[Op::Create { id: id("x"), kind: "Level".into() }]);
}

#[test]
fn delete_removes_entity() {
    let mut doc = Document::new();
    create(&mut doc, "a", "Slab");
    doc.apply(Op::Delete { id: id("a") }).unwrap();
    assert!(doc.get(id("a")).is_none());
    assert_eq!(doc.len(), 0);
    assert_eq!(doc.log().len(), 2);
}

#[test]
fn content_hash_is_order_independent_and_state_based() {
    let mut a = Document::new();
    create(&mut a, "w", "Wall");
    create(&mut a, "l", "Level");
    set(&mut a, "w", "t", Value::Num(0.2));
    set(&mut a, "w", "name", Value::Text("north".into()));

    let mut b = Document::new();
    create(&mut b, "l", "Level");
    create(&mut b, "w", "Wall");
    set(&mut b, "w", "name", Value::Text("north".into()));
    set(&mut b, "w", "t", Value::Num(0.9));
    set(&mut b, "w", "t", Value::Num(0.2));
    assert_eq!(a.content_hash(), b.content_hash());

    let before = a.content_hash();
    set(&mut a, "w", "t", Value::Num(0.25));
    assert_ne!(before, a.content_hash());
    let changed = a.content_hash();
    doc_delete(&mut a, "l");
    assert_ne!(changed, a.content_hash());
}

fn doc_delete(doc: &mut Document, seed: &str) {
    doc.apply(Op::Delete { id: id(seed) }).unwrap();
}

#[test]
fn hash_distinguishes_kind_key_and_value_type() {
    let mut a = Document::new();
    create(&mut a, "e", "Wall");
    let mut b = Document::new();
    create(&mut b, "e", "Door");
    assert_ne!(a.content_hash(), b.content_hash());

    set(&mut a, "e", "k", Value::Num(1.0));
    let mut c = Document::new();
    create(&mut c, "e", "Wall");
    set(&mut c, "e", "j", Value::Num(1.0));
    assert_ne!(a.content_hash(), c.content_hash());

    let mut d = Document::new();
    create(&mut d, "e", "Wall");
    set(&mut d, "e", "k", Value::Text("1".into()));
    assert_ne!(a.content_hash(), d.content_hash());
}

#[test]
fn dangling_refs_are_reported() {
    let mut doc = Document::new();
    create(&mut doc, "door", "Door");
    set(&mut doc, "door", "host", Value::Ref(id("wall")));
    assert_eq!(doc.dangling_refs(), vec![(id("door"), "host".to_string())]);
    create(&mut doc, "wall", "Wall");
    assert!(doc.dangling_refs().is_empty());
    doc_delete(&mut doc, "wall");
    assert_eq!(doc.dangling_refs().len(), 1);
}
