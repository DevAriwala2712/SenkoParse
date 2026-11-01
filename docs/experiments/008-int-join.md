# EXP-008: Integer-keyed, parallel-built join table (Q9)

- **Status:** **Kept: kill rule passed** (Q9 8.7x at 5e7, Q7/Q8 faster, not slower). Hypothesis and kill rule were written before any code. 2026-11-01.
- **Follows:** EXP-006 section 3 (Q9 profile) and "Next experiments" item 3.
- **Machine:** same cloud container as EXP-006/007: 4 vCPU Intel Xeon @ 2.10 GHz, 16 GiB RAM, no swap, Linux 6.18. **Not the M3 Pro: only relative gains (before/after on this machine) are meaningful.** Warm page cache. Data in git-ignored `data_1e7/`, `data_3e7/`, `data_5e7/` (seed 42).
- **Baseline commit:** `cf9a1c1` (engine identical to `a983f8f`).

## What EXP-006 found for Q9
`SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3`, 3e7 rows, self time: `hash_join::probe_batch` 19.9%, `hash_join::build_table` 18.0%, `memcmp` 15.3%, kernel page zeroing 9.8%, `CsvReader::next_batch` 4.5%; glibc `unlink_chunk` + `malloc_consolidate` + hashbrown rehash another 9%. Wall time per row rose from 1.0 us (1e7) to 1.65 us (5e7): 10.2 s, 38.8 s, 82.7 s. Cause named there: the build is serial and keeps every key as its own `Box<[u8]>` (one malloc per build row) in an `FxMap<Box<[u8]>, u32>`, and every probe hashes the 9-byte encoded key, chases a pointer into a separately allocated key and `memcmp`s it. The table is far larger than cache, so the pointer chase misses cache once for the map entry and once for the boxed key.

## Hypothesis
When the join has exactly one key per side and its type is an integer-like type (`i64`, `decimal`, `date`) on both sides, a join table keyed directly on the `i64` removes most of that cost:
1. an open-addressing table whose slots hold `(key: i64, head: u32)` inline: no per-key allocation, no `memcmp`, one cache line touched per probe in the common (unique key) case;
2. built in parallel by hash-partitioning the build rows (fixed 64 partitions for builds of at least 65,536 rows, 1 otherwise, so the layout does not depend on the thread count), each partition owned and filled by one task, in ascending build-row order so duplicate-key chains and therefore output row order are exactly those of the byte-key join;
3. the duplicate-key chain array `next` is allocated only if a duplicate key exists (Q9 keys are unique).

Everything else stays: the byte-key join is the fallback for string, float, boolean and composite keys (and for mixed key types), and `--no-opt` / the serial `HashJoin` keep using it as the reference path.

Predicted effect on Q9 (wall, all 4 cores): build + probe + memcmp + allocator are about 60% of the time at 3e7 and the new join should cut most of that; scan/parse of two tables, the output `take` and the aggregate remain. Prediction at 5e7: **3x to 6x faster**, peak RSS lower or equal (no per-key boxes, no `next` array). Q7 (50-row build side) and Q8 (50,000-row build side): the build side is tiny, so only the per-probe cost changes (a direct-key probe is cheaper than hashing 9 bytes), prediction 0% to 10% faster, not slower.

## Kill rule (fixed now)
Before = binary built from `cf9a1c1`; after = working tree. Interleaved runs, at least 3 each, min and median of whole-process wall time, all cores, default chunk size.
- **Q9 at 5e7 must be at least 2.5x faster**, on both the min and the median.
- **Q7 and Q8 must not be more than 5% slower** at any of 1e7, 3e7, 5e7 (min and median).
- If either fails: save the patch as `docs/experiments/data/008-int-join.patch`, revert the engine change, report. No dense-array path is added in this task even if the table turns out to be dense.
- Tests stay green: `cargo test` and the difftest (default, `--no-opt`, tiny chunks), plus the new difftest cases and the 1/2/4-thread identity test.

## Method
`/tmp/.../ab.py`-style harness as in EXP-007: `os.wait4` gives the wall time and the peak RSS of each child separately; before and after alternate (order flipped every round); stdout to /dev/null. Queries are the `bench/run.py` Q7, Q8, Q9 texts. Both binaries are kept as copies and run against the same files.

