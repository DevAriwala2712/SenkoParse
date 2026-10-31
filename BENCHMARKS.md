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
