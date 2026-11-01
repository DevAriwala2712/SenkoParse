# EXP-009 step 1: Q5 profile at 3e7 (profile only, no engine change)

- **Status:** Done. **Verdict: the per-chunk `COUNT(DISTINCT id2)` sets are the cause** of both Q5's time and its memory. Step 2 (`009-distinct-per-thread.md`) follows. 2026-11-01.
- **Follows:** EXP-006 section 5, which inferred this from RSS and the chunk test but did not profile Q5.
- **Machine:** Apple M3 Pro, 11 cores (5 performance + 6 efficiency), 18 GiB RAM, macOS 27.0.1. Rust 1.99.1. Warm page cache.
- **Code:** `main` at `78a2c06` (engine identical to `53c0e96`), release build.
- **Data:** `data_3e7/` from `scripts/gen_data.py --rows 3e7` (default seed). x.csv 1,642 MB, 30,000,000 rows; 30 distinct `id4`, 30,000 distinct `id2`. Default 16 MiB chunks, so 98 chunks.
- **Query (Q5, `bench/run.py`):** `SELECT id4, MIN(v1), MAX(v1), COUNT(DISTINCT id2) FROM x GROUP BY id4`

## The figure in the task: "about 6 GiB"
That number is EXP-006's Q5 peak RSS at **5e7** on the Linux container (6,044 MiB). At 3e7 the container peaked at 3,543 MiB. On this M3 Pro, Q5 at 3e7 peaks at **3,070 MiB**. The breakdown below is for 3e7 on the M3 Pro.

## 1. Ablation: the same query with and without the DISTINCT (2 runs each, `/usr/bin/time -l`)

| query at 3e7 | wall s | peak RSS MiB |
|---|---|---|
| Q5 as is | 1.44 (first run 3.25, cold) | 3,074 |
| Q5 without `COUNT(DISTINCT id2)` | 0.42, 0.42 | 1,619 |
| only `id4, COUNT(DISTINCT id2) ... GROUP BY id4` | 1.31, 1.35 | 3,062 / 3,055 |
| global `COUNT(DISTINCT id2)` (one group) | 0.33, 0.33 | 1,760 |

Dropping the DISTINCT removes 71% of the wall time (1.44 -> 0.42 s) and 1,455 MiB of peak RSS. The DISTINCT alone, without MIN/MAX, costs the same as the full query. With one group instead of 30, the DISTINCT is cheap (0.33 s, +140 MiB). So the cost scales with groups x chunks x set size, not with the DISTINCT as such.

## 2. CPU profile
Tool: macOS `sample` at 1 ms over 6 s of one process running Q5 8 times in a row (`qe --file`; one Q5 run is too short to attach to before the scan ends). 25,416 busy samples across all threads, with idle waits (`__psynch_cvwait` etc.) excluded. Self time is attributed by the demangled symbol: the hashbrown symbols are the `(Box<[u8]>, ())` instances, which means `FxSet<Box<[u8]>>`, the DISTINCT set type. The group map is `FxMap<Box<[u8]>, u32>`, a different instance.

**Top 5 by self time:**

| # | function | self % of busy samples |
|---|---|---|
| 1 | `hashbrown::HashMap<Box<[u8]>, ()>::insert` (the DISTINCT set) | 19.9 |
| 2 | `_platform_memcmp` (set-key compares on probe) | 16.0 |
| 3 | `hashbrown::RawTable<(Box<[u8]>, ())>::reserve_rehash` (DISTINCT sets growing) | 14.6 |
| 4 | `CsvReader::next_batch` | 11.6 |
| 5 | `AggTable::consume` (includes the inlined `update` -> set `contains`) | 7.9 |

Next are `_platform_memmove` 6.1, `f64::from_str` 4.1, csv `Builder::push` 3.4, `str::from_utf8` 2.5, `CsvSource::plan_chunks` 2.4, and malloc/free about 4 in total.

**By phase** (from the call tree: the rayon `collect` into a `LinkedList<Vec<AggTable>>` is the per-chunk consume, and the `collect` into a `Vec` of merged pairs is the pairwise tree merge in `HashAgg::run`):

