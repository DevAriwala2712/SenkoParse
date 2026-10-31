# EXP-001: Where does M1 spend its time?

- **Status:** Done (2026-10-31)
- **Idea:** ADR-0001 findings 1-3, step 1 of the plan
- **Code state:** M1, uncommitted working tree (repo has no commits yet); `cargo build --release`, `debug = 1`
- **Machine:** Apple M3 Pro, 11 cores, 18 GiB RAM, macOS (Darwin 27.0.0), warm page cache

## Hypothesis
1. Most of a simple query's time is finding field/row boundaries, not parsing values or hashing.
   *Kill threshold:* if `COUNT(*)` (parses nothing) is under half of `SUM(v1)`'s time, boundary
   discovery is not the main cost.
2. The 6.3e6-group aggregate is slow because of per-group allocation and growing a huge table, not
   because of the scan.

## Method
Data: `python3 scripts/gen_data.py --rows 1e7` (seed 42), `x.csv` = 526,671,353 bytes, 7 columns, 1e7 rows.
```
python3 bench/exp.py --runs 7 "<sql>" ...        # raw timings, first run = warm-up
sample <pid> 1 1 -file out.txt                    # macOS sampling profiler, 1 ms interval
```
`COUNT(*)` makes the scan skip every field (nothing is parsed), so it measures boundary discovery alone.

## Results
Timings (full table with all raw runs: [data/exp001-timings.md](data/exp001-timings.md)):

| query | what is parsed | median s |
|---|---|---|
| `SELECT COUNT(*) FROM x` | nothing | 0.550 |
| `SELECT SUM(id1) FROM x` | 1 short int | 0.574 |
| `SELECT SUM(id3) FROM x` | 1 int, 8 digits | 0.577 |
| `SELECT MIN(id6) FROM x` | 1 string | 0.567 |
| `SELECT MIN(id4),MIN(id5),MIN(id6)` | 3 strings | 0.674 |
| `SELECT SUM(v1) FROM x` | 1 float | 0.736 |
| `SUM(id1),SUM(id2),SUM(id3),SUM(v1)` | 3 ints + 1 float | 0.851 |
| `SELECT id3, SUM(v1) ... GROUP BY id3` | 6,320,247 groups | 2.531 |

Reference: `cat data/x.csv > /dev/null` warm = 0.04 s (memory-copy floor). One earlier `cat` took ~10 s
right after the file had been evicted from the page cache (cold read, single observation, not repeated).

Profile, top of stack, 1 ms samples (raw: [data/exp001-sample-count.txt](data/exp001-sample-count.txt),
[data/exp001-sample-q4.txt](data/exp001-sample-q4.txt)); function names are shortened here.

4-column scan (about 600 samples): `Scan::next` (CSV loop, integer/decimal parsing and builders inlined) 217,
`HashAgg::next` 105, `f64::from_str` 100, `str::from_utf8` 88, `next_field` 78.

`GROUP BY id3`: `HashAgg::next` 130, `Scan::next` 97, hashbrown `reserve_rehash` 89,
`Vec<St>::extend` (initialising per-group aggregate state) 84, `from_utf8` 71, `f64::from_str` 60,
`next_field` 51, `memcmp` 35, `memmove` 35, malloc family ~40.

## Interpretation
- Hypothesis 1 holds: `COUNT(*)` is 0.55 s, i.e. 75% of `SUM(v1)` (0.74) and 65% of the 4-column query (0.85). Boundary discovery (`next_field` plus the per-field loop in `Scan::next`) is the dominant cost.
- **New, unplanned finding:** float parsing is about 0.16-0.18 s, and 30% of the 4-column profile is `from_utf8` + `f64::from_str`. The `from_utf8` call is pure waste (validating bytes that were just checked as ASCII-ish); it is a free fix.
- Hypothesis 2 holds, and is more specific: in the high-cardinality query roughly a third of samples go to growing the table (`reserve_rehash`), initialising a state enum per group and allocating a `Box<[u8]>` key per group. The scan itself is only about 15% of the time.
- Single sampling run per query, 1 ms resolution: treat the sample counts as proportions, not exact.

## Decision
- Proceed with ADR-0001 Idea 1 (quote-free fast path), Idea 5 (inline integer keys, pre-sized table, smaller state) as planned.
- Add: replace `from_utf8` + `str::parse` for floats by parsing directly from bytes with an exact fast path (kept as a small follow-up, expected ~0.1 s).
- Also record `x.csv` aggregate numbers in `BENCHMARKS.md` as the pre-optimization reference (they are the M1 baseline rows already there).

## Caveats
- Cold-cache behaviour was not measured (needs `sudo purge`, which this session cannot run). The ~10 s cold `cat` is one observation.
- `sample` attributes inlined code to the enclosing function, so `Scan::next` mixes parsing and boundary scanning; the `COUNT(*)` timing separates them better than the profile does.
