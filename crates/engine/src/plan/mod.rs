//! Logical plan and binder: resolves names/types, inserts casts, extracts equi-join keys,
//! prunes scans to referenced columns (projection pushdown), and rewrites post-aggregate
//! expressions onto the aggregate's output.
pub mod expr;

use crate::sql::*;
use crate::storage::csv::{parse_dec, parse_i64};
use crate::storage::{Catalog, TableDef};
use crate::types::*;
use crate::{err, Result};
pub use expr::*;

#[derive(Clone, Debug)]
pub struct Field {
    pub qual: Option<String>,
    pub name: String,
    pub ty: Ty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
}

#[derive(Clone)]
pub struct Plan {
    pub node: Node,
    pub schema: Vec<Field>,
    /// Estimated input size in bytes (file size for scans); drives hash-join build-side choice.
    pub est: u64,
}

#[derive(Clone)]
pub enum Node {
    Scan { table: TableDef, cols: Vec<usize> },
    Filter { input: Box<Plan>, pred: BExpr },
    Project { input: Box<Plan>, exprs: Vec<BExpr> },
    Aggregate { input: Box<Plan>, keys: Vec<BExpr>, aggs: Vec<AggCall> },
    Join { left: Box<Plan>, right: Box<Plan>, kind: JoinType, lkeys: Vec<BExpr>, rkeys: Vec<BExpr>, build_left: bool },
    Sort { input: Box<Plan>, keys: Vec<(usize, bool)> },
    Limit { input: Box<Plan>, n: u64 },
}

// ------------------------------------------------------------------ AST helpers

fn collect_refs(a: &Ast, out: &mut Vec<(Option<String>, String)>) {
    match a {
        Ast::Col { q, name } => out.push((q.clone(), name.clone())),
        Ast::Num(_) | Ast::Str(_) | Ast::Null => {}
        Ast::Bin(_, l, r) => {
            collect_refs(l, out);
            collect_refs(r, out);
        }
        Ast::Neg(x) | Ast::Not(x) | Ast::IsNull(x, _) => collect_refs(x, out),
        Ast::Agg { arg, .. } => {
            if let Some(x) = arg {
                collect_refs(x, out)
            }
        }
    }
}

fn contains_agg(a: &Ast) -> bool {
    match a {
        Ast::Agg { .. } => true,
        Ast::Bin(_, l, r) => contains_agg(l) || contains_agg(r),
        Ast::Neg(x) | Ast::Not(x) | Ast::IsNull(x, _) => contains_agg(x),
        _ => false,
    }
}

fn split_and(a: &Ast) -> Vec<Ast> {
    match a {
        Ast::Bin(AstOp::And, l, r) => {
            let mut v = split_and(l);
            v.extend(split_and(r));
            v
        }
        _ => vec![a.clone()],
    }
}

// ------------------------------------------------------------------ expression binding

fn resolve(schema: &[Field], q: &Option<String>, name: &str) -> Result<usize> {
    let hits: Vec<usize> = schema
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name == name && (q.is_none() || f.qual == *q))
        .map(|(i, _)| i)
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(err!(
            "unknown column '{}{}'",
            q.as_ref().map(|q| format!("{q}.")).unwrap_or_default(),
            name
        )),
        _ => Err(err!("ambiguous column '{name}' (qualify it with a table alias)")),
    }
}

fn bind_num(s: &str) -> Result<BExpr> {
    if s.contains(['e', 'E']) {
        let x: f64 = s.parse().map_err(|_| err!("bad numeric literal '{s}'"))?;
        return Ok(BExpr::Lit(Value::F64(x), Ty::F64));
    }
    if let Some(dot) = s.find('.') {
        let frac = s.len() - dot - 1;
        if frac <= MAX_SCALE as usize {
            if let Some(v) = parse_dec(s.as_bytes(), frac as u8) {
                return Ok(BExpr::Lit(Value::Dec(v, frac as u8), Ty::Dec(frac as u8)));
            }
        }
        let x: f64 = s.parse().map_err(|_| err!("bad numeric literal '{s}'"))?;
        return Ok(BExpr::Lit(Value::F64(x), Ty::F64));
    }
    let v = parse_i64(s.as_bytes()).ok_or_else(|| err!("integer literal '{s}' out of range"))?;
    Ok(BExpr::Lit(Value::I64(v), Ty::I64))
}

