//! Static checks, stratification and bottom-up evaluation.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::syntax::{Atom, Program, Term};
use crate::{builtin_arity, Edb, Mode, QueryError, Row, Val};

/// A term with variables numbered per rule.
#[derive(Debug, Clone)]
enum CTerm {
    Var(usize),
    Const(Val),
    Any,
}

#[derive(Debug, Clone)]
struct CAtom {
    pred: usize,
    args: Vec<CTerm>,
}

/// Where a key column of a body atom gets its value.
#[derive(Debug, Clone)]
enum KeySrc {
    Const(Val),
    Var(usize),
}

/// A positive body atom compiled for the left-to-right join: the columns looked up (with where
/// their values come from), then per remaining column what to do with the tuple's value.
#[derive(Debug, Clone)]
struct Step {
    pred: usize,
    cols: Vec<usize>,
    key: Vec<KeySrc>,
    /// `(column, var, bind)`: bind the var (first occurrence) or check it equals the bound value.
    rest: Vec<(usize, usize, bool)>,
}

/// A negated body atom: a lookup on its non-anonymous columns (all bound by safety).
#[derive(Debug, Clone)]
struct NegStep {
    pred: usize,
    cols: Vec<usize>,
    key: Vec<KeySrc>,
}

#[derive(Debug, Clone)]
struct CRule {
    head: CAtom,
    steps: Vec<Step>,
    negs: Vec<NegStep>,
    nvars: usize,
    /// Positions in `steps` whose predicate is in the rule's own stratum.
    recursive: Vec<usize>,
}

/// A checked program and query, ready for [`run`].
#[derive(Debug, Clone)]
pub struct Checked {
    preds: Vec<String>,
    /// Built-in relations the program or query uses: `(pred id, name)`.
    builtins: Vec<(usize, String)>,
    rules: Vec<CRule>,
    /// Strata in evaluation order: the rule indices of each and its predicates.
    strata: Vec<(Vec<usize>, Vec<usize>)>,
    query: CAtom,
    query_vars: usize,
}

fn program_error(msg: String) -> QueryError {
    QueryError::Program(msg)
}