| phase | % of busy samples | main contributors |
|---|---|---|
| consume (scan, parse, per-chunk tables) | 50.9 (+2.4 chunk planning) | `next_batch` 11.6, `consume` 7.9, set `reserve_rehash` 5.8, `memcmp` 4.7, `f64::from_str` 4.1 |
| pairwise tree merge of per-chunk tables | 46.2 | set `insert` 18.6, `memcmp` 11.3, set `reserve_rehash` 8.9, `memmove` 2.3, `free` 1.3 |

The merge has only 30 groups, so almost all of its 46% is `merge_state` folding DISTINCT sets (`x.extend(y.drain())`). The main thread spent 1,557 samples waiting on the merge and 1,219 waiting on the consume phase. In wall-clock terms, **the merge takes longer than the scan.** The merge is badly parallel: its upper tree levels have 1 to 3 pairs for 11 cores, and each pair unions sets of up to 30,000 boxed keys serially.

Functions that are DISTINCT-set work: insert, set rehash and the merge-phase memcmp alone add up to 45.8% of busy samples. Adding the consume-phase set work (rehash 5.8, most of memcmp 4.7, and part of `consume`) brings it to **at least 55-60%**, which matches the 71% wall drop in the ablation.

## 3. Where the peak RSS goes
Tool: `vmmap -summary` snapshots every 0.2 s during the same 8-query process (the snapshots slow the process, which does not change what is resident).

| part | resident at peak | evidence |
|---|---|---|
| mapped input `x.csv` | 1.5 GiB | `mapped file` region, 1.5G resident (file is 1,566 MiB) |
| malloc heap (`DefaultMallocZone`) | 1.5 GiB | up to 24.7 M live allocations, 1.2 GiB allocated bytes, in the snapshots at peak |
| everything else (stacks, binary, metadata) | < 50 MiB | |
| **total** | **3.0-3.1 GiB** | matches `ru_maxrss` 3,070 MiB |

The heap is the DISTINCT sets:
- Without the DISTINCT the heap is about 50 MiB (1,619 MiB peak minus the 1,566 MiB file).
- `HashAgg::run` collects **all 98 per-chunk tables before merging**, so every chunk's sets are live at the same time. Expected entries: each chunk holds about 10,200 rows per `id4` group, and drawing 10,200 times from 30,000 `id2` values gives about 30,000 x (1 - e^(-0.34)) = 8,650 distinct values. Then 8,650 x 30 groups x 98 chunks = **25.4 M set entries**, which matches the 24.7 M live allocations seen.
- Each entry is one boxed 9-byte encoded key (a 16-byte malloc) plus a 16-byte `Box<[u8]>` slot and a control byte in a hashbrown table that is up to 2x oversized after doubling. That comes to about 45-60 bytes per entry, or 1.1-1.4 GiB, in line with the measured heap.

## Verdict
- **Yes, the `COUNT(DISTINCT id2)` sets are the cause.** One set per group per chunk means 98 x 30 sets, about 25 M boxed keys live at once. That is about 95% of the 1.5 GiB heap and nearly half of the 3.07 GiB peak RSS (the rest is the mapped input, which any scan pays). Their creation, growth and pairwise union are at least 55-60% of CPU samples, and the union runs in a poorly parallel tree that takes longer than the scan.
- Not the cause: the group table (30 groups), MIN/MAX (0.42 s without the DISTINCT), parsing (linear, about 20% of samples).
- Implication for step 2: one set per worker thread instead of per chunk cuts the live entries from about 98 x 8,650 to about 11 x 30,000 per group, roughly 2.6x fewer, and removes most of the tree-merge union. The mapped input (1.5 GiB) stays, so **halving peak RSS at 3e7 is impossible** with this change (it would need 1.53 GiB total). The speed half of the kill rule is the one that can pass.

## Threats
`sample` profiles a process running 8 Q5s back to back. The first runs in that process were slower (2.0 and 2.6 s against 1.4 s), so the window mixes warm and less warm runs. Symbol attribution relies on hashbrown monomorph names; `memcmp` is split by phase from the call tree, not by caller. The `vmmap` snapshots are 0.2 s apart and may miss the exact peak. One size (3e7) only. EXP-006's container numbers are not comparable in absolute terms.
