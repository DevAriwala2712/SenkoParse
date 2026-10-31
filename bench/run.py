#!/usr/bin/env python3
"""Time the `qe` binary on db-benchmark-shaped queries and append a table to BENCHMARKS.md.

usage: python3 bench/run.py [--data data] [--runs 3] [--rows-label 1e7] [--label M1-baseline] [--no-append]

Timing = wall clock of the whole process (spawn, parse, scan, write CSV to /dev/null).
GB/s = bytes of the CSV files the query reads / wall time. Reported: min and median of --runs
runs on a warm OS page cache (the first run, which may be cold, is discarded when runs > 1;
a truly cold run needs `sudo purge` between runs, which this script does not do).
If the DuckDB CLI or Python module is available, the same SQL is timed on it as a reference.
"""
import argparse, os, platform, shutil, statistics, subprocess, sys, time

QUERIES = [
    ("Q1 global agg",        ["x"],            "SELECT COUNT(*), SUM(v1), AVG(v1) FROM x"),
    ("Q2 group id1 (10)",    ["x"],            "SELECT id1, SUM(v1) FROM x GROUP BY id1"),
    ("Q3 group id2 (1e4)",   ["x"],            "SELECT id2, COUNT(*), SUM(v1) FROM x GROUP BY id2"),
    ("Q4 group id3 (~6e6)",  ["x"],            "SELECT id3, SUM(v1) FROM x GROUP BY id3"),
    ("Q5 group str id4",     ["x"],            "SELECT id4, MIN(v1), MAX(v1), COUNT(DISTINCT id2) FROM x GROUP BY id4"),
    ("Q6 filter + agg",      ["x"],            "SELECT COUNT(*), SUM(v1) FROM x WHERE v1 > 50 AND id1 < 5"),
    ("Q7 join small + grp",  ["x", "small"],   "SELECT s.id4, SUM(x.v1) FROM x JOIN small s ON x.id1 = s.id1 GROUP BY s.id4"),
    ("Q8 join medium + grp", ["x", "medium"],  "SELECT m.id5, COUNT(*), SUM(x.v1) FROM x JOIN medium m ON x.id2 = m.id2 GROUP BY m.id5"),
    ("Q9 join big (1e7x1e7)",["x", "big"],     "SELECT COUNT(*), SUM(x.v1 + b.v2) FROM x JOIN big b ON x.id3 = b.id3"),
]


def timed(cmd):
    t = time.perf_counter()
    r = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    dt = time.perf_counter() - t
    if r.returncode != 0:
        sys.exit(f"command failed: {cmd}\n{r.stderr.decode()}")
    return dt


def machine():
    cpu = platform.processor() or platform.machine()
    try:
        if sys.platform == "darwin":
            cpu = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"]).decode().strip()
            mem = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"])) / 2**30
            ncpu = subprocess.check_output(["sysctl", "-n", "hw.ncpu"]).decode().strip()
            return f"{cpu}, {ncpu} cores, {mem:.0f} GiB RAM, {platform.system()} {platform.release()}"
    except Exception:
        pass
    return f"{cpu}, {os.cpu_count()} cores, {platform.system()} {platform.release()}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="data")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--rows", default="1e7", help="label for the table only")
    ap.add_argument("--label", default="M1 baseline (single thread)")
    ap.add_argument("--qe", default="target/release/qe")
    ap.add_argument("--no-append", action="store_true")
    a = ap.parse_args()

    duck = shutil.which("duckdb")
    lines = [f"\n## {a.label} — {a.rows} rows\n",
             f"- machine: {machine()}",
             f"- date: {time.strftime('%Y-%m-%d')}, warm page cache, {a.runs} runs (first discarded if >1), wall time of the full process\n",
             "| query | file GB | qe min s | qe median s | GB/s (min) |" + (" duckdb min s |" if duck else ""),
             "|---|---|---|---|---|" + ("---|" if duck else "")]
    for name, tables, sql in QUERIES:
        size = sum(os.path.getsize(f"{a.data}/{t}.csv") for t in tables) / 1e9
        ts = [timed([a.qe, "--data", a.data, sql]) for _ in range(a.runs)]
        ts = ts[1:] if len(ts) > 1 else ts
        row = f"| {name} | {size:.2f} | {min(ts):.2f} | {statistics.median(ts):.2f} | {size / min(ts):.2f} |"
        if duck:
            rel = "; ".join(
                f"CREATE VIEW {t} AS SELECT * FROM read_csv_auto('{a.data}/{t}.csv')" for t in tables)
            ds = [timed([duck, "-c", f"{rel}; COPY ({sql}) TO '/dev/null' (FORMAT CSV)"]) for _ in range(a.runs)]
            row += f" {min(ds[1:] or ds):.2f} |"
        print(row, flush=True)
        lines.append(row)
    out = "\n".join(lines) + "\n"
    if not a.no_append:
        with open("BENCHMARKS.md", "a") as f:
            f.write(out)
    print(out)


if __name__ == "__main__":
    main()
