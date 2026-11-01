# BENCHMARKS
Appended by `python3 bench/run.py`. Every performance claim must have a row here.

## M1 baseline (single thread) — 1e7 rows

- machine: Apple M3 Pro, 11 cores, 18 GiB RAM, Darwin 27.0.0
- date: 2026-10-31, warm page cache, 3 runs (first discarded if >1), wall time of the full process

| query | file GB | qe min s | qe median s | GB/s (min) |
|---|---|---|---|---|
| Q1 global agg | 0.53 | 0.80 | 0.83 | 0.66 |
| Q2 group id1 (10) | 0.53 | 0.74 | 0.76 | 0.71 |
| Q3 group id2 (1e4) | 0.53 | 0.77 | 0.77 | 0.68 |
| Q4 group id3 (~6e6) | 0.53 | 2.54 | 2.57 | 0.21 |
| Q5 group str id4 | 0.53 | 0.94 | 0.95 | 0.56 |
| Q6 filter + agg | 0.53 | 0.74 | 0.74 | 0.71 |
| Q7 join small + grp | 0.53 | 0.91 | 0.93 | 0.58 |
| Q8 join medium + grp | 0.53 | 0.98 | 0.98 | 0.54 |
| Q9 join big (1e7x1e7) | 1.05 | 3.59 | 3.60 | 0.29 |

## M2 parallel scan (chunked, default threads=all cores, 16 MiB fixed chunks) — 1e7 rows

- machine: Apple M3 Pro, 11 cores, 18 GiB RAM, Darwin 27.0.0
- date: 2026-10-31, warm page cache, 3 runs (first discarded if >1), wall time of the full process

| query | file GB | qe min s | qe median s | GB/s (min) |
|---|---|---|---|---|
| Q1 global agg | 0.53 | 0.13 | 0.13 | 3.93 |
| Q2 group id1 (10) | 0.53 | 0.13 | 0.13 | 4.18 |
| Q3 group id2 (1e4) | 0.53 | 0.15 | 0.15 | 3.63 |
| Q4 group id3 (~6e6) | 0.53 | 2.31 | 2.31 | 0.23 |
| Q5 group str id4 | 0.53 | 0.22 | 0.22 | 2.45 |
| Q6 filter + agg | 0.53 | 0.13 | 0.13 | 4.06 |
| Q7 join small + grp | 0.53 | 0.15 | 0.16 | 3.42 |
| Q8 join medium + grp | 0.53 | 0.18 | 0.18 | 2.99 |
| Q9 join big (1e7x1e7) | 1.05 | 1.46 | 1.46 | 0.72 |

## M2 + EXP-004 part A/4b (integer-key group table, allocation-free output) — 1e7 rows

- machine: Apple M3 Pro, 11 cores, 18 GiB RAM, Darwin 27.0.0
- date: 2026-10-31, warm page cache, 3 runs (first discarded if >1), wall time of the full process

| query | file GB | qe min s | qe median s | GB/s (min) |
|---|---|---|---|---|
| Q1 global agg | 0.53 | 0.13 | 0.13 | 3.92 |
| Q2 group id1 (10) | 0.53 | 0.13 | 0.13 | 4.21 |
| Q3 group id2 (1e4) | 0.53 | 0.13 | 0.13 | 4.15 |
| Q4 group id3 (~6e6) | 0.53 | 1.29 | 1.29 | 0.41 |
| Q5 group str id4 | 0.53 | 0.21 | 0.22 | 2.46 |
| Q6 filter + agg | 0.53 | 0.12 | 0.13 | 4.30 |
| Q7 join small + grp | 0.53 | 0.16 | 0.16 | 3.35 |
| Q8 join medium + grp | 0.53 | 0.17 | 0.17 | 3.03 |
| Q9 join big (1e7x1e7) | 1.05 | 1.44 | 1.45 | 0.73 |

## M2 + EXP-004 complete (integer-key table, allocation-free output, partition-wise merge) — 1e7 rows

