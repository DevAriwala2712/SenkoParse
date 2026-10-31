#!/usr/bin/env python3
"""Summarize raw jsonl from bench/file_vs_duckdb.py into markdown tables.  usage: summarize_file_bench.py RAW.jsonl"""
import json, sys, statistics, collections
rows = [json.loads(l) for l in open(sys.argv[1])]
d = collections.defaultdict(list); meta = {}
for r in rows:
    if r["kind"] == "time" and r["rc"] == 0:
        d[(r["setting"], r["q"], r["engine"])].append(r); meta[r["q"]] = r
for s, title in (("warm-all", "warm cache, all cores"), ("warm-1t", "warm cache, 1 thread each"), ("cold", "cold cache (first run after purge)")):
    if not any(k[0] == s for k in d): continue
    print(f"\n### {title}\n")
    print("| query | file GB | qe min s | qe med s | duckdb min s | duckdb med s | duckdb in-proc min s | qe/duckdb (min wall) | qe GB/s | duckdb GB/s | qe RSS MiB | duckdb RSS MiB |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    for q in sorted(meta):
        a, b = d.get((s, q, "qe")), d.get((s, q, "duckdb"))
        if not a or not b: continue
        gb = meta[q]["bytes"] / 1e9
        am, bm = min(r["wall_s"] for r in a), min(r["wall_s"] for r in b)
        print(f"| {meta[q]['name']} | {gb:.2f} | {am:.2f} | {statistics.median(r['wall_s'] for r in a):.2f} | {bm:.2f} | {statistics.median(r['wall_s'] for r in b):.2f} | "
              f"{min(r['inproc_s'] for r in b):.2f} | {am/bm:.2f}x | {gb/am:.2f} | {gb/bm:.2f} | {max(r['rss_mib'] for r in a):.0f} | {max(r['rss_mib'] for r in b):.0f} |")
