# ADR-0005: SQLite defines the correct answer for the shared subset; DuckDB for the rest

**Status:** Proposed (decision D-1 in [production-readiness.md §14](../production-readiness.md#14-open-decisions); needs the owner's acceptance).
**Date:** 2026-11-02
**Deciders:** team

## Context
A query engine without a written reference dialect cannot be called correct, only consistent with itself. Today the de facto reference is SQLite: `tests/difftest.rs` compares every generated query with SQLite on five engine configurations, and the Phase 0 audit (`tests/surface_audit.rs`) compares every supported 🟡 feature the same way. The large difftest passed on seeds 1000-1002 with 4,500 queries on 2026-11-02 ([testing.md](../testing.md#test-002-differential-test-vs-sqlite)).

Phase 1 adds features SQLite lacks or spells differently (`ILIKE`, `FILTER`, quantiles, `DATE_TRUNC`, `STRING_AGG ... ORDER BY`). DuckDB supports them, but DuckDB is also the engine we benchmark against, so making it the sole oracle would tie "correct" to our benchmark rival.

## Decision
1. **For every feature SQLite supports, qe returns what SQLite returns**, under the comparison rules of [correctness.md](../correctness.md): NULL equals NULL, unordered results compare as multisets, floats with relative tolerance 1e-9, integers exactly.
2. **For features SQLite lacks, DuckDB is the oracle**, with the same comparison rules. Each such feature names its oracle in the Oracle column of [sql-surface.md](../sql-surface.md).
3. **Where SQLite and DuckDB disagree on a feature both support, SQLite wins**, and the difference is listed in [semantics.md, Dialect differences](../semantics.md#dialect-differences) before the feature ships.
4. **Deliberate deviations from SQLite are allowed only when SQLite's answer comes from dynamic typing** (for example comparing text with an integer by type class). qe is statically typed and rejects such queries at bind time. Every deviation is a row in the Dialect differences table. The one planned non-typing exception is case-sensitive `LIKE` (because `ILIKE` covers the other case); the difftest sets `PRAGMA case_sensitive_like = ON` for it.

## Options rejected
- **DuckDB as the only oracle:** most features in one engine, but it is the benchmark rival, its behaviour changes between major versions, and qe already matches SQLite row for row.
- **PostgreSQL:** the most standard dialect, but it needs a server in CI, and its integer `/` and NULL ordering differ from what qe and its tests already do. It is still listed in the Dialect differences table for users coming from it.

## Consequences
- Positive: no change to existing behaviour or tests. The rule says in advance which engine is right when the two oracles disagree.
- Negative: two oracles to maintain in CI once DuckDB-only features exist (a DuckDB lane, Phase 1). Users coming from DuckDB will see SQLite's NULL order (NULLs first for ASC) until `NULLS FIRST/LAST` (ORD-03) exists.
- Follow-up: every `verify` cell in the Dialect differences table must be resolved by a test before the feature it concerns ships.
