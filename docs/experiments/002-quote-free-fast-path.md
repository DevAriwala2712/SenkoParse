# EXP-002: Quote-free scan fast path (skip fields nobody reads)

- **Status:** Done, kept (2026-10-31)
- **Idea:** ADR-0001 Idea 1
- **Code:** `crates/engine/src/storage/csv/mod.rs` (`next_batch_quote_free`), uncommitted working tree
- **Machine:** Apple M3 Pro, 11 cores, 18 GiB, macOS, warm page cache, single thread

## Hypothesis
If the file has no `"` byte, rows are exactly `\n`-delimited and fields `,`-delimited. Stopping after
the last needed column (jumping to the next newline with SIMD `memchr`) removes most of the 0.55 s
boundary-discovery cost seen in [EXP-001](001-baseline-breakdown.md).
**Gate (from the ADR):** `SELECT SUM(id1) FROM x` well under 0.3 s single-threaded.
**Kill:** if no improvement on early-column queries, drop the idea.
The path must not change behaviour: ragged rows are still errors even when the extra fields are never
parsed, and errors carry identical text on both paths.

## Method
- Reference path = `--no-opt` (`Options::no_opt`), the original quote-aware splitter.
- Fast path is chosen per file at open: one `memchr(b'"')` over the data region; `x.csv` has no quotes.
- To keep ragged-row detection, each line's commas are counted (auto-vectorised loop) and compared
  with the column count; only fields up to the last needed one are located (`memchr(',')` per field) and parsed.
```
python3 bench/exp.py --runs 7 "<sql>" ...                 # default (fast path)
python3 bench/exp.py --runs 7 --extra=--no-opt "<sql>" ... # reference
```
Both runs back to back in one session (raw: [data/exp002-fast.md](data/exp002-fast.md),
[data/exp002-reference.md](data/exp002-reference.md)). Data: `x.csv`, 1e7 rows, 7 columns (`id1,id2,id3,id4,id5,id6,v1`).

## Results (median of 6 warm runs, seconds)

| query | needs columns | reference | fast path | speedup |
|---|---|---|---|---|
| `COUNT(*)` | none | 0.566 | **0.223** | 2.5x |
| `SUM(id1)` | 0 | 0.626 | **0.295** | 2.1x |
| `SUM(id3)` | 0..2 | 0.606 | **0.419** | 1.4x |
| `MIN(id4),MIN(id5),MIN(id6)` | 0..5 | 0.670 | 0.610 | 1.1x |
| `SUM(v1)` | 6 (last) | 0.739 | 0.722 | 1.0x |
| 4 numeric columns | 0..6 | 0.872 | 0.865 | 1.0x |

## Interpretation
- The gate is met, but barely: `SUM(id1)` median 0.295 s, max 0.303 s over the runs (the ADR said "well under 0.3").
- The gain is proportional to the number of fields skipped. Queries that need the **last** column
  or most columns get nothing, because the remaining cost is a `memchr(',')` call per field (~8 ns each,
  70M fields) plus the number parsers.
- `COUNT(*)` at 0.223 s (2.4 GB/s) is now newline search + comma count + the file-wide quote check.
  The quote check is a separate serial pass; in the parallel scan it runs per chunk (free).
- The large effect on early-column queries does not carry over to typical analytic queries
  (they touch several columns, often the measure at the end), so on its own this is a modest win.

## Correctness
- `cargo test --release`: difftest small now runs 6 seeds (odd seeds produce quote-free files, even
  seeds the full quote/newline zoo) and checks **both** paths against SQLite; new edge test
  `default_and_reference_paths_agree_on_tricky_files` (13 awkward files x 6 queries: CRLF, blank lines,
  no final newline, a quote only on the last line, ragged rows, stray `\r` at EOF, bad numbers in read and
  unread columns, header-only) requires equal results and equal error text. All green.
- Large difftest (both paths, seeds 1000-1002, 1000 queries each): see the end of this file.

## Decision
Keep. It is the first stage of the plan and its per-chunk form is what the parallel scan will use.
**Follow-up (new experiment, not done):** locate the last column from the end of the line with `memrchr`
and avoid one call per skipped field (e.g. find all commas of a line in one vectorised pass). Expected to
help the "measure is the last column" case, which is the common one. Add it only with its own
before/after row.

## Caveats
- Per-file decision: one stray `"` anywhere in the file disables the fast path for the whole file
  (still correct, just slower). Per-chunk decisions come with the parallel scan.
- Numbers are single-machine, warm cache, 1 core.

## Addendum: large difftest result
`DIFFTEST_SEED=1000 DIFFTEST_QUERIES=1000 cargo test --release --test difftest -- --ignored --nocapture`
(log: [data/exp002-difftest.log](data/exp002-difftest.log)). Seeds 1000 and 1002 are the quote/newline zoo, seed 1001 is
quote-free (fast path). Each query ran on the default path and on `--no-opt` and matched SQLite on both:
```
difftest seed 1000: 1000 queries OK
difftest seed 1001: 1000 queries OK
difftest seed 1002: 1000 queries OK
test result: ok. 1 passed; finished in 77.53s
```
