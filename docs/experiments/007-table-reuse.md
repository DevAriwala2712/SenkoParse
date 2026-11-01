# EXP-007: One reusable AggTable per worker instead of one fresh table per chunk

- **Status:** **Kill rule not met, engine change reverted** (hypothesis and kill rule were written before the code). 2026-11-01. The patch is kept in `docs/experiments/data/007-table-reuse.patch`; the thread-identity test stays.
- **Follows:** EXP-006 (`006-scaling-diagnosis.md`).
- **Machine:** same as EXP-006: cloud container, 4 vCPU Intel Xeon @ 2.10 GHz, 16 GiB RAM, no swap, Linux 6.18 (NOT the M3 Pro). Warm page cache. Data in git-ignored `data_1e7/`, `data_3e7/`, `data_5e7/` (seed 42).

## Hypothesis
EXP-006 found that Q3 (1e4 groups) spends its excess time allocating and first-touching one `AggTable` per 16 MiB chunk (`AggTable::new_group` 10% of samples as page faults, `Vec` growth via `realloc` 5%, `clear_page_erms` 19% in total), and that 256 MiB chunks (far fewer tables) made Q3 24% faster at 5e7.

Claim: if each rayon worker keeps ONE `AggTable` and reuses it for every chunk it processes, then at the end of a chunk
1. the groups actually seen are drained into a compact partial (exact-capacity keys and accumulators, no hash index),
2. the working table is cleared **without freeing its memory** (hash index, key and state vectors keep their capacity),

the per-chunk allocation, index growth and page zeroing disappear, and Q3 gets faster at large N. The partials are still merged in the fixed per-chunk order (same tree, same partitioned path), so `f64` results do not change with the thread count.

What I expect, and what I do NOT expect: the partials still hold `chunks x groups` states, so the page-zeroing of *that* memory remains and the gain is smaller than the 24% that `--chunk-bytes 256 MiB` gave (which also removed the partials). Prediction: Q3 -10% to -25% at 5e7. Q2 (10 groups) should be unchanged, because 10 groups per chunk costs nothing to allocate. Q4 (millions of groups) is the risk: the drain copies keys and moves states once more. No kill rule on Q4, it is reported either way.

## Kill rule (fixed now)
Compared with the current build (`a983f8f`), min of 2 warm runs per cell, all cores, default chunk size:
- **Q3 must be at least 15% faster at 5e7.** Otherwise: revert the engine change, keep this file with the numbers, say the hypothesis failed.
- **Q2 must not be slower than 5% at any of 1e7, 3e7, 5e7.** Otherwise revert and report.
- Tests must stay green (`cargo test`, difftest default / `--no-opt` / tiny-chunk), plus a new test that results are bit-identical for 1, 2 and 4 threads.
- Results noise: with 2 runs per cell, a gap under about 5% is noise; the 15% bar is above it.

## Method
`/tmp/.../run.py` wrapper, as in EXP-006 (whole-process wall, child `ru_maxrss`). Before = binary built from `a983f8f` (copied before editing); after = binary from the working tree. Queries: Q2 `SELECT id1, SUM(v1) FROM x GROUP BY id1`, Q3 `SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2`, Q4 `SELECT id3, SUM(v1) FROM x GROUP BY id3`. Before and after runs are interleaved per cell to spread machine drift.

## What was built (and reverted)
`AggTable::take_partial` (drain the groups seen into an exact-capacity partial: `ikeys`, `key_out`, `states`, and for non-int keys the key bytes sorted by group id; no hash index) + `AggTable::reset` (clear every vector, the `IntMap` slots and the `FxMap` without freeing); `HashAgg::run` used rayon `map_init` so each task keeps one working table. `merge` rebuilds the hash index lazily only when a compact partial is the merge target (the pairwise-tree left side). The pairwise tree, the partitioned merge and the chunk order were not touched, so float results are the same for any thread count. Plus `Column::clear` and `IntMap::clear` / `IntMap::unindexed`. Diff: `docs/experiments/data/007-table-reuse.patch` (203 lines, applies to `a983f8f`).

## Correctness
- `cargo test --release` green with the change: unit, difftest (default / `--no-opt` / 3 threads, 256-byte chunks, forced partitioned merge), edge cases, parallel, stream. The ignored large difftest was not run.
- New test `tests/parallel.rs::table_reuse_gives_identical_results_for_1_2_and_4_threads` (kept): 40,000 rows, 600-byte chunks (hundreds of chunks per run), int key with NULL group, string key with NULL group, composite key, global aggregate, `f64` SUM/AVG/MIN/MAX, `COUNT(DISTINCT)`, string MIN/MAX. 1, 2 and 4 threads must be bit-identical on both the tree merge and the forced partitioned merge; the exact (integer) aggregates must also equal the single-chunk and `--no-opt` results as multisets. It passes on the new code and on the original code.
- Mutation check: removing `self.null_gid = None` from `reset` makes this test fail (and two older parallel tests).

