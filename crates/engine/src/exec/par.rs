//! Chunk-parallel execution (M2).
//!
//! A *chunkable* plan (Scan, Filter, Project, and the probe side of a Join) can be instantiated once per
//! row-aligned file chunk: `Chunked::make(i)` returns an independent operator pipeline for chunk `i`.
//! Sinks consume the chunks: `HashAgg` builds one partial table per chunk and tree-merges them in a fixed
//! order; `ParStream` runs windows of chunks in parallel and emits their batches in chunk order.
//!
//! Determinism: chunk boundaries depend only on the file and `chunk_bytes`, and every merge/emit order is
//! by chunk index, so output (including `f64` sums) is identical for any number of threads.
//!
//! Errors: any error from this path is re-derived by running the single-threaded reference path, so error
//! text (row numbers etc.) is identical to `--no-opt`.
use super::hash_agg::{AggInput, HashAgg};
use super::hash_join::{build_table, HashJoin, ProbeOp};
use super::ops::{Filter, Limit, Project, Scan, Sort};
use super::Op;
use crate::plan::{JoinType, Node, Plan};
use crate::storage::csv::CsvSource;
use crate::types::{Batch, Ty};
use crate::{err, Options, Result};
use rayon::prelude::*;
use std::collections::VecDeque;
use std::sync::Arc;

pub const DEFAULT_CHUNK_BYTES: usize = 16 << 20;

pub struct ExecCtx {
    pool: Option<rayon::ThreadPool>,
    pub chunk_bytes: usize,
    pub threads: usize,
    pub agg_partition_groups: usize,
}

impl ExecCtx {
    pub fn new(opts: &Options) -> Result<ExecCtx> {
        let pool = match opts.threads {
            Some(n) => Some(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n.max(1))
                    .build()
                    .map_err(|e| err!("cannot start thread pool: {e}"))?,
            ),
            None => None,
        };
        Ok(ExecCtx {
            pool,
            chunk_bytes: opts.chunk_bytes.unwrap_or(DEFAULT_CHUNK_BYTES),
            threads: opts.threads.unwrap_or_else(rayon::current_num_threads).max(1),
            agg_partition_groups: opts.agg_partition_groups.unwrap_or(262_144),
        })
    }

    /// Run `f` on this query's thread pool (the global rayon pool unless `--threads` was given).
    pub fn run<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        match &self.pool {
            Some(p) => p.install(f),
            None => f(),
        }
    }
}

pub type MakeFn = Arc<dyn Fn(usize) -> Result<Box<dyn Op>> + Send + Sync>;

#[derive(Clone)]
pub struct Chunked {
    pub n: usize,
    pub make: MakeFn,
}

/// Instantiate `plan` per chunk, or `None` if it contains a blocking operator (aggregate, sort, limit).
pub fn chunked(plan: &Plan, ctx: &Arc<ExecCtx>) -> Result<Option<Chunked>> {
    Ok(match &plan.node {
        Node::Scan { table, cols } => {
            let src = Arc::new(CsvSource::open(table, cols, false)?);
            let ranges = Arc::new(ctx.run(|| src.plan_chunks(ctx.chunk_bytes)));
            let n = ranges.len();
            let make: MakeFn =
                Arc::new(move |i| Ok(Box::new(Scan::new(src.reader(ranges[i].0, ranges[i].1))) as Box<dyn Op>));
            Some(Chunked { n, make })
        }
        Node::Filter { input, pred } => chunked(input, ctx)?.map(|c| {
            let (inner, pred) = (c.make, pred.clone());
            let make: MakeFn =
                Arc::new(move |i| Ok(Box::new(Filter { input: inner(i)?, pred: pred.clone() }) as Box<dyn Op>));
            Chunked { n: c.n, make }
        }),
        Node::Project { input, exprs } => chunked(input, ctx)?.map(|c| {
            let (inner, exprs) = (c.make, exprs.clone());
            let make: MakeFn =
                Arc::new(move |i| Ok(Box::new(Project { input: inner(i)?, exprs: exprs.clone() }) as Box<dyn Op>));
            Chunked { n: c.n, make }
        }),
        Node::Join { left, right, kind, lkeys, rkeys, build_left } => {
            let (build_plan, probe_plan, bkeys, pkeys) =
                if *build_left { (left, right, lkeys, rkeys) } else { (right, left, rkeys, lkeys) };
            let Some(probe) = chunked(probe_plan, ctx)? else { return Ok(None) };
            // The build side is drained once, up front (its own scan is parallel); the table is then shared.
            let mut bop = build_par(build_plan, ctx)?;
            let types: Vec<Ty> = build_plan.schema.iter().map(|f| f.ty).collect();
            let built = Arc::new(build_table(&mut *bop, bkeys, &types)?);
            let (pk, bl, lj, inner) = (pkeys.clone(), *build_left, *kind == JoinType::Left, probe.make);
            let make: MakeFn = Arc::new(move |i| {
                Ok(Box::new(ProbeOp {
                    input: inner(i)?,
                    built: built.clone(),
                    pkeys: pk.clone(),
                    build_left: bl,
                    left_join: lj,
                    buf: Vec::new(),
                }) as Box<dyn Op>)
            });
            Some(Chunked { n: probe.n, make })
        }
        _ => None,
    })
}

