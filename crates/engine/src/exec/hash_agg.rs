//! Hash aggregation, single-threaded. Group key = canonical byte encoding of the key columns
//! (NULL is its own code, so all NULL keys form one group). Lookup of an existing group does not
//! allocate; only creating a group copies its key.
use super::eval::eval;
use super::hash::{FxMap, FxSet, IntMap};
use super::par::{ExecCtx, Chunked};
use super::Op;
use crate::plan::{AggCall, BExpr};
use crate::sql::AggFunc;
use crate::storage::csv::BATCH_ROWS;
use crate::types::*;
use crate::Result;
use rayon::prelude::*;
use std::sync::Arc;

enum Mm {
    None,
    I(i64),
    F(f64),
    S(Vec<u8>),
}

enum St {
    Count(i64),
    SumI { s: i128, any: bool },
    /// Kahan-Babuska (compensated) float sum.
    SumF { s: f64, c: f64, any: bool },
    AvgI { s: i128, n: i64 },
    AvgF { s: f64, c: f64, n: i64 },
    MinMax(Mm),
    Distinct(Box<FxSet<Box<[u8]>>>),
}

#[inline]
fn kadd(s: &mut f64, c: &mut f64, x: f64) {
    let t = *s + x;
    if s.abs() >= x.abs() {
        *c += (*s - t) + x;
    } else {
        *c += (x - t) + *s;
    }
    *s = t;
}

fn init(call: &AggCall) -> St {
    let in_ty = call.arg.as_ref().map(|a| a.ty());
    match (call.func, in_ty) {
        _ if call.distinct => St::Distinct(Box::default()),
        (AggFunc::Count, _) => St::Count(0),
        (AggFunc::Sum, Some(Ty::F64)) => St::SumF { s: 0.0, c: 0.0, any: false },
        (AggFunc::Sum, _) => St::SumI { s: 0, any: false },
        (AggFunc::Avg, Some(Ty::F64)) => St::AvgF { s: 0.0, c: 0.0, n: 0 },
        (AggFunc::Avg, _) => St::AvgI { s: 0, n: 0 },
        (AggFunc::Min | AggFunc::Max, _) => St::MinMax(Mm::None),
    }
}

#[inline]
fn update(st: &mut St, call: &AggCall, col: &Column, i: usize, buf: &mut Vec<u8>) {
    match st {
        St::Count(n) => *n += 1,
        St::SumI { s, any } => {
            *s += col.int_at(i) as i128;
            *any = true;
        }
        St::SumF { s, c, any } => {
            let ColData::F64(v) = &col.data else { unreachable!() };
            kadd(s, c, v[i]);
            *any = true;
        }
        St::AvgI { s, n } => {
            *s += col.int_at(i) as i128;
            *n += 1;
        }
        St::AvgF { s, c, n } => {
            let ColData::F64(v) = &col.data else { unreachable!() };
            kadd(s, c, v[i]);
            *n += 1;
        }
        St::MinMax(m) => {
            let want_min = call.func == AggFunc::Min;
            match &col.data {
                ColData::F64(v) => {
                    let x = v[i];
                    match m {
                        Mm::F(cur) => {
                            if (want_min && x < *cur) || (!want_min && x > *cur) {
                                *cur = x
                            }
                        }
                        _ => *m = Mm::F(x),
                    }
                }
                ColData::Str(v) => {
                    let x = v.get(i);
                    match m {
                        Mm::S(cur) => {
                            if (want_min && x < cur.as_slice()) || (!want_min && x > cur.as_slice()) {
                                cur.clear();
                                cur.extend_from_slice(x);
                            }
                        }
                        _ => *m = Mm::S(x.to_vec()),
                    }
                }
                _ => {
                    let x = col.int_at(i);
                    match m {
                        Mm::I(cur) => {
                            if (want_min && x < *cur) || (!want_min && x > *cur) {
                                *cur = x
                            }
                        }
                        _ => *m = Mm::I(x),
                    }
                }
            }
        }
        St::Distinct(set) => {
            buf.clear();
            col.encode_key(i, buf);
            if !set.contains(&buf[..]) {
                set.insert(buf.clone().into_boxed_slice());
            }
        }
    }
}