## What was built
- `exec/hash_join.rs`: `Built` now holds an `Index`: `Bytes` (the old `FxMap<Box<[u8]>, u32>` + `next`, unchanged and still the reference/fallback) or `Int(IntJoin)`.
- `int_join_eligible(bkeys, pkeys)`: one key per side, both `i64`/`decimal` (raw `i64`, the same bytes the old encoding compared) or both `date`. Anything else (string, `f64`, bool, composite, mixed classes) takes the byte path.
- `build_int_table` (used only by the parallel `chunked()` path in `par.rs`; the serial `HashJoin`, so `--no-opt` and stream-probe joins, still use the byte path): drains the build side, keeps the key as `Vec<i64>` plus a NULL flag vector allocated only if a NULL key appears; then (1) buckets row ids by partition in parallel over fixed 65,536-row ranges, (2) one task per partition fills its own open-addressing table (linear probing, load factor at most 0.6, slot = `{key: i64, head1: u32}`, zero = empty, so the allocation is `calloc`) walking the ranges in order. 64 partitions for builds of at least 65,536 rows, else 1; both numbers are constants, so layout and output order do not depend on the thread count. Hash: splitmix64 finalizer (a plain multiplicative hash would map keys that differ only in high bits, like `j << 40`, to few slots).
- Duplicate build keys chain exactly as before (slot = newest row, `next[row]` = previous); `next` is allocated only if some key repeats (Q9: never). NULL build keys are never inserted, NULL probe keys never match, LEFT JOIN emits `NONE_IDX` for unmatched probe rows (shared code path after the lookup).
- `probe_batch`: the integer branch is a tight loop over the key slice; the `take`/`take_opt` tail is shared with the byte path.
- `exec::join_stats` (thread-local counters of int/byte builds) exists only so tests can prove which path ran.

## Tests
- `cargo test --release`: all green (unit, difftest small incl. default / `--no-opt` / 3 threads with 256-byte chunks / stream, edge cases, parallel, stream, progress). The ignored large difftest (`DIFFTEST_QUERIES=400`, 3 seeds, 20,000 rows) also passes (506 s).
- New `tests/difftest.rs::difftest_join_key_cases`: 9 data variants (sparse keys `j << 40` and values near the `i64` limits; negative keys; NULL keys on both sides; duplicate build keys with the dimension on either side; unmatched probe rows; empty build side; empty probe side; both empty; a 70,000-row build side with sparse, negative, dense, duplicate and NULL keys so the partitioned build runs), each joined on an integer key, a date key and a decimal key (integer path) and on a string key and a composite key (byte-key fallback), as inner and LEFT joins, build side on either side, 6 shapes; each run with default options, 1, 2 and 4 threads with tiny chunks (64 KiB for the large variant) and `--no-opt`: 1230 engine runs, all equal to SQLite as multisets. Per run, the counters assert that the integer path ran for integer keys and the byte path for the fallback and for `--no-opt`.
- New `tests/parallel.rs::integer_join_results_do_not_depend_on_thread_count`: 70,000-row build side, 30,000 probe rows with `f64` values; 1, 2 and 4 threads must be bit-identical (including the row order of unordered output and the `f64` sums), and the non-float queries must equal the `--no-opt` byte-key join.
- Mutation checks (each made the join test fail, then restored): probe treats NULL as a key; build inserts NULL rows; duplicate chain not extended; LEFT JOIN drops unmatched rows.
- Output bytes of the old and new binaries are identical (md5) for Q7, Q8, Q9 at 3e7 and for a plain `SELECT x.id3, b.v2 ... LIMIT 200000` at 1e7.

## Results (cloud container: 4 vCPU Intel Xeon @ 2.10 GHz, 16 GiB, no swap; whole-process wall seconds, all cores, default chunks; 3 interleaved runs per binary per cell, order flipped each round, one discarded warm-up per cell; relative numbers only, this is not the M3 Pro)