/// Cast with constant folding for literals (avoids per-batch constant-column casts).
fn cast_to(e: BExpr, to: Ty) -> BExpr {
    if e.ty() == to {
        return e;
    }
    if let BExpr::Lit(v, _) = &e {
        let folded = match (v, to) {
            (Value::Null, _) => Some(Value::Null),
            (Value::I64(x), Ty::F64) => Some(Value::F64(*x as f64)),
            (Value::I64(x), Ty::Dec(s)) => x.checked_mul(POW10[s as usize]).map(|r| Value::Dec(r, s)),
            (Value::Dec(x, a), Ty::F64) => Some(Value::F64(*x as f64 / POW10[*a as usize] as f64)),
            (Value::Dec(x, a), Ty::Dec(b)) if b >= *a => x.checked_mul(POW10[(b - a) as usize]).map(|r| Value::Dec(r, b)),
            _ => None,
        };
        if let Some(v) = folded {
            return BExpr::Lit(v, to);
        }
    }
    BExpr::Cast(Box::new(e), to)
}

fn type_err(what: &str, a: Ty, b: Ty) -> crate::Error {
    err!("type mismatch: cannot {what} {} and {}", a.name(), b.name())
}

/// Bring two operands to a common type (numeric widening, or string literal -> date).
fn unify(l: BExpr, r: BExpr) -> Result<(BExpr, BExpr)> {
    if let BExpr::Lit(Value::Null, _) = &l {
        let t = r.ty();
        return Ok((BExpr::Lit(Value::Null, t), r));
    }
    if let BExpr::Lit(Value::Null, _) = &r {
        let t = l.ty();
        return Ok((l, BExpr::Lit(Value::Null, t)));
    }
    let (a, b) = (l.ty(), r.ty());
    if a == b {
        return Ok((l, r));
    }
    let date_lit = |e: &BExpr| -> Result<BExpr> {
        match e {
            BExpr::Lit(Value::Str(s), _) => {
                let d = parse_date(s.as_bytes()).ok_or_else(|| err!("'{s}' is not a valid date (YYYY-MM-DD)"))?;
                Ok(BExpr::Lit(Value::Date(d), Ty::Date))
            }
            _ => Err(err!("a date can only be compared with a 'YYYY-MM-DD' string literal")),
        }
    };
    match (a, b) {
        (Ty::Date, Ty::Str) => Ok((l, date_lit(&r)?)),
        (Ty::Str, Ty::Date) => Ok((date_lit(&l)?, r)),
        (Ty::F64, Ty::I64 | Ty::Dec(_)) => Ok((l, cast_to(r, Ty::F64))),
        (Ty::I64 | Ty::Dec(_), Ty::F64) => Ok((cast_to(l, Ty::F64), r)),
        (Ty::I64, Ty::Dec(s)) => Ok((cast_to(l, Ty::Dec(s)), r)),
        (Ty::Dec(s), Ty::I64) => Ok((l, cast_to(r, Ty::Dec(s)))),
        (Ty::Dec(x), Ty::Dec(y)) => {
            let m = Ty::Dec(x.max(y));
            Ok((cast_to(l, m), cast_to(r, m)))
        }
        _ => Err(type_err("compare/combine", a, b)),
    }
}

