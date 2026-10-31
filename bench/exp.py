#!/usr/bin/env python3
"""Experiment runner: time arbitrary SQL on `qe` and print a markdown table with every run.

usage: python3 bench/exp.py [--data data] [--runs 5] [--qe target/release/qe] [--env K=V ...] "SQL" ["SQL" ...]

Prints min / median / max and ALL individual run times (raw data for docs/experiments/*.md).
The first run is kept in the raw list but excluded from min/median/max (page-cache warm-up).
Stdout of qe is discarded; wall clock includes process start and CSV output.
"""
import argparse, os, statistics, subprocess, sys, time

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="data")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--qe", default="target/release/qe")
    ap.add_argument("--env", action="append", default=[], help="K=V passed to qe (e.g. QE_THREADS=1)")
    ap.add_argument("--extra", default="", help="extra qe args, e.g. '--no-opt'")
    ap.add_argument("sql", nargs="+")
    a = ap.parse_args()
    env = dict(os.environ)
    for kv in a.env:
        k, v = kv.split("=", 1)
        env[k] = v
    print("| query | min s | median s | max s | all runs (s, first = warm-up) |")
    print("|---|---|---|---|---|")
    for q in a.sql:
        ts = []
        for _ in range(a.runs):
            t = time.perf_counter()
            r = subprocess.run([a.qe, "--data", a.data, *a.extra.split(), q], stdout=subprocess.DEVNULL,
                               stderr=subprocess.PIPE, env=env)
            ts.append(time.perf_counter() - t)
            if r.returncode:
                sys.exit(r.stderr.decode())
        w = ts[1:] or ts
        print(f"| `{q}` | {min(w):.3f} | {statistics.median(w):.3f} | {max(w):.3f} | {', '.join(f'{x:.3f}' for x in ts)} |")

if __name__ == "__main__":
    main()
