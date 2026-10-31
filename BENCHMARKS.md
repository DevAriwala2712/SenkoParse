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