- machine: Apple M3 Pro, 11 cores, 18 GiB RAM, Darwin 27.0.0
- date: 2026-10-31, warm page cache, 3 runs (first discarded if >1), wall time of the full process

| query | file GB | qe min s | qe median s | GB/s (min) |
|---|---|---|---|---|
| Q1 global agg | 0.53 | 0.14 | 0.14 | 3.84 |
| Q2 group id1 (10) | 0.53 | 0.13 | 0.13 | 4.04 |
| Q3 group id2 (1e4) | 0.53 | 0.13 | 0.14 | 3.95 |
| Q4 group id3 (~6e6) | 0.53 | 0.67 | 0.69 | 0.78 |
| Q5 group str id4 | 0.53 | 0.23 | 0.24 | 2.30 |
| Q6 filter + agg | 0.53 | 0.13 | 0.14 | 3.95 |
| Q7 join small + grp | 0.53 | 0.16 | 0.16 | 3.30 |
| Q8 join medium + grp | 0.53 | 0.18 | 0.19 | 2.88 |
| Q9 join big (1e7x1e7) | 1.05 | 1.51 | 1.53 | 0.70 |

## Piped streaming, `qe --stdin` vs DuckDB 2.0.0 (EXP-005-stream)

- machine: Intel Xeon 2.10 GHz cloud container, 4 cores, 16 GiB RAM, Linux 6.18 (not the M3 Pro of the tables above)
- date: 2026-10-31, warm page cache, 3 runs per cell (min / median / max), wall time of the full process including the `cat` feeder
- query: `SELECT <all 8 columns of a> FROM a JOIN ids i ON a.center_id = i.id` (50 unique ids, ~5% of rows match)
- data: reconstruction of the shape in DuckDB issue #25165 (6 UUIDs, quoted user agent, integer), 1x = 375 MB; 2x/5x/10x repeat the 1x body
- settings: DuckDB `memory_limit=256MB`, `threads=1`; `qe --threads 1`, default 4 MiB blocks. RSS includes a 12.4 MiB measurement floor; DuckDB runs inside Python (interpreter included)
- reproduce: `bench/gen_stream_data.py`, `bench/exp005_stream.py`; raw runs in `docs/experiments/data/exp005-stream-raw.jsonl`

| size | input MB | engine | wall s (min/med/max) | GB/s (min wall) | peak RSS MiB (min/med/max) | peak temp MiB (max) | output rows |
|---|---|---|---|---|---|---|---|
| 1x | 375 | qe pipe | 0.552/0.648/0.704 | 0.68 | 41.6/41.9/41.9 | 0.0 | 54986 |
| 1x | 375 | duckdb pipe | 2.039/2.105/2.126 | 0.18 | 336.7/337.4/337.6 | 236.7 | 54986 |
| 1x | 375 | duckdb file | 0.898/0.903/0.924 | 0.42 | 216.0/221.7/246.2 | 0.0 | 54986 |
| 2x | 750 | qe pipe | 1.027/1.031/1.031 | 0.73 | 41.8/41.8/41.9 | 0.0 | 109972 |
| 2x | 750 | duckdb pipe | 4.226/4.404/6.152 | 0.18 | 337.7/337.9/338.1 | 644.1 | 109972 |
| 2x | 750 | duckdb file | 1.573/1.623/1.654 | 0.48 | 233.8/245.9/249.8 | 0.0 | 109972 |
| 5x | 1874 | qe pipe | 2.538/2.548/2.637 | 0.74 | 42.0/42.1/42.2 | 0.0 | 274930 |
| 5x | 1874 | duckdb pipe | 18.326/18.836/22.037 | 0.10 | 341.5/341.5/341.9 | 1676.2 | 274930 |
| 5x | 1874 | duckdb file | 3.545/3.651/3.724 | 0.53 | 228.5/244.9/250.5 | 0.0 | 274930 |
| 10x | 3749 | qe pipe | 4.78/4.793/5.05 | 0.78 | 42.0/42.0/42.0 | 0.0 | 549860 |
| 10x | 3749 | duckdb pipe | 69.843/69.903/70.871 | 0.05 | 345.8/345.9/346.0 | 5570.5 | 549860 |
| 10x | 3749 | duckdb file | 6.959/7.223/7.255 | 0.54 | 232.0/244.6/244.8 | 0.0 | 549860 |

