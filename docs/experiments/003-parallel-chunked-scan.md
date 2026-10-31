# EXP-003: Chunk-parallel scan, aggregation and join probe (M2)

- **Status:** Done, kept; exposes two long poles (EXP-004 candidates) (2026-10-31)
- **Idea:** ADR-0001 Ideas 2 and 3 (exact parallel chunks, fixed chunks merged in file order)
- **Code:** `storage/csv/mod.rs` (`CsvSource::plan_chunks`), `exec/par.rs`, `exec/hash_agg.rs` (`AggTable::merge`),
  `exec/hash_join.rs` (`ProbeOp`); uncommitted working tree
- **Machine:** Apple M3 Pro (11 cores, 5 performance + 6 efficiency, as sold), 18 GiB, macOS, warm page cache

## Hypothesis
1. Scan-bound queries scale with cores until memory bandwidth or core heterogeneity limits them;
   target: at least 5x over one thread on 11 cores. *Kill:* under 3x.
2. Output (including `f64` sums and group order) is identical for any thread count, because chunking
   depends only on the file and a fixed 16 MiB chunk size and all merges happen in chunk order.
3. (ADR prediction) Per-thread partial tables plus merge do **not** help when there are millions of
   groups. Expect no scaling on `GROUP BY id3` and possibly a slowdown.

## Design (what was built)
- `plan_chunks`: raw 16 MiB ranges; quote bytes counted per range in parallel, prefix parity gives the quote state at each
  split, then a forward scan to the first newline outside quotes. Exact because stray quotes inside unquoted
  fields are now rejected (`unexpected quote inside an unquoted field`; this is a behaviour change from M1 and from lenient parsers).
- Each chunk gets its own pipeline (scan -> filter -> project -> join probe). The aggregation sink builds one
  `AggTable` per chunk and merges them with a fixed-shape pairwise tree (shape depends only on the chunk count).
  Join: build side drained once, then the table is shared read-only by all probe chunks.
- Non-aggregate queries stream through `ParStream`: windows of `threads` chunks run in parallel, batches are emitted in chunk order.
- `--no-opt` = untouched serial reference path. Any error in the parallel path is re-derived by running the reference path, so
  error text is identical.

## Method
```
python3 bench/exp.py --runs 4 --extra="--threads N" "<sql>" ...      # N = 1 2 4 8 11
python3 bench/run.py --rows 1e7 --label "M2 ..."                     # default threads, appended to BENCHMARKS.md
```
Data: 1e7 rows, `x.csv` and `big.csv` 527 MB each. Raw tables: [threads 1](data/exp003-threads1.md), [2](data/exp003-threads2.md),
[4](data/exp003-threads4.md), [8](data/exp003-threads8.md), [11](data/exp003-threads11.md).

## Results (median seconds; first run excluded as warm-up)

| query | M1 / `--no-opt` | 1 thr | 2 | 4 | 8 | 11 | 1 -> 11 |
|---|---|---|---|---|---|---|---|
| `COUNT(*)` | 0.566 | 0.248 | 0.135 | 0.074 | 0.056 | **0.049** | 5.1x |
| `SUM(v1)` | 0.739 | 0.730 | 0.390 | 0.217 | 0.148 | **0.127** | 5.7x |
| 4 numeric columns | 0.872 | 0.828 | 0.462 | 0.241 | 0.166 | **0.147** | 5.6x |
| `GROUP BY id1` (10 groups) | 0.74 | 0.715 | 0.397 | 0.212 | 0.150 | **0.129** | 5.5x |
| `GROUP BY id2` (1e4 groups) | 0.77 | 0.780 | 0.439 | 0.233 | 0.165 | **0.146** | 5.3x |
| join `small` + group | 0.91 | 0.916 | 0.498 | 0.262 | 0.180 | **0.155** | 5.9x |
| `GROUP BY id3` (6.3e6 groups) | 2.54 | **4.526** | 3.158 | 2.538 | 2.344 | 2.311 | 2.0x |
| join `big` (1e7 x 1e7) | 3.59 | 3.438 | 2.380 | 1.752 | 1.540 | **1.472** | 2.3x |

