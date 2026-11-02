# CLI contract

2026-11-02 · Companion to [production-readiness.md](production-readiness.md) §10-11 · Code: `crates/cli/src/main.rs`

The command-line interface is part of the product: scripts and pipelines depend on its flags, exit codes and output. This page lists what exists **today** (audited at `f80c6e0`), what is proposed, and the compatibility promise that starts at 1.0.

## Invocation forms

| Form | Today | Behaviour |
|---|---|---|
| `qe [OPTIONS] "SELECT ..."` | ✅ | one query, CSV on stdout |
| `qe [OPTIONS]` with a terminal on stdin | ✅ | interactive shell (`qe>`), commands `.tables .schema .queries .q N .read FILE .mode .limit .timer .help` |
| `qe --file queries.sql [--table]` | ✅ | runs every statement in the file |
| `qe --serve [--port N]` | ✅ | local web workbench on `127.0.0.1` |
| `cmd \| qe --stdin NAME "SELECT ..."` | ✅ | stream mode |
| `qe --version` | ❌ | proposed (R-OPS-3) |

## Flags

| Flag | Today | Meaning | Notes for 1.0 |
|---|---|---|---|
| `--data DIR` | ✅ | directory of `<table>.csv` files and `schema.toml` (default `data`) | `schema.toml` becomes optional (inference) |
| `--stdin NAME` | ✅ | bind stdin to table NAME | — |
| `--threads N` | ✅ | worker threads (default all cores); results never depend on N | — |
| `--chunk-bytes N` | ✅ | scan chunk size (default 16 MiB); results never depend on N | keep, mark as tuning |
| `--no-opt` | ✅ | serial reference path | keep; documented as a debugging aid |
| `--progress` | ✅ | live status line on stderr when it is a TTY | — |
| `--table` | ✅ | pretty table output instead of CSV | **rename to `--format table`** and keep `--table` as a deprecated alias until 2.0, because `--table name=path` is wanted for sources (SRC-04) |
| `--file`, `-f` | ✅ | run statements from a file | — |
| `--shell`, `-i` | ✅ | force the interactive shell | — |
| `--serve`, `--port N` | ✅ | web workbench | add a session token (T-5) |
| `--cache` | reserved | exits with "not implemented yet (M6)" | stays reserved |
| `--format csv\|tsv\|json\|table` | ❌ | output format | P1 (csv and table at P0) |
| `--header`, `--no-header` | ❌ | output header on or off; input header handling via `--input-header` | P1 |
| `--src NAME=PATH` (name to decide) | ❌ | add a table from any path | P0 |
| `--schema FILE` | ❌ | explicit column types; overrides inference | P0 |
| `--delimiter C`, `--quote C`, `--escape C`, `--null STR`, `--comment C` | ❌ | input dialect | P1 |
| `--memory-limit SIZE` | ❌ | hard budget, e.g. `256MB`, `2GiB`, `0` = unlimited | P0 (Phase 2) |
| `--temp-dir PATH` | ❌ | spill directory (default: system temp) | P0 (Phase 2) |
| `--timeout DURATION` | ❌ | cancel after a duration, exit 3 | P1 |
| `--max-row-bytes N`, `--max-columns N` | ❌ (64 MiB row limit is fixed in stream mode) | resource limits | P1 |
| `--lenient`, `--ragged pad\|skip`, `--invalid-utf8 error\|replace\|bytes` | ❌ | input error policies | P1 |
| `--explain` | ❌ | print the plan and exit (same as `EXPLAIN`) | P1 |
| `--stats` | ❌ | one JSON object of run statistics on stderr | P1 |
| `--error-format text\|json` | ❌ | machine-readable errors | P1 |
| `--version`, `-V` | ❌ | version, commit, target, profile | P0 |
| `-h`, `--help` | ✅ | usage | generate from the same table as this page |

Size values accept `B`, `KB`/`MB`/`GB` (powers of 1000) and `KiB`/`MiB`/`GiB` (powers of 1024), case-insensitive, so `256MB` and `256MiB` are distinct and documented.

## Streams: stdout and stderr

- **stdout** carries query results only. Nothing else, ever, in non-interactive mode.
- **stderr** carries errors, warnings, `--progress`, `--stats` and `--explain` output when combined with a query.
- In stream mode, output is flushed after each batch so results appear while input is still arriving (today).
- Broken pipe on stdout (`qe ... | head`): exit 0 silently (target; today the write error is reported as `error: ...` with exit 1, to verify).

## Exit codes

| Code | Meaning | Today |
|---|---|---|
| 0 | success | ✅ |
| 1 | query or data error (syntax, unsupported, bind, bad value, I/O) | ✅ |
| 2 | usage error (bad flag, missing argument) | ✅ |
| 3 | resource limit (memory, disk, timeout, row or column limit) | proposed |
| 4 | input changed during the query | proposed |
| 130 | interrupted by SIGINT | proposed (shell default today) |

Codes 0-2 keep their meaning. 3 and 4 are reserved now so that no other meaning takes them.

## `--stats` schema (proposed)

```json
{
  "version": "0.x.y", "commit": "f80c6e0",
  "rows_in": 100000000, "rows_out": 10, "bytes_read": 5550000000,
  "wall_ms": 1300, "cpu_ms": 13900, "threads": 11,
  "peak_accounted_bytes": 41000000, "peak_rss_bytes": 52000000,
  "spill_bytes": 0, "spill_partitions": 0,
  "input_mode": "file", "options_hash": "9c1e..."
}
```

Fields are only ever added after 1.0, never renamed or removed.

## Compatibility promise (from 1.0)

Breaking changes, allowed only in a major version and listed under "Results changed" or "Breaking" in `CHANGELOG.md`:

- removing or renaming a flag, or changing its default in a way that changes results;
- changing an exit code's meaning;
- changing output formatting (float text, NULL text, booleans, quoting, header names);
- changing query results for any query that was supported, including NULL ordering and type inference that changes a column's type;
- removing a `--stats` field or an error class.

Not breaking: faster execution, lower memory, new flags, new error classes, better error wording (the class stays), new supported SQL.
