# EXP-009 step 2: One COUNT(DISTINCT) set per worker per group instead of per chunk

- **Status:** **The change failed the kill rule and the engine change is reverted.** Q5 at 5e7 is 1.47x faster on the min and 1.38x on the median, below the 1.5x bar. Its peak RSS falls 33%, short of the 50% alternative. Q3 and Q2 are unchanged. The patch is kept as `docs/experiments/data/009-distinct-per-thread.patch` (216 lines, applies to `78a2c06`); the new thread-identity test stays. The hypothesis and kill rule were written before any code (2026-11-01); the results were filled in after.
- **Follows:** `009-q5-profile.md`. At 3e7 the per-chunk DISTINCT sets hold about 25 M boxed keys at once (about 95% of a 1.5 GiB heap), and building and unioning them is at least 55-60% of CPU samples. The pairwise tree union takes longer than the scan.
- **Machine:** Apple M3 Pro, 11 cores (5P + 6E), 18 GiB RAM, macOS 27.0.1. Rust 1.99.1.
- **Baseline:** `main` at `78a2c06` (engine = `53c0e96`), binary kept as `target/ab/qe-main-53c0e96`.
- **Rule relaxation:** ADR-0004 (`docs/adr/0004-per-thread-order-independent-aggregates.md`).

## Hypothesis
`COUNT(DISTINCT e)` is a set union, which is exact and does not depend on order: the union of the same row values gives the same set however the rows are split or merged. So the set does not have to follow the fixed per-chunk merge order that keeps `f64` sums identical for any thread count.

Change: on the parallel aggregation path (`AggInput::Parallel`), the per-chunk table keeps every non-DISTINCT aggregate as before. DISTINCT states move to a small pool of **worker tables**: an `AggTable` with the same group keys and only the DISTINCT aggregates. A chunk task takes a worker table from the pool, feeds it the same batches, and returns it. The pool never holds more tables than tasks that ran at the same time (at most the thread count, barring nested rayon stealing). At the end:
1. the per-chunk tables (without DISTINCT states) are merged exactly as today, in the fixed pairwise tree or the partitioned merge, so group order, `f64` sums and AVG are unchanged;
2. the worker tables are merged by set union (order irrelevant);
3. each final group gets its DISTINCT count by looking up its key in the merged worker table (0 if absent).

Unchanged: the serial path (`--no-opt`, stream input, aggregates over non-chunkable inputs) keeps DISTINCT inside the one table. All other aggregates stay per chunk, including order-independent ones (COUNT, integer SUM, MIN/MAX): each is a scalar of 8 to 24 bytes per group per chunk, the profile shows no cost from them, and f64 MIN/MAX is not strictly order-independent (`-0.0` vs `0.0` and NaN keep the first value seen). ADR-0004 allows moving the exact ones later if a profile shows a need.

### Predictions (3e7 and 5e7; before = baseline binary)
- **Q5 wall: 1.8x to 3x faster** at 3e7 and 5e7. Most of the 46% merge phase goes away (11 worker sets per group to union instead of 98 chunk sets, done once). The consume-phase set work stays about the same: each worker set is larger (up to 30,000 keys instead of about 8,650), so cache misses may cost a bit more, and the extra group-key lookup per row for the worker table (the 30-group map) costs a little. At 1e7 (33 chunks) the gain is smaller, 1.3x to 2x.
- **Q5 peak RSS:** live set entries drop from about 25 M to at most 11 x 30 x 30,000 = 9.9 M (fewer if fewer workers touch each group). Heap at 3e7 goes from about 1.5 GiB to about 0.5-0.6 GiB, and peak RSS from about 3.07 GiB to about 2.1 GiB (-30%). **Not halved**, because the 1.5 GiB mapped input stays.
- **Q3 and Q2 (no DISTINCT):** the code path is identical apart from one check whether a DISTINCT aggregate exists. Prediction: within +-3%.

## Kill rule (fixed now)
Before = `target/ab/qe-main-53c0e96`; after = working tree build. Six interleaved runs per binary per cell (order flipped each round, one discarded warm-up per binary per cell), whole-process wall time and peak RSS via `os.wait4`, warm cache, all cores, default chunk size, at 1e7, 3e7 and 5e7.
- **Q5 at 5e7 must be at least 1.5x faster (min and median), OR its peak RSS at least halved.**
- **Q3 and Q2 must not be more than 5% slower** at any of the three sizes, on min or median.
- If either fails: save the diff as `docs/experiments/data/009-distinct-per-thread.patch`, revert the engine change, and report. The tests stay.
- Tests green: `cargo test` and the difftest (default, `--no-opt`, tiny chunks, stream); a new test that results are bit-identical for 1, 2 and 4 threads with `COUNT(DISTINCT)` and `f64` sums in the same query; a mutation check (break the union and confirm a test fails).

