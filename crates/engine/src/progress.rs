//! Live counters for `qe --progress`. Plain relaxed atomics; they never influence results or output.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static ROWS_IN: AtomicU64 = AtomicU64::new(0);
static ROWS_OUT: AtomicU64 = AtomicU64::new(0);

pub fn add_rows_in(n: usize) {
    ROWS_IN.fetch_add(n as u64, Relaxed);
}
pub fn add_rows_out(n: usize) {
    ROWS_OUT.fetch_add(n as u64, Relaxed);
}
/// Rows the stream scan has handed to the plan (before any filter or join).
pub fn rows_in() -> u64 {
    ROWS_IN.load(Relaxed)
}
/// Result rows written so far.
pub fn rows_out() -> u64 {
    ROWS_OUT.load(Relaxed)
}
