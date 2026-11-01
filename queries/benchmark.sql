-- The nine benchmark queries (same text as bench/run.py). Tables: x, small, medium, big.
-- Run one in the shell with `.q N`, or all of them with: qe --data DIR --file queries/benchmark.sql --table

-- Q1 global aggregate
SELECT COUNT(*), SUM(v1), AVG(v1) FROM x;

-- Q2 group by id1 (about 10 groups)
SELECT id1, SUM(v1) FROM x GROUP BY id1;

-- Q3 group by id2 (about 1e4 groups at 1e7 rows)
SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2;

-- Q4 group by id3 (high cardinality, about N groups)
SELECT id3, SUM(v1) FROM x GROUP BY id3;

-- Q5 group by string id4 with COUNT(DISTINCT)
SELECT id4, MIN(v1), MAX(v1), COUNT(DISTINCT id2) FROM x GROUP BY id4;

-- Q6 filter then aggregate
SELECT COUNT(*), SUM(v1) FROM x WHERE v1 > 50 AND id1 < 5;

-- Q7 join to the small table, then group
SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4;

-- Q8 join to the medium table, then group
SELECT m.id5, COUNT(*), SUM(x.v1) FROM x JOIN medium m ON x.id2 = m.id2 GROUP BY m.id5;

-- Q9 join two big tables (unique keys in big)
SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3;
