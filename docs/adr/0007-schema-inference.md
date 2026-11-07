# ADR-0007: Schema inference (decision D-5, first cut)

- Status: Accepted (first cut), 2026-11-07
- Task: production-readiness.md Phase 1, task 1.1
- Supersedes: the "no type sniffing" rule in CLAUDE.md for tables that have no `schema.toml` entry

## Context

`schema.toml` was required for every table, so any new CSV failed with `cannot read schema.toml` or
`unknown table` (gap rank 1 in production-readiness.md §2.2; regression candidate C07a). D-5 recommends sampling
from the start and from chunk starts, widening on conflict, never narrowing, and making a later conflict an error
that names the schema file.

## Decision

1. **`schema.toml` always wins.** A table listed there is used exactly as before. `schema.toml` becomes optional.
2. **Which files.** Every `<name>.csv` in the data directory whose `<name>` is not in `schema.toml` and is a plain
   identifier (`[a-z_][a-z0-9_]*` after lowercasing) becomes table `<name>`. Other file names are skipped.
3. **Sample.** The header plus whole rows from the first 1 MiB of the file (at least one row even if longer).
   Mid-file samples (D-5's "chunk starts") are not taken in this cut: finding a row start mid-file needs the
   quote-parity pass over the whole file, and the catalog is loaded on every query. Follow-up.
4. **Types.** Per non-empty field, using the same parsers the scanner uses (so a sampled value can never fail to
   parse later): `i64` if `parse_i64` accepts it, else `date` if `parse_date` accepts it, else `f64` if it parses
   as a finite `f64`, else `str`. Columns widen along `i64 → f64 → str` and `date → str`; `i64` meeting `date`
   gives `str`. A column with no non-empty sampled value is `str`. `decimal(N)` is **not** inferred in this cut
   (I-9 / D-7: it changes output format; to be decided with measurements).
5. **Later conflict.** A value after the sample that does not parse fails the query with the usual
   `invalid <type> value` error plus a hint: the type was inferred, add the table to `schema.toml`. Never a silent
   NULL and never a silent widening.
6. **Determinism.** The sample depends only on the file bytes, so types are the same on every run, thread count and
   chunk size.
7. **Stream tables.** `--stdin t` still needs `t` in `schema.toml` or a `t.csv` to infer from; a non-seekable
   stream is not sampled (it can be read only once).

## Consequences

- A plain CSV with a header now runs with no setup. Leading zeros are lost on inferred `i64` columns (`007` → 7);
  write `str` in `schema.toml` to keep them.
- Every query reads up to 1 MiB per CSV in the directory that is not in `schema.toml` (header sampling).
- The difftest still uses explicit schemas; inference has its own tests (`tests/infer.rs`).
