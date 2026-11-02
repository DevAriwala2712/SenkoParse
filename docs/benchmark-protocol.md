# Benchmark protocol

2026-11-02 · Companion to [production-readiness.md](production-readiness.md) §8 · Numbers live in [`BENCHMARKS.md`](../BENCHMARKS.md) · Bets: [performance-bets.md](performance-bets.md)

How a speed or memory number becomes a claim. The repo already follows most of this informally ("no number, no claim", kill rules written first, raw runs kept). This page makes it a protocol so that results are comparable across machines, engines and months, and so that a sceptical reader can rerun them.

**Contents:** [Principles](#principles) · [Suites](#suites) · [Datasets](#datasets) · [Engines and settings](#engines-and-settings) · [Machines](#machines) · [Procedure](#procedure) · [Pre-registration](#pre-registration) · [Reporting](#reporting) · [Regression gate](#regression-gate) · [Threats](#threats-to-validity)

## Principles

1. **Thresholds before runs.** What counts as a win, a tie and a loss is committed to `bench/thresholds/` before the run, in the same commit as the harness change if any.
2. **Same answer first.** Each query is checked for equal output across engines (sorted multiset, as `bench/file_vs_duckdb.py check` does) before it is timed. A query whose outputs differ is reported as "not comparable", not timed.
3. **Losses in the same table.** Every cell is published, including the ones where SenkoParse loses.
4. **Name what is measured.** Wall time of the whole process (including start-up) or in-process query time; total RSS, anonymous RSS or accounted bytes ([memory-model.md](memory-model.md#what-memory-means)); warm or cold cache.
5. **Raw data or it did not happen.** Every run's raw numbers go to `docs/experiments/data/*.jsonl` and are linked from the table.

## Suites

### File suite (F)

F1-F9 are the existing Q1-Q9 in `queries/benchmark.sql` and `bench/run.py`, unchanged so history stays comparable. New queries extend the list:

| ID | Query | Exercises |
|---|---|---|
| F1-F9 | Q1-Q9 (global agg, three GROUP BY cardinalities, string GROUP BY with `COUNT(DISTINCT)`, filter, three joins) | existing |
| F10 | `SELECT id6, v1 FROM x ORDER BY v1 DESC LIMIT 10` | top-N without a full sort |
| F11 | `SELECT * FROM x WHERE v1 > 50` (half the rows, every column) | output-bound path |
| F12 | `SELECT COUNT(*) FROM x WHERE id6 LIKE '%123%'` | string scanning |
| F13 | join medium, group by, HAVING, ORDER BY, LIMIT | a realistic mix |
| F14 | `SELECT DISTINCT id4, id5 FROM x` | DISTINCT |

### Stream suite (S)

Run as `cat FILE | qe --stdin t ...` versus DuckDB `read_csv('/dev/stdin')`, and versus each engine reading the file by name.

| ID | Query | Exercises |
|---|---|---|
| S1 | the EXP-005-stream join against a 50-row lookup | the existing headline |
| S2 | selective filter, few columns | best case for streaming |
| S3 | GROUP BY a low-cardinality column | constant state |
| S4 | GROUP BY a high-cardinality column | state grows; spill under a limit |
| S5 | `ORDER BY ... LIMIT 10` | top-N over a pipe |
| S6 | `COUNT(DISTINCT)` on a high-cardinality column | unbounded state |

### Memory suite (M)

F4, F5, F9, F10, S4 and S6 under `--memory-limit 256MB` for SenkoParse and the equivalent setting for every engine that has one (DuckDB `memory_limit`, clickhouse-local `max_memory_usage` and `max_bytes_before_external_group_by`). Reported: wall time, total RSS, anonymous RSS, temp bytes written, pass/fail.

## Datasets

| Dataset | Source | Sizes | Why |
|---|---|---|---|
| **db-benchmark shape** | `scripts/gen_data.py` (seed 42) | 1e7, 1e8 rows | existing history; but keys are dense integers and floats have 6 decimals, which flatters dense-key fast paths |
| **Stream shape** | `bench/gen_stream_data.py` (reconstruction of DuckDB issue #25165) | 1x, 2x, 5x, 10x | existing headline; replace with the issue's own generator when available (claim C14) |
| **Stream shape, fresh seeds** | same generator, a different seed per size instead of repeating the 1x body | 1x-10x | removes the repetition artefact noted in EXP-005-stream |
| **Taxi trips** | NYC TLC trip records exported to CSV | 1-3 years of months | real skew, timestamps, missing values, decimals |
| **Web access log** | a public HTTP log converted to CSV | several GB | long text, low-cardinality status codes, high-cardinality URLs and IPs |
| **Sparse and string keys** | `gen_data.py --sparse-keys` (to add): UUID strings and random 64-bit ids instead of 1..N | 1e8 | the gate for every **[dense]** idea in [performance-bets.md](performance-bets.md) |
| **Sorted variant** | db-benchmark shape sorted by `id3` | 1e8 | for sortedness-detection experiments |

Licences of real datasets are recorded next to the download script; datasets are never committed, only their checksums.

## Engines and settings

| Engine | Modes | Settings | Note |
|---|---|---|---|
| SenkoParse | file, pipe | default; `--threads 1`; `--memory-limit` | release build, commit recorded |
| DuckDB | by filename, piped | `threads`, `memory_limit`, own `temp_directory` | through the CLI binary where possible, so Python start-up does not count against it (today it runs through Python, which favours SenkoParse; state this until changed) |
| clickhouse-local | by filename, piped (`--input-format CSVWithNames` from stdin) | `max_threads`, memory settings | baseline v1 of the plan asked for; not yet in the harness |
| xsv / qsv | where the query fits (F2-like counts, F11-like filters) | defaults | raw CSV speed reference |
| awk | F2, S2 | — | the floor most users compare against |
| MySQL | one-shot load then query | recorded settings | reference only; untuned, and stated as untuned |

Versions are pinned and printed by the harness.

## Machines

| Name | Hardware | Role |
|---|---|---|
| **Reference** | Apple M3 Pro, 11 cores (5P+6E), 18 GiB, macOS (the one used for the 1e8 tables) | headline numbers |
| **Linux** | the 4-core Xeon container used for EXP-005-stream, or a pinned cloud instance type | second machine; memory numbers must reproduce here |
| **Small VM** | 2 vCPU, 4 GiB RAM, Linux | the target user's machine; memory-suite pass/fail |
| **CI runner** | fixed self-hosted machine | regression gate only, never for headline claims |

Every table names its machine. A claim reproduced on only one machine says so (C9's wall-time ratio reproduced as 6.0x on the M3 Pro, not the 14.6x of the Linux container).

## Procedure

1. Build: `cargo build --release` at a clean commit; record `git rev-parse HEAD` and `rustc -V`.
2. Machine state: on macOS, swap usage (`sysctl vm.swapusage`), `vm_stat`, load average below 4 before starting; on Linux, `free -m`, governor set to `performance` where allowed, no other heavy processes. Record all of it (the interim 1e8 run already does this).
3. Check: run every query on every engine once and compare outputs; stop on any mismatch.
4. Time: at least **5 runs per cell** (3 is the current practice; raise to 5), interleaving engines per query to spread drift; no run discarded silently.
5. Cold cache: `sudo purge` (macOS) or `echo 3 > /proc/sys/vm/drop_caches` (Linux) before each cold run; label cold and warm separately.
6. Threads: all cores and 1 thread for each engine.
7. Memory: total RSS by `bench/rss_wrap.py` (`wait4` in a wrapper process, so the harness's own memory does not leak into the child's high-water mark, the P-1 lesson); anonymous RSS by sampling; temp bytes by sampling the temp directory every 20 ms.
8. Save raw runs as JSON lines; generate tables and charts with `bench/make_charts.py` from the raw file only.

## Pre-registration

A file `bench/thresholds/YYYY-MM-DD-<suite>.md` committed before the run:

```markdown
# Pre-registration: <suite>, <date>
- Commit under test: <sha>   Machine: <name>   Engines and versions: <list>
- Hypotheses (one per line, each with a number):
  - H1: qe min wall ≤ 1.5x the best baseline on F1-F3 at 1e8, warm, all cores.
  - H2: qe total RSS ≤ 25% of DuckDB's on S1-S3 at 10x.
- Kill or report rules: what result would make us change the README claim.
- Not tested this time: <list>, so nobody reads the result as covering it.
```

The results file links back to it and states, per hypothesis, held / failed / not run.

## Reporting

Each published table includes: query, dataset and size, machine, engine versions, mode (file/pipe), threads, cache state, min / median / max wall over N runs, the ratio to the best baseline using the **median** (the current tables use min; report both during the transition and say which a claim uses), total RSS, anonymous RSS where available, temp bytes, and a link to the raw file.

Charts: one metric per chart, both engines on the same axis, the losing cases not cropped out, the machine and date in the caption.

The README may quote a number only if it links to its row; [status.md](status.md) claims (C1-C14) are updated only from these tables.

## Regression gate

In CI on the fixed runner, nightly: F1-F9 at 1e7, S1-S3 at 1x, warm, 5 runs. A median more than 10% slower than the rolling baseline of the last 7 nightly runs fails the lane and opens an issue with the two commits and the flame graph diff. Memory gate: total RSS more than 10% higher on the stream suite fails the same way.

## Threats to validity

| Threat | Mitigation |
|---|---|
| Process start-up dominates small queries (DuckDB through Python adds interpreter start-up) | report in-process time where available; use DuckDB's CLI; run sizes where start-up is under 5% |
| Thermal throttling on laptops | interleave engines; record per-run timestamps; pause between cells |
| mmap RSS counted as "memory used" | report anonymous RSS separately ([memory-model.md](memory-model.md#what-memory-means)) |
| Generator flatters dense-key paths | sparse and string key dataset is mandatory for claims from **[dense]** ideas |
| Output size differs between engines (formatting, header) | compare parsed rows; write both to `/dev/null` while timing |
| Swap in use during the run | record swap; rerun if non-zero (the first 1e8 run had 1.3 GiB swap and is marked as such) |
| One query and one data shape behind a headline | S-suite and real datasets before any general streaming claim |
