//! Symbolic scalar expressions ([`Expr`]) with exact differentiation.

use std::collections::BTreeSet;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::rc::Rc;

#[derive(Debug)]
enum Node {
    Const(f64),
    Var(usize),
    Add(Expr, Expr),
    Sub(Expr, Expr),
    Mul(Expr, Expr),
    Div(Expr, Expr),
    Neg(Expr),
    Sqrt(Expr),
}

/// A symbolic scalar expression with exact differentiation.
///
/// An [`Expr`] is an immutable expression tree over numbered variables
/// (`x0, x1, …`) built from constants, `+ − × ÷`, unary negation and `sqrt`.
/// Subtrees are reference counted, so cloning is cheap and derivatives share
/// structure with the expression they came from.
///
/// Constructors simplify locally: operations on two constants are folded,
/// and the identities `0 + x = x`, `x − 0 = x`, `0 − x = −x`, `0·x = 0`,
/// `1·x = x`, `0/x = 0`, `x/1 = x` and `−(−x) = x` are applied. In particular
/// the derivative with respect to a variable the expression does not use is
/// the constant `0`, which evaluates to exactly `0.0`.
///
/// ```
/// use bim_solver::Expr;
/// let (x, y) = (Expr::var(0), Expr::var(1));
/// let r = (x.clone() * x + y.clone() * y).sqrt();
/// let drdx = r.diff(0);
/// assert_eq!(r.eval(&[3.0, 4.0]), 5.0);
/// assert_eq!(drdx.eval(&[3.0, 4.0]), 0.6);
/// assert_eq!(r.diff(2).eval(&[3.0, 4.0]), 0.0);
/// ```
#[derive(Clone)]
pub struct Expr(Rc<Node>);

impl Expr {
    fn node(n: Node) -> Expr {
        Expr(Rc::new(n))
    }

    /// The variable `x_i`, evaluated as `vars[i]`.
    pub fn var(i: usize) -> Expr {
        Expr::node(Node::Var(i))
    }

    /// A constant.
    pub fn constant(c: f64) -> Expr {
        Expr::node(Node::Const(c))
    }

    /// The constant value of this expression, if it is a constant node.
    pub fn as_constant(&self) -> Option<f64> {
        match *self.0 {
            Node::Const(c) => Some(c),
            _ => None,
        }
    }

    fn is_const(&self, v: f64) -> bool {
        self.as_constant() == Some(v)
    }

    /// Principal square root.
    pub fn sqrt(self) -> Expr {
        match self.as_constant() {
            Some(c) => Expr::constant(c.sqrt()),
            None => Expr::node(Node::Sqrt(self)),
        }
    }

    /// Evaluates the expression with `x_i = vars[i]`.
    ///
    /// # Panics
    /// If the expression uses a variable index `≥ vars.len()`.
    pub fn eval(&self, vars: &[f64]) -> f64 {
        match &*self.0 {
            Node::Const(c) => *c,
            Node::Var(i) => match vars.get(*i) {
                Some(v) => *v,
                None => panic!("Expr::eval: variable x{i} not supplied ({} values)", vars.len()),
            },
            Node::Add(a, b) => a.eval(vars) + b.eval(vars),
            Node::Sub(a, b) => a.eval(vars) - b.eval(vars),
            Node::Mul(a, b) => a.eval(vars) * b.eval(vars),
            Node::Div(a, b) => a.eval(vars) / b.eval(vars),
            Node::Neg(a) => -a.eval(vars),
            Node::Sqrt(a) => a.eval(vars).sqrt(),
        }
    }

    /// Symbolic partial derivative with respect to `x_var`.
    ///
    /// Rules: sum/difference, product `(ab)' = a'b + ab'`, quotient
    /// `(a/b)' = a'/b − a·b'/(b·b)`, `(−a)' = −a'` and `(√a)' = a'/(2√a)`.
    pub fn diff(&self, var: usize) -> Expr {
        match &*self.0 {
            Node::Const(_) => Expr::constant(0.0),
            Node::Var(i) => Expr::constant(if *i == var { 1.0 } else { 0.0 }),
            Node::Add(a, b) => a.diff(var) + b.diff(var),
            Node::Sub(a, b) => a.diff(var) - b.diff(var),
            Node::Mul(a, b) => a.diff(var) * b.clone() + a.clone() * b.diff(var),
            Node::Div(a, b) => {
                let da = a.diff(var);
                let db = b.diff(var);
                da / b.clone() - a.clone() * db / (b.clone() * b.clone())
            }
            Node::Neg(a) => -a.diff(var),
            Node::Sqrt(a) => a.diff(var) / (Expr::constant(2.0) * self.clone()),
        }
    }

    /// The set of variable indices the expression refers to.
    pub fn vars(&self) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        self.collect_vars(&mut out);
        out
    }

    fn collect_vars(&self, out: &mut BTreeSet<usize>) {
        match &*self.0 {
            Node::Const(_) => {}
            Node::Var(i) => {
                out.insert(*i);
            }
            Node::Add(a, b) | Node::Sub(a, b) | Node::Mul(a, b) | Node::Div(a, b) => {
                a.collect_vars(out);
                b.collect_vars(out);
            }
            Node::Neg(a) | Node::Sqrt(a) => a.collect_vars(out),
        }
    }
}

