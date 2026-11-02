# Correctness

2026-11-02 · Companion to [production-readiness.md](production-readiness.md) §7 · Test suites as they exist: [testing.md](testing.md)

This page is the **public evidence** for the claim "SenkoParse returns correct answers": which oracles define correct, how much has been checked, how the checking is built, and every known deviation. [testing.md](testing.md) describes the individual suites as they exist today; this page is the strategy and the running totals.

**Contents:** [Current totals](#current-totals) · [Oracles](#oracles) · [Comparison rules](#comparison-rules) · [Query generator](#query-generator) · [Configurations](#engine-configurations) · [Unsupported-SQL fuzzing](#unsupported-sql-fuzzing) · [CSV fuzzing](#csv-and-input-fuzzing) · [Edge cases](#edge-case-checklist) · [Golden and regression](#golden-and-regression-tests) · [Mutation testing](#mutation-testing) · [Deviations register](#deviations-register) · [Triage](#triage-of-a-diff)

## Current totals

Updated by CI once the nightly lane exists; until then, by hand from [testing.md](testing.md).

| Measure | Value | As of |
|---|---|---|
| Oracle | SQLite (bundled with `rusqlite` 0.32) | 2026-10-31 |
| Generated queries per default run | 1,500 (seeds 1-6, 300 rows) × 5 configurations | 2026-10-31 |
| Largest run with stream configurations | seed 1000, 20,000 rows, 1,500 queries; 2,718 stream comparisons, 282 skipped for documented limits | 2026-10-31 |
| Unexplained differences | 0 | 2026-10-31 |
| DuckDB-oracle queries | 0 (not wired; DuckDB is used only by benchmarks) | — |
| Fuzz hours | 0 | — |
| Recorded mutations that turned tests red | 2 (M-1, M-2) | [testing.md](testing.md#falsification-log) |

Target for 1.0: ≥ 100,000 generated queries against SQLite, ≥ 20,000 against DuckDB on its subset, zero unexplained differences, ≥ 24 h fuzzing per week.

## Oracles

| Oracle | Defines | Strength | Weakness |
|---|---|---|---|
| **SQLite** (primary) | every feature SQLite supports, per decision D-1 | independent, mature, row-exact, already wired; same rows inserted directly so the test needs no CSV parser | dynamic typing; lacks some functions; ASCII-insensitive `LIKE` by default |
| **DuckDB** (secondary) | features SQLite lacks: `ILIKE`, `FILTER`, quantiles, `STRING_AGG ... ORDER BY`, `DATE_TRUNC`, statistics | broad, analytical, strict typing | it is the benchmark rival, so it must never be the *only* oracle for a feature both support; Python-driven in this repo |
| **`--no-opt` reference** | every fast path, every thread count, every chunk size | simple, serial, inside the engine | shares the parser and binder with the fast path, so a binder bug passes |
| **File mode** | stream mode | exact same parser | inherits file-mode bugs |
| **Hand-written expectations** | edge cases and errors | can test what no oracle covers (error text) | written by the same people as the engine |

**Two-oracle rule.** For a feature both oracles support, the generator runs it against both. If SQLite and DuckDB disagree with each other, the case goes to [semantics.md](semantics.md#dialect-differences): either the normaliser handles it (for example by always generating explicit `NULLS FIRST/LAST`), or the generator stops producing that shape for DuckDB and the reason is written down.

## Comparison rules

From `CLAUDE.md`, made precise:

1. NULL equals NULL for comparison purposes.
2. Without `ORDER BY`: compare as multisets of rows.
3. With `ORDER BY`: compare in order, **except** within runs of rows whose sort keys are equal, which compare as multisets (the oracle's tie order is not specified).
4. `ORDER BY ... LIMIT n` with ties at the cut-off: the oracle may legally return different rows. The generator either adds a unique tiebreaker key or compares only the sort-key columns of the boundary rows.
5. Integers and decimals exactly; decimals compared as their exact text at the declared scale.
6. Floats with relative tolerance 1e-9 and absolute tolerance 1e-12 near zero; NaN equals NaN; +∞ equals +∞.
7. Error versus error: both must error. Error classes are compared once SQL error codes exist; error text is compared only against the reference path, never against an oracle.
8. Error versus result: always a failure, except where [semantics.md](semantics.md) lists a deliberate deviation (for example SQLite accepting a non-grouped column, which we reject).

## Query generator

The generator in `tests/difftest.rs` already produces expressions, comparisons, `IS [NOT] NULL`, aggregates, GROUP BY, HAVING, ORDER BY, LIMIT and INNER/LEFT joins over three typed tables with NULLs. To reach the 1.0 targets it needs:

| Improvement | Why |
|---|---|
| **Grammar-driven** generation with a weight per production, one production per [sql-surface.md](sql-surface.md) row | every new feature becomes generator coverage by adding one production |
| **Coverage report** per run: how many queries exercised each production and each type combination | "100,000 queries" means little if 90% are the same shape |
| **Type-directed** expression building | produce only well-typed queries for the correctness run, and deliberately ill-typed ones for the error run |
| **Data shapes** beyond uniform random: all-NULL columns, one distinct value, all distinct values, heavy skew (Zipf), extreme numbers (`i64::MIN`, `±0.0`, NaN, ±∞, 18-digit decimals), long and Unicode strings, empty tables | edge cases arise from data, not only from SQL |
| **Shrinking** of a failing case: remove clauses, expressions and rows while the difference persists | turns a 40-line random query into a 2-line regression test |
| **Seeds in failure messages** and a `DIFFTEST_REPLAY` variable | any CI failure reproduces locally in one command |
| **Faster stream configurations** (block sizes from a set like {37, 4096, 1 MiB} rather than always 37) | the large run with stream configurations takes about 20 minutes per seed today |

## Engine configurations

Each generated query runs in every configuration; the list grows with the engine:

| # | Configuration | Exists |
|---|---|---|
| 1 | default (parallel, 16 MiB chunks) | ✅ |
| 2 | `--no-opt` reference | ✅ |
| 3 | 3 threads, 256-byte chunks, forced partition-wise merge | ✅ |
| 4 | stream, random 1-300 byte reads, 37-byte blocks | ✅ |
| 5 | stream + `--no-opt` | ✅ |
| 6 | default + `--memory-limit 1MiB` (every operator spills) | Phase 2 |
| 7 | stream + `--memory-limit 1MiB` | Phase 2 |
| 8 | gzip-compressed input | Phase 5 |
| 9 | inferred schema instead of `schema.toml` (types must match the declared ones on generated data) | Phase 1 |
| 10 | real OS pipe through the CLI binary (subset of queries) | Phase 1 |

## Unsupported-SQL fuzzing

The rule "unsupported means an error, never a result" is checked by its own generator:

1. Take valid generated queries and insert one unsupported construct from a list kept in sync with the ⛔/❌ rows of [sql-surface.md](sql-surface.md) (window functions, `RIGHT JOIN`, `UNION`, `CASE`, subqueries, unknown functions, ...).
2. Also mutate tokens at random (drop, duplicate, swap, insert keywords).
3. Pass condition: the engine returns an error of class `E-SYNTAX`, `E-UNSUPPORTED` or `E-BIND`, never rows, never a panic, within a time limit.
4. Every construct that moves from ⛔ to ✅ leaves this list in the same PR and enters the correctness generator.

## CSV and input fuzzing

`cargo fuzz` targets (libFuzzer), each seeded with the files from `tests/edge_cases.rs` and `tests/stream.rs`:

| Target | Input | Oracle and pass condition |
|---|---|---|
| `fuzz_scanner` | arbitrary bytes as one CSV file | no panic; the parallel scan at a random chunk size returns the same rows or the same error as the serial scan |
| `fuzz_splitter` | arbitrary bytes + random block target and piece sizes | blocks concatenate to the input; every non-final block ends at a newline outside quotes |
| `fuzz_stream_vs_file` | arbitrary bytes + a fixed query set | stream result equals file result |
| `fuzz_parser` | arbitrary UTF-8 as SQL | no panic, no hang; error or a plan |
| `fuzz_end_to_end` | structured: a small typed table + a generated query | equals SQLite (a mini differential test inside the fuzzer) |
| `fuzz_spill` (Phase 2) | generated table + query + random tiny memory limit | equals the unlimited run |

Fuzzers run with debug assertions and overflow checks on. Crashes become regression tests under `tests/regression/` with the minimised input.

## Edge-case checklist

Each item needs at least one hand-written test; ✅ marks those that exist today.

**Files:** empty file (error) ✅ · header only ✅ · one row · no trailing newline ✅ · BOM ✅ · CRLF and mixed endings ✅ · blank lines ✅ · duplicate column names · quoted header · a row longer than the block size ✅ (stream) · a quoted field spanning a chunk boundary ✅ · stray `\r` at EOF ✅.

**Fields:** empty versus quoted empty versus the string `NULL` · quotes containing delimiters, newlines and `""` ✅ · stray quote in an unquoted field ✅ · ragged rows ✅ · leading/trailing spaces in numbers · `+5`, `-0`, `1e3`, `.5`, `5.` in each numeric type · 19-digit integers · invalid UTF-8 · embedded NUL bytes.

**Values:** `i64::MIN` and `MAX`; sum overflow ✅; decimal scales 0, 2, 18 ✅; negative decimals ✅; NaN, ±∞, −0.0 in `f64`; dates before 1970 ✅, leap days, year 9999; very long strings; combining characters and non-ASCII in comparisons, `LIKE` and `ORDER BY`.

**Semantics:** `COUNT(*)` vs `COUNT(col)` ✅ · all-NULL `SUM` ✅ · aggregates on empty input ✅ · NULL keys never join ✅ · NULLs form one group ✅ · `NOT IN` with NULL · HAVING without GROUP BY · `ORDER BY` an aggregate not in SELECT · `LIMIT 0` · `GROUP BY` an expression and its alias.

**Paths:** results equal across threads, chunk sizes, block sizes and input modes ✅ · same error text on every path ✅ · early exit on `LIMIT` from an endless stream ✅ · broken pipe on stdout · file modified during the query · SIGINT during a spill.

## Golden and regression tests

- `tests/golden/`: one `.sql` file per query with its expected `.csv`, covering every query in the README, the cookbook, `queries/benchmark.sql` and the demo. Run in every configuration. Expected outputs are regenerated only by an explicit command and reviewed in the diff.
- `tests/regression/`: one file per bug, named `R-NNNN-short-name.sql` with a header comment linking the issue or commit. The suite only grows. `R-0001` is the failed demo aggregation ([production-readiness.md §2.3](production-readiness.md#23-the-failed-demo-aggregation-triage-procedure)).

## Mutation testing

The falsification log in [testing.md](testing.md#falsification-log) records two hand-made mutations. To make this systematic:

- Run `cargo-mutants` on `crates/engine/src/exec/` and `crates/engine/src/storage/` in the nightly lane (or weekly if too slow).
- Every surviving mutant is triaged: add a test, or record why the mutation is equivalent (it cannot change behaviour).
- Re-run M-2 with the difftest, which the log notes was not done.

## Deviations register

Every place where SenkoParse knowingly returns something different from an oracle. Empty means none are known, not that none exist.

| ID | Query shape | SenkoParse | Oracle | Reason | Since |
|---|---|---|---|---|---|
| DEV-1 | non-grouped column in SELECT | error | SQLite: a value from an arbitrary row | standard SQL; avoids silent nondeterminism | M1 |
| DEV-2 | comparing `str` with a number | bind error | SQLite: compares by type class | static typing | M1 |
| DEV-3 | `""` (quoted empty) in CSV | NULL | DuckDB: empty string | matches the empty-field rule; option planned | M1 |
| DEV-4 | stray `"` in an unquoted field | error | DuckDB: accepted in some modes | exact parallel splitting requires it | M2 |

## Triage of a diff

When the differential test reports a difference:

1. **Reproduce** with the printed seed and `DIFFTEST_REPLAY`.
2. **Shrink** to the smallest query and data that still differ.
3. **Classify**: engine bug · oracle quirk (SQLite and DuckDB disagree) · comparison-rule gap (ties, float tolerance) · generator bug (produced something outside the supported surface).
4. **Act**: engine bug → fix plus a regression test; oracle quirk → [semantics.md](semantics.md#dialect-differences) and the normaliser; comparison gap → fix the comparator here and in the test; generator bug → fix the generator.
5. **Record** in the deviations register if the behaviour is kept on purpose.

A difference is never fixed by widening a tolerance without writing down why.