fn finish(st: &St, call: &AggCall) -> Result<Value> {
    Ok(match st {
        St::Count(n) => Value::I64(*n),
        St::Distinct(s) => Value::I64(s.len() as i64),
        St::SumI { s, any } => {
            if !*any {
                Value::Null
            } else {
                let v = checked_i64(*s, "SUM")?;
                match call.out_ty {
                    Ty::Dec(sc) => Value::Dec(v, sc),
                    _ => Value::I64(v),
                }
            }
        }
        St::SumF { s, c, any } => {
            if *any {
                Value::F64(s + c)
            } else {
                Value::Null
            }
        }
        St::AvgI { s, n } => {
            if *n == 0 {
                Value::Null
            } else {
                let scale = match call.arg.as_ref().map(BExpr::ty) {
                    Some(Ty::Dec(sc)) => sc,
                    _ => 0,
                };
                Value::F64(*s as f64 / *n as f64 / POW10[scale as usize] as f64)
            }
        }
        St::AvgF { s, c, n } => {
            if *n == 0 {
                Value::Null
            } else {
                Value::F64((s + c) / *n as f64)
            }
        }
        St::MinMax(m) => match (m, call.out_ty) {
            (Mm::None, _) => Value::Null,
            (Mm::I(x), Ty::I64) => Value::I64(*x),
            (Mm::I(x), Ty::Dec(s)) => Value::Dec(*x, s),
            (Mm::I(x), Ty::Date) => Value::Date(*x as i32),
            (Mm::F(x), _) => Value::F64(*x),
            (Mm::S(b), _) => Value::Str(String::from_utf8_lossy(b).into_owned()),
            _ => unreachable!(),
        },
    })
}

/// Combine two partial states of the same aggregate. `a` is the earlier chunk, `b` the later one;
/// merging in a fixed order is what makes float results independent of the thread count.
fn merge_state(a: &mut St, b: St, call: &AggCall) {
    match (a, b) {
        (St::Distinct(x), St::Distinct(mut y)) => {
            // owned: move the smaller set into the larger one
            if x.len() < y.len() {
                std::mem::swap(x, &mut y);
            }
            x.extend(y.drain());
        }
        (a, b) => merge_state_ref(a, &b, call),
    }
}

/// Same as `merge_state` but `b` stays in place (used when several tasks read the same source table).
fn merge_state_ref(a: &mut St, b: &St, call: &AggCall) {
    match (a, b) {
        (St::Count(x), St::Count(y)) => *x += y,
        (St::SumI { s, any }, St::SumI { s: s2, any: a2 }) => {
            *s += s2;
            *any |= a2;
        }
        (St::SumF { s, c, any }, St::SumF { s: s2, c: c2, any: a2 }) => {
            kadd(s, c, *s2);
            *c += c2;
            *any |= a2;
        }
        (St::AvgI { s, n }, St::AvgI { s: s2, n: n2 }) => {
            *s += s2;
            *n += n2;
        }
        (St::AvgF { s, c, n }, St::AvgF { s: s2, c: c2, n: n2 }) => {
            kadd(s, c, *s2);
            *c += c2;
            *n += n2;
        }
        (St::MinMax(m), St::MinMax(o)) => {
            let want_min = call.func == AggFunc::Min;
            match (&mut *m, o) {
                (_, Mm::None) => {}
                (m @ Mm::None, o) => {
                    *m = match o {
                        Mm::I(v) => Mm::I(*v),
                        Mm::F(v) => Mm::F(*v),
                        Mm::S(v) => Mm::S(v.clone()),
                        Mm::None => Mm::None,
                    }
                }
                (Mm::I(x), Mm::I(y)) => {
                    if (want_min && y < x) || (!want_min && y > x) {
                        *x = *y
                    }
                }
                (Mm::F(x), Mm::F(y)) => {
                    if (want_min && y < x) || (!want_min && y > x) {
                        *x = *y
                    }
                }
                (Mm::S(x), Mm::S(y)) => {
                    if (want_min && y < x) || (!want_min && y > x) {
                        x.clone_from(y)
                    }
                }
                _ => unreachable!("min/max type mismatch"),
            }
        }
        (St::Distinct(x), St::Distinct(y)) => {
            for k in y.iter() {
                if !x.contains(k) {
                    x.insert(k.clone());
                }
            }
        }
        _ => unreachable!("aggregate state mismatch"),
    }
}

pub struct AggSpec {
    pub keys: Vec<BExpr>,
    pub aggs: Vec<AggCall>,
}

