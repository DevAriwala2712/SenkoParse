#!/usr/bin/env python3
"""EXP-005-stream: peak RSS, peak temp use and wall time, SenkoParse (piped) vs DuckDB (piped and by filename).

usage: python3 bench/exp005_stream.py --data DIR --work DIR --duckdb-python PY [--runs 3] [--sizes 1,2,5,10]
Prints machine info, every raw run, and a summary table; also writes <work>/raw.jsonl.
Peak RSS = ru_maxrss of the engine process from wait4 inside bench/rss_wrap.py (a tiny wrapper, so the harness's
own memory is not inherited; floor ~10 MiB). The DuckDB process is Python+duckdb, so it includes the interpreter. Peak temp = max over 20 ms samples of the allocated bytes under the engine's temp dir.
"""
import argparse, csv, hashlib, json, os, platform, shutil, subprocess, sys, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
COLS = ["id", "center_id", "user_id", "session_id", "device_id", "trace_id", "user_agent", "ts"]
QUERY = "SELECT " + ", ".join("a." + c for c in COLS) + " FROM a JOIN ids i ON a.center_id = i.id"

def du(path):
    total = 0
    for dp, _, fs in os.walk(path):
        for f in fs:
            try:
                total += os.lstat(os.path.join(dp, f)).st_blocks * 512
            except OSError:
                pass
    return total

def digest(path):
    with open(path, newline="") as f:
        rows = sorted(repr(r) for r in csv.reader(f))
    return len(rows) - 1, hashlib.sha256("\n".join(rows).encode()).hexdigest()[:16]

def measure(cmd, infile, temp, out, env=None):
    shutil.rmtree(temp, ignore_errors=True)
    os.makedirs(temp)
    peak = [0]
    stop = threading.Event()
    def sample():
        while not stop.is_set():
            peak[0] = max(peak[0], du(temp))
            time.sleep(0.02)
    th = threading.Thread(target=sample)
    th.start()
    t0 = time.perf_counter()
    feeder = subprocess.Popen(["cat", infile], stdout=subprocess.PIPE) if infile else None
    with open(out, "wb") as o, open(out + ".err", "wb") as e:
        p = subprocess.Popen([sys.executable, f"{HERE}/rss_wrap.py", out + ".rss", *cmd],
                             stdin=feeder.stdout if feeder else subprocess.DEVNULL, stdout=o, stderr=e,
                             env={**os.environ, "TMPDIR": temp, **(env or {})})
        if feeder:
            feeder.stdout.close()
        p.wait()
        r = json.load(open(out + ".rss"))
        status, maxrss_kib = r["status"], r["maxrss_kib"]
    wall = time.perf_counter() - t0
    if feeder:
        feeder.wait()
    stop.set()
    th.join()
    peak[0] = max(peak[0], du(temp))
    if status != 0:
        sys.exit(f"{cmd} failed (status {status}): {open(out + '.err').read()[-500:]}")
    return dict(wall_s=round(wall, 3), rss_mib=round(maxrss_kib / 1024, 1), temp_mib=round(peak[0] / 2**20, 1))

def floor():
    """RSS the wrapper alone reports for a trivial command (the measurement floor)."""
    subprocess.run([sys.executable, f"{HERE}/rss_wrap.py", "/tmp/.rss_floor", "true"], check=True)
    return json.load(open("/tmp/.rss_floor"))["maxrss_kib"] / 1024

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--duckdb-python", required=True)
    ap.add_argument("--qe", default=os.path.join(ROOT, "target/release/qe"))
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--sizes", default="1,2,5,10")
    a = ap.parse_args()
    os.makedirs(a.work, exist_ok=True)
    ver = subprocess.run([a.duckdb_python, "-c", "import duckdb;print(duckdb.__version__)"], capture_output=True, text=True).stdout.strip()
    cpu = next((l.split(":")[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), "?")
    mem = next((l.split()[1] for l in open("/proc/meminfo") if l.startswith("MemTotal")), "?")
    commit = subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    print(f"machine: {platform.platform()}, {cpu}, {os.cpu_count()} cores, {int(mem)//1024} MiB RAM")
    print(f"qe commit {commit}; duckdb {ver}; python {platform.python_version()}; DuckDB: memory_limit=256MB threads=1; qe: --threads 1, default 4 MiB blocks")
    print(f"measurement floor (wrapper + `true`): {floor():.1f} MiB RSS")
    print(f"query: {QUERY}\n")
    engines = {
        "qe pipe": lambda k: (dict(cmd=[a.qe, "--data", a.data, "--stdin", "a", "--threads", "1", QUERY], infile=f"{a.data}/a_{k}x.csv")),
        "duckdb pipe": lambda k: dict(cmd=[a.duckdb_python, f"{HERE}/duckdb_run.py", "pipe", a.data, "", f"{a.work}/tmp", f"{a.work}/out"], infile=f"{a.data}/a_{k}x.csv"),
        "duckdb file": lambda k: dict(cmd=[a.duckdb_python, f"{HERE}/duckdb_run.py", "file", a.data, f"{a.data}/a_{k}x.csv", f"{a.work}/tmp", f"{a.work}/out"], infile=None),
    }
    raw, digests = [], {}
    for k in map(int, a.sizes.split(",")):
        size_mb = os.path.getsize(f"{a.data}/a_{k}x.csv") / 1e6
        for name, mk in engines.items():
            for r in range(a.runs):
                out = f"{a.work}/out"
                res = measure(**mk(k), temp=f"{a.work}/tmp", out=out)
                rows, dg = digest(out)
                res.update(size=f"{k}x", input_mb=round(size_mb), engine=name, run=r + 1, out_rows=rows, digest=dg)
                digests.setdefault(k, set()).add((rows, dg))
                raw.append(res)
                print(json.dumps(res), flush=True)
    with open(f"{a.work}/raw.jsonl", "w") as f:
        f.writelines(json.dumps(r) + "\n" for r in raw)
    print("\n| size | input MB | engine | wall s (min/med/max) | peak RSS MiB (min/med/max) | peak temp MiB (max) | output rows |")
    print("|---|---|---|---|---|---|---|")
    import statistics as st
    for k in map(int, a.sizes.split(",")):
        for name in engines:
            rs = [r for r in raw if r["size"] == f"{k}x" and r["engine"] == name]
            f3 = lambda key: f"{min(r[key] for r in rs)}/{st.median(r[key] for r in rs)}/{max(r[key] for r in rs)}"
            print(f"| {k}x | {rs[0]['input_mb']} | {name} | {f3('wall_s')} | {f3('rss_mib')} | {max(r['temp_mib'] for r in rs)} | {rs[0]['out_rows']} |")
    bad = {k: v for k, v in digests.items() if len(v) != 1}
    print("\noutput check (row multiset digest identical across all engines and runs, per size):", "OK" if not bad else f"MISMATCH {bad}")

if __name__ == "__main__":
    main()