Output rows were compared as sorted multisets and are identical across the three engines at every size. This is one query on one data shape; it says nothing about high-cardinality GROUP BY or large join build sides, which are not bounded.

**Ratio at 10x:** 69.843 s / 4.78 s = 14.6x (qe pipe vs DuckDB 2.0.0 pipe, min wall). The "69" in the wall-time chart is DuckDB's time in seconds, not a speedup.

### Re-run of the full harness on Apple M3 Pro, DuckDB 2.0.0 (2026-11-01)

`bench/exp005_stream.py`, same data (`bench/gen_stream_data.py`, seed 42; 1x is byte-identical to the Linux data), same settings (1 thread each, DuckDB `memory_limit=256MB`), 3 runs per cell, qe commit b205df1, macOS 27.0.1, 11 cores, 18 GiB, DuckDB 2.0.0 on Python 3.11 (the harness itself ran on 3.9; measurement floor 1.0 MiB). Raw: `docs/experiments/data/exp005-mac-m3pro-raw.jsonl`, console: `docs/experiments/data/exp005-mac-m3pro-output.txt`, chart: `docs/img/stream-wall-mac.svg`.

| size | input MB | engine | wall s (min/med/max) | peak RSS MiB (min/med/max) | peak temp MiB (max) | output rows |
|---|---|---|---|---|---|---|
| 1x | 375 | qe pipe | 0.275/0.277/0.763 | 48.4/48.4/49.4 | 0.0 | 54986 |
| 1x | 375 | duckdb pipe | 0.926/0.946/1.005 | 340.6/341.4/342.1 | 235.1 | 54986 |
| 1x | 375 | duckdb file | 0.41/0.411/0.415 | 128.1/128.4/128.5 | 0.0 | 54986 |
| 2x | 750 | qe pipe | 0.534/0.535/0.585 | 47.4/47.5/66.3 | 0.0 | 109972 |
| 2x | 750 | duckdb pipe | 1.802/1.806/2.054 | 334.6/336.0/338.3 | 652.4 | 109972 |
| 2x | 750 | duckdb file | 0.687/0.689/0.693 | 141.9/142.0/142.2 | 0.0 | 109972 |
| 5x | 1874 | qe pipe | 1.231/1.27/1.283 | 47.6/47.6/47.6 | 0.0 | 274930 |
| 5x | 1874 | duckdb pipe | 4.306/4.352/4.419 | 345.4/345.4/345.6 | 1890.3 | 274930 |
| 5x | 1874 | duckdb file | 1.523/1.526/1.53 | 129.6/129.6/129.7 | 0.0 | 274930 |
| 10x | 3749 | qe pipe | 2.495/2.541/2.56 | 48.7/48.7/48.7 | 0.0 | 549860 |
| 10x | 3749 | duckdb pipe | 14.959/17.572/23.015 | 339.1/344.9/346.1 | 5363.0 | 549860 |
| 10x | 3749 | duckdb file | 2.957/3.199/3.346 | 136.4/136.5/137.0 | 0.0 | 549860 |

Output row digests are identical across the three engines, and identical to the Linux run's digests, at every size.

What reproduced and what did not (min wall, qe pipe vs DuckDB 2.0.0 pipe):

| size | Linux 4-core | M3 Pro |
|---|---|---|
| 1x | 3.7x | 3.4x |
| 2x | 4.1x | 3.4x |
| 5x | 7.2x | 3.5x |
| 10x | 14.6x | 6.0x |

