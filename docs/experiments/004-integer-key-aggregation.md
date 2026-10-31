# EXP-004: Integer-key group table (and, if needed, partitioned aggregation)

- **Status:** In progress (Part A and output formatting done; Part B hypothesis below, written before its code, 2026-10-31)
- **Idea:** ADR-0001 Idea 5 (inline integer keys, no per-row allocation), then Idea 4 (partitioned aggregation) only if needed
- **Machine:** Apple M3 Pro, 11 cores (5P + 6E), 18 GiB, macOS, warm page cache

## Baseline (from EXP-003, medians)
`SELECT id3, SUM(v1) FROM x GROUP BY id3` (6,320,247 groups): reference/M1 2.54 s, parallel 1 thread 4.53 s, 11 threads 2.31 s.
`GROUP BY id1` (10 groups) 0.129 s and `GROUP BY id2` (1e4 groups) 0.146 s at 11 threads.

## Hypothesis
The cost is in how groups are stored, not in the arithmetic: every group owns a heap-allocated `Box<[u8]>` key (16-byte fat pointer
+ allocation of ~9 bytes + allocator overhead), the byte-keyed hash table is large and cache-hostile, the merge has to
collect and sort all entries by group id, and each group's aggregate state is initialised through a generic enum.
A table specialised for a single integer-like key (`i64`, decimal, date) stores `u32` slots plus one `i64` per group, never allocates per
row, and merges in group order without sorting.
- Part A prediction: `GROUP BY id3` at 1 thread falls from 4.5 s to **under 2.5 s** (at least the M1 speed), at 11 threads from 2.31 s to **under 1.5 s**.
  Low-cardinality group-bys (`id1`, `id2`) improve slightly or stay equal (they are scan-bound).
- **Kill threshold for Part A:** if `GROUP BY id3` at 11 threads is not at least 25% faster than 2.31 s, the allocation/merge theory is wrong
  and the profile must be redone before anything else.
- Part B (partitioned aggregation: hash-partition rows by key, aggregate each partition independently, no merge) is only built if Part A
  leaves `GROUP BY id3` above **1 s** at 11 threads. Reason: the last levels of the merge tree combine two multi-million-group tables on one thread,
  and that serial tail does not shrink with more cores.

## Method
```
python3 bench/exp.py --runs 5 --extra="--threads N" "<sql>"   # N = 1, 11
```
Queries: `GROUP BY id3` (6.3e6 groups), `GROUP BY id2`, `GROUP BY id1`, plus a string-key group-by (`id4`, unchanged code path) as a control.
Before/after in the same session, same data (seed 42, 1e7 rows).

## Correctness plan (before the numbers count)
- Group order, NULL group, decimal and date keys, growth past the initial table size and the merge path must match the reference:
  new test in `tests/parallel.rs` (5,000 distinct integer keys plus NULLs, decimal and date keys, many tiny chunks, 1 and 4 threads).
- Difftest (three configurations) and the large difftest stay green.

---
## Part A result (integer-key table), measured before Part B

Raw: [before](data/exp004-before.md), [after](data/exp004-after-part-a.md). Medians, seconds:

| query | before, 1 thr | after, 1 thr | before, 11 thr | after, 11 thr |
|---|---|---|---|---|
| `GROUP BY id3` (6.3e6 groups) | 4.410 | 3.100 | 2.279 | **1.651** |
| `GROUP BY id2` (1e4 groups) | 0.799 | 0.740 | 0.147 | 0.130 |
| `GROUP BY id1` (10 groups) | 0.724 | 0.668 | 0.129 | 0.122 |
| `GROUP BY id4` (string key, control) | 0.751 | 0.781 | 0.133 | 0.134 |

The serial `--no-opt` reference path (no per-chunk duplication) takes 1.97 s on the same query.
- Prediction missed: 1-thread `id3` is 3.1 s, not under 2.5 s; 11 threads 1.65 s, not under 1.5 s. The kill threshold (25% faster at 11 threads) was cleared: -27.6%.
- Control (string key) unchanged, as expected; small-cardinality queries gained 7-12%.