Only relative gains (before vs after on this machine) are reported.

## What was built (and reverted)
- `exec/hash_agg.rs`, parallel path only (`HashAgg::run`, `AggInput::Parallel`): when the query has a DISTINCT aggregate, the spec is split. The per-chunk tables get the non-DISTINCT aggregates. A `Mutex<Vec<AggTable>>` pool holds worker tables (same keys, DISTINCT aggregates only). Each chunk task pops a worker table, or creates one if the pool is empty, feeds it the same batches as its chunk table, and pushes it back.
- The per-chunk tables are merged unchanged (pairwise tree or partitioned merge). Then `AggTable::with_distinct` rebuilds a full-spec table: for every final group, and in parallel over groups, `DistinctSide::union_len` looks the group key up in each worker table (`find_group`, int or byte keys), takes the largest set as the base, and counts the keys of the other sets that are missing from it, borrowed and without copying. The DISTINCT state becomes `St::Count(n)`, which `finish` turns into the same `I64` a DISTINCT set gives. The other states keep their chunk-merge values and group order.
- Serial path, `--no-opt` and stream input: unchanged.

## Tests
- With the change: `cargo test --release` is all green, including unit tests, `difftest_small` (default, `--no-opt`, 3 threads with 256-byte chunks and forced partitioned merge, stream with 37-byte blocks, stream `--no-opt`), `difftest_join_key_cases`, edge cases, parallel (8 tests), stream and progress. Output of Q5 at 3e7 is byte-identical (md5) to `main`. The ignored large difftest was not run, because the change was reverted.
- New `tests/parallel.rs::distinct_per_worker_gives_identical_results_for_1_2_and_4_threads` (kept): 40,000 rows, 600-byte chunks (about 1,000 chunks). Int key with NULL group, string key with NULL group, composite key, global aggregate, DISTINCT-only query, DISTINCT over i64 (with NULLs and negatives), over strings and over f64, with DISTINCT columns placed between `f64` SUM/AVG/MIN columns. 1, 2 and 4 threads must be bit-identical, `f64` sums included, on the tree merge and the forced partitioned merge. The non-float columns must equal `--no-opt` as multisets for 1, 2 and 4 threads on both merges. It passes with the change and on the reverted (original) code.
- Mutation check: replacing the union count `sets[big].len() + extra.len()` with `sets[big].len()` (that is, ignoring every worker but the largest) fails `difftest_small` and 4 parallel tests: the new one, `table_reuse_...`, `partitioned_merge_...` and `integer_like_group_keys_...`. Restored afterwards.

## Results (Apple M3 Pro; whole-process wall seconds; 6 interleaved runs per binary per cell after one discarded warm-up each; warm cache, all cores, default chunks; before = `main` 53c0e96 engine, after = the patch)

| query | rows | before min / med s | after min / med s | speedup (min / med) | wall change (min / med) | peak RSS MiB before -> after (max of 6) |
|---|---|---|---|---|---|---|
| Q5 | 1e7 | 0.246 / 0.250 | 0.223 / 0.231 | 1.10x / 1.08x | -9.3% / -7.4% | 698 -> 611 (-12%) |
| Q5 | 3e7 | 1.459 / 1.498 | 1.032 / 1.072 | 1.41x / 1.40x | -29.3% / -28.4% | 3090 -> 2005 (-35%) |
| **Q5** | **5e7** | 3.032 / 3.053 | 2.060 / 2.210 | **1.47x / 1.38x** | -32.1% / -27.6% | **5578 -> 3752 (-33%)** |
| Q3 | 1e7 | 0.143 / 0.145 | 0.141 / 0.145 | 1.01x / 1.00x | -1.4% / +0.3% | 568 -> 569 |
| Q3 | 3e7 | 0.443 / 0.464 | 0.440 / 0.464 | 1.01x / 1.00x | -0.7% / -0.1% | 1884 -> 1882 |
| Q3 | 5e7 | 0.803 / 0.828 | 0.777 / 0.814 | 1.03x / 1.02x | -3.2% / -1.7% | 3491 -> 3490 |
| Q2 | 1e7 | 0.132 / 0.133 | 0.132 / 0.134 | 1.00x / 0.99x | 0.0% / +0.8% | 532 -> 529 |
| Q2 | 3e7 | 0.385 / 0.390 | 0.366 / 0.385 | 1.05x / 1.01x | -4.9% / -1.4% | 1613 -> 1614 |
| Q2 | 5e7 | 0.622 / 0.632 | 0.608 / 0.623 | 1.02x / 1.01x | -2.3% / -1.4% | 2693 -> 2694 |

