# Pitch and hostile Q&A

2026-11-02 · Companion to [production-readiness.md](production-readiness.md) · Claims ledger: [status.md](status.md)

What we say out loud, and when we are allowed to say it. Every answer is tied either to evidence that exists today or to the phase gate that makes it true. Saying a "Phase N" answer before that gate is met is how a pitch loses a room.

## Opening (first 20 seconds)

> "SenkoParse is a read-only SQL query engine for huge CSV files and pipes, written from scratch in Rust with no database library. It is not a database: no writes, no transactions, by design. It trades SQL breadth for a memory bound you can check: on a 3.7 GB pipe it stayed at about 42 MiB, where DuckDB wrote 5.4 GiB of temp files."

This sets the category before anyone asks what it is, and puts the one measured differentiator in the first breath. The 42 MiB / 5.4 GiB numbers are from EXP-005-stream: one query, one reconstructed data shape, one thread each. Say "on our streaming join" if asked.

## Order of the demo

1. **Correctness evidence first**: the differential test against SQLite, the five configurations, the mutation log.
2. **The memory bound**: the live side-by-side stream demo (`./demo.sh`), RSS flat while DuckDB's temp grows.
3. **Speed**: only the streaming query, and the file-mode table with its losses visible.
4. **Limitations slide**, before anyone asks.

Never lead with speed. It is the claim a sceptic attacks first, and on file mode at 1e8 rows we lose 4 of 9 queries.

## One word, then one sentence

| Question | One word | Then one sentence | True since |
|---|---|---|---|
| Is it ACID? | Read-only. | Nothing is written, so atomicity and durability do not apply; it detects a changing input file for isolation. | read-only: today; detection: Phase 3 |
| Why no transactions? | Scope. | It is a query engine, like grep for tables, not a database management system. | today |
| What if the file changes mid-query? | Detected. | Size, inode and modification time are checked and the query fails instead of returning a torn result. | Phase 3 (today: **not detected**; say "planned") |
| Is it memoryless? | Bounded. | Stream input is bounded today; group tables, joins and sorts spill to disk at a set limit after Phase 2. | input: today; operators: Phase 2 |
| Faster than DuckDB? | Sometimes. | On scan-bound queries at 1e8 rows it is 0.75x-0.91x of DuckDB's time; on high-cardinality GROUP BY and big joins DuckDB is 3-4x faster, and we publish both. | today |
| Faster on pipes? | Yes, there. | On our streaming join it was 3.4x-6.0x faster than DuckDB reading the same pipe, because DuckDB spills piped input to temp. | today (one query) |
| Why not just use DuckDB or clickhouse-local? | Use them. | They are excellent; SenkoParse exists to be small, auditable, dependency-free and bounded by design, and to be read end to end. | today |
| GROUP BY on huge cardinality? | Yes. | 63 million groups at 1e8 rows works today, in memory; under a memory limit it spills partitions to disk after Phase 2. | today in memory; Phase 2 under a limit |
| Can it join two big tables? | Yes. | A 1e8 × 1e8 join works today in memory; a partitioned join handles a build side larger than the limit after Phase 2. | today in memory; Phase 2 under a limit |
| Is it correct? | Checked. | Every query runs in five configurations against SQLite on random data and queries; we publish how many and every known deviation. | today (1,500 per run); 100,000 at 1.0 |
| What happens with SQL it doesn't support? | Error. | It names the unsupported feature and returns no rows; a wrong answer is a release blocker. | today |
| Does it run on my CSV? | Yes. | Drop the CSV in a folder; types are inferred from the first 1 MiB. A `schema.toml` overrides them. | today (first cut) |
| What did you not test? | Listed. | Cold cache, more than one stream data shape, real pipes in automated tests, and bit-identical floats between stream and file mode; the status page lists them. | today |
| Who is it for? | Pipelines. | People on laptops and small servers who query large CSV streams without installing a database. | today |
| Is it production-ready? | Not yet. | Here is the plan, the release criteria, and the phase we are in. | today |

## Rules for the room

- Answer the question asked, in one word, then stop. Continue only if invited.
- "Not supported, and it fails with an error" beats any dodge. A stated limit is hard to attack.
- Show the limitations slide before anyone asks.
- Quote numbers with their scope ("on our streaming join", "at 1e8 rows on an M3 Pro").
- If the tone turns sarcastic, stay level and give the next factual answer.
- Do not use a "Phase N" answer before that gate is met; say "planned, in Phase N".

## Limitations slide (keep current)

- Read-only; one query per run; no server.
- SQL subset: no `DISTINCT`, `CASE`, `IN`, `LIKE`, subqueries, set operations or scalar functions yet ([sql-surface.md](sql-surface.md)).
- Needs a `schema.toml` today.
- Memory bounded for stream input; GROUP BY, joins and sorts are in memory until Phase 2.
- File mode maps whole files, so RSS includes the input.
- Slower than DuckDB on high-cardinality GROUP BY, `COUNT(DISTINCT)` and large joins.
- Stream evidence: one query, one reconstructed data shape.
