use super::*;

fn q(program: &str, atom: &str, mode: Mode) -> Result<Vec<Row>, QueryError> {
    query(program, atom, &EmptyEdb, mode)
}

fn rows(program: &str, atom: &str) -> Vec<Vec<String>> {
    let a = q(program, atom, Mode::SemiNaive).unwrap();
    let b = q(program, atom, Mode::Naive).unwrap();
    assert_eq!(a, b, "semi-naive and naive disagree");
    a.iter().map(|r| r.iter().map(|v| v.to_string()).collect()).collect()
}

fn program_error(program: &str, atom: &str) -> String {
    match q(program, atom, Mode::SemiNaive) {
        Err(QueryError::Program(m)) => m,
        other => panic!("wanted a program error, got {other:?}"),
    }
}

#[test]
fn parses_rules_facts_negation_comments_and_escapes() {
    let p = parse_program(
        "% comment\nedge(\"a\", 1).  // another\nedge(\"b\\\"c\", -2.5e1).\np(X) :- edge(X, _), not q(X).\nz.\n",
    )
    .unwrap();
    assert_eq!(p.rules.len(), 4);
    assert_eq!(p.rules[0].head.args[1], Term::Const(Val::num(1.0)));
    assert_eq!(p.rules[1].head.args[0], Term::Const(Val::str("b\"c")));
    assert_eq!(p.rules[1].head.args[1], Term::Const(Val::num(-25.0)));
    assert!(p.rules[2].body[1].negated);
    assert_eq!(p.rules[2].body[0].atom.args[1], Term::Wildcard);
    assert!(p.rules[3].head.args.is_empty());
    assert_eq!(parse_atom("path(\"a\", X).").unwrap().args.len(), 2);
}

#[test]
fn syntax_errors_name_the_place() {
    for (text, place) in [
        ("p(X) :- e(X, Y.\n", "program:1:15"),
        ("p(\"a).\n", "program:1:3"),
        ("p(a).\n", "program:1:3"),
        ("p(1) q(2).\n", "program:1:6"),
        ("p(1)\n", "program:2:1"),
        ("p(1x).\n", "program:1:3"),
    ] {
        let m = match parse_program(text) {
            Err(QueryError::Program(m)) => m,
            other => panic!("{text:?}: {other:?}"),
        };
        assert!(m.contains(place) && m.contains("syntax error"), "{text:?}: {m}");
    }
    assert!(parse_atom("p(X) q").is_err());
}

#[test]
fn transitive_closure_three_ways() {
    let facts = "e(\"a\",\"b\"). e(\"b\",\"c\"). e(\"c\",\"a\"). e(\"c\",\"d\"). e(\"a\",\"b\").\n";
    let mut want = Vec::new();
    for x in ["a", "b", "c"] {
        for y in ["a", "b", "c", "d"] {
            want.push(vec![x.to_string(), y.to_string()]);
        }
    }
    for rules in [
        "p(X,Y) :- e(X,Y). p(X,Z) :- p(X,Y), e(Y,Z).",
        "p(X,Y) :- e(X,Y). p(X,Z) :- e(X,Y), p(Y,Z).",
        "p(X,Y) :- e(X,Y). p(X,Z) :- p(X,Y), p(Y,Z).",
    ] {
        assert_eq!(rows(&format!("{facts}{rules}"), "p(X, Y)"), want, "{rules}");
    }
    // Query constants and repeated query variables filter; constants are reported.
    let prog = format!("{facts}p(X,Y) :- e(X,Y). p(X,Z) :- p(X,Y), e(Y,Z).");
    assert_eq!(rows(&prog, "p(\"d\", Y)"), Vec::<Vec<String>>::new());
    assert_eq!(rows(&prog, "p(X, X)").len(), 3);
    assert_eq!(rows(&prog, "p(\"a\", \"d\")"), vec![vec!["a".to_string(), "d".to_string()]]);
    assert_eq!(rows(&prog, "nothing(X)"), Vec::<Vec<String>>::new());
}

