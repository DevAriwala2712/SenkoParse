//! Physical operators (pull-based, column batches). M1 is single-threaded.
pub mod eval;
pub mod hash;
mod hash_agg;
mod hash_join;
mod ops;
pub mod par;

use crate::plan::{Node, Plan};
use crate::types::{Batch, Column, Ty};
use crate::{Options, Result};

pub trait Op: Send {
    fn next(&mut self) -> Result<Option<Batch>>;
}

pub fn build(plan: Plan, opts: &Options) -> Result<Box<dyn Op>> {
    if opts.no_opt {
        return build_serial(plan, opts);
    }
    let ctx = std::sync::Arc::new(par::ExecCtx::new(opts)?);
    let inner = par::build_par(&plan, &ctx)?;
    Ok(Box::new(par::CanonicalErrors { inner, plan }))
}

/// The single-threaded reference path (`--no-opt`): generic quote-aware scanner, no fast paths.
pub fn build_serial(plan: Plan, opts: &Options) -> Result<Box<dyn Op>> {
    let types: Vec<Ty> = plan.schema.iter().map(|f| f.ty).collect();
    Ok(match plan.node {
        Node::Scan { table, cols } => {
            Box::new(ops::Scan::new(crate::storage::csv::CsvSource::open(&table, &cols, true)?.whole()))
        }
        Node::Filter { input, pred } => Box::new(ops::Filter { input: build_serial(*input, opts)?, pred }),
        Node::Project { input, exprs } => Box::new(ops::Project { input: build_serial(*input, opts)?, exprs }),
        Node::Limit { input, n } => Box::new(ops::Limit { input: build_serial(*input, opts)?, left: n }),
        Node::Sort { input, keys } => Box::new(ops::Sort::new(build_serial(*input, opts)?, keys, types)),
        Node::Aggregate { input, keys, aggs } => {
            Box::new(hash_agg::HashAgg::new(hash_agg::AggInput::Serial(build_serial(*input, opts)?), keys, aggs))
        }
        Node::Join { left, right, kind, lkeys, rkeys, build_left } => {
            let (lt, rt): (Vec<Ty>, Vec<Ty>) =
                (left.schema.iter().map(|f| f.ty).collect(), right.schema.iter().map(|f| f.ty).collect());
            Box::new(hash_join::HashJoin::new(
                build_serial(*left, opts)?,
                build_serial(*right, opts)?,
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