/// Group table: group key = canonical byte encoding of the key columns (NULL is its own code, so all
/// NULL keys form one group). Looking up an existing group does not allocate; creating one copies its key.
pub struct AggTable {
    spec: Arc<AggSpec>,
    map: FxMap<Box<[u8]>, u32>,
    /// Fast path: exactly one key column of an integer-like type (i64, decimal, date). Keys live in
    /// `ikeys[gid]` (0 for the NULL group, which is tracked by `null_gid` and is not in `imap`).
    int_mode: bool,
    imap: IntMap,
    ikeys: Vec<i64>,
    null_gid: Option<u32>,
    key_out: Vec<Column>,
    states: Vec<St>,
    ngroups: usize,
    buf: Vec<u8>,
    buf2: Vec<u8>,
    gids: Vec<u32>,
}

impl AggTable {
    pub fn new(spec: Arc<AggSpec>) -> AggTable {
        let key_out = spec.keys.iter().map(|k| Column::empty(k.ty())).collect();
        let int_mode = spec.keys.len() == 1 && matches!(spec.keys[0].ty(), Ty::I64 | Ty::Dec(_) | Ty::Date);
        AggTable {
            spec,
            map: FxMap::default(),
            int_mode,
            imap: IntMap::default(),
            ikeys: Vec::new(),
            null_gid: None,
            key_out,
            states: Vec::new(),
            ngroups: 0,
            buf: Vec::with_capacity(64),
            buf2: Vec::with_capacity(64),
            gids: Vec::new(),
        }
    }

    fn new_group(&mut self) -> u32 {
        self.states.extend(self.spec.aggs.iter().map(init));
        self.ngroups += 1;
        (self.ngroups - 1) as u32
    }

