# CLAUDE.md — From-scratch SQL query engine

## Goal
Build a query engine that runs JOIN, GROUP BY and aggregates over multi-GB CSV files, **correctly first, then fast**, without any database library inside the engine. Hackathon build (overnight, team). Framing: SDG 9.5, reproducible research capability on a student laptop.

**Headline (direction change, see `docs/adr/0002-stream-first-direction.md`): bounded-memory SQL over non-seekable CSV streams (stdin, pipes).** `gunzip -c big.csv.gz | qe --stdin t "SELECT ..."` must run in memory that does not grow with the input size. We no longer aim to beat DuckDB on general speed. Why: DuckDB issue #25165 reports that `read_csv('/dev/stdin')` buffers piped input and spills it to temp files. We reproduced it (DuckDB 2.0.0, 383 MB synthetic data, `memory_limit=256MB`, 1 thread): by filename 144 MiB peak RSS and 0 temp; through a pipe 324 MiB peak RSS and 198 MiB temp.

**Status of streaming (state it exactly like this): compiled, but NOT tested.** No test suite run, no SenkoParse memory or speed numbers yet. Make no performance or memory claim for SenkoParse; write "to be measured (EXP-005-stream)". Known limits: the stream is parsed single-threaded; no `IN (SELECT ...)` (use `JOIN`, which equals a semi-join only if the lookup keys are unique); the stream cannot be the right side of a `LEFT JOIN`.

