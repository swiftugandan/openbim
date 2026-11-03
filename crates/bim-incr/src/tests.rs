use super::*;

/// Input: a number per name.
struct Num;
impl Input for Num {
    type Key = &'static str;
    type Value = i64;
}

/// Input: which names `Sum` adds up.
struct Terms;
impl Input for Terms {
    type Key = ();
    type Value = Vec<&'static str>;
}

/// `Num(key)` doubled.
struct Double;
impl Query for Double {
    type Key = &'static str;
    type Value = i64;
    fn execute(db: &Db, key: &&'static str) -> i64 {
        db.input::<Num>(key).map_or(0, |v| *v * 2)
    }
}

/// The sum of `Double` over `Terms`.
struct Sum;
impl Query for Sum {
    type Key = ();
    type Value = i64;
    fn execute(db: &Db, _: &()) -> i64 {
        let terms = db.input::<Terms>(&()).map(|t| (*t).clone()).unwrap_or_default();
        terms.iter().map(|t| *db.get::<Double>(t)).sum()
    }
}

/// The sign of `Sum` (so `Sum` changes often, `Sign` rarely).
struct Sign;
impl Query for Sign {
    type Key = ();
    type Value = i8;
    fn execute(db: &Db, _: &()) -> i8 {
        db.get::<Sum>(&()).signum() as i8
    }
}

/// `Sign` plus `Num("bias")`.
struct Biased;
impl Query for Biased {
    type Key = ();
    type Value = i64;
    fn execute(db: &Db, _: &()) -> i64 {
        *db.get::<Sign>(&()) as i64 + db.input::<Num>(&"bias").map_or(0, |v| *v)
    }
}

fn names(log: &[Executed]) -> Vec<String> {
    log.iter()
        .map(|e| {
            let short = e.query_name().rsplit("::").next().unwrap_or("").to_string();
            match e.key::<Double>() {
                Some(k) => format!("{short}({k})"),
                None => short,
            }
        })
        .collect()
}

fn setup() -> Db {
    let db = Db::new();
    db.set::<Num>(&"a", Some(1));
    db.set::<Num>(&"b", Some(2));
    db.set::<Terms>(&(), Some(vec!["a", "b"]));
    db
}

#[test]
fn queries_are_memoized_and_dependencies_recorded() {
    let db = setup();
    assert_eq!(*db.get::<Sum>(&()), 6);
    let mut log = names(&db.take_log());
    log.sort();
    assert_eq!(log, ["Double(a)", "Double(b)", "Sum"]);
    // Demanding again at the same revision executes nothing.
    assert_eq!(*db.get::<Sum>(&()), 6);
    assert!(db.take_log().is_empty());
    // Changing `a` re-runs Double(a) and Sum, not Double(b).
    db.set::<Num>(&"a", Some(5));
    assert_eq!(*db.get::<Sum>(&()), 14);
    let mut log = names(&db.take_log());
    log.sort();
    assert_eq!(log, ["Double(a)", "Sum"]);
    // An unrelated input dirties nothing.
    db.set::<Num>(&"zzz", Some(9));
    assert_eq!(*db.get::<Sum>(&()), 14);
    assert!(db.take_log().is_empty());
}

#[test]
fn setting_an_input_to_an_equal_value_changes_nothing() {
    let db = setup();
    db.get::<Biased>(&());
    db.take_log();
    let rev = db.revision();
    db.set::<Num>(&"a", Some(1));
    db.set::<Terms>(&(), Some(vec!["a", "b"]));
    db.set::<Num>(&"never", None);
    assert_eq!(db.revision(), rev, "equal values do not start a revision");
    db.get::<Biased>(&());
    assert!(db.take_log().is_empty());
}

#[test]
fn early_cutoff_stops_propagation() {
    let db = setup();
    assert_eq!(*db.get::<Biased>(&()), 1);
    db.take_log();
    // a: 1 → 3 changes Sum (6 → 10) but not Sign, so Biased is not re-run.
    db.set::<Num>(&"a", Some(3));
    assert_eq!(*db.get::<Biased>(&()), 1);
    let mut log = names(&db.take_log());
    log.sort();
    assert_eq!(log, ["Double(a)", "Sign", "Sum"]);
    // Through a chain: Biased itself is only re-run when Sign changes.
    db.set::<Num>(&"a", Some(-30));
    assert_eq!(*db.get::<Biased>(&()), -1);
    assert!(names(&db.take_log()).contains(&"Biased".to_string()));
    // A re-executed query whose dependencies changed but whose value did not keeps its old
    // revision, so a later demand of a dependent does not re-run it either.
    db.set::<Num>(&"b", Some(4));
    assert_eq!(*db.get::<Sign>(&()), -1);
    db.take_log();
    assert_eq!(*db.get::<Biased>(&()), -1);
    assert!(db.take_log().is_empty());
}

#[test]
fn dependency_sets_follow_each_execution() {
    let db = setup();
    assert_eq!(*db.get::<Sum>(&()), 6);
    db.take_log();
    // Drop `b` from the terms: Sum no longer depends on Double(b).
    db.set::<Terms>(&(), Some(vec!["a"]));
    assert_eq!(*db.get::<Sum>(&()), 2);
    db.take_log();
    db.set::<Num>(&"b", Some(100));
    assert_eq!(*db.get::<Sum>(&()), 2);
    assert!(db.take_log().is_empty(), "a dropped dependency does not dirty the query");
    // Add a term that was never set: the read of the unset input is a dependency too.
    db.set::<Terms>(&(), Some(vec!["a", "c"]));
    assert_eq!(*db.get::<Sum>(&()), 2);
    db.take_log();
    db.set::<Num>(&"c", Some(10));
    assert_eq!(*db.get::<Sum>(&()), 22);
    let mut log = names(&db.take_log());
    log.sort();
    assert_eq!(log, ["Double(c)", "Sum"]);
    // Clearing an input is a change as well.
    db.set::<Num>(&"c", None);
    assert_eq!(*db.get::<Sum>(&()), 2);
    assert!(!db.take_log().is_empty());
}

#[test]
fn stale_dependencies_are_validated_depth_first() {
    // Sign depends on Sum depends on Double(a). After two edits that leave `a` where it was
    // (1 → 7 → 1), demanding Sign validates Double(a) first: the input changed, so Double(a)
    // re-runs, but to an equal value, so neither Sum nor Sign is re-run.
    let db = setup();
    db.get::<Sign>(&());
    db.take_log();
    db.set::<Num>(&"a", Some(7));
    db.set::<Num>(&"a", Some(1));
    db.get::<Sign>(&());
    assert_eq!(names(&db.take_log()), ["Double(a)"]);
    assert!(db.is_memoized::<Sum>(&()));
    // When Double(a) does change (a: 1 → 3), Sum re-runs (6 → 10) and so does Sign, whose
    // value stays 1 (early cutoff for its own dependents is covered above).
    db.set::<Num>(&"a", Some(3));
    assert_eq!(*db.get::<Sign>(&()), 1);
    let mut log = names(&db.take_log());
    log.sort();
    assert_eq!(log, ["Double(a)", "Sign", "Sum"]);
}

struct Loop;
impl Query for Loop {
    type Key = u8;
    type Value = u8;
    fn execute(db: &Db, key: &u8) -> u8 {
        *db.get::<Loop>(&(1 - key))
    }
}

#[test]
#[should_panic(expected = "cycle")]
fn cycles_panic() {
    let db = Db::new();
    db.get::<Loop>(&0);
}
