#!/usr/bin/env python3
"""File-mode benchmark of `qe` vs DuckDB on db-benchmark-shaped CSVs (queries Q1-Q9 of bench/run.py).

Run with a Python that has `duckdb` (the demo venv, 3.11):
  .demo-venv/bin/python bench/file_vs_duckdb.py check  --data data_1e8 --work WORK
  .demo-venv/bin/python bench/file_vs_duckdb.py time   --data data_1e8 --work WORK --out raw.jsonl [--queries 1,2]
  .demo-venv/bin/python bench/file_vs_duckdb.py cold   --data data_1e8 --work WORK --out raw.jsonl   (needs `sudo purge`)

check: runs both engines on every query, compares output as a sorted multiset (key columns exact,
       aggregate columns with relative tolerance 1e-9). A query that mismatches is NOT timed.
time:  settings "warm-all" (all cores, both engines) and "warm-1t" (qe --threads 1, DuckDB threads=1),
       --runs runs each, immediately after the check run of that query so the page cache is warm.
cold:  one run per engine per query, `sudo purge` before each run.

Wall time = spawn to exit of the whole process (DuckDB: includes Python start + `import duckdb`; its in-process
query time is recorded too, as `inproc_s`). Output goes to /dev/null while timing. Peak RSS is the child's
ru_maxrss from wait4 (same mechanism as bench/rss_wrap.py, without an extra interpreter in between).
DuckDB reads the CSVs with read_csv and explicit column types taken from schema.toml (no auto-detect);
temp_directory is under --work, memory_limit is DuckDB's default.
"""
import argparse, json, os, subprocess, sys, time, statistics, tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from run import QUERIES, machine  # noqa: E402

KEYCOLS = {1: 0, 2: 1, 3: 1, 4: 1, 5: 1, 6: 0, 7: 1, 8: 1, 9: 0}  # leading columns compared exactly
TYPES = {"i64": "BIGINT", "f64": "DOUBLE", "str": "VARCHAR"}
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
QE = os.path.join(ROOT, "target/release/qe")
TIMEOUT = 1500


def duck_child(data, threads, sql, out, stats, tmp):
    import duckdb
    t0 = time.perf_counter()
    schema = tomllib.load(open(f"{data}/schema.toml", "rb"))
    con = duckdb.connect()
    con.execute(f"SET threads={int(threads)}")
    os.makedirs(tmp, exist_ok=True)
    con.execute(f"SET temp_directory='{tmp}'")
    for t, cols in schema.items():
        cs = ", ".join(f"'{c}':'{TYPES[ty]}'" for c, ty in cols.items())
        con.execute(f"CREATE VIEW {t} AS SELECT * FROM read_csv('{data}/{t}.csv', header=true, columns={{{cs}}})")
    q0 = time.perf_counter()
    con.execute(f"COPY ({sql}) TO '{out}' (FORMAT csv, HEADER)")
    q1 = time.perf_counter()
    json.dump({"inproc_s": q1 - q0, "setup_s": q0 - t0}, open(stats, "w"))


def run(cmd, out_path, env=None):
    """spawn cmd with stdout -> out_path, return (wall_s, peak_rss_MiB, returncode)."""
    fa = [(os.POSIX_SPAWN_OPEN, 1, out_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644),
          (os.POSIX_SPAWN_OPEN, 2, os.devnull, os.O_WRONLY, 0)]
    t = time.perf_counter()
    pid = os.posix_spawn(cmd[0], cmd, env or os.environ, file_actions=fa)
    deadline = t + TIMEOUT
    while True:
        p, status, ru = os.wait4(pid, os.WNOHANG)
        if p:
            break
        if time.perf_counter() > deadline:
            os.kill(pid, 9)
            p, status, ru = os.wait4(pid, 0)
            return TIMEOUT, ru.ru_maxrss / 2**20, "timeout"
        time.sleep(0.005)
    wall = time.perf_counter() - t
    rc = os.waitstatus_to_exitcode(status)
    return wall, ru.ru_maxrss / 2**20, rc  # macOS: bytes


def cmds(a, sql, threads, out, stats):
    qe = [QE, "--data", a.data] + (["--threads", str(threads)] if threads else []) + [sql]
    py = [sys.executable, os.path.abspath(__file__), "_duck", a.data, str(threads or os.cpu_count()), sql, out,
          stats, os.path.join(a.work, "duck_tmp")]
    return {"qe": qe, "duckdb": py}


