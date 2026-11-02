# SQL surface

2026-11-02 · Companion to [production-readiness.md](production-readiness.md) §4 · Semantics: [semantics.md](semantics.md)

The single list of what SenkoParse's SQL accepts. Every feature has an ID, its state **today** (audited from `crates/engine/src/sql/` and `plan/` at `f80c6e0`), a priority, the oracle that defines its correct answer, and the acceptance test that proves it. The README's "Supported SQL" section is generated from this table (requirement R-DOC-1), so the two cannot drift.

**Legend.** Today: ✅ supported and tested · 🟡 supported but untested, or state unclear (verify in Phase 0, task 0.1) · ⛔ rejected with a clear error · ❌ not recognised (generic parse error). Priority: **P0** release, **P1** 1.0, **P2** later. Oracle: **S** SQLite, **D** DuckDB, **H** hand-written expectation, **R** `--no-opt` reference.

> ⛔ is the correct state for any feature that is not built. ❌ should become ⛔ before Phase 1 closes: a user must always learn that a feature is unsupported, not that their query has a "syntax error".

## Sources and schema

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| SRC-01 | Table = `<name>.csv` in `--data DIR`, typed by `schema.toml` | ✅ | P0 | S | difftest |
| SRC-02 | `--stdin NAME` binds a pipe to one table | ✅ once per query, probe side only | P0 | file mode | `tests/stream.rs` |
| SRC-03 | Schema inference from a header and a sample (no `schema.toml`) | ❌ (`cannot read schema.toml`) | **P0** | H | inference corpus: 30 files with known types |
| SRC-04 | `--table name=path` for a file outside `--data` | ❌ (`--table` is the pretty-print flag today; needs a new name, see [cli.md](cli.md)) | P0 | S | CLI test |
| SRC-05 | Explicit schema file `--schema FILE` overriding inference | ❌ | P0 | H | override test |
| SRC-06 | gzip and zstd input, detected by magic bytes | ❌ (works through an external `gunzip` pipe) | P1 | file mode | same rows from plain, `.gz`, `.zst`, pipe |
| SRC-07 | Several piped tables (named FIFOs, `--stdin a --fifo b=PATH`) | ❌ | P1 | file mode | two-pipe join test |
| SRC-08 | Glob of files as one table | ❌ | P2 | S | union-equivalence test |
| SRC-09 | Stream table read twice (self-join, subquery) by buffering to temp | ⛔ (`used twice`) | P1 | file mode | buffered self-join equals file mode |
| SRC-10 | Stream on the right side of `LEFT JOIN` | ⛔ | P1 | file mode | build-from-stream under budget |

