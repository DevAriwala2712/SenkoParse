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
        BExpr::DecPrec(x, p) => {
            let c = eval(x, b)?;
            check_precision(&c, *p)?;
            c
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
            (ColData::Bool(x), ColData::Bool(y)) => cmp_kernel!(op, x, y, nl),
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

/// Round `x / p` half away from zero (`p` a power of ten; cannot overflow).
#[inline]
fn round_div(x: i64, p: i64) -> i64 {
    let (q, r) = (x / p, x % p);
    if r.abs() >= p - r.abs() {
        q + x.signum()
    } else {
        q
    }
}

fn cast_err(v: impl std::fmt::Display, to: Ty) -> crate::Error {
    err!("cannot CAST {v} to {}", to.name())
}

/// f64 -> i64: round half away from zero (PostgreSQL rounds too; SQLite truncates, a listed dialect difference).
fn f64_to_i64(x: f64) -> Result<i64> {
    let r = x.round();
    // 2^63 is exactly representable; every f64 in [-2^63, 2^63) converts exactly after rounding
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&r) {
        return Err(cast_err(x, Ty::I64));
    }
    Ok(r as i64)
}

/// Decimal text (`[+-]digits[.digits]`, either part may be empty but not both) to a scaled i64, rounding half away
/// from zero at `scale` digits. `None` if malformed or out of range.
fn dec_round(b: &[u8], scale: u8) -> Option<i64> {
    let (neg, b) = match b.first() {
        Some(b'-') => (true, &b[1..]),
        Some(b'+') => (false, &b[1..]),
        _ => (false, b),
    };
    let (ip, fp) = match b.iter().position(|&c| c == b'.') {
        Some(k) => (&b[..k], &b[k + 1..]),
        None => (b, &b[..0]),
    };
    if ip.is_empty() && fp.is_empty() || !ip.iter().chain(fp).all(u8::is_ascii_digit) {
        return None;
    }
    let mut v: i128 = 0;
    for &c in ip {
        v = v.checked_mul(10)?.checked_add((c - b'0') as i128)?;
    }
    for k in 0..scale as usize {
        let d = fp.get(k).map_or(0, |c| c - b'0');
        v = v.checked_mul(10)?.checked_add(d as i128)?;
    }
    if fp.get(scale as usize).is_some_and(|&c| c >= b'5') {
        v += 1;
    }
    i64::try_from(if neg { -v } else { v }).ok()
}

/// f64 -> decimal via its shortest round-trip text, so `0.285` becomes 0.29 at scale 2 (as written), not 0.28
/// (its binary value is 0.28499999...). PostgreSQL converts the same way.
fn f64_to_dec(x: f64, s: u8) -> Result<i64> {
    dec_round(format!("{x}").as_bytes(), s).ok_or_else(|| cast_err(x, Ty::Dec(s)))
}

fn str_to(t: &[u8], to: Ty) -> Result<Value> {
    let s = std::str::from_utf8(t).unwrap_or("").trim();
    let b = s.as_bytes();
    let bad = || cast_err(format!("'{}'", String::from_utf8_lossy(t)), to);
    Ok(match to {
        Ty::I64 => Value::I64(crate::storage::csv::parse_i64(b).ok_or_else(bad)?),
        Ty::F64 => Value::F64(s.parse::<f64>().ok().filter(|x| x.is_finite()).ok_or_else(bad)?),
        Ty::Dec(sc) => {
            let v = match dec_round(b, sc) {
                Some(v) => v,
                // exponent form (`1.5e3`): through f64, like PostgreSQL's numeric input it accepts it
                None => match s.parse::<f64>().ok().filter(|x| x.is_finite() && s.bytes().any(|c| c | 0x20 == b'e')) {
                    Some(x) => f64_to_dec(x, sc).map_err(|_| bad())?,
                    None => return Err(bad()),
                },
            };
            Value::Dec(v, sc)
        }
        Ty::Date => Value::Date(parse_date(b).ok_or_else(bad)?),
        Ty::Bool => Value::Bool(parse_bool(b).ok_or_else(bad)?),
        Ty::Str => Value::Str(s.to_string()),
    })
}