## Where the remaining time goes (Part A follow-up measurements)
Raw: [agg vs output split](data/exp004-split-agg-vs-output.md), profile [data/exp004-sample-q4-after-a.txt](data/exp004-sample-q4-after-a.txt).
1. Same query with `HAVING SUM(v1) < 0` (zero output rows) takes 1.005 s; the full query 1.644 s. So **0.64 s is writing 6.3M result rows**.
   The output-phase profile ([data/exp004-sample-q4-output-phase.txt](data/exp004-sample-q4-output-phase.txt)) shows float formatting
   plus a heap allocation per cell (`format!`, `to_string`).
2. Temporary phase timers inside the aggregation (removed again) for the zero-row query: per-chunk tables **0.18 s** (parallel),
   merge tree **0.78 s**, finish 0.02 s. The merge is the serial tail: its last levels merge two tables of ~3M groups on one thread.

## EXP-004b: allocation-free output formatting
Hypothesis: writing numbers straight into the output buffer removes the per-cell allocations and cuts the 0.64 s output cost by about half.
Result (raw: [data/exp004b-after-output.md](data/exp004b-after-output.md)): full `GROUP BY id3` 1.644 -> **1.289 s** (output share 0.64 -> ~0.28 s); zero-row variant unchanged at 1.008 s.
Float formatting itself (shortest round-trip, `core::fmt`) remains; a faster algorithm would need a dependency (not added).
Tests: new `output_formatting_of_numbers_and_dates` (extreme integers, scales 0/2/18, negative decimals, dates before 1970).

## Part B: partition-wise parallel merge (hypothesis, written before the code)
ADR-0001 Idea 4 proposed radix-partitioning rows before aggregation. The phase timers show the build is already cheap and parallel (0.18 s)
and only the merge is serial, so a smaller change targets the measured cost: keep per-chunk tables, but merge them **by key partition in parallel**.
Each table buckets its groups into 64 hash partitions; partition `p` is merged by one task that walks the tables in chunk order and folds in only
its own groups; partitions are independent, so there is no serial tail. Output is emitted partition by partition, no concatenation.
- Prediction: merge 0.78 s -> about 0.15-0.25 s on 11 threads (the 8 -> 11 thread scaling seen in EXP-003 suggests ~5x, not 11x).
  Whole `GROUP BY id3` (with output) 1.29 s -> **under 0.8 s**; the zero-row variant 1.0 s -> about 0.45 s.
- **Kill threshold:** if the zero-row variant is not under 0.7 s, revert (the complexity is not worth it).
- Used only when the per-chunk tables hold more than 262,144 groups in total and the key is a single integer-like column; otherwise the existing
  tree merge runs, so low-cardinality queries are untouched. Strings and multi-column keys keep the tree merge (known limitation).
- Determinism: the partition count and function are fixed and each partition folds chunks in index order, so results are identical for any thread count.
  **Trade-off, stated up front:** the *order of groups* in unordered results becomes partition order (still thread-independent) instead of first-seen
  order when this path is taken. SQL gives no order guarantee without `ORDER BY`; tests that compare against the reference order will force this path and compare as multisets.
- Float sums in this path are folded sequentially in chunk order (tree merge is pairwise), so the exact last bits can differ between the two strategies; each is deterministic.
- Tests first: an option to force the path (`agg_partition_groups`), the stress configuration of the difftest uses it, plus a dedicated test with
  5,000+ keys, NULLs, decimals and dates at several chunk sizes and thread counts.

## Part B result

### First measurement: anti-scaling (raw: [first](data/exp004-partb-first-measure-bimodal.md), [distribution](data/exp004-partb-thread-distribution-before-fix.md))
Zero-row variant (`... HAVING SUM(v1) < 0`), median seconds: 2 threads 1.04, 4 threads **0.79**, 6 threads 1.46, 8 threads 1.57, 11 threads 1.54
(11-thread runs bimodal: 1.16-1.65). Faster at 4 threads than at 11 was wrong for a merge with no shared state.
`/usr/bin/time`: system time negligible (0.1-0.2 s) but **user CPU 2.9 s at 4 threads vs 11.8-14.6 s at 11 threads** for the same work, so threads were
burning cycles rather than doing more work (efficiency cores being 2-3x slower does not explain 4x the CPU).

