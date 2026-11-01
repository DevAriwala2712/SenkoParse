# ADR-0004: Order-independent aggregates may leave the fixed per-chunk merge order

**Status:** Proposed. **No code relies on it today.** The first use, EXP-009 (per-worker COUNT(DISTINCT) sets), was correct but missed its speed kill rule and was reverted (`docs/experiments/009-distinct-per-thread.md`). The rule stays available for a future experiment that clears its own kill rule.
**Date:** 2026-11-01
**Deciders:** team (hackathon)

## Context
EXP-003 / ADR-0001 made parallel results identical for any thread count. Chunk boundaries depend only on the file and `--chunk-bytes`, there is one partial aggregate table per chunk, and the tables are merged in a fixed order by chunk index (pairwise tree or partitioned merge). That rule exists for one reason: **`f64` addition is not associative.** A Kahan sum of the same values in a different order can differ in the last bits, and the difftest and `tests/parallel.rs` require bit-identical output across thread counts.

The rule has a cost that grows as `chunks x groups x state size`. For Q5 (`COUNT(DISTINCT id2)` per `id4`, 30 groups), the per-chunk sets hold about 25 M boxed keys at once at 3e7. That is about 95% of a 1.5 GiB heap and at least 55-60% of CPU samples, and the tree of set unions takes longer than the scan (`009-q5-profile.md`). Yet a set union does not need a fixed order at all.

## Decision
An aggregate state may be kept per worker (one per concurrently running chunk task, in any assignment of chunks to workers) and merged in any order **only if its final value is provably independent of how the rows are split and in which order partial states are merged**, bit for bit, for every input the engine accepts. Every other state keeps the fixed per-chunk order.

| aggregate | order-independent? | why |
|---|---|---|
| `COUNT(*)`, `COUNT(col)` | yes | integer addition, exact |
| `SUM`/`AVG` of `i64` and `decimal` (i128 accumulator, scaled i64) | yes | exact integer addition. The overflow check runs on the final i128 when narrowing, which is the same for any order. AVG divides only at the end |
| `COUNT(DISTINCT e)`, any type | yes | set union of canonical key bytes is exact. The count is the size of the union |
| `MIN`/`MAX` of `i64`, `decimal`, `date`, `str` | yes | total order on the value, ties are equal values |
| `MIN`/`MAX` of `f64` | **no** | `-0.0` and `0.0` compare equal, and NaN compares false with everything, so the first value seen wins and the output can depend on order |
| `SUM`/`AVG` of `f64` (Kahan) | **no** | non-associative addition |

Conditions on any use:
1. Group order and every non-order-independent state still come from the fixed per-chunk merge. The output, including the row order of unordered results, stays bit-identical for any thread count.
2. The serial path (`--no-opt`, stream input) is unchanged and remains the reference.
3. Tests: a 1/2/4-thread bit-identity test with an order-independent aggregate and an `f64` sum in the same query, on both merge paths, plus equality with `--no-opt`. A mutation check that breaks the per-worker merge must fail a test. `tests/parallel.rs::distinct_per_worker_gives_identical_results_for_1_2_and_4_threads` exists for DISTINCT.
4. Like any optimization, a use must clear a kill rule written before the code (CLAUDE.md "Docs and experiments").

## Options rejected
- **Keep the fixed order for everything** (status quo): simplest, but `chunks x groups` memory for large states (sets) has no other fix short of bigger chunks, which hurt parallelism (EXP-006: 256 MiB chunks made Q3 at 1e7 slower).
- **Per-worker tables for all aggregates, including `f64`:** breaks thread-count independence of float output. Rejected outright.
- **Larger chunks only for DISTINCT queries:** fewer sets, but chunk count would then depend on the query shape and still costs parallelism. Not tried.

## Consequences
- Positive: memory for large order-independent states scales with workers, not chunks. EXP-009 measured Q5 peak RSS -33% at 5e7 and -35% at 3e7 (M3 Pro, relative), with identical output.
- Negative: two places hold state for one group (chunk table and worker table) and must be rejoined by key at the end. The assignment of chunks to workers is nondeterministic, so the correctness argument depends entirely on the order-independence proof above. That is why `f64` MIN/MAX is excluded even though it looks safe.
- EXP-009's speed gain (1.47x min / 1.38x median on Q5 at 5e7) did not meet its 1.5x bar. The likely next step is the set representation (an inline `i64` set or a bitset for integer DISTINCT arguments), alone or combined with this rule.