    pub fn consume(&mut self, b: &Batch) -> Result<()> {
        let spec = self.spec.clone();
        let na = spec.aggs.len();
        let kc: Vec<Arc<Column>> = spec.keys.iter().map(|k| eval(k, b)).collect::<Result<_>>()?;
        let ac: Vec<Option<Arc<Column>>> =
            spec.aggs.iter().map(|a| a.arg.as_ref().map(|e| eval(e, b)).transpose()).collect::<Result<_>>()?;
        self.gids.clear();
        if self.int_mode {
            let kcol = &kc[0];
            for i in 0..b.len {
                let g = if kcol.is_null(i) {
                    match self.null_gid {
                        Some(g) => g,
                        None => {
                            self.ikeys.push(0);
                            let g = self.new_group();
                            self.null_gid = Some(g);
                            self.key_out[0].push_from(kcol, i);
                            g
                        }
                    }
                } else {
                    let k = kcol.int_at(i);
                    match self.imap.find(k, &self.ikeys) {
                        Some(g) => g,
                        None => {
                            self.ikeys.push(k);
                            let g = self.new_group();
                            self.imap.insert(g, &self.ikeys);
                            self.key_out[0].push_from(kcol, i);
                            g
                        }
                    }
                };
                self.gids.push(g);
            }
        } else {
            for i in 0..b.len {
                self.buf.clear();
                for c in &kc {
                    c.encode_key(i, &mut self.buf);
                }
                let g = match self.map.get(&self.buf[..]) {
                    Some(&g) => g,
                    None => {
                        let g = self.new_group();
                        self.map.insert(self.buf.clone().into_boxed_slice(), g);
                        for (o, c) in self.key_out.iter_mut().zip(&kc) {
                            o.push_from(c, i);
                        }
                        g
                    }
                };
                self.gids.push(g);
            }
        }
        for (j, call) in spec.aggs.iter().enumerate() {
            match &ac[j] {
                None => {
                    for &g in &self.gids {
                        if let St::Count(n) = &mut self.states[g as usize * na + j] {
                            *n += 1;
                        }
                    }
                }
                Some(col) => {
                    for i in 0..b.len {
                        if !col.is_null(i) {
                            update(&mut self.states[self.gids[i] as usize * na + j], call, col, i, &mut self.buf2);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Fold `other` (a later chunk) into `self`. Groups first seen in `other` are appended in `other`'s
    /// own first-seen order, so the merged group order equals the order a serial scan would produce.
    pub fn merge(&mut self, other: AggTable) {
        let spec = self.spec.clone();
        let na = spec.aggs.len();
        if self.int_mode {
            // Groups are already in first-seen order in `other`, so no sorting is needed.
            let mut ostates = other.states;
            for og in 0..other.ngroups {
                let t = if Some(og as u32) == other.null_gid {
                    match self.null_gid {
                        Some(g) => g,
                        None => {
                            self.ikeys.push(0);
                            let g = self.new_group();
                            self.null_gid = Some(g);
                            self.key_out[0].push_from(&other.key_out[0], og);
                            g
                        }
                    }
                } else {
                    let k = other.ikeys[og];
                    match self.imap.find(k, &self.ikeys) {
                        Some(g) => g,
                        None => {
                            self.ikeys.push(k);
                            let g = self.new_group();
                            self.imap.insert(g, &self.ikeys);
                            self.key_out[0].push_from(&other.key_out[0], og);
                            g
                        }
                    }
                } as usize;
                for (j, call) in spec.aggs.iter().enumerate() {
                    let b = std::mem::replace(&mut ostates[og * na + j], St::Count(0));
                    merge_state(&mut self.states[t * na + j], b, call);
                }
            }
            return;
        }
        let mut ents: Vec<(Box<[u8]>, u32)> = other.map.into_iter().collect();
        ents.sort_unstable_by_key(|e| e.1);
        let mut ostates = other.states;
        for (key, og) in ents {
            let og = og as usize;
            let t = match self.map.get(&key[..]) {
                Some(&t) => t,
                None => {
                    let t = self.new_group();
                    self.map.insert(key, t);
                    for (o, c) in self.key_out.iter_mut().zip(&other.key_out) {
                        o.push_from(c, og);
                    }
                    t
                }
            } as usize;
            for (j, call) in spec.aggs.iter().enumerate() {
                let b = std::mem::replace(&mut ostates[og * na + j], St::Count(0));
                merge_state(&mut self.states[t * na + j], b, call);
            }
        }
    }

    /// Number of hash partitions for the partition-wise merge (fixed: independent of the thread count).
    pub const PARTS: usize = 64;

    #[inline]
    fn part_of(k: i64) -> usize {
        // different multiplier than `IntMap`'s, so partition bits are independent of table-index bits
        (((k as u64) ^ ((k as u64) >> 29)).wrapping_mul(0xD6E8_FEB8_6659_FD93) >> 58) as usize
    }

    /// Group ids bucketed by partition, each bucket in ascending group id (first-seen) order.
    fn int_buckets(&self) -> Vec<Vec<u32>> {
        let mut b = vec![Vec::new(); Self::PARTS];
        for g in 0..self.ngroups {
            let p = if Some(g as u32) == self.null_gid { 0 } else { Self::part_of(self.ikeys[g]) };
            b[p].push(g as u32);
        }
        b
    }

    /// Fold group `og` of `other` (int mode) into this table, leaving `other` untouched.
    fn merge_one_int(&mut self, other: &AggTable, og: usize) {
        let na = self.spec.aggs.len();
        let t = if Some(og as u32) == other.null_gid {
            match self.null_gid {
                Some(g) => g,
                None => {
                    self.ikeys.push(0);
                    let g = self.new_group();
                    self.null_gid = Some(g);
                    self.key_out[0].push_from(&other.key_out[0], og);
                    g
                }
            }
        } else {
            let k = other.ikeys[og];
            match self.imap.find(k, &self.ikeys) {
                Some(g) => g,
                None => {
                    self.ikeys.push(k);
                    let g = self.new_group();
                    self.imap.insert(g, &self.ikeys);
                    self.key_out[0].push_from(&other.key_out[0], og);
                    g
                }
            }
        } as usize;
        let spec = self.spec.clone();
        for (j, call) in spec.aggs.iter().enumerate() {
            merge_state_ref(&mut self.states[t * na + j], &other.states[og * na + j], call);
        }
    }

    /// Final output columns (keys first, then aggregates) and the group count.
    pub fn finish(mut self) -> Result<(Vec<Column>, usize)> {
        let na = self.spec.aggs.len();
        // A global aggregate (no GROUP BY) always yields exactly one row, even on empty input.
        if self.spec.keys.is_empty() && self.ngroups == 0 {
            self.new_group();
        }
        let mut cols = std::mem::take(&mut self.key_out);
        for (j, call) in self.spec.aggs.iter().enumerate() {
            let mut c = Column::empty(call.out_ty);
            for g in 0..self.ngroups {
                c.push_value(&finish(&self.states[g * na + j], call)?);
            }
            cols.push(c);
        }
        Ok((cols, self.ngroups))
    }
}

/// Where a `HashAgg` gets its input: one serial operator, or independent per-chunk pipelines.
pub enum AggInput {
    Serial(Box<dyn Op>),
    Parallel(Arc<ExecCtx>, Chunked),
}

pub struct HashAgg {
    input: Option<AggInput>,
    spec: Arc<AggSpec>,
    /// Finished result, as one or more column segments emitted in order (one segment per partition on the
    /// partition-wise path, so nothing is concatenated).
    out: Option<Segments>,
}

struct Segments {
    segs: Vec<(Vec<Column>, usize)>,
    seg: usize,
    pos: usize,
}

/// Merge per-chunk tables by key partition: each partition is folded by one task, walking the tables in chunk
/// order, so there is no serial tail and the result does not depend on the thread count.
fn partitioned_merge(ctx: &ExecCtx, spec: &Arc<AggSpec>, tables: Vec<AggTable>) -> Result<Vec<(Vec<Column>, usize)>> {
    ctx.run(|| {
        let buckets: Vec<Vec<Vec<u32>>> = tables.par_iter().map(|t| t.int_buckets()).collect();
        (0..AggTable::PARTS)
            .into_par_iter()
            .map(|p| {
                let mut part = AggTable::new(spec.clone());
                for (c, t) in tables.iter().enumerate() {
                    for &og in &buckets[c][p] {
                        part.merge_one_int(t, og as usize);
                    }
                }
                part.finish()
            })
            .collect::<Result<Vec<_>>>()
    })
}

impl HashAgg {
    pub fn new(input: AggInput, keys: Vec<BExpr>, aggs: Vec<AggCall>) -> HashAgg {
        HashAgg { input: Some(input), spec: Arc::new(AggSpec { keys, aggs }), out: None }
    }

    fn run(&mut self) -> Result<()> {
        let table = match self.input.take().unwrap() {
            AggInput::Serial(mut op) => {
                let mut t = AggTable::new(self.spec.clone());
                while let Some(b) = op.next()? {
                    t.consume(&b)?;
                }
                t
            }
            AggInput::Parallel(ctx, ch) => {
                let spec = self.spec.clone();
                // One partial table per chunk, collected in chunk order.
                let mut tables: Vec<AggTable> = ctx.run(|| {
                    (0..ch.n)
                        .into_par_iter()
                        .map(|i| {
                            let mut op = (ch.make)(i)?;
                            let mut t = AggTable::new(spec.clone());
                            while let Some(b) = op.next()? {
                                t.consume(&b)?;
                            }
                            Ok(t)
                        })
                        .collect::<Result<Vec<_>>>()
                })?;
                let total: usize = tables.iter().map(|t| t.ngroups).sum();
                if tables.len() > 1 && tables[0].int_mode && total > ctx.agg_partition_groups {
                    let segs = partitioned_merge(&ctx, &spec, tables)?;
                    self.out = Some(Segments { segs, seg: 0, pos: 0 });
                    return Ok(());
                }
                // Fixed-shape pairwise tree merge: the merge order depends only on the chunk count,
                // never on how many threads ran, so float sums are reproducible.
                ctx.run(|| {
                    while tables.len() > 1 {
                        let mut it = std::mem::take(&mut tables).into_iter();
                        let mut pairs = Vec::new();
                        let mut odd = None;
                        loop {
                            match (it.next(), it.next()) {
                                (Some(a), Some(b)) => pairs.push((a, b)),
                                (Some(a), None) => {
                                    odd = Some(a);
                                    break;
                                }
                                _ => break,
                            }
                        }
                        tables = pairs
                            .into_par_iter()
                            .map(|(mut a, b)| {
                                a.merge(b);
                                a
                            })
                            .collect();
                        tables.extend(odd);
                    }
                });
                tables.pop().unwrap_or_else(|| AggTable::new(self.spec.clone()))
            }
        };
        let (cols, n) = table.finish()?;
        self.out = Some(Segments { segs: vec![(cols, n)], seg: 0, pos: 0 });
        Ok(())
    }
}

impl Op for HashAgg {
    fn next(&mut self) -> Result<Option<Batch>> {
        if self.out.is_none() {
            self.run()?;
        }
        let o = self.out.as_mut().unwrap();
        while o.seg < o.segs.len() {
            let (cols, n) = &o.segs[o.seg];
            if o.pos >= *n {
                o.seg += 1;
                o.pos = 0;
                continue;
            }
            let len = (*n - o.pos).min(BATCH_ROWS);
            let b = Batch { cols: cols.iter().map(|c| Arc::new(c.slice(o.pos, len))).collect(), len };
            o.pos += len;
            return Ok(Some(b));
        }
        Ok(None)
    }
}