### Diagnosis and fix
Cause found by reading the merge loop: `merge_one_int` did `self.spec.clone()` (an atomic reference-count increment and decrement on an `Arc`) for
**every merged group** (~10M), and all 64 partition tables share the same `Arc<AggSpec>`. Every thread hit the same cache line. Fix: the caller passes
`&[AggCall]` and nothing is cloned per group. (The same pattern exists once per batch in `consume`, 64K rows apart, which is negligible.)
First check after the fix: 11 threads zero-row ~1.5 s -> 0.45 s and user CPU 14 s -> 3.5 s; full numbers below.

### Final numbers after the fix (raw: [data/exp004-partb-fixed.md](data/exp004-partb-fixed.md)), median seconds

| query | 1 thr | 2 | 4 | 8 | 11 |
|---|---|---|---|---|---|
| `GROUP BY id3 HAVING SUM(v1) < 0` (zero output rows) | 1.827 | 0.949 | 0.524 | 0.485 | **0.421** |
| `GROUP BY id3` (6.3e6 rows written) | 2.151 | 1.236 | 0.820 | 0.990* | **0.799** |

\* one 2.587 s outlier among the 8-thread runs (others 0.77-0.99); unexplained, treated as machine noise, not hidden. The 11-thread full-query runs
were 0.731-1.209 (min 0.738).
Full query at 11 threads: 2.279 s before EXP-004 -> 1.289 s (Part A + 4b) -> **0.799 s** (Part B). The path only triggers above 262,144 total groups, so
low-cardinality queries are untouched (`GROUP BY id2` and `id1` unchanged within noise).

### Against the predictions
- Zero-row variant: prediction "about 0.45 s", kill threshold "not under 0.7 s": measured 0.421 s. Kept.
- Whole query: prediction "under 0.8 s": 0.799 s median (min 0.738) is at the line, not clearly under it. What is left is ~0.28 s of serial output writing,
  ~0.18 s building the per-chunk tables, and the partition merge.
- At 1 thread the path costs 2.15 s vs 1.97 s for the serial `--no-opt` path (per-chunk duplication).

### Correctness
- New `partitioned_merge_matches_reference_and_is_thread_independent` (8,000 rows; int, decimal and date keys with NULLs; COUNT DISTINCT, AVG, float sums; 1/2/5/11 threads must be
  **identical** including order and float bits; equal to the reference after sorting, floats within 1e-9). The difftest stress configuration forces this path for every random query.
- Mutation check: dropping the NULL group from the buckets fails `difftest_small` (row counts off by one in many queries); reverted.
- Full scale (6,320,247 groups, 5 aggregates): output byte-identical at 1, 5 and 11 threads (md5 `a96fbe89ce1b3805c7f88c721ed59f6a`), and every group equals the `--no-opt` result (counts exact, floats within 1e-9).
- Large difftest: see the addendum at the end.

### Decision
Keep. Group order in unordered results is partition order on this path (documented trade-off, thread-independent). Multi-column and string keys still use the tree merge: next candidate if a
string-key high-cardinality benchmark matters.

### Lesson recorded
A shared `Arc` cloned in a hot loop is a scalability bug. When a parallel phase gets *slower* with more threads, compare user CPU time first: here it exposed the problem in one command.

## Addendum: large difftest result (after the fix)
`DIFFTEST_SEED=1000 DIFFTEST_QUERIES=800 cargo test --release --test difftest -- --ignored --nocapture`
(log: [data/exp004-partb-difftest.log](data/exp004-partb-difftest.log)). Each query ran on the default path, `--no-opt`, and 3 threads / 256-byte chunks /
forced partition-wise merge, and matched SQLite on all three:
```
difftest seed 1000: 800 queries OK
difftest seed 1001: 800 queries OK
difftest seed 1002: 800 queries OK
test result: ok. 1 passed; finished in 108.88s
```
