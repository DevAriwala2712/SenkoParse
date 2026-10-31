//! Hash join. The build side is fully materialized; rows with a NULL key are never inserted and
//! NULL probe keys never match. Duplicate build keys chain through `next`.
use super::eval::eval;
use super::hash::FxMap;
use super::Op;
use crate::plan::{BExpr, JoinType};
use crate::types::*;
use crate::Result;
use std::sync::Arc;

const NIL: u32 = u32::MAX;

pub struct Built {
    cols: Vec<Column>,
    table: FxMap<Box<[u8]>, u32>,
    next: Vec<u32>,
}

/// Drain `op` into a hash table keyed on `keys`.
pub fn build_table(op: &mut dyn Op, keys: &[BExpr], types: &[Ty]) -> Result<Built> {
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
    Ok(Built { cols, table, next })
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
            if let Some(&head) = built.table.get(&buf[..]) {
                let mut r = head;
                while r != NIL {
                    pi.push(i as u32);
                    bi.push(r);
                    r = built.next[r as usize];
                }
                matched = true;
            }
        }
        if !matched && left_join {
            pi.push(i as u32);
            bi.push(NONE_IDX);
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
