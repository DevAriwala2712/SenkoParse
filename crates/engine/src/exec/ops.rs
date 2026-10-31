use super::eval::eval;
use super::{concat, Op};
use crate::plan::BExpr;
use crate::storage::csv::CsvReader;
use crate::types::*;
use crate::Result;
use std::sync::Arc;

pub struct Scan {
    reader: CsvReader,
}

impl Scan {
    pub fn new(reader: CsvReader) -> Scan {
        Scan { reader }
    }
}

impl Op for Scan {
    fn next(&mut self) -> Result<Option<Batch>> {
        self.reader.next_batch()
    }
}

pub struct Filter {
    pub input: Box<dyn Op>,
    pub pred: BExpr,
}

impl Op for Filter {
    fn next(&mut self) -> Result<Option<Batch>> {
        while let Some(b) = self.input.next()? {
            let p = eval(&self.pred, &b)?;
            let ColData::Bool(v) = &p.data else { unreachable!() };
            let idx: Vec<u32> =
                (0..b.len).filter(|&i| v[i] && !p.is_null(i)).map(|i| i as u32).collect();
            if idx.len() == b.len {
                return Ok(Some(b));
            }
            if !idx.is_empty() {
                return Ok(Some(b.take(&idx)));
            }
        }
        Ok(None)
    }
}

pub struct Project {
    pub input: Box<dyn Op>,
    pub exprs: Vec<BExpr>,
}

impl Op for Project {
    fn next(&mut self) -> Result<Option<Batch>> {
        match self.input.next()? {
            None => Ok(None),
            Some(b) => {
                let cols = self.exprs.iter().map(|e| eval(e, &b)).collect::<Result<Vec<_>>>()?;
                Ok(Some(Batch { cols, len: b.len }))
            }
        }
    }
}

pub struct Limit {
    pub input: Box<dyn Op>,
    pub left: u64,
}

impl Op for Limit {
    fn next(&mut self) -> Result<Option<Batch>> {
        if self.left == 0 {
            return Ok(None);
        }
        match self.input.next()? {
            None => Ok(None),
            Some(b) => {
                if (b.len as u64) <= self.left {
                    self.left -= b.len as u64;
                    Ok(Some(b))
                } else {
                    let idx: Vec<u32> = (0..self.left as u32).collect();
                    self.left = 0;
                    Ok(Some(b.take(&idx)))
                }
            }
        }
    }
}

/// Full in-memory sort (stable). NULLs first for ASC, last for DESC (SQLite semantics).
pub struct Sort {
    input: Box<dyn Op>,
    keys: Vec<(usize, bool)>,
    types: Vec<Ty>,
    out: Option<(Vec<Column>, Vec<u32>, usize)>,
}

impl Sort {
    pub fn new(input: Box<dyn Op>, keys: Vec<(usize, bool)>, types: Vec<Ty>) -> Sort {
        Sort { input, keys, types, out: None }
    }
}

impl Op for Sort {
    fn next(&mut self) -> Result<Option<Batch>> {
        if self.out.is_none() {
            let mut batches = Vec::new();
            while let Some(b) = self.input.next()? {
                batches.push(b);
            }
            let (cols, n) = concat(&batches, &self.types);
            drop(batches);
            let mut order: Vec<u32> = (0..n as u32).collect();
            let keys = &self.keys;
            order.sort_by(|&a, &b| {
                for &(k, desc) in keys {
                    let o = cols[k].cmp_rows(a as usize, b as usize);
                    let o = if desc { o.reverse() } else { o };
                    if o.is_ne() {
                        return o;
                    }
                }
                std::cmp::Ordering::Equal
            });
            self.out = Some((cols, order, 0));
        }
        let (cols, order, pos) = self.out.as_mut().unwrap();
        if *pos >= order.len() {
            return Ok(None);
        }
        let end = (*pos + crate::storage::csv::BATCH_ROWS).min(order.len());
        let idx = &order[*pos..end];
        let b = Batch { cols: cols.iter().map(|c| Arc::new(c.take(idx))).collect(), len: idx.len() };
        *pos = end;
        Ok(Some(b))
    }
}
