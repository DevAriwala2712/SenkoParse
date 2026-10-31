# EXP-006: Why Q3, Q5 and Q9 scale worse than linearly (diagnosis only, no engine change)

- **Status:** Done. Hypothesis **partly holds** (it is not killed by the 15% rule, but it explains only about a quarter of Q3 and Q5 and none of Q9). 2026-10-31.
- **Code:** unchanged. Release build of the branch as checked out (`debug = 1` is already in the profile, so perf symbols resolve).
- **Machine (NOT the M3 Pro):** cloud container, 4 vCPU Intel Xeon @ 2.10 GHz, 16 GiB RAM, **no swap**, Linux 6.18, 252 GB disk. Warm page cache unless marked. All numbers here are not comparable to the M3 Pro rows in `BENCHMARKS.md`, only the shapes are.
- **Data:** `scripts/gen_data.py --rows N`, seed 42, in git-ignored `data_1e7/`, `data_3e7/`, `data_5e7/`. x.csv is 0.53 / 1.64 / 2.76 GB.
- **Not run: 1e8.** x and big would be 5.55 GB each; Q9 at 5e7 already peaks at 9.4 GiB RSS (about 3.8 GiB of it heap), so 1e8 would need about 7.6 GiB heap plus 11 GB of mapped input on 16 GiB with no swap. Generation alone is about 10 minutes. I stopped at 5e7 rather than risk an OOM kill. The 1e8 curve is extrapolated only in words below, never as a number.
- **Method:** `/tmp/.../run.py` wrapper: wall time of the whole `qe` process, child `ru_maxrss`, user/sys CPU, major faults; stdout to /dev/null. Queries are the `bench/run.py` Q2, Q3, Q5, Q9 texts. "Min" is the min of the listed runs. Only 2 warm runs per cell (3 for the chunk test), so differences under about 10% are noise.

## Hypothesis and kill rule (written before running)
Per-chunk group tables make merge work grow as chunks x groups. **Kill:** if `--chunk-bytes 268435456` changes Q3 wall time by less than 15% at the largest size, the hypothesis is false and the investigation stops.

## 1. Scaling curve (default chunk size 16 MiB, all 4 cores, warm)

| query | 1e7 s | 3e7 s | 5e7 s | 3e7/1e7 (ideal 3x) | 5e7/1e7 (ideal 5x) | ns/row 1e7 -> 3e7 -> 5e7 | peak RSS MiB 1e7 / 3e7 / 5e7 |
|---|---|---|---|---|---|---|---|
| Q2 group id1 (control) | n/m | n/m | 1.93 | | | | 2682 at 5e7 |
| Q3 group id2 (1e4 groups) | 0.43 | 1.49 | 2.89 | 3.5x | 6.7x | 43 -> 50 -> 58 | 553 / 1859 / 3438 |
| Q5 group id4 + COUNT(DISTINCT id2) | 1.18 | 8.80 | 17.84 | **7.5x** | **15.1x** | 118 -> 293 -> 357 | 726 / 3543 / 6044 |
| Q9 join x, big (N x N) | 10.15 | 38.77 | 82.71 | 3.8x | **8.1x** | 1015 -> 1292 -> 1654 | 1614 / 5190 / 9366 |

(n/m = not measured.) The first, cold-cache run of each cell was slower (e.g. Q3 5e7: 7.75 s, Q9 5e7: 91.9 s, thousands of major faults) and is not used. Raw first-run lines: Q3 0.72 / 3.79 / 7.75, Q5 1.25 / 9.06 / 20.07, Q9 9.83 / 43.14 / 91.94.

Reading: Q5 is badly superlinear already by 3x (7.5x), Q9 degrades steadily (cost per row up 63% from 1e7 to 5e7), Q3 is only mildly superlinear (+35% per row). So of the three, Q3 is the weakest case for "far worse than 10x" on this machine; Q5 and Q9 are the real offenders.

## 2. Chunk size test (5e7 rows, 4 cores, warm)

| query | default 16 MiB wall s (runs) | `--chunk-bytes 268435456` wall s (runs) | change | RSS MiB default -> 256 MiB | sys CPU s default -> 256 MiB |
|---|---|---|---|---|---|
| Q2 (control, 10 groups) | 1.93 | 2.22, 2.26 | +15% slower | 2682 -> 2643 | 0.5 -> 0.6 |
| **Q3** | 3.43, 2.89, 3.14 (mean 3.15) | 2.42, 2.35 (mean 2.39) | **-24%** | 3434 -> 2691 | 3.2 -> 0.3 |
| Q5 | 20.44, 19.14, 17.84 (mean 19.1) | 14.57, 15.47 (mean 15.0) | **-21%** | 6054 -> 4025 | 13.8 -> 2.1..4.2 |

Kill rule: Q3 improves by 24% (> 15%), so the hypothesis is **not killed**. But with the large chunk Q3 still takes 2.4 s against 1.9 s for Q2 on the same file, and the sub-linear part (Q5 15 s, still 6.3x slower than Q3 at the same chunk size) is untouched. At 1e7 a 256 MiB chunk is worse (0.79 s vs 0.43 s; only 2 chunks for 4 cores), so this is a diagnostic, not a fix. Single thread, Q3: 1e7 1.56 s, 5e7 9.60 s (6.2x for 5x), so Q3 also has a mild superlinear part that is not about chunks or threads.

What the difference is: the **cost is creating and holding one table per chunk, not merge compute.** At 5e7 there are about 165 chunks x 1e4 groups = 1.65M group states built, then the same number merged. The RSS drop of about 740 MiB and the drop of sys CPU from 3.2 s to 0.3 s match first-touch page zeroing of those tables. `merge_state_ref` is only 3.2% of Q3 samples.

