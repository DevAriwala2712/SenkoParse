# ADR-0006: What "memory" means in a claim, and how file mode stays under a limit

**Status:** Proposed (decision D-3 in [production-readiness.md §14](../production-readiness.md#14-open-decisions); needs the owner's acceptance). Nothing here is implemented yet: there is no `--memory-limit` and no accounting today.
**Date:** 2026-11-02
**Deciders:** team

## Context
"Bounded memory" is the project's headline (ADR-0002), but three different numbers get called memory ([memory-model.md](../memory-model.md#what-memory-means)):

| Measure | What it counts |
|---|---|
| accounted bytes | what the engine charges to its own budget |
| anonymous RSS | heap and stack pages, not backed by a file |
| total RSS | anonymous + file-backed resident pages, including a memory-mapped input |

Today's only measured claim is total RSS in stream mode: about 42 MiB from 375 MB to 3.7 GB of piped input ([EXP-005-stream](../experiments/005-stream-memory.md)). File mode maps each input file whole (`storage/csv/mod.rs`, `Mmap::map`), so total RSS is about the input size (5.4 GB for a 5.55 GB file in [BENCHMARKS.md](../../BENCHMARKS.md)). Those pages are reclaimable page cache, but container limits and `ps` count them. A memory claim that silently means "anonymous only" would be true and misleading at the same time.

The Phase 0 audit also hit the other cost of mapping whole files: a test that rewrote a CSV while another thread had it mapped died with SIGBUS ([sql-surface.md, Phase 0 audit](../sql-surface.md#phase-0-audit-2026-11-02)).

## Decision
1. **Both numbers are reported.** Every published memory number says whether it is anonymous or total RSS, and how it was measured (`ru_maxrss` via `bench/rss_wrap.py`, `RssAnon` / `phys_footprint`).
2. **The budget controls anonymous memory** through accounted bytes: hash tables, sort buffers, queues and spill buffers are charged before they are allocated. The gap between accounted bytes and anonymous RSS is overhead, measured and published as a formula.
3. **With `--memory-limit` set, file mode reads through bounded `pread` buffers** (option A in [memory-model.md, File-mode memory](../memory-model.md#file-mode-memory)) instead of mapping whole files, so total RSS is bounded too and the claim covers what users see.
4. **Without a limit, mmap stays the fast default.** If the experiment in [performance-bets.md](../performance-bets.md) I-8 shows `pread` within 10% of mmap on Q1 and Q6, `pread` becomes the only path and mmap is removed.

## Options rejected
- **Anonymous memory only:** easy to meet, but a 5.4 GB RSS next to a "bounded memory" claim would be read as false by anyone who runs `top`.
- **Total RSS only, without accounting:** the engine cannot control the OS's page cache decisions, so the budget would have nothing exact to enforce.
- **mmap + `madvise(MADV_DONTNEED)` behind the scan (option B):** zero-copy, but platform-specific, pages still count until released, and truncation still raises SIGBUS.

## Consequences
- Positive: the claim is falsifiable with one tool (`ru_maxrss`) and matches what container limits enforce. `pread` removes the SIGBUS hazard under a limit.
- Negative: one copy per input byte under a limit (expected small next to parsing; measured in I-8 before any claim). Strings in batches must point into a live buffer, not the mapping.
- Until `--memory-limit` exists, the only memory claim remains the one in CLAUDE.md: stream mode, one query, one data shape.