/// One value as text: the CSV output format, except `bool` is `true`/`false` (output writes 1/0 like SQLite).
fn to_text(c: &Column, i: usize, out: &mut Vec<u8>) {
    use std::io::Write;
    match &c.data {
        ColData::Bool(v) => out.extend_from_slice(if v[i] { b"true" } else { b"false" }),
        ColData::I64(v) => {
            let _ = write!(out, "{}", v[i]);
        }
        ColData::F64(v) => {
            let _ = write!(out, "{}", v[i]);
        }
        ColData::Dec(v, s) => fmt_dec(v[i], *s, out),
        ColData::Date(v) => fmt_date(v[i], out),
        ColData::Str(v) => out.extend_from_slice(v.get(i)),
    }
}

/// `CAST(x AS DECIMAL(p, s))`: at most `p` digits in all.
fn check_precision(c: &Column, p: u8) -> Result<()> {
    let ColData::Dec(v, s) = &c.data else { unreachable!("DECIMAL precision check on a non-decimal") };
    let lim = POW10[p as usize];
    for (i, &x) in v.iter().enumerate() {
        if !c.is_null(i) && (x >= lim || x <= -lim) {
            let mut t = Vec::new();
            fmt_dec(x, *s, &mut t);
            return Err(err!("value {} does not fit DECIMAL({p}, {s})", String::from_utf8_lossy(&t)));
        }
    }
    Ok(())
}

/// Every conversion the binder allows (internal widening and user `CAST`). NULL stays NULL; a value that cannot be
/// converted is an error naming it, never a silent NULL or 0 (SQLite gives 0 for `CAST('abc' AS INTEGER)`).
pub fn cast(c: &Column, to: Ty) -> Result<Column> {
    if c.ty() == to {
        return Ok(c.clone());
    }
    // element-wise conversion that skips NULL slots (they hold placeholder values that need not convert)
    fn conv<T: Copy, U: Default>(c: &Column, v: &[T], f: impl Fn(T) -> Result<U>) -> Result<Vec<U>> {
        (0..v.len()).map(|i| if c.is_null(i) { Ok(U::default()) } else { f(v[i]) }).collect()
    }
    if let ColData::Str(s) = &c.data {
        let mut out = Column::empty(to);
        for i in 0..s.len() {
            if c.is_null(i) {
                out.push_null();
            } else {
                out.push_value(&str_to(s.get(i), to)?);
            }
        }
        return Ok(out);
    }
    if to == Ty::Str {
        let mut s = StrCol::default();
        let mut buf = Vec::new();
        for i in 0..c.len() {
            buf.clear();
            if !c.is_null(i) {
                to_text(c, i, &mut buf);
            }
            s.push(&buf);
        }
        return Ok(Column::new(ColData::Str(s), c.nulls.clone()));
    }
    let data = match (&c.data, to) {
        (ColData::I64(v), Ty::Bool) => ColData::Bool(v.iter().map(|&x| x != 0).collect()),
        (ColData::F64(v), Ty::I64) => ColData::I64(conv(c, v, f64_to_i64)?),
        (ColData::F64(v), Ty::Dec(s)) => ColData::Dec(conv(c, v, |x| f64_to_dec(x, s))?, s),
        (ColData::Dec(v, a), Ty::I64) => ColData::I64(v.iter().map(|&x| round_div(x, POW10[*a as usize])).collect()),
        (ColData::Dec(v, a), Ty::Dec(b)) if b < *a => {
            let p = POW10[(a - b) as usize];
            ColData::Dec(v.iter().map(|&x| round_div(x, p)).collect(), b)
        }
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
