# qe: a from-scratch SQL engine for big CSV files

SenkoParse (`qe`) runs `JOIN`, `GROUP BY` and aggregates on multi-GB CSV files, and aims to do it on
non-seekable streams (stdin, pipes) in memory that does not grow with the input. DuckDB issue #25165 shows
`read_csv('/dev/stdin')` buffering piped input and spilling it to temp files; `qe --stdin` reads the pipe in
bounded blocks instead. There is no database or dataframe library inside the engine (Rust; only `memmap2`,
`memchr`, `rayon`). Correctness is checked against SQLite on randomized queries. Streaming is implemented but
**not yet tested or measured**.

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

### Streaming from a pipe (implemented, NOT tested)
```bash
gunzip -c big.csv.gz | ./target/release/qe --data data --stdin t "SELECT k, SUM(v) FROM t GROUP BY k"
```
`--stdin t` reads table `t` from stdin; its schema still comes from `data/schema.toml`. Limits:
- The stream is parsed single-threaded.
- No `IN (SELECT ...)`. Use `JOIN`; it equals a semi-join only if the lookup keys are unique.
- The stream cannot be the right side of a `LEFT JOIN`, is never the build side of a join, and can be used once per query.
- Memory for the input is bounded; group tables and join build sides still grow with their own size.

## Why streaming: DuckDB reproduction
Our reproduction of DuckDB issue #25165 (DuckDB's behaviour only; no SenkoParse number is in this table).

| Setting | Value |
|---|---|
| DuckDB version | 2.0.0 |
| Data | the issue's synthetic data, 383 MB |
| Query | semi-join on a 50-id list |
| Limits | `memory_limit=256MB`, 1 thread |

| How DuckDB reads the data | Peak RSS | Temp files |
|---|---|---|
| By filename | 144 MiB | 0 |
| Through a pipe (`/dev/stdin`) | 324 MiB | 198 MiB |

SenkoParse on the same input: to be measured (EXP-005-stream).

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
| Known slow | 1e7 x 1e7 join 1.5 s: the hash table is built serially (deferred, "Later" in `CLAUDE.md`) |
| Streaming (`--stdin`) | implemented, untested: compiles; no test suite run; no memory or speed numbers (to be measured, EXP-005-stream) |
| Not measured | SenkoParse memory on streams, cold cache, DuckDB speed comparison (DuckDB not installed on the benchmark machine), spill to disk (not implemented) |

Full tables: [`BENCHMARKS.md`](BENCHMARKS.md). Reproduce with `python3 bench/run.py --rows 1e7`.

## Docs
- [`CLAUDE.md`](CLAUDE.md): scope, rules, architecture, milestones
- [`docs/adr/0002-stream-first-direction.md`](docs/adr/0002-stream-first-direction.md): why the headline is bounded-memory streaming
- [`docs/adr/0001-parallel-and-fast-path-execution.md`](docs/adr/0001-parallel-and-fast-path-execution.md): earlier speed plan and decisions
- [`docs/experiments/`](docs/experiments/README.md): every experiment with raw numbers and outcome (including the ones that did not pay off)