def compare(a_path, b_path, nkey, tag):
    """qe output vs duckdb output. Returns (ok, message)."""
    import duckdb
    with open(a_path) as f:
        hdr = f.readline().strip().split(",")
    n = len(hdr)
    names = [f"c{i}" for i in range(n)]
    con = duckdb.connect()
    con.execute("SET threads=8")
    for tbl, p in (("a", a_path), ("b", b_path)):
        con.execute(f"CREATE TABLE {tbl} AS SELECT * FROM read_csv('{p}', header=true, all_varchar=true, "
                    f"names={names}, quote='\"')")
    na = con.execute("SELECT count(*) FROM a").fetchone()[0]
    nb = con.execute("SELECT count(*) FROM b").fetchone()[0]
    if na != nb:
        return False, f"row count differs: qe {na} vs duckdb {nb}"
    on = " AND ".join(f"a.c{i} IS NOT DISTINCT FROM b.c{i}" for i in range(nkey)) or "TRUE"
    joined = con.execute(f"SELECT count(*) FROM a JOIN b ON {on}").fetchone()[0]
    if joined != na:
        return False, f"key join matched {joined} of {na} rows (keys differ or duplicated)"
    bad = []
    for i in range(nkey, n):
        cond = (f"NOT (a.c{i} IS NULL AND b.c{i} IS NULL) AND (a.c{i} IS NULL OR b.c{i} IS NULL OR "
                f"abs(TRY_CAST(a.c{i} AS DOUBLE) - TRY_CAST(b.c{i} AS DOUBLE)) > "
                f"1e-9 * greatest(abs(TRY_CAST(a.c{i} AS DOUBLE)), abs(TRY_CAST(b.c{i} AS DOUBLE))))")
        cnt = con.execute(f"SELECT count(*) FROM a JOIN b ON {on} WHERE {cond}").fetchone()[0]
        if cnt:
            ex = con.execute(f"SELECT a.c{i}, b.c{i} FROM a JOIN b ON {on} WHERE {cond} LIMIT 3").fetchall()
            bad.append(f"col {hdr[i]}: {cnt} rows differ, e.g. (qe, duckdb) {ex}")
    if bad:
        return False, "; ".join(bad)
    return True, f"{na} rows, {n} cols ({nkey} key)"


def main():
    if sys.argv[1] == "_duck":
        return duck_child(*sys.argv[2:8])
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["check", "time", "cold"])
    ap.add_argument("--data", default="data_1e8")
    ap.add_argument("--work", required=True)
    ap.add_argument("--out", default=None, help="raw jsonl (appended)")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--queries", default="1,2,3,4,5,6,7,8,9")
    ap.add_argument("--rows", default="1e8")
    a = ap.parse_args()
    os.makedirs(a.work, exist_ok=True)
    qs = [int(q) for q in a.queries.split(",")]
    raw = open(a.out, "a") if a.out else None

    def rec(**kw):
        kw.update(machine=machine(), rows=a.rows, date=time.strftime("%Y-%m-%d %H:%M"))
        print(json.dumps(kw), flush=True)
        if raw:
            raw.write(json.dumps(kw) + "\n"); raw.flush()

    for qn in qs:
        name, tables, sql = QUERIES[qn - 1]
        size = sum(os.path.getsize(f"{a.data}/{t}.csv") for t in tables)
        base = dict(q=qn, name=name, bytes=size, sql=sql)
        if a.mode in ("check", "time"):
            outs, ok = {}, True
            for eng in ("qe", "duckdb"):
                o = os.path.join(a.work, f"q{qn}_{eng}.csv")
                st = os.path.join(a.work, f"q{qn}_{eng}.json")
                w, r, rc = run(cmds(a, sql, None, o, st)[eng], o if eng == "qe" else os.devnull)
                outs[eng] = o
                if rc != 0:
                    ok = False
                    rec(kind="check", engine=eng, ok=False, msg=f"rc={rc}", wall_s=w, rss_mib=r, **base)
            if ok:
                ok, msg = compare(outs["qe"], outs["duckdb"], KEYCOLS[qn], name)
                rec(kind="check", engine="both", ok=ok, msg=msg, **base)
            for o in outs.values():
                try: os.remove(o)
                except OSError: pass
            if not ok:
                print(f"Q{qn}: MISMATCH/FAILURE, not timed", flush=True)
                continue
        if a.mode == "time":
            for setting, th in (("warm-all", None), ("warm-1t", 1)):
                for eng in ("qe", "duckdb"):
                    for i in range(a.runs):
                        st = os.path.join(a.work, f"q{qn}_{eng}_stat.json")
                        w, r, rc = run(cmds(a, sql, th, os.devnull, st)[eng], os.devnull)
                        extra = {}
                        if eng == "duckdb" and os.path.exists(st) and rc == 0:
                            extra = json.load(open(st)); os.remove(st)
                        rec(kind="time", setting=setting, engine=eng, run=i, wall_s=w, rss_mib=r, rc=rc, **extra, **base)
        if a.mode == "cold":
            for eng in ("qe", "duckdb"):
                subprocess.run(["sudo", "purge"], check=True)
                time.sleep(2)
                st = os.path.join(a.work, f"q{qn}_{eng}_stat.json")
                w, r, rc = run(cmds(a, sql, None, os.devnull, st)[eng], os.devnull)
                extra = {}
                if eng == "duckdb" and os.path.exists(st) and rc == 0:
                    extra = json.load(open(st)); os.remove(st)
                rec(kind="time", setting="cold", engine=eng, run=0, wall_s=w, rss_mib=r, rc=rc, **extra, **base)


if __name__ == "__main__":
    main()
