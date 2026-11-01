# EXP-008b: Integer-keyed join (EXP-008) re-measured on the Apple M3 Pro

- **Status:** Reproduction run, no engine change. 2026-11-01.
- **Question:** does the Q9 gain EXP-008 measured on a 4 vCPU Linux container reproduce on the team's M3 Pro, and by how much?
- **Machine:** Apple M3 Pro, 11 cores (5 performance + 6 efficiency), 18 GiB RAM, macOS 27.0.1, internal SSD. Rust 1.99.1.
- **Binaries (both kept in git-ignored `target/ab/`):**
  - before: `qe-before-cf9a1c1`, built from `cf9a1c1` (byte-key join only).
  - after: `qe-after-53c0e96`, built from `main` at `53c0e96` (merge of PR #6, EXP-008 integer join `bff545a`).
- **Data:** `python3 scripts/gen_data.py --rows 3e7 --out data_3e7` (default seed), git-ignored. x.csv and big.csv 1,642 MB each, 30,000,000 rows; medium 30,000 rows; small 30 rows.

## Correctness first (main build)
- `cargo test --release --test difftest`: `difftest_small` and `difftest_join_key_cases` ok (3.5 s).
- `cargo test --release`: every test binary ok, no failures (unit, difftest, edge_cases, parallel incl. the 1/2/4-thread integer-join identity test, stream, progress, demo).
- `cargo test --release --test difftest -- --ignored` (`difftest_large`): ok, 580 s. It ran while the data generator was running, so its time is not a benchmark.
- Output bytes of before and after are identical (md5) for Q7, Q8 and Q9 at 3e7.

## Method
Queries are the `bench/run.py` texts:
- Q7 `SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4`
- Q8 `SELECT m.id5, COUNT(*), SUM(x.v1) FROM x JOIN medium m ON x.id2 = m.id2 GROUP BY m.id5`
- Q9 `SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3`

Harness as in EXP-007/008: Python, `os.wait4` per child gives whole-process wall time and peak RSS (`ru_maxrss`, bytes on macOS); stdout to a file. Per query: one discarded warm-up run of each binary (page cache warm), then 3 rounds, before and after alternating and the order flipped every round. All cores, default chunk size, no other flags. Swap usage was 2,566 MiB before and after the runs (unchanged; it predates them), memory 67% free at the start.

## Results (M3 Pro, 3e7 rows, warm cache, all cores; whole-process wall seconds)

| query | before min / med s | after min / med s | speedup (min / med) | wall change (min / med) | peak RSS MiB before -> after (max of 3) |
|---|---|---|---|---|---|
| Q7 | 0.51 / 0.51 | 0.47 / 0.47 | 1.08x / 1.08x | -7.1% / -7.3% | 1630 -> 1628 |
| Q8 | 0.73 / 0.73 | 0.68 / 0.69 | 1.07x / 1.07x | -6.8% / -6.5% | 2080 -> 2076 |
| Q9 | 5.89 / 5.90 | 1.47 / 1.48 | 4.00x / 3.97x | -75.0% / -74.8% | 5346 -> 5038 |

Raw runs (seconds; peak RSS MiB):
- Q7: before [0.509, 0.521, 0.507] (1628, 1627, 1630); after [0.471, 0.476, 0.472] (1627, 1628, 1626)
- Q8: before [0.734, 0.733, 0.745] (2080, 2077, 2079); after [0.696, 0.683, 0.686] (2076, 2075, 2076)
- Q9: before [5.984, 5.888, 5.898] (5346, 5339, 5340); after [1.489, 1.484, 1.471] (5038, 5036, 5036)

## Comparison with EXP-008 (container, 3e7)

| query | container speedup (min / med) | M3 Pro speedup (min / med) |
|---|---|---|
| Q7 | 1.16x / 1.13x | 1.08x / 1.08x |
| Q8 | 1.20x / 1.20x | 1.07x / 1.07x |
| Q9 | 6.36x / 5.96x | 4.00x / 3.97x |

## Reading the numbers
- **The Q9 gain reproduces on the M3 Pro, smaller: 4.0x (5.89 s -> 1.47 s min, 5.90 s -> 1.48 s median) against 6.0-6.4x on the container at the same 3e7 size.** The run-to-run spread is under 2% on both binaries, far smaller than the gap.
- The smaller ratio comes from the before side: the old byte-key join is 6.3x faster on the M3 Pro than on the container (5.9 s vs 37-38 s), while the new join is 4.0-4.3x faster (1.47 s vs 5.85-6.40 s). A plausible reason is that the M3 Pro's memory system and allocator absorb more of the old path's cost (one malloc per build key and two dependent cache misses per probe), so there is less left to remove. I did not profile this.
- Q7 and Q8 get 7-8% faster (container: 13-20%). Not slower, so EXP-008's "must not be more than 5% slower" rule also holds here.
- Peak RSS: Q9 -6% (5346 -> 5038 MiB), Q7 and Q8 unchanged within 0.3%. RSS includes the mapped inputs (about 3.3 GB for x and big).

## Threats
One size (3e7) only; 1e7, 5e7 and 1e8 were not run here. Three measured runs per binary per query. Warm cache only, no cold run. `ru_maxrss` includes mapped file pages. The M3 Pro mixes performance and efficiency cores and I did not control which cores the workers ran on. Some swap (2.5 GiB) was already in use from earlier work on the machine, though it did not change during the runs. The large difftest overlapped with data generation, so its run time is not a benchmark. `BENCHMARKS.md` and `docs/status.md` were deliberately not updated in this run.
