# Future work and targets

These are **hypotheses with tests that would prove them wrong**, not claims. Nothing here is implemented
unless the status board says so. We do not claim that SenkoParse will beat DuckDB overall: DuckDB is mature
and broad. The aim is to close the measured gaps and to widen the niche where a small, auditable engine
differs in kind, not only in speed.

## Baseline (what is true today)

Interim 1e8-row benchmark, Apple M3 Pro, swap 0, warm cache, all cores, min wall time, DuckDB 2.0.0
([BENCHMARKS.md](../BENCHMARKS.md), section "Interim, after EXP-008 (integer join)"; the final run with cold cache
is still to do):

| Result | Queries |
|---|---|
| qe faster or tied (0.75x to 0.91x of DuckDB's time) | Q1, Q2, Q3, Q6, Q7 |
| qe slower | Q8 1.37x, Q9 2.95x, Q5 3.13x, Q4 4.04x |

Streaming (EXP-005-stream, M3 Pro re-run): peak RSS stays near 48 MiB from 375 MB to 3.7 GB of piped input with
no temp files; DuckDB 2.0.0 piped uses about 340 MiB and writes 0.2 to 5.3 GiB of temp files. qe is 3.4x to 6.0x
faster than DuckDB piped and about 1.2x faster than DuckDB reading the same file by name (10x size). One query, one
reconstructed data shape.

## Directions

Each row gives the mechanism, a target, and the result that would show the idea does not work. Every one must keep
the differential test green and add a before/after row to BENCHMARKS.md before it is claimed.

### Close the measured gaps

| Direction | Mechanism | Target | Falsifier |
|---|---|---|---|
| Scatter-first parallel aggregation (Q4) | With about 6e7 groups from 1e8 rows nearly every key is distinct, so per-chunk tables reduce nothing and only add merge work. Partition rows by key hash first, then aggregate each partition on its own thread. Choose this path from a sample of the distinct ratio. | Q4 within 1.5x of DuckDB | Less than 1.3x gain on Q4 |
| Integer-keyed distinct sets (Q5) | The per-chunk `COUNT(DISTINCT)` sets use boxed keys (one allocation and byte compare per new value). Store `i64` keys inline in a flat table, per worker thread (see ADR-0004 and EXP-009). | Q5 within 2x of DuckDB | Less than 1.5x gain on Q5 |
| Probe prefetching and a direct-address path for dense keys (Q9) | The 1e8-entry join table is cache-miss bound. Prefetching hides the latency for any key type; a direct array replaces hashing when the key range is small (measured, with hash fallback). | Q9 within 1.5x of DuckDB | Less than 1.3x gain on Q9 |
| Aggregation table reuse (Q4) | Reuse one table per worker across chunks (patch kept from EXP-007; Q4 gained 18 to 31% in the cloud container, not yet confirmed on the M3 Pro). | Q4 at least 15% faster on the M3 Pro | Under 15%, or Q2/Q3 more than 5% slower |
| Cheaper per-row work on Q8 | Q8's ratio is the same at 1 thread and at all cores, so the cost is per row. Profile first. | Q8 within 1.15x | Profile shows no single dominant cost |

### Widen the streaming and memory niche

| Direction | Mechanism | Target | Falsifier |
|---|---|---|---|
| Multi-threaded stream parsing | The stream is parsed on one thread. Quote-correct block cuts already exist, so blocks could be parsed in parallel and merged in order. | Several times today's piped throughput | No speedup beyond 2 threads |
| Bounded memory beyond input buffering | Group tables and join build sides still grow with their own size. Add Grace-style partitioning to disk when an estimate exceeds a limit. | Flat memory on high-cardinality GROUP BY and large joins over a stream, measured | Memory still grows with input |
| Memory-capped file mode | File mode maps whole files (peak RSS 8 to 10 GiB at 1e8). Releasing pages behind the scan might hold it near a fixed cap. | A measured run under a 4 GiB limit | Slower than DuckDB's spill under the same limit, or killed |
| Right side of LEFT JOIN, `IN (SELECT ...)` | Remove current scope limits so more queries can use the stream path. | The queries run, equal to SQLite | Results differ from SQLite |

### Robustness and ease of use

| Direction | Mechanism | Target | Falsifier |
|---|---|---|---|
| Schema inference | Today `schema.toml` is required and there is no type sniffing. Infer types from a sample; `schema.toml` still wins. | Runs on a plain CSV with a header | Wrong types on a test set of mixed files |
| Cold-cache read-ahead | Cold first runs were up to 2.7x slower than warm in the cloud container. Test larger read-ahead and access hints. | Cold gap under 1.5x | No change from the hints |
| Vectorized float parsing | `f64::from_str` was 7.5% of Q3's profile in the container. | A few percent on scan-bound queries | Under 2% |
| Optional columnar cache, off by default | Skips parsing on repeat queries. Must be reported separately from cold results, because the cold path is the one that gets timed. | Repeat queries several times faster | Stale-cache bugs, or no gain |

## Where this points

DuckDB is the general-purpose engine. SenkoParse is the small engine whose behaviour can be read end to end,
that stays correct against an independent oracle, and that keeps memory bounded on pipes and on constrained
machines. The likeliest places to beat DuckDB are streaming and a capped-memory file mode. Both are testable and
neither is proven beyond the single streaming query above.

## Rules for moving an item out of this file

1. Write the hypothesis and the kill rule in `docs/experiments/` before the code.
2. Keep the differential test and the thread-identity tests green.
3. Report relative gains with the machine named; final claims come from the M3 Pro tables in BENCHMARKS.md.
4. Update `docs/status.md` (claims ledger) only from those tables.
