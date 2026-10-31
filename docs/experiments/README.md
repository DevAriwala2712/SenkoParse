# Experiment log

Every performance idea from [ADR-0001](../adr/0001-parallel-and-fast-path-execution.md) gets one file
here, **written before the code** (hypothesis, success/failure threshold) and **completed after**
(raw numbers, result, decision). A result of "idea did not work" is a valid, documented outcome.
No number, no claim: headline figures also go into [`BENCHMARKS.md`](../../BENCHMARKS.md).

| ID | Idea | Status | Outcome |
|---|---|---|---|
| [EXP-001](001-baseline-breakdown.md) | Where does M1 spend its time? | Done | ~70% of a simple scan is field-boundary discovery; float parsing and per-group allocation are the other costs |
| [EXP-002](002-quote-free-fast-path.md) | Quote-free scan fast path (skip unread fields) | Done, kept | `COUNT(*)` 0.57 -> 0.22 s, `SUM(id1)` 0.63 -> 0.30 s; no gain when the last column is read |
| [EXP-003](003-parallel-chunked-scan.md) | Chunk-parallel scan, aggregation, join probe (M2) | Done, kept | 5.1-5.9x on 11 threads for scans; `GROUP BY id3` and the big join do not scale (next experiments) |
| [EXP-004](004-integer-key-aggregation.md) | Integer-key group table, allocation-free output, partition-wise merge | Done, merged | `GROUP BY id3` 2.28 -> 1.29 s (A + 4b) -> 0.80 s (B); a per-group `Arc` clone cost 4x CPU until found |
| EXP-005 | Parallel build / dense-key array join | Deferred (Later, see [ADR-0002](../adr/0002-stream-first-direction.md)) | target join with `big` < 1 s |
| [EXP-005-stream](005-stream-memory.md) | Bounded-memory streaming (`qe --stdin`) vs DuckDB `/dev/stdin` | Done, kept | piped `qe` peak RSS 42 MiB at 1x, 2x, 5x and 10x (375 MB to 3.7 GB), no temp files; DuckDB piped temp 237 MiB to 5.4 GiB, RSS about 340 MiB. Data is a reconstruction of the issue's shape. |

## Rules
- Same machine, release build (`cargo build --release`), data from `scripts/gen_data.py --rows 1e7` (seed 42), warm page cache. Cold-cache runs need `sudo purge` and are labelled when done.
- `python3 bench/exp.py` prints min / median / max **and every raw run**; paste the table verbatim.
- Correctness gate for every code change: `cargo test` and `cargo test --release --test difftest -- --ignored` green on **both** paths (default and `--no-opt`). Record the result.
- Record the commit/diff state the numbers came from. Noisy numbers are reported as noisy, not rerun until pretty.

## Template
```
# EXP-NNN: <title>
- Status / Date / Idea (ADR link) / Commit
## Hypothesis  (mechanism, expected size, kill threshold)
## Method      (exact commands, data, machine, what is held constant)
## Results     (raw table, then interpretation)
## Correctness (tests run + results)
## Decision    (keep / drop / follow-up) and what changes in BENCHMARKS.md / ADR
## Caveats
```
