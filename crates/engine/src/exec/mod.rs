//! Physical operators (pull-based, column batches). M1 is single-threaded.
pub mod eval;
pub mod hash;
mod hash_agg;
mod hash_join;
mod ops;
pub mod par;

use crate::plan::{Node, Plan};
use crate::storage::csv::stream::{EmptyScan, StreamInput, StreamScan};
use crate::types::{Batch, Column, Ty};
use std::sync::Arc;
use crate::{Options, Result};

pub trait Op: Send {
    fn next(&mut self) -> Result<Option<Batch>>;
}

pub fn build(plan: Plan, opts: &Options) -> Result<Box<dyn Op>> {
    build_stream(plan, opts, None)
}

/// Like `build`, with one table fed from a non-seekable stream (`--stdin`). The caller has already run
/// `place_stream` on the plan.
pub fn build_stream(plan: Plan, opts: &Options, stream: Option<Arc<StreamInput>>) -> Result<Box<dyn Op>> {
    if opts.no_opt {
        return build_serial(plan, opts, stream.as_ref());
    }
    let ctx = Arc::new(par::ExecCtx::new(opts, stream.clone())?);
    let inner = par::build_par(&plan, &ctx)?;
    let stream_table = stream.map(|s| s.table.clone());
    Ok(Box::new(par::CanonicalErrors { inner, plan, stream_table }))
}

/// Number of scans of `table` in the plan.
pub fn stream_scans(plan: &Plan, table: &str) -> usize {
    match &plan.node {
        Node::Scan { table: t, .. } => (t.name == table) as usize,
        Node::Filter { input, .. }
        | Node::Project { input, .. }
        | Node::Limit { input, .. }
        | Node::Sort { input, .. }
        | Node::Aggregate { input, .. } => stream_scans(input, table),
        Node::Join { left, right, .. } => stream_scans(left, table) + stream_scans(right, table),
    }
}

/// Validate and adapt a bound plan for a query whose `table` comes from a stream.
///
/// * The stream can be read once: the table must appear exactly once.
/// * The stream is always the probe side of a join (a build side is held in memory in full, and a stream has
///   no known size), overriding the size-based choice. Probe and build give the same rows for an INNER join.
/// * For a LEFT JOIN the probe side is the left one, so the stream cannot be its right side.
pub fn place_stream(plan: Plan, table: &str) -> Result<Plan> {
    let n = stream_scans(&plan, table);
    if n == 0 {
        return Err(crate::err!("--stdin table '{table}' is not used by the query"));
    }
    if n > 1 {
        return Err(crate::err!(
            "table '{table}' is read from stdin but appears {n} times in the query; a stream can be scanned only once"
        ));
    }
    place(plan, table)
}

fn place(plan: Plan, table: &str) -> Result<Plan> {
    let Plan { node, schema, est } = plan;
    let node = match node {
        Node::Filter { input, pred } => Node::Filter { input: Box::new(place(*input, table)?), pred },
        Node::Project { input, exprs } => Node::Project { input: Box::new(place(*input, table)?), exprs },
        Node::Limit { input, n } => Node::Limit { input: Box::new(place(*input, table)?), n },
        Node::Sort { input, keys } => Node::Sort { input: Box::new(place(*input, table)?), keys },
        Node::Aggregate { input, keys, aggs } => Node::Aggregate { input: Box::new(place(*input, table)?), keys, aggs },
        Node::Join { left, right, kind, lkeys, rkeys, mut build_left } => {
            let (ls, rs) = (stream_scans(&left, table), stream_scans(&right, table));
            if rs > 0 {
                if kind == crate::plan::JoinType::Left {
                    return Err(crate::err!(
                        "the stdin table '{table}' cannot be the right side of a LEFT JOIN (a stream can only be the probe side)"
                    ));
                }
                build_left = true;
            } else if ls > 0 {
                build_left = false;
            }
            Node::Join {
                left: Box::new(place(*left, table)?),
                right: Box::new(place(*right, table)?),
                kind,
                lkeys,
                rkeys,
                build_left,
            }
        }
        n @ Node::Scan { .. } => n,
    };
    Ok(Plan { node, schema, est })
}

/// The single-threaded reference path (`--no-opt`): generic quote-aware scanner, no fast paths.
pub fn build_serial(plan: Plan, opts: &Options, stream: Option<&Arc<StreamInput>>) -> Result<Box<dyn Op>> {
    let types: Vec<Ty> = plan.schema.iter().map(|f| f.ty).collect();
    Ok(match plan.node {
        Node::Scan { table, cols } if stream.map_or(false, |s| s.table == table.name) => {
            match stream.unwrap().take() {
                Some(r) => Box::new(StreamScan::new(r, table, cols, true, opts.chunk_bytes)),
                None => Box::new(EmptyScan),
            }
        }
        Node::Scan { table, cols } => {
            Box::new(ops::Scan::new(crate::storage::csv::CsvSource::open(&table, &cols, true)?.whole()))
        }
        Node::Filter { input, pred } => Box::new(ops::Filter { input: build_serial(*input, opts, stream)?, pred }),
        Node::Project { input, exprs } => Box::new(ops::Project { input: build_serial(*input, opts, stream)?, exprs }),
        Node::Limit { input, n } => Box::new(ops::Limit { input: build_serial(*input, opts, stream)?, left: n }),
        Node::Sort { input, keys } => Box::new(ops::Sort::new(build_serial(*input, opts, stream)?, keys, types)),
        Node::Aggregate { input, keys, aggs } => {
            Box::new(hash_agg::HashAgg::new(hash_agg::AggInput::Serial(build_serial(*input, opts, stream)?), keys, aggs))
        }
        Node::Join { left, right, kind, lkeys, rkeys, build_left } => {
            let (lt, rt): (Vec<Ty>, Vec<Ty>) =
                (left.schema.iter().map(|f| f.ty).collect(), right.schema.iter().map(|f| f.ty).collect());
            Box::new(hash_join::HashJoin::new(
                build_serial(*left, opts, stream)?,
                build_serial(*right, opts, stream)?,
                kind,
                lkeys,
                rkeys,
                build_left,
                lt,
                rt,
            ))
        }
    })
}

/// Concatenate batches into one set of columns (types given for the empty case).
pub fn concat(batches: &[Batch], types: &[Ty]) -> (Vec<Column>, usize) {
    let mut cols: Vec<Column> = types.iter().map(|t| Column::empty(*t)).collect();
    let mut n = 0;
    for b in batches {
        for (c, s) in cols.iter_mut().zip(&b.cols) {
            c.append(s);
        }
        n += b.len;
    }
    (cols, n)
}