## Results (4 vCPU Xeon, warm, all cores, default 16 MiB chunks, before = `a983f8f`, after = working tree; runs interleaved, wall seconds of the whole process, RSS = that process's own peak)

| query | rows | before min s (runs) | after min s (runs) | change (min) | RSS MiB before -> after |
|---|---|---|---|---|---|
| Q2 (10 groups) | 1e7 | 0.38 (0.38, 0.42) | 0.39 (0.40, 0.39) | +0.4% | 520 -> 513 |
| Q2 | 3e7 | 1.18 (1.21, 1.18) | 1.22 (1.22, 1.24) | +3.0% | 1601 -> 1577 |
| Q2 | 5e7 | 2.04 (2.04, 2.08) | 1.93 (1.93, 2.17) | -5.5% | 2683 -> 2641 |
| Q3 (1e4 groups) | 1e7 | 0.47 (0.52, 0.47) | 0.45 (0.48, 0.45) | -2.8% | 553 -> 539 |
| Q3 | 3e7 | 1.48 (1.48, 1.49) | 1.46 (1.46, 1.50) | -1.7% | 1859 -> 1811 |
| **Q3** | **5e7** | 3.04 (3.04, 3.12) | 2.66 (2.90, 2.66) | **-12.6%** | 3440 -> 3310 |
| Q4 (~all-distinct id3) | 1e7 | 2.38 (2.39, 2.38) | 1.96 (1.96, 2.00) | -17.8% | 1320 -> 1177 |
| Q4 | 3e7 | 8.46 (9.17, 8.46) | 5.83 (6.69, 5.83) | -31.1% | 4130 -> 3596 |
| Q4 | 5e7 | 14.64 (14.64, 14.90) | 10.50 (10.50, 11.34) | -28.3% | 6654 -> 5855 |

The decisive cell, Q3 at 5e7, was measured three times:
1. first pass (an earlier harness whose RSS column was wrong, wall times valid): before 3.78, 3.12; after 2.71, 2.62 -> min **-16.0%**;
2. the table above: min **-12.6%**;
3. six interleaved runs (alternating order): before 3.24, 3.29, 3.19, 3.23, 3.38, 3.14; after 2.94, 2.95, 2.79, 2.80, 2.85, 2.90 -> min **-11.0%**, median **-11.0%**.

## Decision
- **Q3 kill rule (>= 15% at 5e7): not met.** One of three measurements reached 16.0%, but its "before" included a slow outlier (3.78 s against 3.04-3.38 s in every later run), and the better-controlled six-run measurement gives -11.0% for both min and median. The honest estimate is about -11% to -13%. Per the rule, the engine change is reverted.
- **Q2 rule (no more than 5% slower): met** (+3.0% at worst, within the noise of 2 runs), so Q2 was not the reason for the revert.
- Prediction check: I predicted -10% to -25% at 5e7 and that the partials would keep part of the cost. The result sits at the low end, which is consistent with the partials (`chunks x groups` states) still being allocated and zeroed. At 1e7 and 3e7 Q3 does not move at all (-2.8%, -1.7%, inside noise), so the saving only shows once there are enough chunks.
- Peak RSS fell only 4% for Q3 (3440 -> 3310 MiB), against the 740 MiB (22%) saving of 256 MiB chunks in EXP-006. That fits: reuse removes the working tables, not the retained partials.

## Unplanned finding: Q4 gained 18% to 31%
Not part of the kill rule and not a reason to keep the change on its own, but it is large and consistent across three sizes: Q4 (millions of groups per run) -17.8% / -31.1% / -28.3% with RSS down 11-14%. Likely cause (not profiled): the old code grew `ikeys`, `key_out`, `states` and the `IntMap` by repeated doubling with `realloc` copies and fresh page faults in every chunk table; reuse removes that. The partial's exact-size copy is cheaper than that growth. Q4's 5e7 timings include a fresh-table-per-chunk baseline of 14.6 s, so a profile of Q4 before/after is the next thing to run if this is revisited.

## Threats
Two to six runs per cell, one machine with 4 cores (chunks per worker, memory bandwidth and page-zeroing cost differ on the M3 Pro); the 15% line sits inside the run-to-run spread of the "before" binary at 5e7 (3.04-3.78 s), which is why the decision rests on the six-run set; Q4 differences were not profiled; the large ignored difftest (`--ignored`) was not run on the new code; `ru_maxrss` of the first-pass harness was cumulative over children and is not used.

## Next
1. Apply this patch only for aggregations with many groups per chunk (for example when the first chunk's group count exceeds some threshold) and re-measure Q3/Q4 — the user decides whether to treat Q4's gain as a separate experiment with its own kill rule.
2. Remove the retained `chunks x groups` partials: merge each completed chunk into a per-worker accumulator in a fixed order (needs a deterministic schedule, so that f64 sums stay independent of the thread count; this is the larger change EXP-006 pointed at).