fn mk_bin(op: AstOp, l: BExpr, r: BExpr) -> Result<BExpr> {
    let b = |o, l, r, t| Ok(BExpr::Bin(o, Box::new(l), Box::new(r), t));
    match op {
        AstOp::And | AstOp::Or => {
            if l.ty() != Ty::Bool || r.ty() != Ty::Bool {
                return Err(type_err("apply AND/OR to", l.ty(), r.ty()));
            }
            b(if op == AstOp::And { BinOp::And } else { BinOp::Or }, l, r, Ty::Bool)
        }
        AstOp::Eq | AstOp::Ne | AstOp::Lt | AstOp::Le | AstOp::Gt | AstOp::Ge => {
            if l.ty() == Ty::Bool || r.ty() == Ty::Bool {
                return Err(err!("comparing boolean expressions is not supported"));
            }
            let (l, r) = unify(l, r)?;
            let o = match op {
                AstOp::Eq => BinOp::Eq,
                AstOp::Ne => BinOp::Ne,
                AstOp::Lt => BinOp::Lt,
                AstOp::Le => BinOp::Le,
                AstOp::Gt => BinOp::Gt,
                _ => BinOp::Ge,
            };
            b(o, l, r, Ty::Bool)
        }
        AstOp::Add | AstOp::Sub => {
            if !l.ty().is_numeric() || !r.ty().is_numeric() {
                return Err(type_err("do arithmetic on", l.ty(), r.ty()));
            }
            let (l, r) = unify(l, r)?;
            let t = l.ty();
            b(if op == AstOp::Add { BinOp::Add } else { BinOp::Sub }, l, r, t)
        }
        AstOp::Mul => {
            if !l.ty().is_numeric() || !r.ty().is_numeric() {
                return Err(type_err("do arithmetic on", l.ty(), r.ty()));
            }
            let (l, r) = match (l.ty(), r.ty()) {
                (Ty::F64, _) | (_, Ty::F64) => (cast_to(l, Ty::F64), cast_to(r, Ty::F64)),
                (Ty::I64, Ty::Dec(_)) => (cast_to(l, Ty::Dec(0)), r),
                (Ty::Dec(_), Ty::I64) => (l, cast_to(r, Ty::Dec(0))),
                _ => (l, r),
            };
            let t = match (l.ty(), r.ty()) {
                (Ty::Dec(x), Ty::Dec(y)) => {
                    if x + y > MAX_SCALE {
                        return Err(err!("decimal multiplication would exceed scale {MAX_SCALE}"));
                    }
                    Ty::Dec(x + y)
                }
                (t, _) => t,
            };
            b(BinOp::Mul, l, r, t)
        }
        AstOp::Div => {
            if !l.ty().is_numeric() || !r.ty().is_numeric() {
                return Err(type_err("do arithmetic on", l.ty(), r.ty()));
            }
            // `/` is always floating point (never integer division); x / 0 is NULL.
            b(BinOp::Div, cast_to(l, Ty::F64), cast_to(r, Ty::F64), Ty::F64)
        }
    }
}

fn bind_expr(a: &Ast, schema: &[Field]) -> Result<BExpr> {
    Ok(match a {
        Ast::Col { q, name } => {
            let i = resolve(schema, q, name)?;
            BExpr::Col(i, schema[i].ty)
        }
        Ast::Num(s) => bind_num(s)?,
        Ast::Str(s) => BExpr::Lit(Value::Str(s.clone()), Ty::Str),
        Ast::Null => BExpr::Lit(Value::Null, Ty::I64),
        Ast::Bin(op, l, r) => mk_bin(*op, bind_expr(l, schema)?, bind_expr(r, schema)?)?,
        Ast::Neg(x) => mk_neg(bind_expr(x, schema)?)?,
        Ast::Not(x) => mk_not(bind_expr(x, schema)?)?,
        Ast::IsNull(x, neg) => BExpr::IsNull(Box::new(bind_expr(x, schema)?), *neg),
        Ast::Agg { .. } => return Err(err!("aggregate functions are not allowed here")),
    })
}