| query | rows | before min / med s | after min / med s | speedup (min / med) | wall change (min / med) | peak RSS MiB before -> after |
|---|---|---|---|---|---|---|
| Q7 | 1e7 | 0.62 / 0.62 | 0.54 / 0.55 | 1.14x / 1.13x | -12.0% / -11.2% | 536 -> 532 |
| Q8 | 1e7 | 0.80 / 0.86 | 0.67 / 0.69 | 1.20x / 1.25x | -16.7% / -19.9% | 596 -> 592 |
| Q9 | 1e7 | 9.32 / 9.33 | 1.64 / 1.68 | 5.69x / 5.54x | -82.4% / -82.0% | 1615 -> 1621 |
| Q7 | 3e7 | 1.93 / 1.93 | 1.66 / 1.71 | 1.16x / 1.13x | -13.7% / -11.6% | 1619 -> 1619 |
| Q8 | 3e7 | 3.42 / 3.44 | 2.86 / 2.88 | 1.20x / 1.20x | -16.3% / -16.4% | 2091 -> 2091 |
| Q9 | 3e7 | 37.18 / 38.14 | 5.85 / 6.40 | 6.36x / 5.96x | -84.3% / -83.2% | 5190 -> 5166 |
| Q7 | 5e7 | 3.04 / 3.17 | 2.68 / 2.69 | 1.13x / 1.18x | -11.8% / -14.9% | 2703 -> 2706 |
| Q8 | 5e7 | 6.20 / 6.56 | 5.42 / 5.54 | 1.14x / 1.18x | -12.5% / -15.5% | 3879 -> 3876 |
| Q9 | 5e7 | 78.80 / 80.66 | 9.04 / 9.13 | 8.71x / 8.84x | -88.5% / -88.7% | 9366 -> 8726 |

Raw runs (seconds):
- Q7 1e7: before [0.638, 0.618, 0.617], after [0.543, 0.549, 0.565]
- Q8 1e7: before [0.858, 0.858, 0.798], after [0.665, 0.691, 0.687]
- Q9 1e7: before [9.328, 9.323, 9.376], after [1.684, 1.64, 1.718]
- Q7 3e7: before [1.986, 1.926, 1.934], after [1.762, 1.71, 1.662]
- Q8 3e7: before [3.758, 3.442, 3.42], after [2.891, 2.88, 2.861]
- Q9 3e7: before [37.182, 38.141, 38.59], after [6.401, 5.85, 6.791]
- Q7 5e7: before [3.041, 3.168, 3.197], after [2.681, 2.695, 2.924]
- Q8 5e7: before [7.274, 6.2, 6.562], after [5.745, 5.424, 5.543]
- Q9 5e7: before [78.802, 80.663, 82.956], after [9.126, 9.044, 9.714]

## Decision against the kill rule
- **Q9 at 5e7 must be at least 2.5x: 8.71x on the min, 8.84x on the median. Passed.** (3e7: 6.4x / 6.0x; 1e7: 5.7x / 5.5x.)
- **Q7 and Q8 must not be more than 5% slower: they are 11% to 20% faster at every size on both min and median. Passed.**
- Engine change kept. Not added: no dense-array path, as instructed.

## Reading the numbers
- Q9 per row at 5e7 is now 0.18 us against 1.58 us, and Q9's cost per row grows from 1e7 to 5e7 by 1.1x instead of 1.7x (before: 9.32 s -> 78.8 s for 5x data is 8.5x; after: 1.64 s -> 9.04 s is 5.5x), so most of the superlinear growth EXP-006 saw came from the byte-key table (cache misses plus malloc), as hypothesised. It is still slightly superlinear after the change (5.5x for 5x data); I did not profile what remains.
- Peak RSS: Q9 at 5e7 9366 -> 8726 MiB (-7%); at 1e7 and 3e7 unchanged within 1%. This is lower than the "no per-key box" argument suggests because the mapped input (about 5.5 GB for x and big) and the retained build columns dominate RSS; the transient `Vec<i64>` keys and the bucket lists offset part of the saving.
- Q7 and Q8 gaining 11-20% was not predicted as strongly (I predicted 0-10%); the likely reason is the cheaper probe (one mixed integer hash and an inline key compare instead of encoding 9 bytes and hashing them) over 5e7 probe rows, but I did not profile it.

## Threats
Three runs per cell, one 4-core machine (the parallel build has 4 workers here; speedups on more cores or with a different memory system can differ); the "before" Q9 numbers on this machine are slower than a cold first run suggests (a one-off 65 s Q9 at 1e7 was seen while a cold page cache was filling and is not used); queries use unique build keys (Q7-Q9), so duplicate-heavy joins are covered for correctness (difftest) but not benchmarked; `ru_maxrss` includes the mapped input; the large 1e8 case and a cold-cache run were not measured; the stream-probe join still uses the byte path.

## Next
1. Profile Q9 after the change (what remains of the 5.5x growth: the serial drain of the build side, `Column::append`, `take`, scan).
2. Let the stream-probe join (`HashJoin`, serial) use the integer index too; it is currently the byte path.