/// Checks `program` and the query atom `query` (see the crate docs) and compiles them.
pub fn check(program: &Program, query: &Atom) -> Result<Checked, QueryError> {
    // Arities.
    let mut arity: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut note = |a: &Atom, at: String| -> Result<(), QueryError> {
        if let Some(n) = builtin_arity(&a.pred) {
            if a.args.len() != n {
                return Err(program_error(format!(
                    "{at}: built-in relation `{}` has {n} argument(s), used with {}",
                    a.pred,
                    a.args.len()
                )));
            }
            return Ok(());
        }
        match arity.get(a.pred.as_str()) {
            Some((n, first)) if *n != a.args.len() => Err(program_error(format!(
                "{at}: `{}` used with {} argument(s), but with {n} at {first}",
                a.pred,
                a.args.len()
            ))),
            Some(_) => Ok(()),
            None => {
                arity.insert(a.pred.clone(), (a.args.len(), at));
                Ok(())
            }
        }
    };
    for r in &program.rules {
        let at = format!("program line {}", r.line);
        if builtin_arity(&r.head.pred).is_some() {
            return Err(program_error(format!("{at}: `{}` is a built-in relation and cannot be defined", r.head.pred)));
        }
        note(&r.head, at.clone())?;
        for l in &r.body {
            note(&l.atom, at.clone())?;
        }
    }
    note(query, "query".to_string())?;

    // Predicate ids.
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut preds: Vec<String> = Vec::new();
    let mut id_of = |name: &str| -> usize {
        if let Some(&i) = ids.get(name) {
            return i;
        }
        preds.push(name.to_string());
        ids.insert(name.to_string(), preds.len() - 1);
        preds.len() - 1
    };
    let idb: HashSet<&str> = program.rules.iter().map(|r| r.head.pred.as_str()).collect();

    // Compile rules, checking safety.
    let mut rules = Vec::new();
    for r in &program.rules {
        let at = format!("program line {}", r.line);
        let mut vars: HashMap<String, usize> = HashMap::new();
        let var = |name: &str, vars: &mut HashMap<String, usize>| -> usize {
            let n = vars.len();
            *vars.entry(name.to_string()).or_insert(n)
        };
        let mut bound: HashSet<usize> = HashSet::new();
        let mut steps = Vec::new();
        for l in r.body.iter().filter(|l| !l.negated) {
            let mut cols = Vec::new();
            let mut key = Vec::new();
            let mut rest = Vec::new();
            let mut fresh: HashSet<usize> = HashSet::new();
            for (c, t) in l.atom.args.iter().enumerate() {
                match t {
                    Term::Const(v) => {
                        cols.push(c);
                        key.push(KeySrc::Const(v.clone()));
                    }
                    Term::Wildcard => {}
                    Term::Var(name) => {
                        let v = var(name, &mut vars);
                        if bound.contains(&v) {
                            cols.push(c);
                            key.push(KeySrc::Var(v));
                        } else {
                            let first = fresh.insert(v);
                            rest.push((c, v, first));
                        }
                    }
                }
            }
            bound.extend(fresh);
            steps.push(Step { pred: id_of(&l.atom.pred), cols, key, rest });
        }
        let mut negs = Vec::new();
        for l in r.body.iter().filter(|l| l.negated) {
            let mut cols = Vec::new();
            let mut key = Vec::new();
            for (c, t) in l.atom.args.iter().enumerate() {
                match t {
                    Term::Const(v) => {
                        cols.push(c);
                        key.push(KeySrc::Const(v.clone()));
                    }
                    Term::Wildcard => {}
                    Term::Var(name) => {
                        let v = var(name, &mut vars);
                        if !bound.contains(&v) {
                            return Err(program_error(format!(
                                "{at}: unsafe rule: variable `{name}` of `not {}` is not bound by a positive body atom",
                                l.atom.pred
                            )));
                        }
                        cols.push(c);
                        key.push(KeySrc::Var(v));
                    }
                }
            }
            negs.push(NegStep { pred: id_of(&l.atom.pred), cols, key });
        }
        let mut head_args = Vec::new();
        for t in &r.head.args {
            head_args.push(match t {
                Term::Const(v) => CTerm::Const(v.clone()),
                Term::Wildcard => {
                    return Err(program_error(format!("{at}: unsafe rule: `_` in the head of `{}`", r.head.pred)))
                }
                Term::Var(name) => {
                    let v = var(name, &mut vars);
                    if !bound.contains(&v) {
                        return Err(program_error(format!(
                            "{at}: unsafe rule: head variable `{name}` of `{}` is not bound by a positive body atom",
                            r.head.pred
                        )));
                    }
                    CTerm::Var(v)
                }
            });
        }
        let head = CAtom { pred: id_of(&r.head.pred), args: head_args };
        rules.push(CRule { head, steps, negs, nvars: vars.len(), recursive: Vec::new() });
    }

    // The query atom.
    let mut qvars: HashMap<String, usize> = HashMap::new();
    let qargs = query
        .args
        .iter()
        .map(|t| match t {
            Term::Const(v) => CTerm::Const(v.clone()),
            Term::Wildcard => CTerm::Any,
            Term::Var(name) => {
                let n = qvars.len();
                CTerm::Var(*qvars.entry(name.clone()).or_insert(n))
            }
        })
        .collect();
    let qatom = CAtom { pred: id_of(&query.pred), args: qargs };

    let builtins: Vec<(usize, String)> =
        preds.iter().enumerate().filter(|(_, p)| builtin_arity(p).is_some()).map(|(i, p)| (i, p.clone())).collect();

    // Stratification: strongly connected components of the IDB dependency graph (head → body).
    let n = preds.len();
    let is_idb: Vec<bool> = preds.iter().map(|p| idb.contains(p.as_str())).collect();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for r in &rules {
        for p in r.steps.iter().map(|s| s.pred).chain(r.negs.iter().map(|s| s.pred)) {
            if is_idb[p] {
                succ[r.head.pred].push(p);
            }
        }
    }
    let comps = tarjan(&succ, &is_idb);
    let mut comp_of = vec![usize::MAX; n];
    for (k, c) in comps.iter().enumerate() {
        for &p in c {
            comp_of[p] = k;
        }
    }
    for (r, src) in rules.iter().zip(&program.rules) {
        for neg in &r.negs {
            if is_idb[neg.pred] && comp_of[neg.pred] == comp_of[r.head.pred] {
                return Err(program_error(format!(
                    "program line {}: program is not stratifiable: `{}` depends negatively on `{}` through recursion",
                    src.line, preds[r.head.pred], preds[neg.pred]
                )));
            }
        }
    }
    for r in rules.iter_mut() {
        let own = comp_of[r.head.pred];
        r.recursive = (0..r.steps.len()).filter(|&k| is_idb[r.steps[k].pred] && comp_of[r.steps[k].pred] == own).collect();
    }
    let strata = comps
        .into_iter()
        .enumerate()
        .map(|(k, members)| ((0..rules.len()).filter(|&i| comp_of[rules[i].head.pred] == k).collect(), members))
        .collect();

    Ok(Checked { preds, builtins, rules, strata, query: qatom, query_vars: qvars.len() })
}