M1 column: `COUNT(*)`, `SUM(v1)`, 4-column from EXP-002's `--no-opt` run; the others from the M1 rows in `BENCHMARKS.md`.
Full benchmark at default threads (appended to `BENCHMARKS.md`): scans/aggregates/small joins 0.13-0.22 s, i.e. 2.5-4.2 GB/s
(was 0.55-0.7 GB/s); `GROUP BY id3` 2.31 s; join with `big` 1.46 s.

Determinism and correctness on the full 1e7 data (`target/chk`, commands in the session notes below):
5 queries (group-bys, a join, a selective filter returning rows, the 1e7 x 1e7 join) give **byte-identical output at 1 thread,
11 threads and default threads**, and match the `--no-opt` reference within 1e-9 after sorting.

## Interpretation
- Hypothesis 1 holds: 5.1-5.9x from 1 to 11 threads on scan-bound queries (target 5x). The machine has 5 performance and 6 efficiency
  cores, so 11 threads is not 11x; going from 8 to 11 threads gains only ~13%. Whether memory bandwidth or core heterogeneity
  is the limit was not isolated.
- Combined with EXP-002, `COUNT(*)` went 0.566 -> 0.049 s (11.5x) and `SUM(v1)` 0.739 -> 0.127 s (5.8x).
- Hypothesis 3 holds and is worse than predicted: with one thread, `GROUP BY id3` takes 4.5 s versus 2.5 s in M1, because the
  per-chunk tables hold ~10M group entries in total (vs 6.3M final) and the merge re-hashes and sorts them. With 11 threads it only
  returns to M1 speed (2.3 s). This is the case ADR-0001 Idea 4/5 (integer-key table, partitioned aggregation) targets.
- The 1e7 x 1e7 join only reaches 2.3x: the build side is drained and its hash table built serially (not yet profiled; the
  1-thread time of 3.4 s vs 1.5 s at 11 threads suggests roughly 1.3 s is serial). This is the case for Idea 6 / a parallel build.
- Both long poles now dominate every other query by 10x or more.

## Correctness
- New `tests/parallel.rs`: (a) every chunk size from 1 to 70 bytes (plus 97, 128, 255, 1000, 100000) at 1 and 4 threads gives the
  reference result on a CRLF file with quoted commas, doubled quotes, embedded LF and CRLF, empty quoted fields and a quoted newline in
  the last row (no final newline); (b) float sums, averages, min/max and unordered group order are bit-identical for 1, 2, 3, 5, 8, 11 threads;
  (c) a bad value in a late chunk reports the reference path's error text (`row 502`).
- Difftest now checks three configurations per query against SQLite: default, `--no-opt`, and 3 threads with 256-byte chunks.
- **Mutation check:** making the splitter ignore the quote state (`row_start_at_or_after(.., false)`) makes `difftest_small` fail with
  `unterminated quoted field`, so the tests do detect broken splitting. The mutation was reverted.
- Large difftest result: see the addendum at the end.

## Decision
Keep. Next experiments, in order of measured payoff:
1. **EXP-004:** integer-key hash table without per-row allocation, pre-sized; then partitioned aggregation for high cardinality (target: `GROUP BY id3` < 1 s).
2. **EXP-005:** parallel join build / dense-key array join (target: join with `big` < 1 s).
3. Float parsing without `from_utf8` (EXP-001 finding), and `memrchr` for the last column (EXP-002 follow-up).

## Caveats
- 11 "cores" are 5 P + 6 E; results on a symmetric 8-core laptop will scale differently.
- Warm cache only; cold I/O not measured.
- `ParStream` buffers `threads` chunks (~16 MiB x threads of input worth of output) before emitting, so `SELECT ... LIMIT 10` on a huge file
  scans more than necessary. Acceptable for now; not measured.
- Memory: all per-chunk aggregation tables are alive until merged (33 chunks at 527 MB); not measured.
- Behaviour change: a `"` inside an unquoted field is now an error on every path.

## Addendum: large difftest result
`DIFFTEST_SEED=1000 DIFFTEST_QUERIES=800 cargo test --release --test difftest -- --ignored --nocapture`
(log: [data/exp003-difftest.log](data/exp003-difftest.log)). Every query ran on three configurations (default, `--no-opt`,
3 threads with 256-byte chunks) and matched SQLite on all of them:
```
difftest seed 1000: 800 queries OK
difftest seed 1001: 800 queries OK
difftest seed 1002: 800 queries OK
test result: ok. 1 passed; finished in 108.00s
```
