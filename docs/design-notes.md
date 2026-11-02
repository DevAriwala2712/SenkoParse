# Design notes: where the ideas come from

2026-11-02 · Companion to [production-readiness.md](production-readiness.md) · Replaces section 2 ("Landscape") of plan v1

SenkoParse is not novel as an idea: several tools already run SQL over CSV files without a server. Its case is a from-scratch, auditable engine with a measured memory bound and an independent correctness oracle. This page records every idea we study or borrow, with its source, so the project can always answer "where did this come from?", and it keeps the licence question explicit.

## Rules

1. **Ideas, not code.** Read source, papers and docs for design; reimplement in our own code.
2. **Licences before any code reuse.** Copying code requires compliance with its licence, even between open-source projects. The licence column below is what we believe; **verify against the project's own `LICENSE` file before reusing anything**, and record the check.
3. **One row per borrowed idea**, added in the same PR that introduces it, with a link to the code that uses it.
4. **Benchmark against the tool you borrow from** when it is comparable, so the comparison is honest.

## Comparable tools

| Tool | What it is | What to learn | Verify before relying on it | Licence (verify) |
|---|---|---|---|---|
| [DuckDB](https://duckdb.org) | embedded analytical engine | CSV sniffer, parallel CSV reader, radix-partitioned hash aggregation, out-of-core sort and join, `memory_limit` | why piped input is retained or spilled (issue #25165; measured in [EXP-005-stream](experiments/005-stream-memory.md)) | MIT |
| [clickhouse-local](https://clickhouse.com/docs/operations/utilities/clickhouse-local) | ClickHouse without a server, over files and stdin | block pipeline, external GROUP BY and sort settings, format breadth | memory behaviour of joins and aggregates over piped input under a limit; not yet in our harness | Apache-2.0 |
| [sieswi](https://pkg.go.dev/github.com/melihbirim/sieswi) | streaming SQL on CSV in Go, reads stdin | closest in spirit; small honest scope | what is actually implemented (early stage) | check |
| [OctoSQL](https://github.com/cube2222/octosql) | SQL over files and databases from the CLI | multi-source joins, output formats | streaming behaviour and memory | MPL-2.0 (check) |
| [xsv](https://github.com/BurntSushi/xsv) / [qsv](https://github.com/dathere/qsv) | fast CSV slicing in Rust | raw CSV parsing speed; the `csv` crate's state machine | xsv is unmaintained; qsv is the active fork | xsv: MIT or Unlicense; qsv: check |
| [Miller](https://github.com/johnkerl/miller) | record-oriented CSV/TSV/JSON transforms | format breadth, CLI ergonomics | streams by record; its SQL-like verbs are not SQL | BSD-2-Clause (check) |
| [q](https://github.com/harelba/q) | SQL on CSV via SQLite | CLI ergonomics | loads into SQLite, so not streaming | GPL-3.0 (check; do not copy code) |
| [SQLFlow (turbolytics)](https://pkg.go.dev/github.com/turbolytics/sql-flow) | stream processor running DuckDB SQL over batches | pipeline config, sinks | wraps DuckDB, so not a from-scratch engine | check |

## Techniques and their sources

| ID | Technique | Source | Used for | State |
|---|---|---|---|---|
| DN-01 | Quote-parity chunk splitting for parallel CSV | well known; see Ge et al., "Speculative Distributed CSV Data Parsing for Big Data Analytics", SIGMOD 2019, for the speculative variant | `plan_chunks`, stream `BlockSplitter` | ✅ in code ([EXP-003](experiments/003-parallel-chunked-scan.md)) |
| DN-02 | SIMD byte search for delimiters | the `memchr` crate; Mühlbauer et al., "Instant Loading for Main Memory Databases", VLDB 2013 | quote-free fast path | ✅ ([EXP-002](experiments/002-quote-free-fast-path.md)) |
| DN-03 | Vectorised (batch-at-a-time) execution | Boncz, Zukowski, Nes, "MonetDB/X100", CIDR 2005 | operators over 65,536-row batches | ✅ |
| DN-04 | Morsel-driven parallelism | Leis et al., "Morsel-Driven Parallelism", SIGMOD 2014 | chunked parallel scan; future adaptive scheduling | partial |
| DN-05 | Kahan compensated summation | Kahan, 1965 | `f64` SUM/AVG | ✅ |
| DN-06 | Fixed merge order for float determinism | own design, ADR-0001 / EXP-003 | thread-independent results | ✅ |
| DN-07 | Grace and hybrid hash join, external sort | Graefe, "Query Evaluation Techniques for Large Databases", ACM Computing Surveys 1993 | Phase 2 spilling ([memory-model.md](memory-model.md)) | planned |
| DN-08 | Radix-partitioned hash join | Boncz, Manegold, Kersten, "Database Architecture Optimized for the New Bottleneck: Memory Access", VLDB 1999; Balkesen et al., "Main-Memory Hash Joins on Multi-Core CPUs", ICDE 2013 | Q9 ([performance-bets.md](performance-bets.md) I-5) | planned |
| DN-09 | Group prefetching / AMAC for hash probes | Chen et al., "Improving Hash Join Performance through Prefetching", ICDE 2004; Kocberber et al., "Asynchronous Memory Access Chaining", VLDB 2015 | Q9 probe | planned |
| DN-10 | Adaptive pre-aggregation (skip partial aggregation when it does not reduce) | used in HyPer, Umbra and DuckDB; see DuckDB's blog post on parallel grouped aggregation (2022) | Q4 (I-3) | planned |
| DN-11 | Short-string inline keys ("German strings") | Neumann and Freitag, "Umbra: A Disk-Based System with In-Memory Performance", CIDR 2020 | string GROUP BY keys (I-2) | planned |
| DN-12 | Welford's online variance and Chan's parallel merge | Welford, 1962; Chan, Golub, LeVeque, 1979 | STDDEV/VARIANCE | planned |
| DN-13 | HyperLogLog | Flajolet et al., 2007 | `APPROX_COUNT_DISTINCT` (P2) | planned |
| DN-14 | Order-independent aggregate states may leave the fixed merge order | own design, ADR-0004 | per-worker DISTINCT sets | proposed |
| DN-15 | Zone maps (per-block min/max) | Netezza; widely used | optional `.qe-stats` sidecar (performance-bets wild round) | idea |
| DN-16 | Run detection for partly sorted input | Peters, Timsort, 2002 | sortedness detection → streaming GROUP BY and merge join | idea |

## Ideas considered and not taken

| Idea | Source | Why not |
|---|---|---|
| Embedding DuckDB or DataFusion for unsupported SQL | — | violates the core rule (no database library in the engine) |
| Arrow as the in-memory format | Apache Arrow | a dependency whose value is interoperability, which is a non-goal; our batches are simpler |
| Columnar cache by default | many engines | hides the cold path; stale-data risk (M6, dropped for now) |
| Cost-based optimizer | System R lineage | non-goal; documented rewrites only |
