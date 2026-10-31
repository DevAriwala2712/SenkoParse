//! Vectorized expression evaluation over a batch. Operands arrive already type-unified by the
//! binder, so every kernel is monomorphic. NULL propagates (three-valued logic for AND/OR/NOT).
use crate::plan::{BExpr, BinOp};
use crate::types::*;
use crate::{err, Result};
use std::sync::Arc;

fn ovf() -> crate::Error {
    err!("integer overflow in arithmetic expression")
}

pub fn eval(e: &BExpr, b: &Batch) -> Result<Arc<Column>> {
    Ok(match e {
        BExpr::Col(i, _) => b.cols[*i].clone(),
        BExpr::Lit(v, t) => Arc::new(Column::constant(v, *t, b.len)),
        BExpr::Cast(x, to) => Arc::new(cast(&*eval(x, b)?, *to)?),
        BExpr::Neg(x, _) => Arc::new(neg(&*eval(x, b)?)?),
        BExpr::Not(x) => {
            let c = eval(x, b)?;
            let ColData::Bool(v) = &c.data else { unreachable!() };
            Arc::new(Column::new(ColData::Bool(v.iter().map(|x| !x).collect()), c.nulls.clone()))
        }
        BExpr::IsNull(x, negate) => {
            let c = eval(x, b)?;
            let v = (0..b.len).map(|i| c.is_null(i) != *negate).collect();
            Arc::new(Column::new(ColData::Bool(v), None))
        }
        BExpr::Bin(op, l, r, _) => {
            let (a, c) = (eval(l, b)?, eval(r, b)?);
            Arc::new(binary(*op, &a, &c)?)
        }
    })
}

fn merge_nulls(a: &Option<Vec<bool>>, b: &Option<Vec<bool>>) -> Option<Vec<bool>> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (Some(x), Some(y)) => Some(x.iter().zip(y).map(|(p, q)| *p || *q).collect()),
    }
}

#[inline]
fn zip<T: Copy, U: Default>(x: &[T], y: &[T], nl: Option<&[bool]>, f: impl Fn(T, T) -> Result<U>) -> Result<Vec<U>> {
    let mut out = Vec::with_capacity(x.len());
    match nl {
        None => {
            for i in 0..x.len() {
                out.push(f(x[i], y[i])?);
            }
        }
        Some(n) => {
            for i in 0..x.len() {
                out.push(if n[i] { U::default() } else { f(x[i], y[i])? });
            }
        }
    }
    Ok(out)
}

macro_rules! cmp_kernel {
    ($op:expr, $x:expr, $y:expr, $nl:expr) => {
        ColData::Bool(match $op {
            BinOp::Eq => zip($x, $y, $nl, |p, q| Ok(p == q))?,
            BinOp::Ne => zip($x, $y, $nl, |p, q| Ok(p != q))?,
            BinOp::Lt => zip($x, $y, $nl, |p, q| Ok(p < q))?,
            BinOp::Le => zip($x, $y, $nl, |p, q| Ok(p <= q))?,
            BinOp::Gt => zip($x, $y, $nl, |p, q| Ok(p > q))?,
            BinOp::Ge => zip($x, $y, $nl, |p, q| Ok(p >= q))?,
            _ => unreachable!(),
        })
    };
}

