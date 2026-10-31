# ADR-0001: Parallel execution and fast paths after M1

**Status:** Accepted ("go for it", 2026-10-31); implementation in progress, see the progress table at the end
**Date:** 2026-10-31
**Deciders:** team (hackathon), final say on the open items at the bottom

## Context

M1 is correct (difftest green) but slow: 0.74-0.98 s for scan+aggregate and join-small queries on
527 MB (0.55-0.7 GB/s), 2.5 s for a 6.3e6-group aggregate, 3.6 s for a 1e7 x 1e7 join (M3 Pro, 11
cores, warm cache, `BENCHMARKS.md`). `CLAUDE.md` plans M2 = chunked parallel scan with thread-local
partial aggregates merged at the end. Before building it I measured where M1's time goes
(`qe`, 1e7 rows, 7 columns, warm):

| query (parses what) | wall s |
|---|---|
| `COUNT(*)` (no column parsed; only finds field/row boundaries) | 0.55 |
| `SUM(id1)` (1 short int) | 0.65 |
| `SUM(id3)` (1 int, 8 digits) | 0.57 |
| `SUM(v1)` (1 float) | 0.73 |
| `MIN(id4),MIN(id5),MIN(id6)` (3 strings) | 0.67 |
| 4 numeric columns | 0.84 |
| `GROUP BY id3, SUM(v1)` (6.3e6 groups) | 4.9 (2.5 in the earlier bench run; noisy, re-measure) |

**Finding 1: ~70% of simple-query time is boundary discovery, not parsing or hashing.** Doing
nothing with the data but walking all 70M fields costs 0.55 of 0.8 s. Parsing a column adds 0.02-0.2 s.
**Finding 2: the high-cardinality aggregate and the big join are 5-6x slower than a scan** and will
dominate once scans are parallel. Cause (hypothesis, not yet profiled): a heap-allocated key per group
(`Box<[u8]>`, 6.3e6 allocations) and random access into a table far larger than cache.
**Finding 3 (unverified):** a `cat` of the file took ~10 s once, almost certainly because it was no
longer in the page cache after the SQLite spot check. If true that is ~50 MB/s cold, which would make the
judges' cold path I/O-bound. Re-measure with `sudo purge` before drawing conclusions.

## Defaults being broken

1. "Every field must be located" (even fields nobody reads).
2. "Parallelism = N threads each running the same pipeline" (merge cost of per-thread tables is ignored).
3. "A join means a hash table" (dense integer keys need none).
4. "Parse a row, then filter it".
5. "Keys are hashed byte strings".

## Options considered

### Option A: M2 as written in CLAUDE.md (chunked scan, thread-local tables, merge)
Simple, matches the plan. Scan-bound queries get ~5-8x. **Cons:** boundary discovery stays the
per-core bottleneck; for 6.3e6 groups each of 11 threads builds a near-full-size table and the merge is
serial and large, so Q4 may get *slower per core* than M1. Q9 build is still serial or contended.

### Option B (recommended): fast path first, then parallel, with adaptive aggregation and join
Same chunked parallel scan, but (1) a quote-free fast path makes each core's scan ~5x cheaper,
(2) aggregation picks its strategy from observed cardinality, (3) dense-integer joins use an array.
Costs more code than A; every piece has a fallback to the M1 path, so correctness risk is contained by
the difftest.

### Option C: build a columnar cache / sidecar index first (M6 early)
Biggest warm-path win, but `CLAUDE.md` says the cold path must be the fast path and the judges' timing
method is unknown. Rejected as the *first* move; kept as an add-on.

| Dimension | A | B | C |
|---|---|---|---|
| Complexity | Low | Medium | High |
| Cold-path speedup | ~5-8x scans only | scans ~25x+ (hypothesis), agg/join fixed | none |
| Risk to correctness | Low | Medium (mitigated by fallbacks + difftest) | Medium |

## Decision

