# Semantics

2026-11-02 · Companion to [production-readiness.md](production-readiness.md) · Features: [sql-surface.md](sql-surface.md)

What a correct answer **is**. Tests compare SenkoParse with an oracle, but an oracle only helps once we know which oracle's answer we mean and where we deliberately differ. Every rule below has a "today" (from the code at `f80c6e0`) and a "target"; rules marked **verify** are believed true but have no dedicated test yet.

**Contents:** [Dialect](#reference-dialect) · [Values and types](#values-and-types) · [NULL](#null-and-three-valued-logic) · [Coercion](#comparison-and-coercion) · [Arithmetic](#arithmetic) · [Aggregates](#aggregates) · [Grouping](#grouping-and-distinct) · [Ordering](#ordering) · [Joins](#joins) · [Cast](#cast) · [Type inference](#type-inference) · [CSV input](#csv-input) · [Output](#output) · [Determinism](#determinism) · [Dialect differences](#dialect-differences)

## Reference dialect

**Decision D-1 (recommended, to be recorded as an ADR):** for every feature that SQLite supports, SenkoParse returns what SQLite returns, under the comparison rules of the differential test. For features SQLite lacks, DuckDB is the reference. Where the two would give different answers for a feature both support, SQLite wins and the difference is listed in [Dialect differences](#dialect-differences) so users coming from DuckDB or PostgreSQL are not surprised.

Why SQLite first: it is already the oracle in `tests/difftest.rs` and the engine already matches it; it is independent of DuckDB, which is the engine we benchmark against; and its behaviour is stable across versions. Why not only SQLite: it lacks `ILIKE`, `FILTER` on all aggregates, `STRING_AGG ... ORDER BY` in older versions, quantiles, `DATE_TRUNC` and strict typing.

Where SQLite's behaviour comes from its dynamic typing (for example comparing a text and an integer by type class), SenkoParse is statically typed and **rejects** the query at bind time instead of imitating it. That is a deliberate deviation, listed below.

## Values and types

| Type | Representation | Range and precision | Today |
|---|---|---|---|
| `i64` | 64-bit signed integer | −2⁶³ … 2⁶³−1 | ✅ |
| `f64` | IEEE 754 binary64 | NaN and ±∞ representable | ✅ |
| `decimal(N)` | `i64` scaled by 10ᴺ, N ≤ 18 | exact | ✅ |
| `str` | UTF-8 bytes (validation policy: [CSV input](#csv-input)) | any length up to the row limit | ✅ |
| `date` | days since 1970-01-01 | proleptic Gregorian, years 0001-9999 (**verify** bounds) | ✅ |
| `bool` | true / false / NULL | expressions only today; column type planned (TYP-06) | 🟡 |
| `timestamp` | microseconds since epoch, UTC | planned (TYP-07) | ❌ |

## NULL and three-valued logic

- An **empty field** in CSV is NULL. A quoted empty field `""` is also NULL today (TEST-004 `empty quoted = NULL`). Target: keep this as the default, and with `--null STR` set, only `STR` is NULL and `""` is the empty string. **This is a results-affecting option and must be documented on the first use.**
- Comparisons with NULL yield NULL. `WHERE` and `HAVING` keep a row only if the predicate is TRUE.
- `NOT NULL` is NULL. `AND` and `OR` follow the standard truth tables:

| a | b | a AND b | a OR b |
|---|---|---|---|
| TRUE | NULL | NULL | TRUE |
| FALSE | NULL | FALSE | NULL |
| NULL | NULL | NULL | NULL |

- `x IN (1, NULL)` is TRUE if x = 1, otherwise NULL (never FALSE). `x NOT IN (1, NULL)` is never TRUE. This trap must have a test (FLT-04).
- Arithmetic with a NULL operand is NULL. `x / 0` is NULL (SQLite rule; DuckDB and PostgreSQL raise an error or return NULL depending on type, see differences).

## Comparison and coercion

| Left | Right | Today | Rule |
|---|---|---|---|
| `i64` | `decimal(N)` | ✅ | `i64` is scaled to `decimal(N)`; overflow while scaling is an error |
| `i64` or decimal | `f64` | ✅ | both become `f64` |
| `date` | string literal `'YYYY-MM-DD'` | ✅ | literal parsed at bind time; any other string is an error |
| `date` | `date` | ✅ | by day number |
| `str` | `str` | ✅ | by bytes (see [Ordering](#ordering)) |
| `str` | number | ⛔ | bind error (SQLite would compare by type class) |
| `bool` | `bool` | ⛔ | `comparing boolean expressions is not supported` (FLT-08) |

Integer literals are classified at bind time: a literal without a decimal point is an integer, with a point it is a decimal of the literal's scale, with an exponent it is `f64` (**verify** exponent handling).

## Arithmetic

| Operation | Today | Target |
|---|---|---|
| `i64 + - *` | checked; overflow is `integer overflow in arithmetic expression` | same |
| decimal `+ -` | checked at the common scale | same |
| decimal `*` | scale = sum of scales; error if it exceeds 18 | same |
| `/` on any numeric types | always `f64` (never integer division); `x / 0` is NULL | same; integer division gets `//` or `DIV` only if users ask (P2) |
| Unary minus | checked (`-i64::MIN` is an error) | same |
| `%` | missing | sign of the dividend, `x % 0` is NULL |
| `f64` NaN and ±∞ | propagate per IEEE 754 | same; printed per [Output](#output) |

## Aggregates

| Aggregate | Empty input or all NULL | Accumulator | Notes |
|---|---|---|---|
| `COUNT(*)` | 0 | `i64` | counts rows |
| `COUNT(x)` | 0 | `i64` | skips NULL |
| `COUNT(DISTINCT x)` | 0 | set of canonical key bytes | NULL not counted |
| `SUM(DISTINCT x)`, `AVG(DISTINCT x)` | NULL | same set; decoded at the end | integer and decimal: exact `i128`; `f64`: values sorted, then Kahan-summed, so the result does not depend on chunking or threads. `MIN/MAX(DISTINCT x)` = `MIN/MAX(x)` |
| `SUM` on `i64` | NULL | `i128`, error when the final value does not fit `i64` | overflow checked once, at the end, so it does not depend on order |
| `SUM` on decimal | NULL | `i128` at column scale | exact |
| `SUM` on `f64` | NULL | Kahan-compensated, fixed merge order | bit-identical across threads |
| `AVG` | NULL | sum and count; divides at the end | integer and decimal inputs give `f64` (**verify**: SQLite returns REAL) |
| `MIN`, `MAX` | NULL | one value | `f64`: NaN and −0.0 handling per [Ordering](#ordering) |
| Statistics (planned) | NULL; sample variance of one row is NULL | Welford, merged with Chan's formula | |

An aggregate query with no GROUP BY returns exactly one row, even on empty input.

## Grouping and DISTINCT

- All NULL keys form **one** group (GROUP BY, DISTINCT, COUNT(DISTINCT) of multi-column keys).
- `GROUP BY` names are resolved first as input columns, then as select aliases (an alias that shadows a column loses, as in SQLite and PostgreSQL). `GROUP BY 1` refers to the first select item.
- A non-grouped, non-aggregated column in SELECT is an error (SQLite would accept it and pick an arbitrary row; we do not, deliberately).
- `SELECT DISTINCT` is rewritten to `GROUP BY` over all select items (binder, `plan/mod.rs`). With GROUP BY, HAVING or aggregates it is rejected as unsupported. As with GROUP BY, an ORDER BY term must be a select item.

## Ordering

| Rule | Today | Target |
|---|---|---|
| NULL placement | NULLs first for ASC, last for DESC (SQLite) | same default; `NULLS FIRST/LAST` overrides |
| Strings | byte order of UTF-8, which equals Unicode code-point order | same; collations are a non-goal for 1.0 (`--collate` P2) |
| `f64` NaN | `partial_cmp` falls back to "equal", so NaN's position depends on input order (**bug risk**) | NaN sorts after +∞ for ASC (PostgreSQL and DuckDB rule; SQLite stores NaN as NULL), stated as a deviation |
| `f64` −0.0 vs 0.0 | compare equal; stable sort keeps input order | same, documented; `MIN`/`MAX` keep the first seen, which is why they are order-sensitive (ADR-0004) |
| Ties | stable sort over input in chunk order, so ties keep file order and are thread-independent | same; documented as "file order", not a SQL guarantee |
| Unordered results | thread-independent but unspecified order; partition-wise merge orders by partition | same; only `ORDER BY` gives an order |

## Joins

- Equality keys only (today). NULL keys never match, in INNER and LEFT joins.
- A LEFT join emits the left row with NULLs for every right column when nothing matches.
- Key types: `i64` with decimal compares after scaling (**verify** JN-07); `str` keys compare by bytes; `f64` keys compare by value with −0.0 = 0.0 and NaN never matching (**verify**).
- Row order of a join without `ORDER BY` is unspecified; tests compare joins as multisets.

## Cast

Planned (TYP-08). Rules to adopt:

| From → to | Rule | Failure |
|---|---|---|
| `str` → number | trim ASCII spaces, parse the full string | error with row and column (strict); `TRY_CAST` returns NULL |
| `f64` → `i64` | round half away from zero (**decide**: DuckDB rounds, PostgreSQL rounds, SQLite truncates) | out of range is an error |
| number → `str` | same text as [Output](#output) | — |
| `str` → `date` | `YYYY-MM-DD` only | error |
| anything → `bool` | `true/false/t/f/1/0`, case-insensitive | error |

## Type inference

**First cut implemented** ([ADR-0007](adr/0007-schema-inference.md), `storage/csv/infer.rs`, tests in `tests/infer.rs`). What it does today: header plus whole rows from the first 1 MiB; `i64` → `f64` → `str` and `date` → `str`; no `bool` or `decimal(N)` inference; duplicate or empty header names are an error (no suffixes); a later misfit fails with `invalid <type> value` plus a hint to declare the table in `schema.toml`; `schema.toml` always wins; no `DESCRIBE` yet (the shell `.schema` marks inferred tables). The target rules below remain the plan for the rest of task 1.1.

The rules must make inference **deterministic** (same file, same types, every run and every thread count) and **conservative** (a later surprise is an error that names `--schema`, never a silent NULL).

1. Read the header. Column names are lowercased (as `schema.toml` does today); duplicate names get suffixes `_2`, `_3` with a warning on stderr.
2. Sample: the first 10,000 rows, plus 1,000 rows from the start of each of up to 16 evenly spaced chunks in file mode. In stream mode only the start of the stream can be sampled, so stream inference is weaker; this is stated.
3. Candidate ladder per column, widest wins: `bool` → `i64` → `decimal(N)` (if every value has at most N ≤ 18 fractional digits and fits) → `f64` → `date` (only `YYYY-MM-DD`) → `str`. Empty fields are ignored for inference; an all-empty column is `str`.
4. **Decimal versus float** is a policy choice with visible consequences (exact sums, printed scale): see [performance-bets.md](performance-bets.md) I-9. Default recommendation: infer `f64` for compatibility with DuckDB's default, and offer `--infer-decimals`.
5. A value outside the sample that breaks the inferred type is `E-DATA` at row R, column C, with the inferred type and a hint to pass `--schema` or `--lenient`.
6. `DESCRIBE t` prints each column's type and whether it was declared or inferred.

## CSV input

| Rule | Today |
|---|---|
| RFC 4180 quoting, `""` escape, delimiters and newlines inside quotes | ✅ |
| LF and CRLF, mixed | ✅ |
| Header row required | ✅ (option to disable planned) |
| UTF-8 BOM skipped | ✅ |
| Missing final newline | ✅ |
| Blank lines | ✅ skipped (**verify** they never count as rows with all NULL) |
| Stray `"` inside an unquoted field | error on every path (needed for exact parallel splitting) |
| Ragged row | error naming the row and the field counts |
| Row longer than 64 MiB in stream mode | error |
| Invalid UTF-8 in a `str` column | **verify** (believed: passed through as bytes); target: error by default, `--invalid-utf8 replace\|bytes` |

## Output

| Value | CSV text today | Target |
|---|---|---|
| NULL | empty field | same |
| empty string (only possible once `--null` exists) | **verify** that it is written as `""` so it stays distinct from NULL | `""` |
| `i64` | decimal digits | same |
| decimal(N) | exactly N fractional digits (`12.500000` at N=6) | same |
| `f64` | Rust shortest round-trip (`0.1`, `1e21`, `NaN`, `inf`) | shortest round-trip, with `NaN`, `Infinity`, `-Infinity` (decision D-7) |
| `bool` | `1` / `0` | `true` / `false` (results-changing; before 1.0) |
| `date` | `YYYY-MM-DD`, including years before 1970 | same |
| strings | quoted when they contain `,` `"` CR or LF | same |

Header names: the alias, or the expression's source text for unnamed expressions (today). Output is flushed per batch in stream mode.

## Determinism

The contract users can rely on, and that the property tests enforce:

1. The same query on the same bytes gives the same **multiset** of rows on every run, machine, thread count, chunk size, block size, input mode and (once it exists) memory limit.
2. If `ORDER BY` orders the rows completely, the output **bytes** are identical under the same conditions, including `f64` values.
3. Without a complete `ORDER BY`, row order is thread-independent today but may change between versions; it is not part of the compatibility promise.

## Dialect differences

Known places where SenkoParse deliberately differs from SQLite, DuckDB or PostgreSQL. This list is part of the public correctness page.

| Topic | SenkoParse | SQLite | DuckDB | PostgreSQL |
|---|---|---|---|---|
| Typing | static; mismatches are bind errors | dynamic, compares by type class | static | static |
| Non-grouped column in SELECT | error | allowed (arbitrary row) | error | error |
| `/` on integers | `f64` result | integer division | `f64` result (`//` is integer division) | integer division |
| `x / 0` | NULL | NULL | NULL for `/` on floats; **verify** integers | error |
| NULLs in ASC order | first | first | last | last |
| `LIKE` case sensitivity | case-sensitive (implemented, task 1.4; an explicit exception to D-1, because `ILIKE` exists for the other case); the difftest sets `PRAGMA case_sensitive_like = ON` | case-insensitive for ASCII by default | case-sensitive | case-sensitive |
| NaN | a value, sorts after +∞ (planned) | stored as NULL | sorts after +∞ | sorts after +∞ |
| Integer overflow in `SUM` | error | error (`integer overflow`; `TOTAL()` returns REAL instead) | error, or widens to `HUGEINT` depending on type (**verify**) | error |
| Empty quoted CSV field `""` | NULL (default) | n/a | empty string by default | n/a |

Before a feature in this table is generated by the differential test, the test's normaliser must apply the difference (for example, generated queries always state `NULLS FIRST` or `NULLS LAST` when DuckDB is the oracle).