impl fmt::Debug for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.0 {
            Node::Const(c) => write!(f, "{c}"),
            Node::Var(i) => write!(f, "x{i}"),
            Node::Add(a, b) => write!(f, "({a} + {b})"),
            Node::Sub(a, b) => write!(f, "({a} - {b})"),
            Node::Mul(a, b) => write!(f, "({a} * {b})"),
            Node::Div(a, b) => write!(f, "({a} / {b})"),
            Node::Neg(a) => write!(f, "-{a}"),
            Node::Sqrt(a) => write!(f, "sqrt({a})"),
        }
    }
}

impl Add for Expr {
    type Output = Expr;
    fn add(self, rhs: Expr) -> Expr {
        match (self.as_constant(), rhs.as_constant()) {
            (Some(a), Some(b)) => Expr::constant(a + b),
            (Some(a), _) if a == 0.0 => rhs,
            (_, Some(b)) if b == 0.0 => self,
            _ => Expr::node(Node::Add(self, rhs)),
        }
    }
}

impl Sub for Expr {
    type Output = Expr;
    fn sub(self, rhs: Expr) -> Expr {
        match (self.as_constant(), rhs.as_constant()) {
            (Some(a), Some(b)) => Expr::constant(a - b),
            (_, Some(b)) if b == 0.0 => self,
            (Some(a), _) if a == 0.0 => -rhs,
            _ => Expr::node(Node::Sub(self, rhs)),
        }
    }
}

impl Mul for Expr {
    type Output = Expr;
    fn mul(self, rhs: Expr) -> Expr {
        match (self.as_constant(), rhs.as_constant()) {
            (Some(a), Some(b)) => Expr::constant(a * b),
            _ if self.is_const(0.0) || rhs.is_const(0.0) => Expr::constant(0.0),
            _ if self.is_const(1.0) => rhs,
            _ if rhs.is_const(1.0) => self,
            _ if self.is_const(-1.0) => -rhs,
            _ if rhs.is_const(-1.0) => -self,
            _ => Expr::node(Node::Mul(self, rhs)),
        }
    }
}

impl Div for Expr {
    type Output = Expr;
    fn div(self, rhs: Expr) -> Expr {
        match (self.as_constant(), rhs.as_constant()) {
            (Some(a), Some(b)) => Expr::constant(a / b),
            (Some(a), _) if a == 0.0 => Expr::constant(0.0),
            (_, Some(b)) if b == 1.0 => self,
            _ => Expr::node(Node::Div(self, rhs)),
        }
    }
}

impl Neg for Expr {
    type Output = Expr;
    fn neg(self) -> Expr {
        if let Some(c) = self.as_constant() {
            return Expr::constant(-c);
        }
        if let Node::Neg(inner) = &*self.0 {
            return inner.clone();
        }
        Expr::node(Node::Neg(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x() -> Expr {
        Expr::var(0)
    }
    fn y() -> Expr {
        Expr::var(1)
    }

    /// Central finite difference, for cross-checking only.
    fn fd(e: &Expr, at: &[f64], var: usize) -> f64 {
        let h = 1e-6;
        let mut p = at.to_vec();
        let mut m = at.to_vec();
        p[var] += h;
        m[var] -= h;
        (e.eval(&p) - e.eval(&m)) / (2.0 * h)
    }

    #[test]
    fn simplification_folds_constants_and_identities() {
        assert_eq!((Expr::constant(2.0) + Expr::constant(3.0)).as_constant(), Some(5.0));
        assert_eq!((Expr::constant(0.0) * x()).as_constant(), Some(0.0));
        assert_eq!((x() * Expr::constant(0.0)).as_constant(), Some(0.0));
        assert_eq!(format!("{}", x() + Expr::constant(0.0)), "x0");
        assert_eq!(format!("{}", Expr::constant(1.0) * x()), "x0");
        assert_eq!(format!("{}", x() / Expr::constant(1.0)), "x0");
        assert_eq!(format!("{}", Expr::constant(0.0) - x()), "-x0");
        assert_eq!(format!("{}", -(-x())), "x0");
        assert_eq!((Expr::constant(0.0) / x()).as_constant(), Some(0.0));
        assert_eq!(Expr::constant(9.0).sqrt().as_constant(), Some(3.0));
    }

    #[test]
    fn derivative_of_unused_variable_is_the_constant_zero() {
        let e = (x() * x() + Expr::constant(3.0)).sqrt() / (x() - Expr::constant(7.0));
        assert_eq!(e.diff(1).as_constant(), Some(0.0));
        assert_eq!(e.diff(5).eval(&[2.0]), 0.0);
        assert_eq!(e.vars().into_iter().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn derivatives_match_closed_forms_and_finite_differences() {
        let f = (x() * y() + Expr::constant(1.0)).sqrt() / (x() - y()) - -(y() * y());
        let at = [2.3, 0.7];
        for var in 0..2 {
            let d = f.diff(var).eval(&at);
            assert!((d - fd(&f, &at, var)).abs() < 1e-6, "var {var}: {d}");
        }
        let (a, b) = (at[0], at[1]);
        let s = (a * b + 1.0).sqrt();
        let want_dx = (b / (2.0 * s)) / (a - b) - s / ((a - b) * (a - b));
        assert!((f.diff(0).eval(&at) - want_dx).abs() < 1e-14);
        // Second derivatives.
        let g = x() * x() * y();
        assert_eq!(g.diff(0).diff(1).eval(&[3.0, 5.0]), 6.0);
        assert_eq!(g.diff(0).diff(0).eval(&[3.0, 5.0]), 10.0);
    }

    #[test]
    #[should_panic(expected = "variable x3 not supplied")]
    fn eval_with_missing_variable_panics() {
        Expr::var(3).eval(&[1.0]);
    }
}
