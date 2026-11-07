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
        BExpr::Like(x, pat, negate) => {
            let c = eval(x, b)?;
            let ColData::Str(s) = &c.data else { unreachable!() };
            let p = pat.as_bytes();
            let v = (0..b.len).map(|i| !c.is_null(i) && like(s.get(i), p) != *negate).collect();
            Arc::new(Column::new(ColData::Bool(v), c.nulls.clone()))
        }
        BExpr::Case(whens, else_, ty) => Arc::new(case(whens, else_, *ty, b)?),
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
                    if nl.is_some_and(|m| m[i]) {
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
        ColData::Dec(v, s) => {
            ColData::Dec(v.iter().map(|x| x.checked_neg().ok_or_else(ovf)).collect::<Result<_>>()?, *s)
        }
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
        (d, t) => {
            return Err(err!("internal: unsupported cast {:?} -> {}", Column::new(d.clone(), None).ty(), t.name()))
        }
    };
    Ok(Column::new(data, c.nulls.clone()))
}

/// UTF-8 sequence length from its first byte (1 for a stray continuation byte, so progress is guaranteed).
#[inline]
fn char_len(b: u8) -> usize {
    match b {
        0xF0..=0xFF => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

/// SQL LIKE: `%` matches any run of characters (including none), `_` exactly one character (not one byte),
/// everything else itself, case-sensitively (PostgreSQL and DuckDB; SQLite folds ASCII case by default, a
/// dialect difference listed in docs/semantics.md). Greedy matching that backtracks only to the last `%`, which
/// is complete for patterns without character classes: a later `%` subsumes any choice made by an earlier one.
pub fn like(t: &[u8], p: &[u8]) -> bool {
    let (mut ti, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (pattern index of the last %, text index it resumes from)
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'%' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && p[pi] == b'_' {
            ti = (ti + char_len(t[ti])).min(t.len());
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            ti += 1;
            pi += 1;
        } else if let Some((sp, st)) = star {
            let nt = (st + char_len(t[st])).min(t.len());
            star = Some((sp, nt));
            pi = sp + 1;
            ti = nt;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'%' {
        pi += 1;
    }
    pi == p.len()
}

/// Searched CASE. Each condition is evaluated only on the rows no earlier branch took, and each result only on
/// the rows its condition selected, so an expression in a branch that is not taken cannot raise an error (for
/// example an integer overflow), as SQL requires. A NULL condition counts as not true.
fn case(whens: &[(BExpr, BExpr)], else_: &BExpr, ty: Ty, b: &Batch) -> Result<Column> {
    let n = b.len;
    let mut remaining: Vec<u32> = (0..n as u32).collect();
    // per row: (branch, position in that branch's result column)
    let mut pick: Vec<(u32, u32)> = vec![(u32::MAX, 0); n];
    let mut results: Vec<Option<Arc<Column>>> = vec![None; whens.len() + 1];
    let sub_of = |rows: &[u32]| -> Batch {
        if rows.len() == n {
            Batch { cols: b.cols.clone(), len: n }
        } else {
            b.take(rows)
        }
    };
    for (k, (c, r)) in whens.iter().enumerate() {
        if remaining.is_empty() {
            break;
        }
        let sub = sub_of(&remaining);
        let cc = eval(c, &sub)?;
        let ColData::Bool(v) = &cc.data else { unreachable!() };
        let hits: Vec<u32> = (0..remaining.len()).filter(|&j| v[j] && !cc.is_null(j)).map(|j| j as u32).collect();
        if hits.is_empty() {
            continue;
        }
        let rc = eval(r, &sub.take(&hits))?;
        for (pos, &j) in hits.iter().enumerate() {
            pick[remaining[j as usize] as usize] = (k as u32, pos as u32);
        }
        results[k] = Some(rc);
        let mut hit = hits.iter().peekable();
        let mut j = 0u32;
        remaining.retain(|_| {
            let taken = hit.peek().is_some_and(|&&h| h == j);
            if taken {
                hit.next();
            }
            j += 1;
            !taken
        });
    }
    if !remaining.is_empty() {
        let ec = eval(else_, &sub_of(&remaining))?;
        for (pos, &row) in remaining.iter().enumerate() {
            pick[row as usize] = (whens.len() as u32, pos as u32);
        }
        results[whens.len()] = Some(ec);
    }
    let mut out = Column::empty(ty);
    for &(k, pos) in &pick {
        out.push_from(results[k as usize].as_ref().expect("every row picked a branch"), pos as usize);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::like;

    #[test]
    fn like_patterns() {
        let t = |s: &str, p: &str| like(s.as_bytes(), p.as_bytes());
        assert!(t("abc", "abc") && !t("abc", "ab") && !t("ab", "abc"));
        assert!(t("abc", "a%") && t("abc", "%c") && t("abc", "%b%") && t("abc", "%") && t("", "%"));
        assert!(t("abc", "a_c") && !t("ac", "a_c") && t("abc", "___") && !t("abc", "__"));
        assert!(t("aXbXc", "a%b%c") && !t("aXbXd", "a%b%c") && t("abcbc", "%bc") && t("mississippi", "%iss%pi"));
        assert!(!t("ABC", "abc") && !t("", "_") && t("", "") && !t("a", ""));
        // `_` is one character, not one byte
        assert!(t("é", "_") && t("aéb", "a_b") && !t("é", "__") && t("naïve café", "%ï%é"));
    }
}
