//! Bound (typed, column-index-resolved) expressions.
use crate::sql::AggFunc;
use crate::types::{Ty, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BExpr {
    Col(usize, Ty),
    Lit(Value, Ty),
    Bin(BinOp, Box<BExpr>, Box<BExpr>, Ty),
    Neg(Box<BExpr>, Ty),
    Not(Box<BExpr>),
    IsNull(Box<BExpr>, bool),
    Cast(Box<BExpr>, Ty),
    /// `x LIKE pattern` (negated if the flag is set); `x` is a string.
    Like(Box<BExpr>, String, bool),
    /// Searched CASE: the first true condition picks its result; none -> `else_` (a NULL literal if absent).
    /// All results already cast to `ty`.
    Case(Vec<(BExpr, BExpr)>, Box<BExpr>, Ty),
}

impl BExpr {
    pub fn ty(&self) -> Ty {
        match self {
            BExpr::Col(_, t)
            | BExpr::Lit(_, t)
            | BExpr::Bin(_, _, _, t)
            | BExpr::Neg(_, t)
            | BExpr::Cast(_, t)
            | BExpr::Case(_, _, t) => *t,
            BExpr::Not(_) | BExpr::IsNull(..) | BExpr::Like(..) => Ty::Bool,
        }
    }

    pub fn cols(&self, out: &mut Vec<usize>) {
        match self {
            BExpr::Col(i, _) => out.push(*i),
            BExpr::Lit(..) => {}
            BExpr::Bin(_, a, b, _) => {
                a.cols(out);
                b.cols(out);
            }
            BExpr::Neg(a, _) | BExpr::Not(a) | BExpr::IsNull(a, _) | BExpr::Cast(a, _) | BExpr::Like(a, ..) => {
                a.cols(out)
            }
            BExpr::Case(w, e, _) => {
                for (c, r) in w {
                    c.cols(out);
                    r.cols(out);
                }
                e.cols(out);
            }
        }
    }

    pub fn shift(&self, by: isize) -> BExpr {
        match self {
            BExpr::Col(i, t) => BExpr::Col((*i as isize + by) as usize, *t),
            BExpr::Lit(..) => self.clone(),
            BExpr::Bin(o, a, b, t) => BExpr::Bin(*o, Box::new(a.shift(by)), Box::new(b.shift(by)), *t),
            BExpr::Neg(a, t) => BExpr::Neg(Box::new(a.shift(by)), *t),
            BExpr::Not(a) => BExpr::Not(Box::new(a.shift(by))),
            BExpr::IsNull(a, n) => BExpr::IsNull(Box::new(a.shift(by)), *n),
            BExpr::Cast(a, t) => BExpr::Cast(Box::new(a.shift(by)), *t),
            BExpr::Like(a, p, n) => BExpr::Like(Box::new(a.shift(by)), p.clone(), *n),
            BExpr::Case(w, e, t) => {
                BExpr::Case(w.iter().map(|(c, r)| (c.shift(by), r.shift(by))).collect(), Box::new(e.shift(by)), *t)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AggCall {
    pub func: AggFunc,
    /// `None` = COUNT(*)
    pub arg: Option<BExpr>,
    pub distinct: bool,
    pub out_ty: Ty,
}
