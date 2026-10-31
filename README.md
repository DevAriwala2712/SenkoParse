# qe: a from-scratch SQL engine for big CSV files

Runs `JOIN`, `GROUP BY` and aggregates directly on multi-GB CSV files, on a laptop, with no database or
dataframe library inside the engine (Rust; only `memmap2`, `memchr`, `rayon`). Correctness is checked against SQLite
on thousands of randomized queries; every performance claim has a documented experiment.

## Quick start
```bash
cargo build --release
python3 scripts/gen_data.py --rows 1e7            # ~1.2 GB into data/ (tables x, small, medium, big + schema.toml)
./target/release/qe --data data/ "SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4 ORDER BY 1"
```
Schema comes from `schema.toml` next to the CSVs (no type sniffing):
```toml
[x]                      # file is x.csv
id1 = "i64"              # i64 | f64 | decimal(N) | str | date
v1  = "f64"
```
Output is CSV with a header; NULL is an empty field. Supported SQL: see `CLAUDE.md`. Anything else fails with a clear error.

## Tests
```bash
cargo test                                          # fast: unit-style + small differential tests vs SQLite
cargo test --release --test difftest -- --ignored  # large randomized differential test (env: DIFFTEST_SEED/ROWS/QUERIES)
```
Each random query runs on three configurations (default parallel, `--no-opt` reference, 3 threads with 256-byte chunks) and must match SQLite.

## Status (M1 and M2 done)
| | |
|---|---|
| Correct vs SQLite | yes: differential tests green on every configuration |
| 1e7 rows, 11 threads, warm cache (M3 Pro) | scans/aggregates/small joins 0.13-0.22 s (2.5-4.2 GB/s) |
| 6.3e6 groups | `GROUP BY` 0.67 s (was 2.5 s in M1; about 0.28 s of it is writing 6.3M rows), EXP-004 |
| Known slow | 1e7 x 1e7 join 1.5 s: the hash table is built serially (next: EXP-005) |
| Not measured | cold cache, DuckDB comparison (DuckDB not installed), spill to disk (not implemented) |

Full tables: [`BENCHMARKS.md`](BENCHMARKS.md). Reproduce with `python3 bench/run.py --rows 1e7`.

## Docs
- [`CLAUDE.md`](CLAUDE.md): scope, rules, architecture, milestones
- [`docs/adr/0001-parallel-and-fast-path-execution.md`](docs/adr/0001-parallel-and-fast-path-execution.md): performance plan and decisions
- [`docs/experiments/`](docs/experiments/README.md): every experiment with raw numbers and outcome (including the ones that did not pay off)