fn binary(op: BinOp, a: &Column, b: &Column) -> Result<Column> {
    if matches!(op, BinOp::And | BinOp::Or) {
        return logic(op, a, b);
    }
    let mut nulls = merge_nulls(&a.nulls, &b.nulls);
    let data = {
        let nl = nulls.as_deref();
        match (&a.data, &b.data) {
            (ColData::I64(x), ColData::I64(y)) => match op {
                BinOp::Add => ColData::I64(zip(x, y, nl, |p, q| p.checked_add(q).ok_or_else(ovf))?),
                BinOp::Sub => ColData::I64(zip(x, y, nl, |p, q| p.checked_sub(q).ok_or_else(ovf))?),
                BinOp::Mul => ColData::I64(zip(x, y, nl, |p, q| p.checked_mul(q).ok_or_else(ovf))?),
                _ => cmp_kernel!(op, x, y, nl),
            },
            (ColData::Dec(x, sa), ColData::Dec(y, sb)) => match op {
                BinOp::Add => ColData::Dec(zip(x, y, nl, |p, q| p.checked_add(q).ok_or_else(ovf))?, *sa),
                BinOp::Sub => ColData::Dec(zip(x, y, nl, |p, q| p.checked_sub(q).ok_or_else(ovf))?, *sa),
                BinOp::Mul => ColData::Dec(zip(x, y, nl, |p, q| p.checked_mul(q).ok_or_else(ovf))?, sa + sb),
                _ => cmp_kernel!(op, x, y, nl),
            },
            (ColData::F64(x), ColData::F64(y)) => match op {
                BinOp::Add => ColData::F64(zip(x, y, nl, |p, q| Ok(p + q))?),
                BinOp::Sub => ColData::F64(zip(x, y, nl, |p, q| Ok(p - q))?),
                BinOp::Mul => ColData::F64(zip(x, y, nl, |p, q| Ok(p * q))?),
                BinOp::Div => {
                    // x / 0 is NULL
                    let mut n = nulls.take().unwrap_or_else(|| vec![false; x.len()]);
                    let out: Vec<f64> = (0..x.len())
                        .map(|i| {
                            if n[i] {
                                0.0
                            } else if y[i] == 0.0 {
                                n[i] = true;
                                0.0
                            } else {
                                let r = x[i] / y[i];
                                if r.is_finite() {
                                    r
                                } else {
                                    n[i] = true;
                                    0.0
                                }
                            }
                        })
                        .collect();
                    nulls = Some(n);
                    ColData::F64(out)
                }
                _ => cmp_kernel!(op, x, y, nl),
            },
            (ColData::Date(x), ColData::Date(y)) => cmp_kernel!(op, x, y, nl),
            (ColData::Str(x), ColData::Str(y)) => {
                let n = x.len();
                let f = |i: usize| x.get(i).cmp(y.get(i));
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    if nl.map_or(false, |m| m[i]) {
                        out.push(false);
                        continue;
                    }
                    let o = f(i);
                    out.push(match op {
                        BinOp::Eq => o.is_eq(),
                        BinOp::Ne => o.is_ne(),
                        BinOp::Lt => o.is_lt(),
                        BinOp::Le => o.is_le(),
                        BinOp::Gt => o.is_gt(),
                        BinOp::Ge => o.is_ge(),
                        _ => return Err(err!("operator not defined for strings")),
                    });
                }
                ColData::Bool(out)
            }
            _ => return Err(err!("internal: operand type mismatch in {op:?}")),
        }
    };
    Ok(Column::new(data, nulls))
}

/// SQL three-valued AND/OR.
fn logic(op: BinOp, a: &Column, b: &Column) -> Result<Column> {
    let (ColData::Bool(x), ColData::Bool(y)) = (&a.data, &b.data) else { unreachable!() };
    let n = x.len();
    let mut val = Vec::with_capacity(n);
    let mut nul = Vec::with_capacity(n);
    let mut any = false;
    for i in 0..n {
        let (an, bn) = (a.is_null(i), b.is_null(i));
        let (av, bv) = (x[i] && !an, y[i] && !bn); // "definitely true"
        let (af, bf) = (!x[i] && !an, !y[i] && !bn); // "definitely false"
        let (v, u) = if op == BinOp::And {
            if af || bf {
                (false, false)
            } else if an || bn {
                (false, true)
            } else {
                (av && bv, false)
            }
        } else if av || bv {
            (true, false)
        } else if an || bn {
            (false, true)
        } else {
            (false, false)
        };
        any |= u;
        val.push(v);
        nul.push(u);
    }
    Ok(Column::new(ColData::Bool(val), any.then_some(nul)))
}

fn neg(c: &Column) -> Result<Column> {
    let data = match &c.data {
        ColData::I64(v) => ColData::I64(v.iter().map(|x| x.checked_neg().ok_or_else(ovf)).collect::<Result<_>>()?),
        ColData::Dec(v, s) => ColData::Dec(v.iter().map(|x| x.checked_neg().ok_or_else(ovf)).collect::<Result<_>>()?, *s),
        ColData::F64(v) => ColData::F64(v.iter().map(|x| -x).collect()),
        _ => unreachable!(),
    };
    Ok(Column::new(data, c.nulls.clone()))
}

pub fn cast(c: &Column, to: Ty) -> Result<Column> {
    if c.ty() == to {
        return Ok(c.clone());
    }
    let data = match (&c.data, to) {
        (ColData::Bool(v), Ty::I64) => ColData::I64(v.iter().map(|&b| b as i64).collect()),
        (ColData::I64(v), Ty::F64) => ColData::F64(v.iter().map(|&x| x as f64).collect()),
        (ColData::I64(v), Ty::Dec(s)) => {
            let p = POW10[s as usize];
            ColData::Dec(v.iter().map(|&x| x.checked_mul(p).ok_or_else(ovf)).collect::<Result<_>>()?, s)
        }
        (ColData::Dec(v, a), Ty::F64) => {
            let p = POW10[*a as usize] as f64;
            ColData::F64(v.iter().map(|&x| x as f64 / p).collect())
        }
        (ColData::Dec(v, a), Ty::Dec(b)) if b >= *a => {
            let p = POW10[(b - a) as usize];
            ColData::Dec(v.iter().map(|&x| x.checked_mul(p).ok_or_else(ovf)).collect::<Result<_>>()?, b)
        }
        (d, t) => return Err(err!("internal: unsupported cast {:?} -> {}", Column::new(d.clone(), None).ty(), t.name())),
    };
    Ok(Column::new(data, c.nulls.clone()))
}
