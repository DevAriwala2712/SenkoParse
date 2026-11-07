-- Demo regression candidates (docs/production-readiness.md §2.3, Phase 0 task 0.2).
--
-- The aggregation a judge ran during the demo failed, and its exact text was not recorded. Every candidate
-- below is a query shape that failed at the demo; most still fail today. `tests/regression.rs` runs each statement and checks that it fails
-- with exactly the `expect-error` text, so this file records what a user sees today.
--
-- TODO R-0001: the real demo query is not known yet. When it is recovered (ask the teammates who were at the
-- demo, or a photo of the screen), add it as tests/regression/R-0001-demo-aggregation.sql with the SQLite /
-- DuckDB expected output. Phase 1 cannot close until it passes. If it is never recovered, every candidate here
-- becomes a regression test that must return the oracle's answer.
--
-- Format: directive comments, then one statement ending in `;`.
--   -- id: <name>          -- data: default | no-schema
--   -- expect-error: <exact error text>   or   -- expect-rows: <row> | <row> ...  (fields joined by `,`, NULL as `NULL`)
-- `{data}` in an expected error stands for the data directory. Tables (data = default):
--   t(col str, x i64, a i64, b i64)

-- id: C01 SELECT DISTINCT
-- data: default
-- expect-rows: a | b | NULL
SELECT DISTINCT col FROM t;

-- id: C02 conditional aggregation with CASE
-- data: default
-- expect-error: unsupported SQL feature: CASE
SELECT SUM(CASE WHEN x > 0 THEN 1 ELSE 0 END) FROM t;

-- id: C03 scalar function around an aggregate
-- data: default
-- expect-error: unsupported function 'ROUND'
SELECT col, ROUND(AVG(x), 2) FROM t GROUP BY col;

-- id: C04 SUM(DISTINCT)
-- data: default
-- expect-rows: 2
SELECT SUM(DISTINCT x) FROM t;

-- id: C05 AVG(DISTINCT)
-- data: default
-- expect-rows: 0.6666666666666666
SELECT AVG(DISTINCT x) FROM t;

-- id: C06 COUNT(DISTINCT a, b)
-- data: default
-- expect-error: syntax error at offset 23: expected ')', found ','
SELECT COUNT(DISTINCT a, b) FROM t;

-- id: C07a the judge's own CSV, no schema.toml (supported since ADR-0007 schema inference; rows checked by hand
--   against SQLite semantics: the all-empty row forms one NULL group)
-- data: no-schema
-- expect-rows: a,2 | b,1 | NULL,1
SELECT col, COUNT(*) FROM t GROUP BY col;

-- id: C07b the judge's own CSV, table not in schema.toml
-- data: default
-- expect-error: unknown table 'sales'
SELECT region, SUM(amount) FROM sales GROUP BY region;

-- id: C08a IN list before GROUP BY
-- data: default
-- expect-error: unsupported SQL feature: IN
SELECT col, COUNT(*) FROM t WHERE x IN (1, 2) GROUP BY col;

-- id: C08b BETWEEN before GROUP BY
-- data: default
-- expect-error: unsupported SQL feature: BETWEEN
SELECT col, COUNT(*) FROM t WHERE x BETWEEN 1 AND 2 GROUP BY col;

-- id: C08c LIKE before GROUP BY
-- data: default
-- expect-error: unsupported SQL feature: LIKE
SELECT col, COUNT(*) FROM t WHERE col LIKE 'a%' GROUP BY col;

-- id: C09 OFFSET after ORDER BY ... LIMIT
-- data: default
-- expect-error: unsupported SQL feature: OFFSET
SELECT col, SUM(x) FROM t GROUP BY col ORDER BY 2 DESC LIMIT 10 OFFSET 10;

-- id: C10 non-grouped column without GROUP BY (rejected correctly)
-- data: default
-- expect-error: column 'a' must appear in the GROUP BY clause or be used in an aggregate function
SELECT a, COUNT(*) FROM t;

-- id: C11a MEDIAN
-- data: default
-- expect-error: unsupported function 'MEDIAN'
SELECT col, MEDIAN(x) FROM t GROUP BY col;

-- id: C11b STDDEV
-- data: default
-- expect-error: unsupported function 'STDDEV'
SELECT col, STDDEV(x) FROM t GROUP BY col;

-- id: C11c STRING_AGG
-- data: default
-- expect-error: unsupported function 'STRING_AGG'
SELECT a, STRING_AGG(col, ',') FROM t GROUP BY a;