## Types and literals

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| TYP-01 | `i64` | ✅ | P0 | S | difftest |
| TYP-02 | `f64` | ✅ | P0 | S | difftest, tolerance 1e-9 |
| TYP-03 | `decimal(N)` as scaled `i64`, exact | ✅ | P0 | S (as text) | TEST-001 decimals |
| TYP-04 | `str` | ✅ | P0 | S | difftest |
| TYP-05 | `date`, compared with `'YYYY-MM-DD'` literals | ✅ | P0 | S | difftest |
| TYP-06 | `bool` column type (`true/false/t/f/1/0`) | ❌ (boolean expressions exist; no column type; output `1`/`0`) | P0 | D | bool round-trip |
| TYP-07 | `timestamp` (ISO 8601, optional fraction and zone) | ❌ | P1 | D | parse and compare table |
| TYP-08 | `CAST(x AS type)` and `x::type` | ⛔ (`CAST` not a keyword) | P0 | D | cast matrix, failure rule per [semantics.md](semantics.md#cast) |
| TYP-09 | Numeric literal classification (integer, decimal, float) at bind time | ✅ | P0 | S | literal tests |
| TYP-10 | String literals with `''` escape | 🟡 | P0 | S | lexer tests |
| TYP-11 | `DATE '2026-01-01'` typed literal | ❌ | P1 | D | literal tests |

## Projection

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| PRJ-01 | Column list, qualified `t.col` | ✅ | P0 | S | difftest |
| PRJ-02 | Aliases with `AS` and bare | ✅ | P0 | S | difftest |
| PRJ-03 | `+ - * /`, unary minus, parentheses | ✅ (`/` always float; `x/0` is NULL) | P0 | S | difftest |
| PRJ-04 | `%` modulo | ❌ | P1 | S | arithmetic table |
| PRJ-05 | `\|\|` string concatenation | ❌ | P1 | S | string table |
| PRJ-06 | `SELECT *` | 🟡 parser accepts a bare `*`; README says unsupported | **P0** | S | resolve in Phase 0 (task 0.4) |
| PRJ-07 | `t.*` | ❌ | P0 | S | join projection test |
| PRJ-08 | `*` mixed with other items | ⛔ | P1 | S | — |
| PRJ-09 | Constant expressions without FROM (`SELECT 1+1`) | ❌ | P2 | S | — |

## Filters and predicates

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| FLT-01 | `= <> != < <= > >=` | ✅ (`!=` 🟡) | P0 | S | difftest |
| FLT-02 | `AND`, `OR`, `NOT` with three-valued logic | ✅ | P0 | S | 3VL truth-table test |
| FLT-03 | `IS NULL`, `IS NOT NULL` | ✅ | P0 | S | difftest |
| FLT-04 | `IN (list)`, `NOT IN (list)` with NULL semantics | ⛔ | **P0** | S | NULL-in-list cases |
| FLT-05 | `BETWEEN`, `NOT BETWEEN` | ⛔ | **P0** | S | bounds inclusive |
| FLT-06 | `LIKE`, `NOT LIKE`, `ESCAPE` | ⛔ | **P0** | S (case rule differs, see semantics) | pattern table incl. `%`, `_`, escapes, Unicode |
| FLT-07 | `ILIKE` | ⛔ | P1 | D | — |
| FLT-08 | Comparing boolean expressions (`(a>b) = (c>d)`) | ⛔ | P2 | S | — |
| FLT-09 | `IS DISTINCT FROM` | ❌ | P1 | D | NULL-safe equality |

## Conditional expressions

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| CND-01 | `CASE WHEN ... THEN ... ELSE ... END` (searched) | ⛔ | **P0** | S | type unification, missing ELSE = NULL |
| CND-02 | Simple `CASE x WHEN v THEN ...` | ⛔ | P0 | S | — |
| CND-03 | `COALESCE` | ⛔ (unsupported function) | **P0** | S | — |
| CND-04 | `NULLIF` | ⛔ | P1 | S | — |
| CND-05 | `GREATEST`, `LEAST` | ⛔ | P1 | D | NULL rule documented |

## Joins

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| JN-01 | `[INNER] JOIN ... ON a = b [AND c = d]` | ✅ | P0 | S | difftest |
| JN-02 | `LEFT [OUTER] JOIN` | ✅ | P0 | S | difftest |
| JN-03 | Comma join with the equality in WHERE | ✅ | P0 | S | difftest |
| JN-04 | Self-join with aliases | 🟡 (alias rules allow it; no dedicated test) | P0 | S | self-join test |
| JN-05 | 3+-way joins | 🟡 (join loop allows it; verify generator coverage) | P0 | S | 3- and 4-way tests |
| JN-06 | Duplicate and NULL keys | ✅ NULL never matches | P0 | S | TEST-001 |
| JN-07 | Join keys of different numeric types (`i64` = `decimal`) | 🟡 | P0 | S | coercion test |
| JN-08 | `RIGHT`, `FULL` | ⛔ | P1 | S (3.39+ supports both) | no-match rows on each side |
| JN-09 | `CROSS JOIN`, comma join without equality | ⛔ | P1 | S | row-count guard (`--max-cross-rows`) |
| JN-10 | `USING (cols)`, `NATURAL` | ⛔ | P1 / P2 | S | column merge rule |
| JN-11 | Non-equality conditions in `ON` (`a.x < b.y`) | ⛔ | P1 | S | as a post-join filter for INNER; nested loop with a budget for LEFT |
| JN-12 | Partitioned join when the build side exceeds the budget | ❌ (in memory) | P1 | R | join under `--memory-limit 64MB` |

## Aggregation and grouping

The aggregate catalog with state shapes is in [production-readiness.md §5](production-readiness.md#52-aggregate-catalog).

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| AGG-01 | `COUNT(*)`, `COUNT(x)`, `SUM`, `AVG`, `MIN`, `MAX` | ✅ | P0 | S | difftest |
| AGG-02 | `COUNT(DISTINCT x)` | ✅ | P0 | S | difftest |
| AGG-03 | `SELECT DISTINCT` | ⛔ | **P0** | S | equals GROUP BY all columns |
| AGG-04 | `SUM(DISTINCT x)`, `AVG(DISTINCT x)` | ⛔ | **P0** | S | duplicates across chunks |
| AGG-05 | `COUNT(DISTINCT a, b)` | ❌ | P2 | D | — |
| AGG-06 | GROUP BY column, expression, ordinal, alias | ✅ | P0 | S | difftest, `GROUP BY 1` |
| AGG-07 | `HAVING`, including aggregates not in SELECT | ✅ | P0 | S | difftest |
| AGG-08 | Aggregate without GROUP BY returns one row on empty input | ✅ | P0 | S | TEST-001 `empty_table_aggregates` |
| AGG-09 | Invalid uses rejected (aggregate in WHERE or GROUP BY, nested, non-grouped column) | ✅ | P0 | H | error tests |
| AGG-10 | Statistics: `STDDEV_SAMP/POP`, `VAR_SAMP/POP` (+ `STDDEV`, `VARIANCE` aliases) | ⛔ | P1 | D | Welford vs two-pass on adversarial data |
| AGG-11 | `STRING_AGG(x, sep [ORDER BY])`, `GROUP_CONCAT` | ⛔ | P1 | D | in-group order |
| AGG-12 | `ANY_VALUE`, `ARG_MIN`, `ARG_MAX`, `BOOL_AND`, `BOOL_OR` | ⛔ | P1 | D | tie rule |
| AGG-13 | `MEDIAN`, `QUANTILE_CONT`, `QUANTILE_DISC` | ⛔ | P1 | D | even counts, interpolation |
| AGG-14 | `FILTER (WHERE ...)` | ❌ | P1 | D | — |
| AGG-15 | `APPROX_COUNT_DISTINCT`, `APPROX_QUANTILE` | ⛔ | P2 | D (within bound) | error-bound test |
| AGG-16 | `GROUPING SETS`, `ROLLUP`, `CUBE` | ❌ | P2 | D | totals rows |
| AGG-17 | Spilling aggregation under a budget | ❌ | P1 | R | 50M groups under 256 MB |

## Ordering and limits

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| ORD-01 | `ORDER BY` several keys, `ASC`/`DESC`, ordinals, aliases, aggregates | ✅ | P0 | S | difftest |
| ORD-02 | NULL placement: NULLs first for ASC, last for DESC (SQLite rule) | ✅ | P0 | S | difftest |
| ORD-03 | `NULLS FIRST` / `NULLS LAST` syntax | ❌ | **P0** | S (3.30+) | all four combinations |
| ORD-04 | `LIMIT n` with early exit | ✅ | P0 | S | endless-stream test |
| ORD-05 | `OFFSET m` and `LIMIT m, n` | ⛔ | **P0** | S | — |
| ORD-06 | Top-N heap for `ORDER BY ... LIMIT n` | ❌ (full sort) | P0 | R | memory stays O(n) |
| ORD-07 | External merge sort under a budget | ❌ | P1 | R | sort larger than the budget |
| ORD-08 | Deterministic tie order across thread counts | ✅ (stable sort over fixed-order input) | P0 | R | `tests/parallel.rs` |

## Composition

| ID | Feature | Today | Pri | Oracle | Acceptance test |
|---|---|---|---|---|---|
| CMP-01 | Derived tables `FROM (SELECT ...) t` | ❌ | P1 | S | — |
| CMP-02 | `IN (SELECT ...)` as a semi-join, `NOT IN` with NULL trap | ⛔ | P1 | S | NULL in subquery result |
| CMP-03 | `EXISTS`, `NOT EXISTS` (correlated) | ⛔ | P1 | S | decorrelated to semi/anti join |
| CMP-04 | Scalar subquery | ❌ | P1 | S | more-than-one-row error |
| CMP-05 | `WITH` (non-recursive CTEs) | ⛔ | P1 | S | referenced 0, 1, 2 times |
| CMP-06 | `UNION`, `UNION ALL`, `INTERSECT`, `EXCEPT` | ⛔ | P1 | S | multiset rules |
| CMP-07 | Window functions (`ROW_NUMBER`, `RANK`, `SUM OVER`, `LAG`, `LEAD`) | ❌ | P2 | D | frames |
| CMP-08 | Several statements per invocation (`;`-separated, `--file`) | ✅ (shell and `--file`) | P0 | H | script test |

## Scalar functions (none exist today)

| ID | Group | Functions | Pri | Oracle |
|---|---|---|---|---|
| FN-01 | Numeric, first set | `ROUND(x[, d])`, `ABS` | **P0** | S |
| FN-02 | String, first set | `LOWER`, `UPPER`, `LENGTH` | **P0** | S (ASCII), D (Unicode) |
| FN-03 | Numeric, rest | `FLOOR`, `CEIL`, `MOD`, `POWER`, `SQRT`, `LN`, `EXP`, `SIGN` | P1 | D |
| FN-04 | String, rest | `SUBSTR`, `TRIM/LTRIM/RTRIM`, `REPLACE`, `CONCAT`, `SPLIT_PART`, `STARTS_WITH`, `POSITION`, `LEFT`, `RIGHT` | P1 | D |
| FN-05 | Date and time | `EXTRACT`, `DATE_TRUNC`, `DATE_PART`, `STRPTIME`, `CURRENT_DATE` (pinned per run) | P1 | D |
| FN-06 | Regex | `REGEXP_MATCHES`, `REGEXP_REPLACE` (own engine or none; no regex crate unless D-10 allows) | P2 | D |

## Output and introspection

| ID | Feature | Today | Pri | Acceptance test |
|---|---|---|---|---|
| OUT-01 | CSV with header, RFC 4180 quoting | ✅ | P0 | round trip through a CSV parser |
| OUT-02 | Pretty table (`--table`, shell) | ✅ | P0 | golden |
| OUT-03 | TSV, JSON Lines, header off | ❌ | P1 | round trip |
| INT-01 | `DESCRIBE t`, `--schema-only` | ❌ (shell `.schema` exists) | P0 | golden |
| INT-02 | `EXPLAIN`, `EXPLAIN ANALYZE` | ❌ | P1 | golden plans |
| INT-03 | `--stats` JSON on stderr | ❌ (`--progress` and shell `.timer` exist) | P1 | schema test |

## Errors

| ID | Feature | Today | Pri | Acceptance test |
|---|---|---|---|---|
| ERR-01 | Recognised-but-unsupported keywords give `unsupported SQL feature: X` | ✅ for `RIGHT FULL CROSS NATURAL UNION INTERSECT EXCEPT LIKE IN BETWEEN CASE OFFSET USING WITH EXISTS` and `SELECT DISTINCT` | P0 | unsupported fuzz |
| ERR-02 | Error class and position (`--> query:1:8`) | ❌ | P0 | error golden tests |
| ERR-03 | Same error text on every path | ✅ | P0 | TEST-003, TEST-004 |
| ERR-04 | Unknown functions name the function | ✅ | P0 | — |

## Definition of done for a new feature

A row moves to ✅ only when all of these hold:

1. Parser, binder and executor support it on **both** the reference path and the fast path, and in stream mode.
2. The random query generator emits it (or the reason it cannot is written here).
3. The differential test passes against its oracle; disagreements between SQLite and DuckDB are resolved by [semantics.md](semantics.md) and listed there.
4. Edge cases from its row are hand-written tests.
5. It works under a tiny memory limit (once Phase 2 exists) with identical results.
6. [semantics.md](semantics.md) states its NULL and type rules; this table and the generated README section are updated in the same PR.