fn mk_neg(e: BExpr) -> Result<BExpr> {
    if !e.ty().is_numeric() {
        return Err(err!("cannot negate {}", e.ty().name()));
    }
    let t = e.ty();
    Ok(BExpr::Neg(Box::new(e), t))
}
fn mk_not(e: BExpr) -> Result<BExpr> {
    if e.ty() != Ty::Bool {
        return Err(err!("NOT requires a boolean expression, found {}", e.ty().name()));
    }
    Ok(BExpr::Not(Box::new(e)))
}

// ------------------------------------------------------------------ aggregate context

struct AggCtx<'a> {
    from: &'a [Field],
    keys: Vec<BExpr>,
    aggs: Vec<AggCall>,
}

impl AggCtx<'_> {
    fn register(&mut self, a: &Ast) -> Result<BExpr> {
        let Ast::Agg { func, arg, distinct } = a else { unreachable!() };
        let (arg_b, in_ty) = match arg {
            None => (None, None),
            Some(x) => {
                if contains_agg(x) {
                    return Err(err!("nested aggregate functions are not allowed"));
                }
                let e = bind_expr(x, self.from)?;
                let t = e.ty();
                (Some(e), Some(t))
            }
        };
        let out_ty = match (func, in_ty) {
            (AggFunc::Count, _) => Ty::I64,
            (_, Some(Ty::Bool)) => return Err(err!("aggregate over a boolean expression is not supported")),
            (AggFunc::Sum, Some(t @ (Ty::I64 | Ty::F64 | Ty::Dec(_)))) => t,
            (AggFunc::Avg, Some(Ty::I64 | Ty::F64 | Ty::Dec(_))) => Ty::F64,
            (AggFunc::Min | AggFunc::Max, Some(t)) => t,
            (f, Some(t)) => return Err(err!("{f:?} is not defined for type {}", t.name())),
            (_, None) => unreachable!(),
        };
        let call = AggCall { func: *func, arg: arg_b, distinct: *distinct, out_ty };
        let idx = match self.aggs.iter().position(|c| *c == call) {
            Some(i) => i,
            None => {
                self.aggs.push(call);
                self.aggs.len() - 1
            }
        };
        Ok(BExpr::Col(self.keys.len() + idx, self.aggs[idx].out_ty))
    }

    /// Bind an expression evaluated *after* aggregation: whole subtrees equal to a GROUP BY
    /// expression become key refs, aggregate calls become agg slots, bare columns are errors.
    fn bind_post(&mut self, a: &Ast) -> Result<BExpr> {
        if matches!(a, Ast::Agg { .. }) {
            return self.register(a);
        }
        if !contains_agg(a) {
            if let Ok(b) = bind_expr(a, self.from) {
                if let Some(k) = self.keys.iter().position(|x| *x == b) {
                    return Ok(BExpr::Col(k, b.ty()));
                }
                if matches!(a, Ast::Num(_) | Ast::Str(_) | Ast::Null) {
                    return Ok(b);
                }
            }
        }
        Ok(match a {
            Ast::Num(_) | Ast::Str(_) | Ast::Null => bind_expr(a, self.from)?,
            Ast::Col { q, name } => {
                resolve(self.from, q, name)?;
                return Err(err!("column '{name}' must appear in the GROUP BY clause or be used in an aggregate function"));
            }
            Ast::Bin(op, l, r) => {
                let (l, r) = (self.bind_post(l)?, self.bind_post(r)?);
                mk_bin(*op, l, r)?
            }
            Ast::Neg(x) => mk_neg(self.bind_post(x)?)?,
            Ast::Not(x) => mk_not(self.bind_post(x)?)?,
            Ast::IsNull(x, n) => BExpr::IsNull(Box::new(self.bind_post(x)?), *n),
            Ast::Agg { .. } => unreachable!(),
        })
    }
}

// ------------------------------------------------------------------ join keys