Reproduced: flat qe memory (about 48 MiB here vs 42 MiB there; floors differ), DuckDB piped about 335-346 MiB, DuckDB piped temp growing with input (235 / 652 / 1890 / 5363 MiB), 0 temp by filename and for qe. Not reproduced: the size of the wall-time gap at 5x and 10x. DuckDB piped at 10x took 15.0-23.0 s here against 69.8-70.9 s on Linux, so the 14.6x is specific to that container (likely its temp-file I/O); the 6.0x here is itself a three-run figure with a wide DuckDB range. DuckDB by filename was 3.0 s at 10x (qe 1.2x faster).


## File mode, `qe` vs DuckDB 2.0.0 — 1e8 rows (warm cache; cold pending)

- machine: Apple M3 Pro, 11 cores, 18 GiB RAM, Darwin 27.0.0 (swap in use: 1.3 GiB of 2 GiB at the end of the run, see caveats)
- date: 2026-11-01, qe release build of current `main`, DuckDB 2.0.0 (Python 3.11)
- data: `scripts/gen_data.py --rows 1e8` (seed 42): x and big 5.55 GB each, medium 3.4 MB, small 1.9 KB. Q1-Q8 read 5.55 GB, Q9 reads 11.10 GB
- harness: `bench/file_vs_duckdb.py` (raw: `docs/experiments/data/file1e8-raw.jsonl`), tables by `bench/summarize_file_bench.py`. DuckDB reads the CSVs with `read_csv` and explicit column types from `schema.toml`; default memory limit. 3 runs per cell, no run discarded; each query is run once on each engine for the correctness check just before, so its files are warm.
- wall time = whole process (DuckDB includes Python start and `import duckdb`, about 0.07 s of setup plus interpreter; its in-process time is also shown). Output to /dev/null. RSS = child `ru_maxrss`.
- correctness: all nine outputs compared before timing, sorted multiset, keys exact, other columns within 1e-9 relative; all nine match (Q4: 63,211,759 groups).
- ratio < 1 means qe is faster.

### warm cache, all cores

| query | file GB | qe min s | qe med s | duckdb min s | duckdb med s | duckdb in-proc min s | qe/duckdb (min wall) | qe GB/s | duckdb GB/s | qe RSS MiB | duckdb RSS MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Q1 global agg | 5.55 | 1.37 | 1.37 | 1.48 | 1.59 | 1.32 | 0.92x | 4.06 | 3.75 | 5389 | 832 |
| Q2 group id1 (10) | 5.55 | 1.33 | 1.35 | 1.68 | 1.72 | 1.50 | 0.79x | 4.17 | 3.29 | 5397 | 697 |
| Q3 group id2 (1e4) | 5.55 | 5.82 | 6.19 | 1.92 | 1.96 | 1.77 | 3.03x | 0.95 | 2.89 | 7092 | 1119 |
| Q4 group id3 (~6e6) | 5.55 | 15.02 | 15.12 | 4.14 | 4.27 | 3.90 | 3.63x | 0.37 | 1.34 | 7335 | 4527 |
| Q5 group str id4 | 5.55 | 11.74 | 11.91 | 3.35 | 3.42 | 3.09 | 3.51x | 0.47 | 1.66 | 8275 | 4838 |
| Q6 filter + agg | 5.55 | 1.54 | 1.55 | 1.70 | 1.79 | 1.51 | 0.91x | 3.59 | 3.26 | 5323 | 756 |
| Q7 join small + grp | 5.55 | 1.67 | 1.67 | 1.75 | 1.76 | 1.56 | 0.95x | 3.33 | 3.16 | 5421 | 697 |
| Q8 join medium + grp | 5.55 | 4.74 | 7.18 | 2.93 | 2.96 | 2.72 | 1.61x | 1.17 | 1.89 | 6641 | 1019 |
| Q9 join big (1e7x1e7) | 11.10 | 85.39 | 124.89 | 5.23 | 7.38 | 4.90 | 16.32x | 0.13 | 2.12 | 8247 | 5177 |

