#!/usr/bin/env python3
"""One DuckDB run for EXP-005-stream (driven by exp005_stream.py, run with a Python that has `duckdb`).
usage: duckdb_run.py {pipe|file} DATA_DIR CSV_FILE TEMP_DIR OUT_CSV     (pipe: reads /dev/stdin, CSV_FILE ignored)
"""
import sys, duckdb
mode, data, csv, temp, out = sys.argv[1:6]
con = duckdb.connect()
con.execute("SET memory_limit='256MB'")
con.execute("SET threads=1")
con.execute(f"SET temp_directory='{temp}'")
cols = "{'id':'VARCHAR','center_id':'VARCHAR','user_id':'VARCHAR','session_id':'VARCHAR','device_id':'VARCHAR','trace_id':'VARCHAR','user_agent':'VARCHAR','ts':'BIGINT'}"
src = "/dev/stdin" if mode == "pipe" else csv
con.execute(
    f"COPY (SELECT a.* FROM read_csv('{src}', header=true, columns={cols}) a "
    f"JOIN read_csv('{data}/ids.csv', header=true, columns={{'id':'VARCHAR'}}) i ON a.center_id = i.id) "
    f"TO '{out}' (FORMAT csv, HEADER)"
)
