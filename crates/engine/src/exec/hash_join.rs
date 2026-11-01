//! Hash join. The build side is fully materialized; rows with a NULL key are never inserted and
//! NULL probe keys never match. Duplicate build keys chain through `next`.
use super::eval::eval;
use super::hash::FxMap;
use super::par::ExecCtx;
use super::Op;
use crate::plan::{BExpr, JoinType};
use crate::types::*;
use crate::Result;
use rayon::prelude::*;
use std::sync::Arc;

const NIL: u32 = u32::MAX;

/// How the build side is indexed. `Bytes` is the general join (any key types, composite keys) and the reference
/// path; `Int` is the EXP-008 fast path for one integer-like key per side.
enum Index {
    Bytes { table: FxMap<Box<[u8]>, u32>, next: Vec<u32> },
    Int(IntJoin),
}

pub struct Built {
    cols: Vec<Column>,
    index: Index,
}

// ------------------------------------------------------------------------------------------ integer-key join

/// Key class of an integer-like join key. The byte-key join encodes `i64` and `decimal` as the same 8 raw bytes and
/// `date` as 4 bytes, so keys of different classes never match there; the integer path keeps exactly that rule by
/// only being used when both sides are in the same class (compared as raw `i64`).
fn int_class(t: Ty) -> Option<u8> {
    match t {
        Ty::I64 | Ty::Dec(_) => Some(0),
        Ty::Date => Some(1),
        _ => None,
    }
}

/// True when the join has one key per side and both are integer-like of the same class.
pub fn int_join_eligible(bkeys: &[BExpr], pkeys: &[BExpr]) -> bool {
    bkeys.len() == 1
        && pkeys.len() == 1
        && int_class(bkeys[0].ty()).is_some()
        && int_class(bkeys[0].ty()) == int_class(pkeys[0].ty())
}

/// Thread-local counters of which index the joins built on this thread used (tests assert that the integer path
/// really ran and that the fallback really ran).
pub mod stats {
    use std::cell::Cell;
    thread_local! {
        pub static INT: Cell<usize> = const { Cell::new(0) };
        pub static BYTES: Cell<usize> = const { Cell::new(0) };
    }
    pub fn int_builds() -> usize {
        INT.with(|c| c.get())
    }
    pub fn byte_builds() -> usize {
        BYTES.with(|c| c.get())
    }
}

