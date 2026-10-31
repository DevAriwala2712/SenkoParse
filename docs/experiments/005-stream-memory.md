# EXP-005-stream: bounded-memory piped CSV vs DuckDB `/dev/stdin`

- Status: **Done** (hypothesis and kill threshold above were written before the measurements)
- Date: 2026-10-31
- Idea: [ADR-0002](../adr/0002-stream-first-direction.md) (stream-first). Not the join experiment EXP-005 of ADR-0001.
- Commit under test: `67cc23d` (the `--stdin` implementation and tests; the harness files were added after, unchanged engine).

## Hypothesis
1. `cat a.csv | qe --stdin a "<join>"` has a peak RSS that stays roughly constant as the piped input grows
   from 1x to 10x, because input is buffered in a bounded queue (16 x up to 1 MiB) plus one block (4 MiB default)
   and the join build side (50 ids) and the output do not depend on input size.
2. DuckDB 2.0.0 reading the same bytes through a pipe (`read_csv('/dev/stdin')`) spills to temp files, so its temp
   use grows with the input (issue #25165), while by filename it stays flat with 0 temp.
3. Expected size: SenkoParse RSS in the tens of MiB for all four sizes; DuckDB piped temp growing roughly with input.

**Kill threshold:** SenkoParse peak RSS grows by more than 25% from 1x to 10x. If hit: stop, report, do not tune.
Second, informational: SenkoParse peak RSS above DuckDB's piped peak RSS at any size means the headline
"bounded memory beats DuckDB" is not supported at that size and must be reported as such.

## Method
- Machine/versions/settings are recorded by the script in the results (`bench/exp005_stream.py`).
- Data (`bench/gen_stream_data.py`): a reconstruction of the issue's shape, **not the issue's own script** (the issue
  text was not reachable from the build environment): 8 columns (6 UUIDs, a quoted user agent containing commas,
  an integer timestamp), 1x about 383 MB, a 50-row `ids` lookup table (unique ids), about 5% of rows match.
  2x / 5x / 10x repeat the body of the 1x file; the header appears once.
- Query: `SELECT a.id, a.center_id, ... FROM a JOIN ids i ON a.center_id = i.id` (every column of `a`; `a.*` is not
  in the supported SQL). Equal to the issue's semi-join because `ids.id` is unique.
- SenkoParse: `cat a.csv | qe --data D --stdin a --threads 1 "<query>"`.
- DuckDB: `memory_limit='256MB'`, `threads=1`, own `temp_directory`; `read_csv('/dev/stdin')` (pipe) and
  `read_csv('<file>')` (by filename); result written with `COPY ... TO`.
- Per run: wall time, peak RSS of the engine process (`wait4` rusage), peak size of the temp directory (sampled
  every 20 ms), output row count. Outputs of both engines are parsed and compared as sorted multisets of rows.
- 3 runs per cell. Page cache is warm; the feeder is `cat` in all piped cases.

## Results
Machine and settings as printed by the script (a cloud container, **not** the M3 Pro of the other experiments):
```
machine: Linux-6.18.44-fc-v185-x86_64-with-glibc2.39, Intel(R) Xeon(R) Processor @ 2.10GHz, 4 cores, 16091 MiB RAM
qe commit 67cc23d; duckdb 2.0.0; python 3.13.18; DuckDB: memory_limit=256MB threads=1; qe: --threads 1, default 4 MiB blocks
measurement floor (wrapper + `true`): 12.4 MiB RSS
```
All 36 raw runs (JSON lines): [`data/exp005-stream-raw.jsonl`](data/exp005-stream-raw.jsonl); full console output
[`data/exp005-stream-output.txt`](data/exp005-stream-output.txt). Command:
`python3 bench/gen_stream_data.py --out D && python3 bench/exp005_stream.py --data D --work W --duckdb-python PY --runs 3`.

| size | input MB | engine | wall s (min/med/max) | peak RSS MiB (min/med/max) | peak temp MiB (max) | output rows |
|---|---|---|---|---|---|---|
| 1x | 375 | qe pipe | 0.552/0.648/0.704 | 41.6/41.9/41.9 | 0.0 | 54986 |
| 1x | 375 | duckdb pipe | 2.039/2.105/2.126 | 336.7/337.4/337.6 | 236.7 | 54986 |
| 1x | 375 | duckdb file | 0.898/0.903/0.924 | 216.0/221.7/246.2 | 0.0 | 54986 |
| 2x | 750 | qe pipe | 1.027/1.031/1.031 | 41.8/41.8/41.9 | 0.0 | 109972 |
| 2x | 750 | duckdb pipe | 4.226/4.404/6.152 | 337.7/337.9/338.1 | 644.1 | 109972 |
| 2x | 750 | duckdb file | 1.573/1.623/1.654 | 233.8/245.9/249.8 | 0.0 | 109972 |
| 5x | 1874 | qe pipe | 2.538/2.548/2.637 | 42.0/42.1/42.2 | 0.0 | 274930 |
| 5x | 1874 | duckdb pipe | 18.326/18.836/22.037 | 341.5/341.5/341.9 | 1676.2 | 274930 |
| 5x | 1874 | duckdb file | 3.545/3.651/3.724 | 228.5/244.9/250.5 | 0.0 | 274930 |
| 10x | 3749 | qe pipe | 4.78/4.793/5.05 | 42.0/42.0/42.0 | 0.0 | 549860 |
| 10x | 3749 | duckdb pipe | 69.843/69.903/70.871 | 345.8/345.9/346.0 | 5570.5 | 549860 |
| 10x | 3749 | duckdb file | 6.959/7.223/7.255 | 232.0/244.6/244.8 | 0.0 | 549860 |

Output check: for every size the sorted row multiset (parsed CSV) is identical for SenkoParse, DuckDB piped and
DuckDB by filename, in all 3 runs (digests in the raw file).

Interpretation:
- SenkoParse peak RSS: 41.6-42.2 MiB at every size, 1x to 10x (3.7 GB). Growth 1x to 10x: 41.9 -> 42.0 MiB (0.2%).
  About 12 MiB of that is the measurement floor (the wrapper process, see Caveats). It created no temp files.
- DuckDB piped: peak RSS about 337-346 MiB (flat, held by `memory_limit`), but temp use grows with the input:
  237 / 644 / 1676 / 5571 MiB at 1x / 2x / 5x / 10x (max of 3 runs). At 10x it is larger than the 3.7 GB input.
  By filename it uses 0 temp.
- Wall time (this machine, one thread, query-specific, not a general speed claim): SenkoParse piped 0.55-0.70 s at
  1x and 4.8-5.1 s at 10x; DuckDB by filename 0.90 s and 7.0-7.3 s; DuckDB piped 2.0-2.1 s and 69.8-70.9 s.

## Correctness
- `cargo test` green (see Decision), including `tests/stream.rs` and the stream configuration of the difftest.
- The three engines' outputs are identical at every size (above).

## Correctness
(to be filled)

## Decision
- Hypothesis 1 holds on this query: RSS grew 0.2% from 1x to 10x, far below the 25% kill threshold. Not hit.
- Hypothesis 2 holds: DuckDB piped spilled 237 -> 5571 MiB of temp as the input grew; by filename 0.
- Keep the design. Claims allowed in README/BENCHMARKS: the measured table, for this query and data shape only.
- Not shown: queries with many groups or a large join build side (not bounded, by design), multi-column group
  keys, and any input shape other than this one.

## Caveats
- **The data is my reconstruction of the issue's shape, not the issue's script.** Rows are 6 UUIDs, a quoted user agent
  and an integer; 1x is 375 MB (the issue/ADR number is 383 MB). 2x/5x/10x repeat the 1x body, so rows repeat.
- **Measurement fix:** the first full run used `wait4` from the harness itself. Linux carries the parent's address-space
  high-water mark into the child's `ru_maxrss` across `exec`, and the harness had grown (it sorts output rows), so
  later runs reported 341-666 MiB for every engine. That run was discarded and the harness now measures through
  `bench/rss_wrap.py`. The wrapper-and-`true` floor is 12.4 MiB and is included in every RSS number above.
- DuckDB's by-filename RSS here (216-250 MiB) is higher than the 144 MiB in the ADR's reproduction; the piped value
  (337 MiB) is close to the ADR's 324 MiB. The differences are not explained: this DuckDB run goes through
  Python (interpreter included), the ADR's setup is not recorded in the repo.
- DuckDB is driven from Python, so its RSS includes the interpreter (about 25-40 MiB); this favours SenkoParse.
- Group tables and join build sides are not bounded; this query has neither a large one.