Raw runs (seconds, in run order):
- Q5 1e7: before [0.250, 0.252, 0.249, 0.246, 0.247, 0.265], after [0.232, 0.229, 0.232, 0.231, 0.223, 0.231]
- Q3 1e7: before [0.148, 0.148, 0.144, 0.143, 0.145, 0.144], after [0.146, 0.156, 0.148, 0.141, 0.145, 0.145]
- Q2 1e7: before [0.133, 0.133, 0.140, 0.133, 0.132, 0.139], after [0.134, 0.132, 0.136, 0.132, 0.135, 0.134]
- Q5 3e7: before [1.485, 1.462, 1.459, 1.593, 1.511, 1.510], after [1.081, 1.069, 1.107, 1.074, 1.070, 1.032]
- Q3 3e7: before [0.460, 0.514, 0.470, 0.464, 0.464, 0.443], after [0.469, 0.465, 0.443, 0.440, 0.463, 0.464]
- Q2 3e7: before [0.391, 0.395, 0.395, 0.385, 0.389, 0.389], after [0.391, 0.385, 0.366, 0.373, 0.384, 0.389]
- Q5 5e7: before [3.067, 3.044, 3.041, 3.061, 3.095, 3.032], after [2.147, 2.332, 2.060, 2.116, 2.273, 2.283]
- Q3 5e7: before [0.829, 0.803, 0.830, 0.813, 0.827, 0.828], after [0.820, 0.807, 0.829, 0.801, 0.825, 0.777]
- Q2 5e7: before [0.622, 0.637, 0.628, 0.641, 0.636, 0.623], after [0.620, 0.640, 0.608, 0.626, 0.628, 0.617]

Swap: 2,534 MiB in use before the runs and 2,526 MiB after (pre-existing, unchanged).

## Decision against the kill rule
- **Q5 at 5e7 >= 1.5x (min and median): not met.** 1.47x on the min, 1.38x on the median.
- **Or peak RSS at least halved: not met.** 5578 -> 3752 MiB is -33%. As step 1 predicted, the mapped input alone (2.76 GB at 5e7) rules out halving.
- **Q3 and Q2 no more than 5% slower: met** (the worst cell is +0.8% on a median, and all within noise).
- Per the rule, the engine change is reverted and the patch kept. The test stays.

## Prediction check
- Q5 speed: I predicted 1.8x to 3x at 3e7 and 5e7 and got 1.4x to 1.47x (min). **The prediction was wrong.** Removing the chunk-by-chunk union took out less than the merge phase's 46% share of busy samples suggested. My unverified reading: that share was spread over 11 cores at the low tree levels, and the consume-phase set work (one boxed key and a `memcmp` per new value, now in larger sets that miss cache more) remains and may have grown. I did not profile the patched build, so this is unconfirmed.
- Q5 RSS: predicted 3.07 -> about 2.1 GiB at 3e7; measured 3090 -> 2005 MiB. That matches.
- Q3 and Q2: predicted within +-3%; measured -4.9% to +0.8%. Within noise.

## Threats
Six runs per cell on one machine. The P/E core mix was not controlled. The after-side spread at 5e7 is wider (2.06-2.33 s) than the before side (3.03-3.10 s), so the median ratio (1.38x) is the more conservative figure. Only the bench data shape was measured (30 groups, 1e4-5e4 distinct values). The large difftest was not run on the patched build.

## What this suggests (not done)
The remaining Q5 cost is likely the set representation, not where the set lives: a `FxSet<Box<[u8]>>` allocates one boxed 9-byte key per new value and compares through a pointer. For integer-like DISTINCT arguments, an inline `i64` set (or a bitset when the measured range is small, CLAUDE.md plan P4) would remove that allocation and `memcmp`. That is a separate experiment with its own kill rule. It could combine with this patch, which already passed its correctness tests.