### warm cache, 1 thread each

| query | file GB | qe min s | qe med s | duckdb min s | duckdb med s | duckdb in-proc min s | qe/duckdb (min wall) | qe GB/s | duckdb GB/s | qe RSS MiB | duckdb RSS MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Q1 global agg | 5.55 | 9.17 | 9.35 | 7.86 | 7.90 | 7.69 | 1.17x | 0.60 | 0.71 | 5380 | 142 |
| Q2 group id1 (10) | 5.55 | 7.87 | 7.98 | 8.98 | 9.14 | 8.80 | 0.88x | 0.71 | 0.62 | 5384 | 143 |
| Q3 group id2 (1e4) | 5.55 | 16.32 | 20.91 | 10.24 | 10.47 | 10.05 | 1.59x | 0.34 | 0.54 | 6333 | 147 |
| Q4 group id3 (~6e6) | 5.55 | 51.52 | 67.15 | 21.01 | 22.57 | 20.72 | 2.45x | 0.11 | 0.26 | 7621 | 4530 |
| Q5 group str id4 | 5.55 | 36.25 | 36.81 | 14.66 | 14.91 | 14.44 | 2.47x | 0.15 | 0.38 | 8306 | 779 |
| Q6 filter + agg | 5.55 | 11.23 | 11.33 | 8.91 | 8.93 | 8.73 | 1.26x | 0.49 | 0.62 | 5296 | 142 |
| Q7 join small + grp | 5.55 | 10.49 | 10.64 | 9.32 | 9.58 | 9.14 | 1.13x | 0.53 | 0.60 | 5389 | 142 |
| Q8 join medium + grp | 5.55 | 24.12 | 26.60 | 12.62 | 12.85 | 12.41 | 1.91x | 0.23 | 0.44 | 6046 | 174 |
| Q9 join big (1e7x1e7) | 11.10 | 148.47 | 164.36 | 25.02 | 25.02 | 24.77 | 5.94x | 0.07 | 0.44 | 8150 | 4629 |

Caveats: qe peak RSS includes the memory-mapped input (about 5.4 GB for a 5.55 GB file), so it is not comparable to DuckDB's RSS as "memory used". The machine has 18 GiB and swap was in use, so large-RSS cells (qe Q3, Q8, Q9; DuckDB Q4, Q5, Q9) may include memory-pressure effects; qe's run-to-run spread is large on Q3 (1t: 16-35 s), Q8 and Q9 (all cores: 85-135 s). Cold-cache runs are not done yet (need `sudo purge`).

## EXP-008: integer-keyed parallel join table, before/after (cloud container, NOT the M3 Pro)

- machine: 4 vCPU Intel Xeon @ 2.10 GHz, 16 GiB RAM, no swap, Linux 6.18; warm cache; 2026-11-01. Relative gains only.
- data: `scripts/gen_data.py --rows 1e7|3e7|5e7` (seed 42). Wall time of the whole process, all cores, min of 3 interleaved runs per binary (medians and raw runs in `docs/experiments/008-int-join.md`).
- before = `cf9a1c1`, after = this commit.

| query | rows | before s | after s | speedup |
|---|---|---|---|---|
| Q7 join small + grp | 1e7 / 3e7 / 5e7 | 0.62 / 1.93 / 3.04 | 0.54 / 1.66 / 2.68 | 1.14x / 1.16x / 1.13x |
| Q8 join medium + grp | 1e7 / 3e7 / 5e7 | 0.80 / 3.42 / 6.20 | 0.67 / 2.86 / 5.42 | 1.20x / 1.20x / 1.14x |
| Q9 join big (N x N) | 1e7 / 3e7 / 5e7 | 9.32 / 37.18 / 78.80 | 1.64 / 5.85 / 9.04 | 5.69x / 6.36x / 8.71x |