/// Is `c` an equality `A = B` whose sides come from different join inputs?
/// Returns (left key, right key shifted into the right input's own column space), unified in type.
fn try_equi(c: &Ast, combined: &[Field], nl: usize) -> Result<Option<(BExpr, BExpr)>> {
    let Ast::Bin(AstOp::Eq, a, b) = c else { return Ok(None) };
    let (ba, bb) = (bind_expr(a, combined)?, bind_expr(b, combined)?);
    let side = |e: &BExpr| -> Option<bool> {
        let mut cs = Vec::new();
        e.cols(&mut cs);
        if cs.is_empty() {
            None
        } else if cs.iter().all(|&i| i < nl) {
            Some(true)
        } else if cs.iter().all(|&i| i >= nl) {
            Some(false)
        } else {
            None
        }
    };
    let (l, r) = match (side(&ba), side(&bb)) {
        (Some(true), Some(false)) => (ba, bb),
        (Some(false), Some(true)) => (bb, ba),
        _ => return Ok(None),
    };
    let (l, r) = unify(l, r.shift(-(nl as isize)))?;
    if l.ty() == Ty::Bool {
        return Ok(None);
    }
    Ok(Some((l, r)))
}

// ------------------------------------------------------------------ main binder

fn scan(tr: &TableRef, cat: &Catalog, refs: &[(Option<String>, String)], all: bool) -> Result<Plan> {
    let def = cat.table(&tr.name)?;
    let alias = tr.alias.clone().unwrap_or_else(|| def.name.clone());
    let cols: Vec<usize> = def
        .cols
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| all || refs.iter().any(|(q, rn)| rn == n && (q.is_none() || q.as_deref() == Some(alias.as_str()))))
        .map(|(i, _)| i)
        .collect();
    let schema = cols
        .iter()
        .map(|&i| Field { qual: Some(alias.clone()), name: def.cols[i].0.clone(), ty: def.cols[i].1 })
        .collect();
    let est = std::fs::metadata(&def.path).map(|m| m.len()).unwrap_or(0);
    Ok(Plan { node: Node::Scan { table: def.clone(), cols }, schema, est })
}

