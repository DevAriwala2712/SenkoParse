//! From-scratch SQL engine over CSV files. No database/dataframe library inside.
pub mod error;
pub mod exec;
pub mod output;
pub mod plan;
pub mod sql;
pub mod storage;
pub mod types;

pub use error::{Error, Result};

use std::path::Path;
use types::{Ty, Value};

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Reference path: single-threaded, generic quote-aware scanner, no fast paths.
    /// Every optimized path must produce identical results (and identical errors).
    pub no_opt: bool,
    /// Worker threads for the parallel path (default: all cores). Results never depend on this.
    pub threads: Option<usize>,
    /// Target bytes per scan chunk (default 16 MiB). Results never depend on this either; tests shrink it
    /// to force chunk boundaries inside quoted fields.
    pub chunk_bytes: Option<usize>,
    /// Use the partition-wise parallel merge for single-integer-key aggregates once the per-chunk tables
    /// hold more than this many groups in total (default 262,144). Tests set 0 to force it.
    pub agg_partition_groups: Option<usize>,
}

pub struct Prepared {
    pub names: Vec<String>,
    pub types: Vec<Ty>,
    pub root: Box<dyn exec::Op>,
}

pub fn prepare(data_dir: &Path, sql: &str, opts: &Options) -> Result<Prepared> {
    prepare_inner(data_dir, sql, opts, None)
}

/// Like `prepare`, but table `table` is read from `input` (a non-seekable stream such as stdin) instead of
/// `<table>.csv`. The schema still comes from `schema.toml`. The stream is read once, single-threaded, in
/// bounded memory (see `storage::csv::stream`); `opts.chunk_bytes` is the block size.
pub fn prepare_stream(
    data_dir: &Path,
    sql: &str,
    opts: &Options,
    table: &str,
    input: Box<dyn std::io::Read + Send>,
) -> Result<Prepared> {
    prepare_inner(data_dir, sql, opts, Some(std::sync::Arc::new(storage::StreamInput::new(table, input))))
}

fn prepare_inner(
    data_dir: &Path,
    sql: &str,
    opts: &Options,
    stream: Option<std::sync::Arc<storage::StreamInput>>,
) -> Result<Prepared> {
    let catalog = storage::Catalog::load(data_dir)?;
    if let Some(s) = &stream {
        catalog.table(&s.table)?;
    }
    let stmt = sql::parse(sql)?;
    let (mut plan, names) = plan::bind(&stmt, &catalog)?;
    if let Some(s) = &stream {
        plan = exec::place_stream(plan, &s.table)?;
    }
    let types = plan.schema.iter().map(|f| f.ty).collect();
    let root = exec::build_stream(plan, opts, stream)?;
    Ok(Prepared { names, types, root })
}

/// Run a query and collect all rows as `Value`s (used by tests).
pub fn run_collect(data_dir: &Path, sql: &str) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    run_collect_opts(data_dir, sql, &Options::default())
}

pub fn run_collect_opts(data_dir: &Path, sql: &str, opts: &Options) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut p = prepare(data_dir, sql, opts)?;
    let mut rows = Vec::new();
    while let Some(b) = p.root.next()? {
        for i in 0..b.len {
            rows.push(b.cols.iter().map(|c| c.value(i)).collect());
        }
    }
    Ok((p.names, rows))
}

/// `run_collect_opts` with table `table` read from `input` instead of its file.
pub fn run_collect_stream(
    data_dir: &Path,
    sql: &str,
    opts: &Options,
    table: &str,
    input: Box<dyn std::io::Read + Send>,
) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut p = prepare_stream(data_dir, sql, opts, table, input)?;
    let mut rows = Vec::new();
    while let Some(b) = p.root.next()? {
        for i in 0..b.len {
            rows.push(b.cols.iter().map(|c| c.value(i)).collect());
        }
    }
    Ok((p.names, rows))
}