pub fn build_par(plan: &Plan, ctx: &Arc<ExecCtx>) -> Result<Box<dyn Op>> {
    let types: Vec<Ty> = plan.schema.iter().map(|f| f.ty).collect();
    Ok(match &plan.node {
        Node::Aggregate { input, keys, aggs } => {
            let inp = match chunked(input, ctx)? {
                Some(ch) => AggInput::Parallel(ctx.clone(), ch),
                None => AggInput::Serial(build_par(input, ctx)?),
            };
            Box::new(HashAgg::new(inp, keys.clone(), aggs.clone()))
        }
        Node::Sort { input, keys } => Box::new(Sort::new(build_par(input, ctx)?, keys.clone(), types)),
        Node::Limit { input, n } => Box::new(Limit { input: build_par(input, ctx)?, left: *n }),
        _ => match chunked(plan, ctx)? {
            Some(ch) => Box::new(ParStream::new(ctx.clone(), ch)),
            None => match &plan.node {
                // e.g. HAVING / projection over an aggregate: cheap, stays serial
                Node::Filter { input, pred } => Box::new(Filter { input: build_par(input, ctx)?, pred: pred.clone() }),
                Node::Project { input, exprs } => Box::new(Project { input: build_par(input, ctx)?, exprs: exprs.clone() }),
                Node::Join { left, right, kind, lkeys, rkeys, build_left } => Box::new(HashJoin::new(
                    build_par(left, ctx)?,
                    build_par(right, ctx)?,
                    *kind,
                    lkeys.clone(),
                    rkeys.clone(),
                    *build_left,
                    left.schema.iter().map(|f| f.ty).collect(),
                    right.schema.iter().map(|f| f.ty).collect(),
                )),
                _ => unreachable!("scan is always chunkable"),
            },
        },
    })
}

/// Runs windows of chunks in parallel and yields their batches in chunk order.
pub struct ParStream {
    ctx: Arc<ExecCtx>,
    ch: Chunked,
    next_chunk: usize,
    queue: VecDeque<Batch>,
}

impl ParStream {
    pub fn new(ctx: Arc<ExecCtx>, ch: Chunked) -> ParStream {
        ParStream { ctx, ch, next_chunk: 0, queue: VecDeque::new() }
    }
}

impl Op for ParStream {
    fn next(&mut self) -> Result<Option<Batch>> {
        loop {
            if let Some(b) = self.queue.pop_front() {
                return Ok(Some(b));
            }
            if self.next_chunk >= self.ch.n {
                return Ok(None);
            }
            // Window size only bounds memory and affects LIMIT early-exit; output order never depends on it.
            let end = (self.next_chunk + self.ctx.threads).min(self.ch.n);
            let make = self.ch.make.clone();
            let range = self.next_chunk..end;
            let results: Vec<Result<Vec<Batch>>> = self.ctx.run(|| {
                range
                    .into_par_iter()
                    .map(|i| {
                        let mut op = make(i)?;
                        let mut out = Vec::new();
                        while let Some(b) = op.next()? {
                            out.push(b);
                        }
                        Ok(out)
                    })
                    .collect()
            });
            self.next_chunk = end;
            for r in results {
                self.queue.extend(r?);
            }
        }
    }
}

/// Root wrapper: if the parallel path fails, report the error the single-threaded reference path gives.
pub struct CanonicalErrors {
    pub inner: Box<dyn Op>,
    pub plan: Plan,
}

impl Op for CanonicalErrors {
    fn next(&mut self) -> Result<Option<Batch>> {
        match self.inner.next() {
            Ok(b) => Ok(b),
            Err(e) => Err(self.canonical(e)),
        }
    }
}

impl CanonicalErrors {
    fn canonical(&self, parallel_err: crate::Error) -> crate::Error {
        let reference = Options { no_opt: true, ..Options::default() };
        let run = || -> Result<()> {
            let mut op = super::build_serial(self.plan.clone(), &reference)?;
            while op.next()?.is_some() {}
            Ok(())
        };
        match run() {
            Err(e) => e,
            Ok(()) => err!("{} (internal: the reference path completed without error)", parallel_err.0),
        }
    }
}