Adopt **Option B**, in the order below. Each step has a falsifying test and a gate: difftest green on
both the fast and the fallback path, and a before/after row in `BENCHMARKS.md`. All speedups below are
hypotheses until measured.

### Ideas, mechanism, test

1. **Skip fields nobody reads (lens: don't do the work, specialize for the real distribution).**
   If a chunk contains no `"` byte (one SIMD `memchr` pass, ~free), rows are exactly newline-delimited
   and fields comma-delimited. Then stop splitting after the last *needed* column and jump to the
   next `\n` with `memchr`. `COUNT(*)` becomes a pure newline count. Chunks that do contain quotes use
   today's quote-aware path, so exactness is unchanged. *Effect:* removes most of the 0.55 s.
   *Test:* `SUM(id1)` (column 0 of 7) should fall well under 0.3 s single-threaded. If not, the cost is
   elsewhere and this idea is dropped. *Risk:* two scan paths to keep equivalent (difftest data already
   mixes quoted/unquoted files; add a quote-free dataset).
2. **Exact row boundaries for parallel chunks (lens: reformulate).** Count `"` per fixed-size chunk in
   parallel; the prefix parity gives the quote state at every split, so splits are exact even with
   embedded newlines and `""` escapes. No "fail loudly" fallback needed. *Test:* difftest with embedded
   newlines, run with tiny chunk sizes (e.g. 64 bytes) to force splits inside quoted fields.
3. **Fixed-size chunks merged in file order (lens: add a constraint to buy freedom).** Chunk size does
   not depend on core count, so `f64` sums are bit-identical on any machine. *Cost:* a few percent.
   *Test:* same query at 1, 2, 11 threads gives byte-identical output.
4. **Adaptive aggregation (breaks default 2; conflicts with CLAUDE.md M2, see below).** Watch the group
   count during the first chunks. Low cardinality (say <= 64K groups): thread-local tables + merge.
   High cardinality: radix-partition rows by key hash into P partitions, then aggregate each partition
   on one thread. Partitions are disjoint, so there is no merge, and each partition's table is cache
   sized (an extra pass over the data that buys cache hits). *Test:* Q4 target < 1 s; if the 11-thread
   run is not faster than the 1-thread run, the partitioning is wrong.
5. **Integer-key open-addressing table, no per-row allocation (CLAUDE.md M3).** Replace `Box<[u8]>` keys
   with an inline `i64` key (and a 16-byte inline form for two int keys). *Test:* Q4 single-threaded
   before any parallelism; expect most of the 2.5-4.9 s to go.
6. **Join as array lookup for dense integer keys (lens: delete the required component).** Sample
   min/max of the build key; if the range is within ~4x the row count, build `Vec<u32>` indexed by key
   while verifying uniqueness and range during the build (fallback to the hash table if violated).
   Probe is one array load per row. Small dimension tables (Q7/Q8) become array lookups, and group-by
   on a dimension column becomes group-by on a small int. *Test:* Q9 target < 1 s.
7. **Late materialization (lens: don't do the work).** Parse only predicate columns first; parse the
   remaining columns only for rows that survive. *Effect only for selective filters; do after 1-6.*

### Wild-round ideas and what survived
- *Never parse floats exactly, approximate instead:* **rejected**, floats must match SQLite to 1e-9 and
  sums must be reproducible. Kernel that survives: a fast exact path (<= 15 significant digits, small
  exponent: integer divided by an exact power of ten is correctly rounded). Worth ~0.1 s, so low priority.
- *Skip reading the file for `COUNT(*)`/`MIN`/`MAX`:* impossible without metadata. Kernel that
  survives: the optional sidecar cache (M6), not on the cold path.
- *Replace `mmap` with parallel `pread`:* plausible on macOS where page faults are costly. Cheap to
  test (one hour); do it only if Step 1 leaves the scan far from memory bandwidth.

### Discarded
- **Bloom filters / zone maps on CSV:** need a pre-pass over the data or a sidecar; no gain on a
  cold single query.
- **SIMD quote/comma bitmasks (simdjson style) via `core::arch`:** likely faster than Step 1 but is
  `unsafe`, platform specific, and `CLAUDE.md` ranks it after M4. Revisit only if Step 1 under-delivers.
- **GPU:** out of scope per `CLAUDE.md`.

## Consequences
- Scans: expected to be limited by memory bandwidth rather than parsing (to be measured).
- Aggregation and join gain a runtime strategy choice, which adds test surface: difftest must exercise
  every strategy (force each via an env var or option; random data alone will not hit them all).
- `--no-opt` keeps the M1 single-threaded generic path as the oracle for the fast paths.
- Float results become deterministic across core counts (Idea 3).

## Conflict with CLAUDE.md (flagging, as it asks)
M2 says "thread-local partial aggregates and hash tables, merged at the end." For high-cardinality
group-bys that design merges ~cores x groups entries serially. I propose it only for low cardinality
and radix partitioning otherwise (Idea 4). `CLAUDE.md` wins unless you agree to change it.

## Action items / plan
Each step: write the failing test first, implement, difftest (both paths), bench row, 3-5 line summary.

1. [ ] Re-measure baselines with `sudo purge` (cold) and warm; run a sampling profiler (`xcrun xctrace`
       or install `samply`, which needs your OK) to confirm Findings 1 and 2.
2. [ ] Idea 1: quote-free fast path + `COUNT(*)` newline count. Gate: `SUM(id1)` < 0.3 s.
3. [ ] Ideas 2 and 3: parallel chunked scan, ordered merge, thread-count-independence test.
4. [ ] Idea 5: integer-key table, then Idea 4 adaptive/partitioned aggregation. Gate: Q4 < 1 s.
5. [ ] Idea 6: dense-key array join and small-dim lookup. Gate: Q9 < 1 s.
6. [ ] Install DuckDB (needs your OK) and record its numbers next to ours in `BENCHMARKS.md`.
7. [ ] Idea 7 and M4 eager aggregation, only if measurements still show the gap.

## Open items for the team
- Is bit-identical `f64` across core counts wanted? (Assumed yes.)
- OK to change the M2 wording in `CLAUDE.md` per the conflict above?
- OK to install `samply` and DuckDB (downloads)?
- What do the judges time, cold or repeated? It decides whether Finding 3 matters most.

## Progress (updated as experiments complete)

| Step | Experiment | State | Result |
|---|---|---|---|
| Re-measure + profile | [EXP-001](../experiments/001-baseline-breakdown.md) | Done (warm only; cold needs `sudo purge`) | Finding 1 and 2 confirmed; new: `from_utf8` waste in float parse |
| Idea 1 quote-free fast path | [EXP-002](../experiments/002-quote-free-fast-path.md) | Done | 2.1-2.5x only when early columns are read; gate `SUM(id1)` < 0.3 s met barely (0.295) |
| Ideas 2+3 parallel chunks, ordered merge | [EXP-003](../experiments/003-parallel-chunked-scan.md) | Done | 5.1-5.9x on 11 threads; byte-identical across thread counts |
| Idea 5 + 4 integer-key table, partitioned aggregation | [EXP-004](../experiments/004-integer-key-aggregation.md) | Done (merged) | `GROUP BY id3` 2.5 s (M1) -> 0.67 s at 11 threads, target < 1 s met; the serial merge tree (0.78 s) was removed by partition-wise merge |
| Idea 6 dense-key array join / parallel build | EXP-005 | Next | join with `big`: 1.46 s now, target < 1 s |
| DuckDB reference numbers | n/a | Blocked: needs permission to install | |
| Idea 7 late materialization, M4 | n/a | Not started | |

Decisions taken without the team (revisit if wrong): float results are thread-count independent (fixed 16 MiB chunks, fixed merge tree);
a `"` inside an unquoted field is a hard error (needed for exact parallel splitting).
