#!/usr/bin/env bash
# One-command demo: build, check prerequisites, run qe-demo (live, 5x by default).
#   ./demo.sh                      live, --size 5x
#   ./demo.sh --size 1x            any qe-demo option is passed through
#   ./demo.sh --replay docs/experiments/data/demo-live-5x.jsonl     stage fallback (recorded run)
#   DEMO_INSTALL_DUCKDB=1 ./demo.sh   create .demo-venv and `pip install duckdb==2.0.0` if DuckDB is missing
set -euo pipefail
cd "$(dirname "$0")"

need() { command -v "$1" >/dev/null 2>&1 || { echo "demo.sh: missing prerequisite: $1 ($2)" >&2; exit 1; }; }
need cargo "install Rust stable from https://rustup.rs"
need python3 "needed to generate the EXP-005 data and to run DuckDB"
need ps "used to sample RSS"
need stty "used for terminal control"

args=("$@")
[ ${#args[@]} -eq 0 ] && args=(--size 5x)

replay=0
for a in "${args[@]}"; do [ "$a" = "--replay" ] && replay=1; done

echo "demo.sh: building release binaries…"
cargo build --release -q -p qe -p qe-demo

if [ "$replay" = 0 ]; then
  # DuckDB is optional: without it the demo shows qe alone plus the recorded DuckDB numbers.
  has_duck() { "$1" -c "import duckdb" >/dev/null 2>&1; }
  py="${DUCKDB_PY:-}"
  if [ -z "$py" ]; then
    for cand in .demo-venv/bin/python python3; do
      if has_duck "$cand"; then py="$cand"; break; fi
    done
  fi
  if [ -z "$py" ] && [ "${DEMO_INSTALL_DUCKDB:-0}" = 1 ]; then
    echo "demo.sh: installing duckdb==2.0.0 into .demo-venv…"
    # duckdb 2.0.0 wheels need Python >= 3.10 (cp311 / abi3-cp312); the system python3 may be older (macOS: 3.9)
    vpy=python3
    for cand in python3.13 python3.12 python3.11 python3.10; do command -v "$cand" >/dev/null 2>&1 && { vpy="$cand"; break; }; done
    "$vpy" -m venv .demo-venv && .demo-venv/bin/pip install -q duckdb==2.0.0 && py=.demo-venv/bin/python
  fi
  if [ -z "$py" ]; then
    echo "demo.sh: DuckDB (python package) not found: the DuckDB panel will show RECORDED numbers." >&2
    echo "         To run it live: DEMO_INSTALL_DUCKDB=1 ./demo.sh   (or set DUCKDB_PY=/path/to/python)" >&2
  else
    export DUCKDB_PY="$py"
  fi
  # Disk: 5x input is 1.9 GB, DuckDB's temp use reaches ~1.6 GB (10x: 3.7 GB + 5.4 GiB).
  free_kb=$(df -Pk . | awk 'NR==2{print $4}')
  [ "$free_kb" -lt 6000000 ] && echo "demo.sh: warning: under 6 GB free disk here; use --size 1x or 2x." >&2
fi

exec ./target/release/qe-demo "${args[@]}"
