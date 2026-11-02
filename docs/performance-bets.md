# Performance bets: counterintuitive moves for the losing queries

2026-11-02 · Companion to [production-readiness.md](production-readiness.md) §8 · Feeds [roadmap.md](roadmap.md) and [experiments/](experiments/README.md)

This page applies one method to the queries where SenkoParse loses: find the resource that actually binds, list the defaults the engine follows without thinking, break the defaults on purpose, and keep only the ideas whose mechanism survives a plain check. Nothing here is a claim. Each bet becomes an experiment file with a kill rule written **before** the code, and nothing reaches the README without a row in [BENCHMARKS.md](../BENCHMARKS.md).

**Contents:** [Bottleneck read](#1-bottleneck-read) · [Defaults](#2-defaults-worth-breaking) · [Ideas](#3-ideas) · [Wild round](#4-wild-round) · [Discarded](#5-discarded) · [Bets and order](#6-bets-and-order) · [How this changes the plan](#7-how-this-changes-the-production-readiness-plan)

---

## 1. Bottleneck read

The metric is min wall time against DuckDB 2.0.0 at 1e8 rows on the M3 Pro (11 cores, 18 GiB), with the same output, plus the production constraint that memory stays under a budget. The most useful single number is how the gap changes between 1 thread and all cores ([BENCHMARKS](../BENCHMARKS.md), "Interim, after EXP-008"):

| Query | qe/DuckDB, 1 thread | qe/DuckDB, all cores | Reading |
|---|---|---|---|
| Q4 GROUP BY id3, 63M groups | 1.24x | **4.04x** | per-row cost is close; the gap is **scaling**: per-chunk partial tables reduce nothing at this cardinality, so all-core runs pay for merging and DRAM bandwidth |
| Q5 `COUNT(DISTINCT id2)` per id4 | 1.94x | **3.13x** | both per-row cost (boxed keys, `memcmp`) and scaling (per-chunk sets, EXP-009) |
| Q9 join big, 1e8 × 1e8 | 1.64x | **2.95x** | serial build plus a probe bound by DRAM latency into a ~1e8-entry table |
| Q8 join medium + group | 1.36x | 1.37x | ratio does not change with threads, so the cost is **per row**; the group key `id5` is a string |

Two more facts from the repo change what is worth trying:

- At 1e7, Q4 took 1.00 s when a `HAVING` clause suppressed the output and 1.64 s with the full output ([exp004-split-agg-vs-output](experiments/data/exp004-split-agg-vs-output.md)). **About 40% of Q4 was writing 10M rows of CSV.** At 1e8 the output is 63M rows and is still written serially.
- The benchmark generator (`scripts/gen_data.py`) produces integer keys in dense ranges (`id1` in 1..N/1e6, `id2` in 1..N/1e3, `id3` in 1..N; `big.id3` is a permutation of 1..N) and `v1` rounded to 6 decimals. Real data is often less kind (UUIDs, sparse ids, free text). Ideas that only pay off on dense integers are labelled **[dense]**; the plan must not depend on them, and `CLAUDE.md` already requires fast paths to be chosen from measured data, never hard-coded to the generator.

**Assumption.** The Q4 and Q9 gaps are dominated by random DRAM access, not by instruction count. The 1-thread vs all-core table supports this; a `samply` or `perf` profile with cache-miss counters is the first step of each experiment and can overturn it.

## 2. Defaults worth breaking

| # | Default in the current engine | Where |
|---|---|---|
| D1 | Always pre-aggregate per chunk, then merge | `exec/hash_agg.rs`, ADR-0001 |
| D2 | One hash table per operator, as large as the data, accessed at random | hash agg, hash join |
| D3 | Parse every referenced column of a row before deciding whether the row matters | `storage/csv` |
| D4 | The output is a serial tail after the parallel work | `output.rs` |
| D5 | Keys are compared as byte slices (strings) or boxed values (distinct sets) | `exec/hash.rs`, distinct sets |
| D6 | Memory-map the whole input | file mode scan |
| D7 | Each run starts from nothing; nothing learned from a scan survives it | by design |
| D8 | A float column is parsed as `f64` because the text has a decimal point | `schema.toml`, inference plan |

## 3. Ideas

Each idea: the move, the default it breaks, the mechanism, the expected effect, the cost or failure mode, and the cheapest test that could kill it.

### I-1. Bitset `COUNT(DISTINCT)` for small-range integer arguments **[dense]** · Q5

- **Breaks:** D5, and the per-chunk part of D1.
- **Mechanism:** Q5 has about 100 groups (`id4` is derived from `id1`) and `id2` ranges over 1..1e5. A bitset of 1e5 bits is 12.5 KB, so all groups together need 1.25 MB, which fits in L2. Setting a bit replaces allocating a boxed key, hashing it, and comparing bytes. Merging is a bitwise OR, which is exact and order-independent, so ADR-0004 allows one bitset per worker instead of per chunk (11 × 1.25 MB instead of about 330 × 1.25 MB).
- **Expected:** Q5 moves from set-bound towards scan-bound: a 2x to 4x gain is plausible (Q2, a scan-bound GROUP BY, takes 1.21 s against Q5's 8.64 s).
- **Cost and failure:** only applies when `max - min` of the argument is small relative to rows × groups; needs the range from a sample or from the first chunks, with a fallback when a later value falls outside (switch that group to a hash set; correctness unaffected).
- **Cheapest test:** a branch hard-wiring the bitset for Q5 at 3e7; kill if under 1.5x (the bar EXP-009 missed).

### I-2. Inline short keys (fixed 16-byte normalised keys) · Q5, Q8, every string GROUP BY

- **Breaks:** D5.
- **Mechanism:** strings of at most 15 bytes are stored inside the table slot as two `u64` words plus a length byte; longer strings keep a 4-byte prefix, length and a pointer. Equality becomes two integer compares for short keys and a prefix check that rejects most mismatches before touching the heap. This is the "German string" layout used by Umbra and DuckDB. `id4` (`id001`), `id5` (`id000123`) and most real categorical columns (country, status, category) are short.
- **Expected:** removes a pointer chase and a `memcmp` per probe on string keys. Q8 is per-row bound and groups on `id5`, so 1.2x to 1.5x on Q8 is the honest range.
- **Cost:** a second key layout to maintain in the table; output must reconstruct the bytes.
- **Cheapest test:** micro-benchmark the group table alone on `id5` values, then Q8 at 3e7; kill if under 1.15x on Q8.

### I-3. Adaptive radix-partitioned aggregation: skip pre-aggregation when it does not reduce · Q4

- **Breaks:** D1, D2.
- **Mechanism:** watch the reduction ratio (groups ÷ rows) on the first chunks. When it is high (Q4: 63M groups from 1e8 rows), stop building per-chunk tables. Instead each worker scatters `(key, value)` into P partition buffers by the high bits of the key hash, with P chosen so that one partition's table fits in L2 (63M groups × ~24 B ≈ 1.5 GB; P = 1024 gives about 1.5 MB per partition). Then each partition is aggregated by one thread in a cache-resident table, with no merge phase. Scatter writes are sequential per buffer, so DRAM is used for bandwidth, not latency. This is the adaptive pre-aggregation and radix partitioning that HyPer, Umbra and DuckDB use.
- **Expected:** Q4 at all cores 2x to 3x faster; the 1-thread number barely moves, which is itself a check on the mechanism.
- **Cost:** one extra pass over the data in memory (the scatter); a second code path for aggregation. Determinism: inside each partition, rows are folded in chunk-index order (the scatter keeps per-chunk runs), so `f64` sums stay bit-identical across thread counts.
- **Cheapest test:** force the partitioned path on Q4 at 3e7 and 1e8; kill if under 1.3x (the roadmap's existing bar).

### I-4. Range partitioning instead of hash partitioning for dense integer keys **[dense]** · Q4, Q9

- **Breaks:** D2 (hashing at all).
- **Mechanism:** when a key column's measured `[min, max]` is within a small multiple of the row count, partition by the key's high bits instead of its hash. Each partition then covers a contiguous key subrange, so its "table" is a plain array indexed by `key - base`: no hashing, no key comparison, no collision handling, and the output of an unordered GROUP BY comes out sorted for free.
- **Expected:** on top of I-3, removes hashing and key compares, perhaps another 1.3x to 2x on Q4; for Q9 (`big.id3` is a permutation of 1..N) a direct-address build plus range-partitioned probe makes every probe hit a cache-sized slice.
- **Cost:** only for dense integer keys; the density test must be measured, with fallback to I-3.
- **Cheapest test:** Q4 at 3e7 with I-3 on, range vs hash partitioning; kill if under 1.2x over I-3 alone.

### I-5. Radix-partitioned join with parallel build and software prefetch · Q9

- **Breaks:** D2.
- **Mechanism:** partition both sides by key bits so that each build partition fits in cache, then join partition by partition on separate threads (radix join, Boncz and Manegold). For keys where partitioning is not chosen, keep the shared table but probe in groups of 16-32 keys: compute hashes, issue prefetches, then do the lookups, so several DRAM misses overlap instead of serialising (group prefetching / AMAC).
- **Expected:** the build becomes parallel (today it is serial) and the probe stops waiting on one miss at a time: 2x or more on Q9 at all cores.
- **Cost:** the partitioning pass costs memory bandwidth; prefetch needs a small `unsafe` block with a `// SAFETY:` note (an x86 intrinsic or `prfm` via `asm!` on AArch64).
- **Cheapest test:** prefetch alone on the existing probe (smallest change) at 3e7; kill if under 1.2x. Then partitioning.

### I-6. Parallel output formatting · Q4, any query with large output

- **Breaks:** D4.
- **Mechanism:** after aggregation, each partition (I-3) or each slice of the result is formatted to bytes on its own thread into its own buffer; the buffers are written in partition or chunk order, so the output is identical to the serial writer. Integer and decimal formatting is already allocation-free; the change is doing it on 11 cores instead of 1.
- **Expected:** if the 40% output share measured at 1e7 holds at 1e8, Q4 could lose a large part of its 13.2 s; even half of that share is worth more than most scan tweaks.
- **Cost:** buffered output for unordered results (bounded by the memory budget; with a budget, the writer streams partition by partition).
- **Cheapest test:** re-run the HAVING-vs-full-output split on Q4 at 1e8 first (no code). If output is under 15% of wall time, drop the idea.

### I-7. Two-phase parsing: key first, the rest only for rows that survive · stream join (EXP-005-stream), selective filters

- **Breaks:** D3.
- **Mechanism:** the streaming benchmark query projects all 8 columns but only about 5% of rows match the 50-row lookup. Parse the join key (or the filter columns) for the whole batch first, compute the selection, then parse the remaining referenced columns only for the selected rows, using the field offsets already found. Field boundaries are found once; numeric parsing and string copies happen only for survivors.
- **Expected:** on the stream query, most parse work disappears: 1.5x to 3x on S1 is plausible; nothing on Q1-Q9, where every row survives.
- **Cost:** the scanner must keep per-row field offsets for the batch (bounded by batch size).
- **Cheapest test:** a hard-wired two-phase parse for the EXP-005-stream query at 1x; kill if under 1.3x.

### I-8. Read with `pread` into reused buffers instead of mapping the file · file-mode memory

- **Breaks:** D6.
- **Mechanism:** file mode RSS today is about the input size (5.4 GB on a 5.55 GB file) because mapped pages count toward RSS. Reading each chunk with `pread` into a per-worker buffer (2 × 16 MiB per worker) copies data out of the page cache, which costs memory bandwidth (≥ 10 GB/s per core) that is small next to CSV parsing (about 0.5-1 GB/s per core). RSS falls to tens of MiB per worker. This is "do more work to hold less memory".
- **Expected:** file-mode RSS from about input size to under 400 MiB at 11 threads; wall time within a few percent.
- **Cost:** one copy per byte; loses zero-copy string references into the map (strings must point into the buffer while it is alive, which batches already respect).
- **Cheapest test:** Q1 and Q6 at 1e8 with a `pread` scan; kill if more than 10% slower than mmap. If it passes, it is the default whenever `--memory-limit` is set ([memory-model.md](memory-model.md#file-mode-memory)).

### I-9. Infer `decimal(N)` instead of `f64` for fixed-precision text **(semantic change; decision D-5/D-7)**

- **Breaks:** D8.
- **Mechanism:** `v1` is written with at most 6 decimals. If inference sees only fixed-precision decimals that fit in `i64` after scaling, it can type the column `decimal(6)`: parsing becomes integer parsing, sums become exact `i128` additions, and ADR-0004 then allows any merge order, which removes the fixed-order constraint from aggregation over that column.
- **Expected:** a few percent on scan-bound queries from cheaper parsing (`f64::from_str` was 7.5% of Q3's profile), more where order freedom enables per-worker states.
- **Cost:** a visible semantic change: results print as `12.345600` style decimals and `AVG` follows decimal rules. It must be an explicit inference rule, documented in [semantics.md](semantics.md#type-inference), never a silent per-chunk switch (mixing decimal and float chunks would break bit-identity).
- **Cheapest test:** declare `v1 = "decimal(6)"` in `schema.toml` today (already supported) and time Q1-Q6. No code needed.

## 4. Wild round

Three ideas that sound wrong, and the kernel of each that might be right.

| Absurd version | Kernel worth keeping | Mechanism | Test |
|---|---|---|---|
| "Never build the join table at all." | If both inputs arrive **sorted by the key** (common for exports by id and logs by time), a merge join needs no table, and a GROUP BY on a sorted key needs only the current group: **constant memory for high-cardinality GROUP BY over a pipe**. Detect sortedness while scanning (one compare per row, as Timsort detects runs) and switch strategy when the first chunks are sorted, with a fallback the moment a row is out of order. | turns state-bound operators into constant-memory ones for sorted input, which strengthens the bounded-memory claim exactly where it is weakest | generate Q4 data sorted by `id3`; run a streaming GROUP BY; peak RSS must stay flat from 1x to 10x |
| "Remember every file forever." | A tiny optional sidecar (`.qe-stats`, off by default) written after a full scan: chunk boundaries and quote flags, per-chunk min/max of each column, distinct-count sketches, inferred schema. The next run skips quote counting, skips chunks a filter excludes (zone maps), and picks aggregation strategies without sampling. Validity is checked with the same `(device, inode, size, mtime)` test used for change detection. | moves work from every run to the first run (lens: move the work), without the stale-data risk of a full columnar cache | time Q6 with a selective filter on a file sorted by the filter column, with and without zone maps |
| "Make the spill path the fast path." | The radix partitioning of I-3/I-5 and the grace spilling of Phase 2 are **the same machinery**: partitions held in memory buffers versus written to temp files. Build one partitioner and use it for both speed and bounded memory. | one code path, tested twice as hard; the memory claim stops costing speed because the in-memory case is just the spill path with buffers that never hit disk | implement I-3 behind the spill interface; Q4 must not slow down by more than 5% compared with a dedicated in-memory version |

## 5. Discarded

| Idea | Why it fails the check |
|---|---|
| Approximate `COUNT(DISTINCT)` to win Q5 | Changes the answer; a benchmark win that compares different results is not a win. Approximate aggregates stay a P2 user feature with published error bounds. |
| GPU offload of parsing | CSV parsing is bound by memory and branchy byte scanning; moving data over PCIe adds cost, adds a heavy dependency, and conflicts with the small-auditable-engine goal. |
| A columnar cache to beat DuckDB on repeat runs | Hides the cold path, which is what users and benchmarks hit first; carries stale-data risk. Already dropped as M6; the sidecar above is the cheap honest version. |
| "More threads" or bigger chunks for Q4 and Q5 | EXP-006 showed 256 MiB chunks made Q3 at 1e7 slower; Q4's problem is that partial tables reduce nothing, which more threads make worse. |
| Hand-written SIMD everywhere before the algorithms change | The losing queries are bound by random memory access and merge work, not by instruction count; `CLAUDE.md` already ranks micro-optimisations last. |
| Per-worker `f64` sums to speed merges | Breaks bit-identity across thread counts (ADR-0004 rejects it outright). |

## 6. Bets and order

Ranked by expected gain divided by cost to try, for a small team after the hackathon:

1. **I-6 measurement, then I-1 and I-2.** The I-6 split costs one benchmark run and may show that Q4's biggest cost is output. I-1 is small, exact, and targets the clearest mechanism (Q5's set work). I-2 helps every string GROUP BY users will write, not only the benchmark.
2. **I-3 built as the shared partitioner (wild idea 3), with I-6 on top, then I-4 as its dense-key specialisation.** This is the bet that pays twice: Q4 speed in Phase 4 and grace spilling in Phase 2.
3. **I-5: prefetch first (small change), radix join second, reusing the partitioner.**

Run alongside, because they are cheap and serve production rather than benchmarks: **I-8** (makes the file-mode memory claim true), **I-9** as a zero-code `schema.toml` test that informs decision D-5, and the **sortedness detector** from the wild round as an experiment for the bounded-memory story.

Each bet gets an experiment file (next free IDs from EXP-010) with: hypothesis and mechanism, kill rule, the 1-thread versus all-core prediction (I-3 and I-5 predict that the all-core gap shrinks while 1-thread barely moves; if 1-thread improves a lot instead, the mechanism was something else), the difftest and thread-identity results, and raw numbers.

## 7. How this changes the production-readiness plan

- **Phase 2 and Phase 4 share one component.** The grace-spilling partitioner in Phase 2 (task 2.3, 2.5) is designed as the radix partitioner of I-3 and I-5 from the start, with an in-memory buffer backend and a temp-file backend. Phase 4 then tunes it instead of building a second aggregation path.
- **File mode under a budget uses I-8.** Decision D-3 in the plan recommends bounded `pread` buffers when `--memory-limit` is set; I-8 is the experiment that decides whether it can become the default.
- **Inference rules (D-5) consider decimals (I-9).** The decision records whether fixed-precision text is inferred as `decimal(N)`, with the result-format consequence stated.
- **Benchmark protocol gains a non-generator dataset gate.** Because several bets are **[dense]**, any speed claim from them must also be reported on a dataset with sparse or string keys ([benchmark-protocol.md](benchmark-protocol.md#datasets)).