Strategy and rationale live in `docs/strategy.md` (an export of the team's Claude Doc "From-Scratch SQL Engine: Existing Solutions and Where to Beat Them"). Read it before making design decisions. If this file and the doc disagree, this file wins for implementation details; flag the conflict. (As of the direction change that file is not in the repo; see ADR-0002 open items.)

## Hard rules
- The engine crate depends on no database or dataframe library (no DuckDB, SQLite, Polars, Arrow, DataFusion). SQLite and DuckDB are allowed **only** in `tests/` and `bench/` as reference oracles.
- Every optimization must keep the differential test green. A faster wrong answer is a regression.
- Every performance claim needs a before/after number in `BENCHMARKS.md` (query, data size, machine, wall time, GB/s). No number, no claim.
- Ask before adding a dependency. Allowed without asking: `memmap2`, `memchr`, `rayon`. Dev-only: `rusqlite`, `criterion`, `proptest`.

## Stack
- Rust stable, single Cargo workspace. Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`.
- Why Rust: memory-mapped files, SIMD `memchr`, data-parallel `rayon`, no GC pauses, safe threading for the chunked scan.

## Commands
```
cargo build --release
cargo test                                   # unit + differential tests (small data); checks default, --no-opt and tiny-chunk parallel paths
./target/release/qe --data data/ "SELECT ..." # run one query, CSV result to stdout
#   --no-opt          single-threaded reference path (no fast paths); everything else must match it
#   --threads N       worker threads (default: all cores); results never depend on N
#   --chunk-bytes N   scan chunk size (default 16 MiB); results never depend on N
#   --stdin TABLE     read TABLE from stdin instead of TABLE.csv (schema still from schema.toml).
#                     Compiled, NOT tested. Used at most once per query; never the join build side.
gunzip -c big.csv.gz | ./target/release/qe --data data/ --stdin t "SELECT ..."
python3 scripts/gen_data.py --rows 1e7       # db-benchmark-shaped data (x, small, medium, big tables)
cargo test --release --test difftest -- --ignored   # randomized difftest vs SQLite, larger data
python3 bench/run.py --rows 1e8              # timings vs DuckDB, appends to BENCHMARKS.md
```
Keep these commands working. Update this section if any change.

## Supported SQL (scope — reject anything else with a clear error)
`SELECT` columns, expressions (+ − × ÷, comparisons) and aggregates `COUNT(*)`, `COUNT(col)`, `SUM`, `AVG`, `MIN`, `MAX`, `COUNT(DISTINCT col)` · `FROM` one or more CSV tables (file name = table name) · `[INNER|LEFT] JOIN ... ON a = b [AND c = d]` · `WHERE` with `AND`/`OR`, comparisons, `IS [NOT] NULL` · `GROUP BY` · `HAVING` · `ORDER BY` · `LIMIT`. Schema comes from a `schema.toml` next to the data (column names, types: i64, f64, decimal(scale), str, date); no type sniffing.

## Architecture
```
crates/engine/src/
  sql/      lexer, parser, AST
  plan/     logical plan, binder (resolve columns/types), rewrites:
            projection pushdown, filter pushdown, eager aggregation
  exec/     physical operators over column batches:
            scan, filter, project, hash_agg, hash_join, sort, limit
  storage/  csv/ (chunker, field scanner, number parsers,
            stream.rs: bounded-memory reader for non-seekable input; compiled, NOT tested),
            cache/ (optional columnar cache)
  types/    values, NULL bitmaps, decimal as scaled i64
crates/cli/  `qe` binary
tests/difftest.rs   random data + random queries, compared against SQLite
scripts/, bench/
```
Operators process batches of columns (vectorized), not one row at a time. Keep a simple single-threaded reference path behind `--no-opt` for debugging.

## Correctness rules (the traps that cost points)
- **NULL**: empty field = NULL. `COUNT(col)` skips NULLs, `COUNT(*)` doesn't. `AVG`/`SUM` ignore NULLs; `SUM` of all-NULL = NULL. NULL keys never match in joins. All NULL keys form one GROUP BY group.
- **CSV**: RFC 4180 quoting, commas and newlines inside quotes, `""` escapes, CRLF, header row, missing final newline.
- **Chunk splitting**: a quoted field can contain a newline. Split points respect quote state exactly (per-chunk quote parity). A `"` inside an unquoted field is a hard error on every path, because it would make the parity argument wrong.
- **Numbers**: integer `SUM` in i128, error on overflow when narrowing. `AVG` is never integer division. Decimal columns are scaled i64 so sums are exact and independent of merge order. f64 sums use Kahan summation.
- **Output**: CSV with header; deterministic order when `ORDER BY` is given. Difftest compares unordered results as multisets and floats with a relative tolerance of 1e-9.

### Streaming rules (`--stdin`, `storage/csv/stream.rs`)
- **Bounded memory**: a reader thread fills a bounded queue (16 x 1 MiB pieces). Memory for the input must not grow with input size. Group tables and join build sides are not part of this bound; they grow with group count and build-side size as in file mode.
- **Quote-correct cuts**: each block is cut after the last newline outside quotes, using the same quote-parity rule as `plan_chunks`. The partial tail row carries over to the next block. A row longer than 64 MiB is an error.
- **Stream is always the probe side**: it is never the hash/build side of a join, and it cannot be the right side of a `LEFT JOIN`.
- **Single scan**: a stream can be read once. A query that uses the stream table twice is rejected.
- **Same answers as file mode**: results and error text are identical to reading the same data by file name, apart from the `<stdin>` label. Output is flushed per batch. The difftest must compare stream mode against file mode (not yet done).

## Performance plan (in this order; don't skip ahead while an earlier step is red)
1. **Verify streaming**: run the tests and diff `--stdin` against file mode on the same data. Until this is green nothing below counts.
2. **Memory proof vs DuckDB** at 1x / 2x / 5x / 10x input size (piped), with peak RSS and temp-file use recorded for both (EXP-005-stream). No number, no claim.
3. **Tricky-input tests**: quoted newlines, CRLF, byte-dribbled reads (1 byte per `read`), tiny `--chunk-bytes`.
4. **Demo**: the `gunzip -c ... | qe --stdin` pipeline, side by side with DuckDB.
5. **Optional: `IN (SELECT ...)`** (a real semi-join, so no unique-key caveat).

### Later (old speed plan; only after the streaming steps above)
EXP-005 parallel build / dense-key array join (1e7 x 1e7 join), exact-decimal sums, `COUNT(DISTINCT)` rewrite, top-N for `ORDER BY ... LIMIT`, filter pushdown, then the milestones below. The `--stdin` single-threaded parse limit also belongs here.

Old milestone list, kept for reference:
1. **M1, correct baseline**: single-thread mmap scan, parse only referenced columns, hash aggregation, hash join building on the smaller input (size estimated from file size and a sample). Difftest green.
2. **M2, parallel scan** (DONE, see `docs/experiments/003-parallel-chunked-scan.md`): fixed 16 MiB chunks split at row boundaries (exact, via quote parity); one partial aggregate per chunk, merged in a fixed tree so results are identical for any core count. Per ADR-0001, per-chunk tables are only right for low group cardinality; for millions of groups use partitioned aggregation (EXP-004).
3. **M3, specialization**: hand-written integer and decimal parsers; open-addressing hash table on raw byte-slice keys (no per-row allocation); direct array indexing when an integer key's range is small. Choose per query from a sample.
4. **M4, eager aggregation**: for a join on a dimension's unique key followed by GROUP BY on dimension columns, aggregate the fact side by the join key first. Verify key uniqueness during build; fall back if not unique.
5. **M5, fused loops** for the db-benchmark query shapes, only if measurements show operator overhead matters.
6. **M6, columnar cache** behind `--cache` (off by default). Judges' timing method is unknown: the cold path must be the fast path, and the cache is an extra.
7. **Spill**: Grace hash partitioning only when the build side's estimated size exceeds available RAM.

Low priority: SWAR tricks, `unsafe` micro-optimizations, GPU. Use them only after M4 is done, measured and green.

## Docs and experiments
- `docs/adr/` decisions, `docs/experiments/` one file per experiment (hypothesis, method, raw numbers, correctness, decision). Write the hypothesis and kill threshold *before* the code, fill results after. `bench/exp.py` prints every raw run.
- `README.md` has the quick start and current status.

## Working style
- Small, reviewable steps. After each milestone: run the difftest, run the bench, update `BENCHMARKS.md`, summarize what changed in 3–5 lines.
- When a design choice is non-obvious (e.g. a rewrite's validity condition), write the reasoning in a doc comment at the code site.
- Profile before optimizing (`perf`, or `cargo flamegraph`); name the bottleneck in the commit message.
- If a requirement is ambiguous, check `docs/strategy.md`, then ask; don't guess silently.

## Open questions (update when answered)
- How judges time queries: cold per query or repeated over the same files.
- Exact data format, schema and file sizes the judges will use.
- Whether non-database crates (`rayon`, `memchr`, `memmap2`) are allowed.
- Required output format and float precision.
- Judging hardware (cores, RAM).
