# EXP-004: Integer-key group table (and, if needed, partitioned aggregation)

- **Status:** Part A and 4b done (2026-10-31); Part B (partition-wise merge) is being evaluated on branch `exp/004-partitioned-merge`
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