pub fn bind(stmt: &Select, cat: &Catalog) -> Result<(Plan, Vec<String>)> {
    // 1. which columns are referenced anywhere (projection pushdown)
    let mut refs = Vec::new();
    for it in &stmt.items {
        collect_refs(&it.expr, &mut refs);
    }
    for j in &stmt.joins {
        if let Some(o) = &j.on {
            collect_refs(o, &mut refs);
        }
    }
    for a in stmt.where_.iter().chain(stmt.having.iter()).chain(stmt.group_by.iter()) {
        collect_refs(a, &mut refs);
    }
    for (a, _) in &stmt.order_by {
        collect_refs(a, &mut refs);
    }

    // 2. FROM / JOIN tree
    let mut aliases = vec![stmt.from.alias.clone().unwrap_or_else(|| stmt.from.name.clone())];
    let mut plan = scan(&stmt.from, cat, &refs, stmt.star)?;
    let mut conj: Vec<Ast> = stmt.where_.as_ref().map(split_and).unwrap_or_default();
    for j in &stmt.joins {
        let al = j.table.alias.clone().unwrap_or_else(|| j.table.name.clone());
        if aliases.contains(&al) {
            return Err(err!("duplicate table name/alias '{al}' (use an alias)"));
        }
        aliases.push(al);
        let right = scan(&j.table, cat, &refs, stmt.star)?;
        let nl = plan.schema.len();
        let mut combined = plan.schema.clone();
        combined.extend(right.schema.iter().cloned());
        let (mut lk, mut rk) = (vec![], vec![]);
        match j.kind {
            JoinKind::Comma => {
                let mut rest = vec![];
                for c in conj.drain(..) {
                    match try_equi(&c, &combined, nl) {
                        Ok(Some((a, b))) => {
                            lk.push(a);
                            rk.push(b);
                        }
                        _ => rest.push(c),
                    }
                }
                conj = rest;
                if lk.is_empty() {
                    return Err(err!("comma join of '{}' needs an equality condition in WHERE (cross joins are unsupported)", j.table.name));
                }
            }
            JoinKind::Inner | JoinKind::Left => {
                for c in split_and(j.on.as_ref().unwrap()) {
                    match try_equi(&c, &combined, nl)? {
                        Some((a, b)) => {
                            lk.push(a);
                            rk.push(b);
                        }
                        None => return Err(err!("unsupported ON condition: only `a = b [AND c = d]` between the two join inputs is supported")),
                    }
                }
            }
        }
        let kind = if j.kind == JoinKind::Left { JoinType::Left } else { JoinType::Inner };
        // Build the hash table on the smaller input (by file-size estimate). LEFT JOIN always builds the right.
        let build_left = kind == JoinType::Inner && plan.est < right.est;
        let est = plan.est + right.est;
        plan = Plan {
            node: Node::Join { left: Box::new(plan), right: Box::new(right), kind, lkeys: lk, rkeys: rk, build_left },
            schema: combined,
            est,
        };
    }

    // 3. WHERE
    if !conj.is_empty() {
        let mut pred: Option<BExpr> = None;
        for c in &conj {
            if contains_agg(c) {
                return Err(err!("aggregate functions are not allowed in WHERE"));
            }
            let b = bind_expr(c, &plan.schema)?;
            if b.ty() != Ty::Bool {
                return Err(err!("WHERE condition must be boolean, found {}", b.ty().name()));
            }
            pred = Some(match pred {
                None => b,
                Some(p) => BExpr::Bin(BinOp::And, Box::new(p), Box::new(b), Ty::Bool),
            });
        }
        let (schema, est) = (plan.schema.clone(), plan.est);
        plan = Plan { node: Node::Filter { input: Box::new(plan), pred: pred.unwrap() }, schema, est };
    }

    // 4. select list (expand *)
    let items: Vec<SelectItem> = if stmt.star {
        plan.schema
            .iter()
            .map(|f| SelectItem {
                expr: Ast::Col { q: f.qual.clone(), name: f.name.clone() },
                alias: None,
                text: f.name.clone(),
            })
            .collect()
    } else {
        stmt.items.clone()
    };
    let names: Vec<String> = items
        .iter()
        .map(|it| match (&it.alias, &it.expr) {
            (Some(a), _) => a.clone(),
            (None, Ast::Col { name, .. }) => name.clone(),
            _ => it.text.clone(),
        })
        .collect();

    let is_agg = !stmt.group_by.is_empty()
        || stmt.having.is_some()
        || items.iter().any(|i| contains_agg(&i.expr))
        || stmt.order_by.iter().any(|(a, _)| contains_agg(a));

    let from_schema = plan.schema.clone();
    let mut actx = AggCtx { from: &from_schema, keys: vec![], aggs: vec![] };
    let mut outs: Vec<BExpr> = Vec::new();

    if is_agg {
        for g in &stmt.group_by {
            let g = match g {
                Ast::Num(n) => {
                    let k: usize = n.parse().map_err(|_| err!("bad GROUP BY ordinal"))?;
                    items.get(k.wrapping_sub(1)).ok_or_else(|| err!("GROUP BY position {k} is out of range"))?.expr.clone()
                }
                Ast::Col { q: None, name } if resolve(&from_schema, &None, name).is_err() => items
                    .iter()
                    .find(|i| i.alias.as_deref() == Some(name.as_str()))
                    .map(|i| i.expr.clone())
                    .unwrap_or_else(|| g.clone()),
                _ => g.clone(),
            };
            if contains_agg(&g) {
                return Err(err!("aggregate functions are not allowed in GROUP BY"));
            }
            let b = bind_expr(&g, &from_schema)?;
            if !actx.keys.contains(&b) {
                actx.keys.push(b);
            }
        }
        for it in &items {
            outs.push(actx.bind_post(&it.expr)?);
        }
    } else {
        for it in &items {
            outs.push(bind_expr(&it.expr, &from_schema)?);
        }
    }
    let having = match (&stmt.having, is_agg) {
        (Some(h), true) => {
            let b = actx.bind_post(h)?;
            if b.ty() != Ty::Bool {
                return Err(err!("HAVING condition must be boolean"));
            }
            Some(b)
        }
        _ => None,
    };

    // 5. ORDER BY -> column refs on the projected output (hidden columns appended if needed)
    let n_visible = outs.len();
    let mut order_keys: Vec<(usize, bool)> = Vec::new();
    for (a, desc) in &stmt.order_by {
        let idx = match a {
            Ast::Num(n) => {
                let k: usize = n.parse().map_err(|_| err!("ORDER BY position must be a positive integer"))?;
                if k == 0 || k > n_visible {
                    return Err(err!("ORDER BY position {k} is out of range"));
                }
                k - 1
            }
            Ast::Col { q: None, name } if items.iter().any(|i| i.alias.as_deref() == Some(name.as_str())) => {
                items.iter().position(|i| i.alias.as_deref() == Some(name.as_str())).unwrap()
            }
            _ => {
                let b = if is_agg { actx.bind_post(a)? } else { bind_expr(a, &from_schema)? };
                match outs.iter().position(|o| *o == b) {
                    Some(i) => i,
                    None => {
                        outs.push(b);
                        outs.len() - 1
                    }
                }
            }
        };
        order_keys.push((idx, *desc));
    }

    // 6. assemble operators
    if is_agg {
        let nk = actx.keys.len();
        let mut schema: Vec<Field> =
            actx.keys.iter().enumerate().map(|(i, k)| Field { qual: None, name: format!("key{i}"), ty: k.ty() }).collect();
        schema.extend(actx.aggs.iter().enumerate().map(|(i, a)| Field { qual: None, name: format!("agg{i}"), ty: a.out_ty }));
        debug_assert_eq!(schema.len(), nk + actx.aggs.len());
        let est = plan.est;
        plan = Plan { node: Node::Aggregate { input: Box::new(plan), keys: actx.keys, aggs: actx.aggs }, schema, est };
        if let Some(h) = having {
            let (schema, est) = (plan.schema.clone(), plan.est);
            plan = Plan { node: Node::Filter { input: Box::new(plan), pred: h }, schema, est };
        }
    }
    // booleans are exposed as 0/1 integers
    let outs: Vec<BExpr> = outs.into_iter().map(|e| if e.ty() == Ty::Bool { cast_to(e, Ty::I64) } else { e }).collect();
    let schema: Vec<Field> = outs
        .iter()
        .enumerate()
        .map(|(i, e)| Field { qual: None, name: names.get(i).cloned().unwrap_or_else(|| format!("_order{i}")), ty: e.ty() })
        .collect();
    let est = plan.est;
    plan = Plan { node: Node::Project { input: Box::new(plan), exprs: outs }, schema, est };
    if !order_keys.is_empty() {
        let (schema, est) = (plan.schema.clone(), plan.est);
        plan = Plan { node: Node::Sort { input: Box::new(plan), keys: order_keys }, schema, est };
    }
    if plan.schema.len() > n_visible {
        let schema: Vec<Field> = plan.schema[..n_visible].to_vec();
        let exprs = schema.iter().enumerate().map(|(i, f)| BExpr::Col(i, f.ty)).collect();
        let est = plan.est;
        plan = Plan { node: Node::Project { input: Box::new(plan), exprs }, schema, est };
    }
    if let Some(n) = stmt.limit {
        let (schema, est) = (plan.schema.clone(), plan.est);
        plan = Plan { node: Node::Limit { input: Box::new(plan), n }, schema, est };
    }
    Ok((plan, names))
}