/// splitmix64 finalizer: well mixed in every bit, so keys that differ only in high bits (sparse keys such as
/// `j << 40`) and negative keys spread as evenly as dense ones.
#[inline]
fn mix(k: i64) -> u64 {
    let mut z = (k as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Slot of an open-addressing table. `head1` is the newest build row with this key plus one (0 = empty slot), so a
/// zeroed allocation is an empty table.
#[derive(Clone, Copy, Default)]
struct Slot {
    key: i64,
    head1: u32,
}

struct Part {
    slots: Vec<Slot>,
    tbits: u32,
}

/// Build rows are hash-partitioned: the top `pbits` bits of the hash pick the partition, the next `tbits` bits the
/// slot inside it. Linear probing, load factor at most 0.6, key stored inline: a probe touches one cache line in the
/// common case and never compares bytes. Duplicate keys chain through `next` exactly as in the byte-key join (the
/// slot holds the newest row, `next[row]` the previous one); `next` stays empty when no key repeats.
pub struct IntJoin {
    parts: Vec<Part>,
    pbits: u32,
    next: Vec<u32>,
}

impl IntJoin {
    /// Newest build row with this key, or `NIL`.
    #[inline]
    fn lookup(&self, k: i64) -> u32 {
        let h = mix(k);
        let part = &self.parts[if self.pbits == 0 { 0 } else { (h >> (64 - self.pbits)) as usize }];
        let mask = part.slots.len() - 1;
        let mut i = ((h << self.pbits) >> (64 - part.tbits)) as usize;
        loop {
            let s = part.slots[i];
            if s.head1 == 0 {
                return NIL;
            }
            if s.key == k {
                return s.head1 - 1;
            }
            i = (i + 1) & mask;
        }
    }
}

/// Builds with fewer rows than this use one partition (the parallel machinery would cost more than it saves).
const INT_PAR_MIN_ROWS: usize = 1 << 16;
const INT_PART_BITS: u32 = 6;
/// Rows per bucketing task. A constant, so the table layout never depends on the thread count.
const INT_RANGE_ROWS: usize = 1 << 16;

/// Drain `op` into an integer-keyed table (see `IntJoin`). Same observable behaviour as `build_table`: NULL keys are
/// never inserted, duplicate keys chain newest-first, the output row order of probes is identical.
pub fn build_int_table(op: &mut dyn Op, keys: &[BExpr], types: &[Ty], ctx: &ExecCtx) -> Result<Built> {
    stats::INT.with(|c| c.set(c.get() + 1));
    let mut cols: Vec<Column> = types.iter().map(|t| Column::empty(*t)).collect();
    let mut kv: Vec<i64> = Vec::new();
    let mut nulls: Option<Vec<bool>> = None; // allocated only once a NULL key is seen
    while let Some(b) = op.next()? {
        let kc = eval(&keys[0], &b)?;
        match &kc.data {
            ColData::I64(v) | ColData::Dec(v, _) => kv.extend_from_slice(v),
            ColData::Date(v) => kv.extend(v.iter().map(|&d| d as i64)),
            _ => unreachable!("integer join on a non-integer key"),
        }
        match (&mut nulls, &kc.nulls) {
            (Some(n), Some(bn)) => n.extend_from_slice(bn),
            (Some(n), None) => n.resize(kv.len(), false),
            (None, Some(bn)) if bn.iter().any(|&x| x) => {
                let mut n = vec![false; kv.len() - b.len];
                n.extend_from_slice(bn);
                nulls = Some(n);
            }
            _ => {}
        }
        for (c, s) in cols.iter_mut().zip(&b.cols) {
            c.append(s);
        }
    }
    let n = kv.len();
    if n > u32::MAX as usize - 1 {
        return Err(crate::err!("join build side exceeds 4 billion rows"));
    }
    let pbits = if n >= INT_PAR_MIN_ROWS { INT_PART_BITS } else { 0 };
    let np = 1usize << pbits;
    let nranges = n.div_ceil(INT_RANGE_ROWS);
    let (kv, nulls) = (&kv, nulls.as_deref());
    let (parts, dups): (Vec<Part>, Vec<Vec<(u32, u32)>>) = ctx.run(|| {
        // 1. bucket row ids by partition, in parallel over fixed row ranges (ids ascending inside each bucket)
        let buckets: Vec<Vec<Vec<u32>>> = (0..nranges)
            .into_par_iter()
            .map(|r| {
                let (lo, hi) = (r * INT_RANGE_ROWS, ((r + 1) * INT_RANGE_ROWS).min(n));
                let mut b: Vec<Vec<u32>> = (0..np).map(|_| Vec::with_capacity((hi - lo) / np + 8)).collect();
                for i in lo..hi {
                    if nulls.map_or(false, |nn| nn[i]) {
                        continue;
                    }
                    let p = if pbits == 0 { 0 } else { (mix(kv[i]) >> (64 - pbits)) as usize };
                    b[p].push(i as u32);
                }
                b
            })
            .collect();
        // 2. one task per partition fills its own table, walking the ranges in order, so rows are inserted in
        //    ascending id order (this fixes the duplicate chains and therefore the output order)
        (0..np)
            .into_par_iter()
            .map(|p| {
                let count: usize = buckets.iter().map(|b| b[p].len()).sum();
                let tbits = (count * 5 / 3 + 2).next_power_of_two().trailing_zeros().max(1);
                let mut slots = vec![Slot::default(); 1usize << tbits];
                let mask = slots.len() - 1;
                let mut dups: Vec<(u32, u32)> = Vec::new();
                for b in &buckets {
                    for &row in &b[p] {
                        let k = kv[row as usize];
                        let h = mix(k);
                        let mut i = ((h << pbits) >> (64 - tbits)) as usize;
                        loop {
                            let s = &mut slots[i];
                            if s.head1 == 0 {
                                *s = Slot { key: k, head1: row + 1 };
                                break;
                            }
                            if s.key == k {
                                dups.push((row, s.head1 - 1));
                                s.head1 = row + 1;
                                break;
                            }
                            i = (i + 1) & mask;
                        }
                    }
                }
                (Part { slots, tbits }, dups)
            })
            .unzip()
    });
    let mut next = Vec::new();
    if dups.iter().any(|d| !d.is_empty()) {
        next = vec![NIL; n];
        for (row, prev) in dups.into_iter().flatten() {
            next[row as usize] = prev;
        }
    }
    Ok(Built { cols, index: Index::Int(IntJoin { parts, pbits, next }) })
}

/// Probe an integer-keyed table with `n` rows whose key values come from `get`. Appends (probe row, build row)
/// pairs; a NULL key never matches; LEFT JOIN emits `NONE_IDX` for unmatched rows.
fn probe_ints(
    ij: &IntJoin,
    n: usize,
    nulls: Option<&[bool]>,
    get: impl Fn(usize) -> i64,
    left_join: bool,
    pi: &mut Vec<u32>,
    bi: &mut Vec<u32>,
) {
    for i in 0..n {
        let mut matched = false;
        if !nulls.map_or(false, |nn| nn[i]) {
            let mut r = ij.lookup(get(i));
            if r != NIL {
                matched = true;
                loop {
                    pi.push(i as u32);
                    bi.push(r);
                    r = if ij.next.is_empty() { NIL } else { ij.next[r as usize] };
                    if r == NIL {
                        break;
                    }
                }
            }
        }
        if !matched && left_join {
            pi.push(i as u32);
            bi.push(NONE_IDX);
        }
    }
}

/// Drain `op` into a hash table keyed on `keys`.
pub fn build_table(op: &mut dyn Op, keys: &[BExpr], types: &[Ty]) -> Result<Built> {
    stats::BYTES.with(|c| c.set(c.get() + 1));
    let mut cols: Vec<Column> = types.iter().map(|t| Column::empty(*t)).collect();
    let mut table: FxMap<Box<[u8]>, u32> = FxMap::default();
    let mut next: Vec<u32> = Vec::new();
    let mut buf = Vec::new();
    let mut base = 0usize;
    while let Some(b) = op.next()? {
        let kc: Vec<Arc<Column>> = keys.iter().map(|k| eval(k, &b)).collect::<Result<_>>()?;
        next.resize(base + b.len, NIL);
        'rows: for i in 0..b.len {
            buf.clear();
            for c in &kc {
                if c.is_null(i) {
                    continue 'rows;
                }
                c.encode_key(i, &mut buf);
            }
            let id = (base + i) as u32;
            match table.get_mut(&buf[..]) {
                Some(head) => {
                    next[id as usize] = *head;
                    *head = id;
                }
                None => {
                    table.insert(buf.clone().into_boxed_slice(), id);
                }
            }
        }
        for (c, s) in cols.iter_mut().zip(&b.cols) {
            c.append(s);
        }
        base += b.len;
    }
    if base > u32::MAX as usize - 1 {
        return Err(crate::err!("join build side exceeds 4 billion rows"));
    }
    Ok(Built { cols, index: Index::Bytes { table, next } })
}

/// Probe one batch. Output columns are always `left ++ right`.
pub fn probe_batch(
    built: &Built,
    pb: &Batch,
    pkeys: &[BExpr],
    build_left: bool,
    left_join: bool,
    buf: &mut Vec<u8>,
) -> Result<Option<Batch>> {
    let kc: Vec<Arc<Column>> = pkeys.iter().map(|k| eval(k, pb)).collect::<Result<_>>()?;
    let mut pi: Vec<u32> = Vec::new();
    let mut bi: Vec<u32> = Vec::new();
    match &built.index {
        Index::Int(ij) => {
            let c = &kc[0];
            let nulls = c.nulls.as_deref();
            pi.reserve(pb.len);
            bi.reserve(pb.len);
            match &c.data {
                ColData::I64(v) | ColData::Dec(v, _) => probe_ints(ij, pb.len, nulls, |i| v[i], left_join, &mut pi, &mut bi),
                ColData::Date(v) => probe_ints(ij, pb.len, nulls, |i| v[i] as i64, left_join, &mut pi, &mut bi),
                _ => unreachable!("integer join on a non-integer key"),
            }
        }
        Index::Bytes { table, next } => {
            for i in 0..pb.len {
                buf.clear();
                let mut has_null = false;
                for c in &kc {
                    if c.is_null(i) {
                        has_null = true;
                        break;
                    }
                    c.encode_key(i, buf);
                }
                let mut matched = false;
                if !has_null {
                    if let Some(&head) = table.get(&buf[..]) {
                        let mut r = head;
                        while r != NIL {
                            pi.push(i as u32);
                            bi.push(r);
                            r = next[r as usize];
                        }
                        matched = true;
                    }
                }
                if !matched && left_join {
                    pi.push(i as u32);
                    bi.push(NONE_IDX);
                }
            }
        }
    }
    if pi.is_empty() {
        return Ok(None);
    }
    let probe_cols = pb.cols.iter().map(|c| Arc::new(c.take(&pi)));
    let build_cols = built.cols.iter().map(|c| Arc::new(if left_join { c.take_opt(&bi) } else { c.take(&bi) }));
    let cols: Vec<Arc<Column>> =
        if build_left { build_cols.chain(probe_cols).collect() } else { probe_cols.chain(build_cols).collect() };
    Ok(Some(Batch { cols, len: pi.len() }))
}

/// Streams the probe side through an already built table (one instance per chunk in the parallel path).
pub struct ProbeOp {
    pub input: Box<dyn Op>,
    pub built: Arc<Built>,
    pub pkeys: Vec<BExpr>,
    pub build_left: bool,
    pub left_join: bool,
    pub buf: Vec<u8>,
}

impl Op for ProbeOp {
    fn next(&mut self) -> Result<Option<Batch>> {
        while let Some(pb) = self.input.next()? {
            if let Some(out) = probe_batch(&self.built, &pb, &self.pkeys, self.build_left, self.left_join, &mut self.buf)? {
                return Ok(Some(out));
            }
        }
        Ok(None)
    }
}

/// Serial join (reference path): builds lazily on first `next`.
pub struct HashJoin {
    left: Box<dyn Op>,
    right: Box<dyn Op>,
    kind: JoinType,
    lkeys: Vec<BExpr>,
    rkeys: Vec<BExpr>,
    build_left: bool,
    lt: Vec<Ty>,
    rt: Vec<Ty>,
    built: Option<Built>,
    buf: Vec<u8>,
}

impl HashJoin {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        left: Box<dyn Op>,
        right: Box<dyn Op>,
        kind: JoinType,
        lkeys: Vec<BExpr>,
        rkeys: Vec<BExpr>,
        build_left: bool,
        lt: Vec<Ty>,
        rt: Vec<Ty>,
    ) -> HashJoin {
        HashJoin { left, right, kind, lkeys, rkeys, build_left, lt, rt, built: None, buf: Vec::new() }
    }
}

impl Op for HashJoin {
    fn next(&mut self) -> Result<Option<Batch>> {
        if self.built.is_none() {
            self.built = Some(if self.build_left {
                build_table(&mut *self.left, &self.lkeys, &self.lt)?
            } else {
                build_table(&mut *self.right, &self.rkeys, &self.rt)?
            });
        }
        let built = self.built.as_ref().unwrap();
        let (probe_op, pkeys) = if self.build_left { (&mut self.right, &self.rkeys) } else { (&mut self.left, &self.lkeys) };
        while let Some(pb) = probe_op.next()? {
            if let Some(out) =
                probe_batch(built, &pb, pkeys, self.build_left, self.kind == JoinType::Left, &mut self.buf)?
            {
                return Ok(Some(out));
            }
        }
        Ok(None)
    }
}