## 3. Profile (perf 6.8 tools on a 6.18 kernel; `-F 299 -g`, all threads)

**Q3, 5e7 rows (default chunks):**

| # | function | self % |
|---|---|---|
| 1 | `clear_page_erms` (kernel, zeroing newly touched anonymous pages) | 19.0 |
| 2 | `CsvReader::next_batch` | 17.3 |
| 3 | `AggTable::consume` | 10.6 |
| 4 | `memchr` avx2 `find_raw` (One) | 8.2 |
| 5 | `f64::from_str` | 7.5 |

Next: `Builder::push` 6.1, more memchr variants (5.0 + 4.0 + 3.1, about 20% memchr in total), `merge_state_ref` 3.2. By DSO: 73% `qe`, 26% kernel.
The page faults (25% of samples under `do_user_addr_fault`) come from: `AggTable::new_group` 10.3% (extending `states` for each new group in every chunk's table), `Vec` growth via `realloc` 5.5%, other 1.4%. So about half of the page-fault cost is new-group state allocation, which scales with chunks x groups.

**Q9, 3e7 rows (default chunks):**

| # | function | self % |
|---|---|---|
| 1 | `hash_join::probe_batch` | 19.9 |
| 2 | `hash_join::build_table` | 18.0 |
| 3 | `__memcmp_evex_movbe` | 15.3 |
| 4 | `clear_page_erms` (kernel) | 9.8 |
| 5 | `CsvReader::next_batch` | 4.5 |

Next: hashbrown `HashMap<Box<[u8]>, u32>` lookup 3.6, `unlink_chunk` 3.6 + `malloc_consolidate` 2.6 (glibc malloc), `reserve_rehash` 3.1. By DSO: 62% `qe`, 25% libc, 12% kernel. Reading the code (not changed): `build_table` stores each key as its own `Box<[u8]>` (`buf.clone().into_boxed_slice()` per unique key) in `FxMap<Box<[u8]>, u32>`, so the build is serial, one malloc per row, and every probe does a pointer chase plus a `memcmp` on a randomly located 8-byte key. A table of N random keys falls out of cache as N grows, which fits per-row cost rising from 1.0 to 1.65 us. 25% of time in libc is the allocator.

## 4. Memory pressure
- No swap exists on this machine (0 MB total), so swap use is 0 by construction. `/proc/pressure/memory` total stall after all runs: 13 ms. `allocstall_*` and `pgscan_direct` are 0, no OOM kill. Warm runs have 0 major faults.
- Peak RSS grows linearly with rows for every query, including Q3 with 1e4 groups: 553 -> 1859 -> 3438 MiB. Most of that is the mmap'd input (RSS counts it), but Q3 is 750 MiB above Q2 at 5e7 and Q5 is 6.0 GiB for a 2.76 GB file.
- So on this machine memory pressure is **not** involved up to 5e7. It could be at 1e8: the M3 Pro run in `BENCHMARKS.md` recorded swap in use with 18 GiB, and Q9 at 1e8 maps 11 GB plus a heap that is about 2x the 5e7 heap. That is not tested here.

## 5. Verdict
- **Hypothesis (merge work grows as chunks x groups): partly true.** For Q3, larger chunks give -24% wall, -740 MiB RSS and -90% sys CPU at 5e7, which clears the 15% kill threshold. The mechanism is allocating and first-touching one `AggTable` per chunk (`new_group`, `Vec` growth), not the merge arithmetic. It explains the Q3 per-row growth (43 -> 58 ns) and about a fifth of Q5's time.
- **Q5 (largest superlinear factor, 15x at 5x data): not explained by group count.** It has about 10-50 groups but `COUNT(DISTINCT id2)` keeps a distinct set per group per chunk; with id2 up to N/1e3 values, memory (6 GiB) and the time (357 ns/row) look driven by the size of those sets across all chunks. I did not profile Q5 (only Q3 and Q9 were asked for), so this is an inference from RSS and the chunk test, not a measurement.
- **Q9: hypothesis does not apply.** No group tables. The bottleneck is the single-threaded `build_table` with a per-key boxed `Box<[u8]>` hash map, plus `memcmp` and malloc in probe and build, with cache misses growing with the build size.
- **Bottleneck, one line each:** Q3 = per-chunk table allocation and page zeroing on top of scan/parse (parse is 40%+ of time and scales linearly); Q5 = per-chunk DISTINCT sets (inferred); Q9 = serial byte-key hash build and byte-compare probe.

## 6. Threats
4 vCPU Xeon vs 11-core M3 Pro (memory system, core count and therefore chunk-parallel behaviour differ); N only to 5e7; 2-3 runs per cell, no confidence intervals; Q5 mechanism inferred; perf and kernel versions mismatch (6.8 tools on 6.18, symbols resolved normally); the first-run (cold) numbers differ by up to 2.7x from warm, so any bench that does not discard them overstates scaling loss; machine had the other generators idle during timing.

## Next experiments (not done)
1. Q3/Q5 with a per-thread (not per-chunk) table: kills the chunks x groups term. Predicted Q3 -24% or better at 5e7.
2. Profile Q5 at 3e7 to confirm the DISTINCT-set cost.
3. Q9 with an integer-keyed (`u64`) build table and parallel build (the "EXP-005 dense-key array join" already in CLAUDE.md's Later list).
4. Re-run at 1e8 on a machine with at least 32 GiB or with swap accounted for.