#[test]
fn stratified_negation_and_numbers() {
    let prog = "n(1). n(2). n(3.0). big(X) :- n(X), not small(X). small(X) :- n(X), not mid(X). mid(2).";
    // small = {1, 3}; big = {2}.
    assert_eq!(rows(prog, "big(X)"), vec![vec!["2.0".to_string()]]);
    assert_eq!(rows(prog, "small(3)"), vec![vec!["3.0".to_string()]]);
    let r = q("w(\"a\", 3.0). w(\"c\", 3). w(\"d\", -0.0). t(X) :- w(X, 3). z(X) :- w(X, 0).", "t(X)", Mode::Naive)
        .unwrap();
    assert_eq!(r, vec![vec![Val::str("a")], vec![Val::str("c")]]);
    assert_eq!(q("w(\"d\", -0.0). z(X) :- w(X, 0).", "z(X)", Mode::Naive).unwrap().len(), 1);
}

#[test]
fn rows_sorted_numbers_before_strings_and_deduplicated() {
    let r = q("v(\"b\"). v(10). v(9). v(\"a\"). v(9.0). u(X, \"k\") :- v(X). u(X, \"k\") :- v(X).", "u(X, Y)", Mode::SemiNaive)
        .unwrap();
    let shown: Vec<String> = r.iter().map(|r| r[0].to_string()).collect();
    assert_eq!(shown, ["9.0", "10.0", "a", "b"]);
}

#[test]
fn unstratifiable_unsafe_and_inconsistent_programs_are_rejected() {
    let m = program_error("e(1,2). p(X) :- e(X,Y), not q(X). q(X) :- e(X,Y), not p(X).", "p(X)");
    assert!(m.contains("not stratifiable"), "{m}");
    let m = program_error("e(1,2). p(X) :- e(X,Y), not p(Y).", "p(X)");
    assert!(m.contains("not stratifiable"), "{m}");
    assert!(program_error("p(X) :- e(Y).", "p(X)").contains("unsafe"));
    assert!(program_error("p(X).", "p(X)").contains("unsafe"));
    assert!(program_error("p(X) :- e(X), not f(X, Z).", "p(X)").contains("unsafe"));
    assert!(program_error("p(_) :- e(X).", "p(X)").contains("unsafe"));
    assert!(program_error("e(1). e(1, 2).", "e(X)").contains("argument"));
    assert!(program_error("e(1).", "e(X, Y)").contains("argument"));
    assert!(program_error("entity(\"a\", \"b\").", "entity(X, Y)").contains("built-in"));
    assert!(program_error("p(X) :- entity(X).", "p(X)").contains("built-in"));
    // Negation through a lower stratum, and anonymous variables in a negated atom, are fine.
    assert_eq!(rows("e(1,2). e(2,3). s(X) :- e(X,_). t(Y) :- e(_,Y), not s(Y).", "t(Y)"), vec![vec!["3.0"]]);
}

/// Semi-naive and naive evaluation agree on random graphs with recursion and negation.
#[test]
fn semi_naive_matches_naive_on_random_programs() {
    let mut seed: u64 = 0x5eed;
    let mut rnd = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for _ in 0..20 {
        let mut text = String::new();
        for _ in 0..40 {
            text.push_str(&format!("e({}, {}).\n", rnd(25), rnd(25)));
        }
        text.push_str(
            "p(X,Y) :- e(X,Y).\np(X,Z) :- p(X,Y), p(Y,Z).\nq(X,Z) :- e(X,Y), q(Y,Z).\nq(X,Y) :- e(X,Y), not p(Y,X).\n\
             n(X) :- e(X,_).\nn(Y) :- e(_,Y).\nr(X,Y) :- n(X), n(Y), not p(X,Y), not q(Y,X).\n",
        );
        for atom in ["p(X,Y)", "q(X,Y)", "r(X,Y)"] {
            let a = q(&text, atom, Mode::SemiNaive).unwrap();
            let b = q(&text, atom, Mode::Naive).unwrap();
            assert_eq!(a, b, "{atom}");
        }
    }
}