/// Tarjan's algorithm over the nodes with `include`; components come out dependencies first.
fn tarjan(succ: &[Vec<usize>], include: &[bool]) -> Vec<Vec<usize>> {
    struct State<'a> {
        succ: &'a [Vec<usize>],
        index: Vec<usize>,
        low: Vec<usize>,
        on: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Vec<usize>>,
    }
    fn visit(s: &mut State<'_>, v: usize) {
        s.index[v] = s.next;
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on[v] = true;
        for k in 0..s.succ[v].len() {
            let w = s.succ[v][k];
            if s.index[w] == usize::MAX {
                visit(s, w);
                s.low[v] = s.low[v].min(s.low[w]);
            } else if s.on[w] {
                s.low[v] = s.low[v].min(s.index[w]);
            }
        }
        if s.low[v] == s.index[v] {
            let mut comp = Vec::new();
            loop {
                let w = s.stack.pop().expect("tarjan stack");
                s.on[w] = false;
                comp.push(w);
                if w == v {
                    break;
                }
            }
            comp.sort_unstable();
            s.out.push(comp);
        }
    }
    let n = succ.len();
    let mut s = State {
        succ,
        index: vec![usize::MAX; n],
        low: vec![0; n],
        on: vec![false; n],
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for v in 0..n {
        if include[v] && s.index[v] == usize::MAX {
            visit(&mut s, v);
        }
    }
    s.out
}

/// A relation: its tuples in insertion order and as a set.
#[derive(Default)]
struct Rel {
    rows: Vec<Row>,
    set: HashSet<Row>,
}

impl Rel {
    fn insert(&mut self, row: Row) -> bool {
        if self.set.contains(&row) {
            return false;
        }
        self.set.insert(row.clone());
        self.rows.push(row);
        true
    }
}

/// Which version of a relation a body atom reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Src {
    Full,
    Delta,
}

/// Hash indexes on `(pred, version, key columns)`: key → row positions.
type Indexes = HashMap<(usize, Src, Vec<usize>), HashMap<Row, Vec<usize>>>;

struct Db {
    full: Vec<Rel>,
    delta: Vec<Vec<Row>>,
}

impl Db {
    fn rows(&self, pred: usize, src: Src) -> &[Row] {
        match src {
            Src::Full => &self.full[pred].rows,
            Src::Delta => &self.delta[pred],
        }
    }

    fn build(&self, idx: &mut Indexes, pred: usize, src: Src, cols: &[usize]) {
        let k = (pred, src, cols.to_vec());
        if idx.contains_key(&k) {
            return;
        }
        let mut m: HashMap<Row, Vec<usize>> = HashMap::new();
        for (i, row) in self.rows(pred, src).iter().enumerate() {
            m.entry(cols.iter().map(|&c| row[c].clone()).collect()).or_default().push(i);
        }
        idx.insert(k, m);
    }
}

/// One rule evaluation in a round: the rule and the version each positive atom reads.
struct Job {
    rule: usize,
    srcs: Vec<Src>,
}

fn key_of(key: &[KeySrc], env: &[Option<Val>]) -> Row {
    key.iter()
        .map(|k| match k {
            KeySrc::Const(v) => v.clone(),
            KeySrc::Var(v) => env[*v].clone().expect("bound variable"),
        })
        .collect()
}

fn join(
    rule: &CRule,
    srcs: &[Src],
    k: usize,
    env: &mut Vec<Option<Val>>,
    db: &Db,
    idx: &Indexes,
    out: &mut Vec<Row>,
) {
    if k == rule.steps.len() {
        for neg in &rule.negs {
            let key = key_of(&neg.key, env);
            let hit = match idx.get(&(neg.pred, Src::Full, neg.cols.clone())) {
                Some(m) => m.contains_key(&key),
                None => false,
            };
            if hit {
                return;
            }
        }
        out.push(
            rule.head
                .args
                .iter()
                .map(|t| match t {
                    CTerm::Const(v) => v.clone(),
                    CTerm::Var(v) => env[*v].clone().expect("safe rule"),
                    CTerm::Any => unreachable!("no `_` in heads"),
                })
                .collect(),
        );
        return;
    }
    let step = &rule.steps[k];
    let key = key_of(&step.key, env);
    let Some(m) = idx.get(&(step.pred, srcs[k], step.cols.clone())) else { return };
    let Some(hits) = m.get(&key) else { return };
    let rows = db.rows(step.pred, srcs[k]);
    for &i in hits {
        let row = &rows[i];
        let mut ok = true;
        let mut bound = Vec::new();
        for &(c, v, first) in &step.rest {
            if first {
                env[v] = Some(row[c].clone());
                bound.push(v);
            } else if env[v].as_ref() != Some(&row[c]) {
                ok = false;
                break;
            }
        }
        if ok {
            join(rule, srcs, k + 1, env, db, idx, out);
        }
        for v in bound {
            env[v] = None;
        }
    }
}

