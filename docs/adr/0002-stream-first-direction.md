# ADR-0002: Stream-first direction

**Status:** Accepted for the headline; implementation tested and memory measured on one query (EXP-005-stream), 2026-10-31
**Date:** 2026-10-31
**Deciders:** team (hackathon)
**Supersedes:** the "beat DuckDB on general speed" goal behind [ADR-0001](0001-parallel-and-fast-path-execution.md). ADR-0001's decisions stay valid; its remaining steps move to "Later".

## Context

DuckDB issue #25165: `read_csv('/dev/stdin')` buffers the piped input and spills it to temp files, so
temp use grows with input size. Reading the same data by filename uses flat memory.

We reproduced it on DuckDB 2.0.0 with the issue's synthetic data (383 MB), a semi-join on a 50-id list,
`memory_limit=256MB`, 1 thread:

| How DuckDB reads the data | Peak RSS | Temp files |
|---|---|---|
| By filename | 144 MiB | 0 |
| Through a pipe | 324 MiB | 198 MiB |

Meanwhile general speed is a crowded race: DuckDB is mature and fast, and we have no DuckDB timings on
our benchmark machine yet (`BENCHMARKS.md`). A pipe is a case where a small engine can differ in kind,
not only in speed.

SenkoParse now has `qe --stdin TABLE` (`crates/engine/src/storage/csv/stream.rs`, wired through `exec/`
and the CLI). It was first compiled-but-untested; the original commit was lost and the module was re-implemented from this ADR, then tested (`tests/stream.rs`, difftest stream configuration) and measured (EXP-005-stream: peak RSS 42 MiB at 1x to 10x piped input of 375 MB to 3.7 GB, no temp files; DuckDB 2.0.0 piped spilled 237 MiB to 5.6 GB).

## Decision

Make **bounded-memory SQL over non-seekable CSV streams** the headline. Design of `--stdin`:

- A reader thread fills a bounded queue (16 x 1 MiB pieces).
- Each block is cut after the last newline outside quotes (same quote-parity rule as `plan_chunks`).
- Each block is parsed by the existing `CsvReader`; the partial tail row carries over to the next block.
- The stream is never the hash/build side of a join. It is rejected if used twice in a query.
- Output is flushed per batch.
- A row longer than 64 MiB is an error.

Rules and plan are in `CLAUDE.md` ("Streaming rules", "Performance plan"). All hard rules stay.

## Options rejected

1. **Keep chasing general speed.** Rejected: the remaining ideas (parallel build, array join, exact-decimal
   sums, top-N) only close a gap to a mature engine, and nothing in the repo yet shows we can win there.
   They stay as "Later".
2. **Make the stream seekable by spooling it to disk.** This is what DuckDB effectively does. It would let us
   reuse the mmap and parallel-chunk path unchanged, but temp use then grows with the input, which is the
   exact failure we want to avoid, and it needs free disk about the size of the input.

## Consequences

- The mmap, parallel-chunk path (M2) is not used for stdin. The stream is parsed single-threaded.
- Limits (known): no `IN (SELECT ...)`; use `JOIN`, which equals a semi-join only if the lookup keys are
  unique; the stream cannot be the right side of a `LEFT JOIN`; one scan per query.
- "Bounded memory" covers the input buffering. Group tables and join build sides still grow with group
  count and build-side size; for high-cardinality queries total memory is not flat. Claims must say this.
- Stream mode must give the same results and error text as file mode, apart from the `<stdin>` label.
  `f64` sums need a check: file mode merges per-chunk partials in a fixed tree, the stream adds in order,
  so bit-identical sums are not guaranteed until tested.
- Old speed work (EXP-005 join, exact-decimal sums, `COUNT(DISTINCT)` rewrite, top-N, filter pushdown) is
  paused. EXP-005 in ADR-0001 is the join experiment; the new one is named EXP-005-stream to avoid a clash.

## Open items

- [x] Run the tests and diff stream mode against file mode (done: `tests/stream.rs`, difftest).
- [x] EXP-005-stream: peak RSS and temp use, SenkoParse vs DuckDB, at 1x / 2x / 5x / 10x piped input (done, one query; data is a reconstruction of the issue's shape).
- [x] Tricky-input tests: quoted newlines, CRLF, byte-dribbled reads, tiny `--chunk-bytes` (done).
- [ ] `f64` sums in stream mode vs file mode are only compared with tolerance, not bit-for-bit.
- [ ] `SELECT a.*` / `SELECT *` are not in the supported SQL; the EXP-005-stream query lists the columns.
- [ ] Re-run EXP-005-stream with the issue's own generator once its text is available.
- [ ] Decide `IN (SELECT ...)` (optional, true semi-join).
- [ ] `docs/strategy.md` is referenced by `CLAUDE.md` but is not in the repo; export it or drop the reference.
- [ ] Decide whether to rename the join experiment to avoid two "EXP-005" IDs.
- [ ] Open questions in `CLAUDE.md` (how judges time queries, data format, hardware) may no longer apply; revisit.