/// Runs `jobs` against `db` and returns the derived head tuples (with repeats).
fn round(c: &Checked, db: &Db, jobs: &[Job]) -> Vec<(usize, Row)> {
    let mut idx: Indexes = HashMap::new();
    for job in jobs {
        let r = &c.rules[job.rule];
        for (s, &src) in r.steps.iter().zip(&job.srcs) {
            db.build(&mut idx, s.pred, src, &s.cols);
        }
        for n in &r.negs {
            db.build(&mut idx, n.pred, Src::Full, &n.cols);
        }
    }
    let mut out = Vec::new();
    for job in jobs {
        let r = &c.rules[job.rule];
        let mut rows = Vec::new();
        let mut env = vec![None; r.nvars];
        join(r, &job.srcs, 0, &mut env, db, &idx, &mut rows);
        out.extend(rows.into_iter().map(|row| (r.head.pred, row)));
    }
    out
}

/// Evaluates a checked program over the built-in relations of `edb` and answers its query: one
/// value per query argument, sorted and deduplicated.
pub fn run(c: &Checked, edb: &dyn Edb, mode: Mode) -> Result<Vec<Row>, QueryError> {
    let n = c.preds.len();
    let mut db = Db { full: (0..n).map(|_| Rel::default()).collect(), delta: vec![Vec::new(); n] };
    for (p, name) in &c.builtins {
        for row in edb.relation(name)? {
            db.full[*p].insert(row);
        }
    }
    for (rules, members) in &c.strata {
        let all = |srcs: &dyn Fn(&CRule) -> Vec<Src>| -> Vec<Job> {
            rules.iter().map(|&i| Job { rule: i, srcs: srcs(&c.rules[i]) }).collect()
        };
        let full_jobs = all(&|r: &CRule| vec![Src::Full; r.steps.len()]);
        // First round: every rule over what is known (the stratum's own relations are empty).
        let derived = round(c, &db, &full_jobs);
        let mut fresh = merge(&mut db, derived);
        loop {
            if fresh.iter().all(|(_, rows)| rows.is_empty()) {
                break;
            }
            let jobs = match mode {
                Mode::Naive => all(&|r: &CRule| vec![Src::Full; r.steps.len()]),
                Mode::SemiNaive => {
                    for &p in members {
                        db.delta[p] = fresh.remove(&p).unwrap_or_default();
                    }
                    let mut jobs = Vec::new();
                    for &i in rules {
                        let r = &c.rules[i];
                        for &k in &r.recursive {
                            let mut srcs = vec![Src::Full; r.steps.len()];
                            srcs[k] = Src::Delta;
                            jobs.push(Job { rule: i, srcs });
                        }
                    }
                    jobs
                }
            };
            let derived = round(c, &db, &jobs);
            fresh = merge(&mut db, derived);
        }
        for &p in members {
            db.delta[p].clear();
        }
    }

    // Answer the query.
    let q = &c.query;
    let mut rows: Vec<Row> = Vec::new();
    'tuples: for t in &db.full[q.pred].rows {
        let mut env: Vec<Option<&Val>> = vec![None; c.query_vars];
        for (a, v) in q.args.iter().zip(t) {
            match a {
                CTerm::Const(k) if k != v => continue 'tuples,
                CTerm::Var(x) => match env[*x] {
                    Some(b) if b != v => continue 'tuples,
                    _ => env[*x] = Some(v),
                },
                _ => {}
            }
        }
        rows.push(t.clone());
    }
    rows.sort();
    rows.dedup();
    Ok(rows)
}

/// Adds derived tuples to the full relations; returns the ones that were new, per predicate.
fn merge(db: &mut Db, derived: Vec<(usize, Row)>) -> HashMap<usize, Vec<Row>> {
    let mut fresh: HashMap<usize, Vec<Row>> = HashMap::new();
    for (p, row) in derived {
        if db.full[p].insert(row.clone()) {
            fresh.entry(p).or_default().push(row);
        }
    }
    fresh
}
