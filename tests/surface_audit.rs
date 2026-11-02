//! Phase 0 audit of the SQL surface (docs/sql-surface.md, task 0.1 of docs/production-readiness.md).
//!
//! Every row marked 🟡 or ❌ in the surface table has a probe here, named after its ID (`typ10_...`,
//! `prj06_...`). Two kinds of probe:
//!
//! * **Supported** features must equal SQLite under the comparison rules of docs/correctness.md (NULL = NULL,
//!   multisets unless ORDER BY, floats with relative tolerance 1e-9), on every engine configuration: default,
//!   `--no-opt`, 3 threads with tiny chunks and the forced partitioned merge, and stream mode (default and
//!   `--no-opt`, 1-byte reads, 7-byte blocks) where the query streams.
//! * **Rejected** features must fail before any row is produced. The probe pins down *how* they fail today:
//!   `Unsupported` (the error names the feature with `unsupported`, state ⛔) or `Generic` (a syntax or other
//!   error that does not say `unsupported`, state ❌). The default and `--no-opt` paths must give the same text.
//!
//! A probe that finds a wrong answer is recorded as a bug with a failing `#[ignore]` test, not fixed here.
//! CLI-only rows (flags such as `--schema`, `--format`) are probed in `crates/cli/tests/surface_audit_cli.rs`.
mod common;
use common::Dribble;
use engine::types::{fmt_date, Value};
use rusqlite::{types::Value as SqlVal, types::ValueRef, Connection};
use std::path::{Path, PathBuf};

// ----------------------------------------------------------------------------- data

/// (table, columns with schema type, CSV body). No quoting in the data, so the same text loads into SQLite.
/// (column name, schema type)
type Cols = &'static [(&'static str, &'static str)];

const TABLES: &[(&str, Cols, &str)] = &[
    (
        "t",
        &[("id", "i64"), ("s", "str"), ("v", "i64"), ("p", "decimal(2)"), ("f", "f64"), ("d", "date")],
        "1,a,10,1.50,0.5,2026-01-01\n\
         2,b,,2.00,1.25,2026-01-02\n\
         3,a,30,,2.5,\n\
         4,,40,1.50,,2026-01-04\n\
         5,o'k,50,0.25,-3.75,2025-12-31\n\
         6,c,60,3.00,-1.0,2026-01-03\n\
         7,a,-5,-0.50,0,2026-01-01\n",
    ),
    (
        "u",
        &[("k", "i64"), ("w", "str"), ("kd", "decimal(2)")],
        "1,x,1.00\n\
         2,y,2.00\n\
         2,y2,2.00\n\
         ,z,\n\
         7,q,7.00\n\
         9,r,9.50\n",
    ),
    ("w", &[("m", "i64"), ("n", "str")], "1,m1\n2,m2\n3,m3\n7,m7\n"),
    ("x", &[("a", "i64"), ("b", "i64")], "1,1\n2,2\n2,1\n"),
];

/// Written once per process: rewriting a CSV while another test thread has it memory-mapped is a SIGBUS
/// (observed while writing this file; see production-readiness.md §9.1 on files that change under a scan).
fn data_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(write_data).clone()
}

fn write_data() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("surface-audit");
    std::fs::create_dir_all(&d).unwrap();
    let mut schema = String::new();
    for (name, cols, body) in TABLES {
        let header: Vec<&str> = cols.iter().map(|c| c.0).collect();
        std::fs::write(d.join(format!("{name}.csv")), format!("{}\n{body}", header.join(","))).unwrap();
        schema.push_str(&format!("[{name}]\n"));
        for (c, ty) in *cols {
            schema.push_str(&format!("{c} = \"{ty}\"\n"));
        }
        schema.push('\n');
    }
    std::fs::write(d.join("schema.toml"), schema).unwrap();
    d
}

fn sqlite() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    for (name, cols, body) in TABLES {
        let defs: Vec<String> = cols
            .iter()
            .map(|(c, ty)| {
                let t = match *ty {
                    "i64" => "INTEGER",
                    "str" | "date" => "TEXT",
                    _ => "REAL",
                };
                format!("{c} {t}")
            })
            .collect();
        conn.execute_batch(&format!("CREATE TABLE {name}({});", defs.join(","))).unwrap();
        let ph = vec!["?"; cols.len()].join(",");
        let mut st = conn.prepare(&format!("INSERT INTO {name} VALUES ({ph})")).unwrap();
        for line in body.lines() {
            let vals: Vec<SqlVal> = line
                .split(',')
                .zip(cols.iter())
                .map(|(f, (_, ty))| match (f, *ty) {
                    ("", _) => SqlVal::Null,
                    (f, "i64") => SqlVal::Integer(f.parse().unwrap()),
                    (f, "str" | "date") => SqlVal::Text(f.to_string()),
                    (f, _) => SqlVal::Real(f.parse().unwrap()),
                })
                .collect();
            st.execute(rusqlite::params_from_iter(vals)).unwrap();
        }
    }
    conn
}

// ----------------------------------------------------------------------------- comparison (docs/correctness.md)

#[derive(Clone, Debug)]
enum V {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

fn from_engine(v: &Value) -> V {
    match v {
        Value::Null => V::Null,
        Value::Bool(b) => V::Int(*b as i64),
        Value::I64(x) => V::Int(*x),
        Value::F64(x) => V::Real(*x),
        Value::Dec(x, s) => V::Real(*x as f64 / 10f64.powi(*s as i32)),
        Value::Str(s) => V::Text(s.clone()),
        Value::Date(d) => {
            let mut b = Vec::new();
            fmt_date(*d, &mut b);
            V::Text(String::from_utf8(b).unwrap())
        }
    }
}

fn cell_eq(a: &V, b: &V) -> bool {
    let num = |v: &V| match v {
        V::Int(x) => Some(*x as f64),
        V::Real(x) => Some(*x),
        _ => None,
    };
    match (a, b) {
        (V::Null, V::Null) => true,
        (V::Int(x), V::Int(y)) => x == y,
        (V::Text(x), V::Text(y)) => x == y,
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0),
            _ => false,
        },
    }
}

fn row_eq(a: &[V], b: &[V]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| cell_eq(x, y))
}

/// Ordered: row by row. Unordered: greedy multiset matching (the audit tables are tiny).
fn compare(got: &[Vec<V>], want: &[Vec<V>], ordered: bool) -> Result<(), String> {
    if got.len() != want.len() {
        return Err(format!("row count {} vs SQLite {}\n  engine: {got:?}\n  sqlite: {want:?}", got.len(), want.len()));
    }
    if ordered {
        for (i, (a, b)) in got.iter().zip(want).enumerate() {
            if !row_eq(a, b) {
                return Err(format!("row {i}: engine {a:?} vs SQLite {b:?}"));
            }
        }
        return Ok(());
    }
    let mut used = vec![false; want.len()];
    for a in got {
        match (0..want.len()).find(|&j| !used[j] && row_eq(a, &want[j])) {
            Some(j) => used[j] = true,
            None => {
                return Err(format!(
                    "engine row {a:?} has no SQLite counterpart\n  engine: {got:?}\n  sqlite: {want:?}"
                ))
            }
        }
    }
    Ok(())
}

fn sqlite_rows(conn: &Connection, sql: &str) -> Vec<Vec<V>> {
    let mut st = conn.prepare(sql).unwrap_or_else(|e| panic!("SQLite rejects {sql}: {e}"));
    let n = st.column_count();
    let mut rows = st.query([]).unwrap();
    let mut out = Vec::new();
    while let Some(r) = rows.next().unwrap() {
        out.push(
            (0..n)
                .map(|i| match r.get_ref(i).unwrap() {
                    ValueRef::Null => V::Null,
                    ValueRef::Integer(x) => V::Int(x),
                    ValueRef::Real(x) => V::Real(x),
                    ValueRef::Text(t) => V::Text(String::from_utf8_lossy(t).into_owned()),
                    ValueRef::Blob(_) => V::Null,
                })
                .collect(),
        );
    }
    out
}

// ----------------------------------------------------------------------------- probes

/// A supported feature: every engine configuration must equal SQLite. Returns the column names (default path).
fn supported(sql: &str) -> Vec<String> {
    supported_vs(sql, sql)
}

/// As `supported`, with a separate SQLite spelling for syntax SQLite reads differently.
fn supported_vs(sql: &str, sqlite_sql: &str) -> Vec<String> {
    let d = data_dir();
    let want = sqlite_rows(&sqlite(), sqlite_sql);
    let ordered = sql.to_ascii_uppercase().contains("ORDER BY");
    let configs: [(&str, engine::Options); 3] = [
        ("default", engine::Options::default()),
        ("--no-opt", engine::Options { no_opt: true, ..Default::default() }),
        (
            "3 threads/16B chunks/partitioned merge",
            engine::Options {
                threads: Some(3),
                chunk_bytes: Some(16),
                agg_partition_groups: Some(0),
                ..Default::default()
            },
        ),
    ];
    let mut names = None;
    for (label, o) in configs {
        let (n, rows) = engine::run_collect_opts(&d, sql, &o).unwrap_or_else(|e| panic!("[{label}] {sql}: {e}"));
        let got: Vec<Vec<V>> = rows.iter().map(|r| r.iter().map(from_engine).collect()).collect();
        if let Err(m) = compare(&got, &want, ordered) {
            panic!("[{label}] {sql}\n{m}");
        }
        names.get_or_insert(n);
    }
    // stream mode: the FROM table is read through 1-byte reads and 7-byte blocks
    let ftable =
        sql.split(" FROM ").nth(1).and_then(|r| r.split_whitespace().next()).unwrap().trim_end_matches(',').to_string();
    let csv = std::fs::read(d.join(format!("{ftable}.csv"))).unwrap();
    for no_opt in [false, true] {
        let o = engine::Options { chunk_bytes: Some(7), no_opt, ..Default::default() };
        match engine::run_collect_stream(&d, sql, &o, &ftable, Box::new(Dribble::bytewise(&csv))) {
            // documented stream limit (the stream table appears twice): not a surface result
            Err(e) if e.0.contains("can be scanned only once") => {}
            Err(e) => panic!("[stream no_opt={no_opt}] {sql}: {e}"),
            Ok((_, rows)) => {
                let got: Vec<Vec<V>> = rows.iter().map(|r| r.iter().map(from_engine).collect()).collect();
                if let Err(m) = compare(&got, &want, ordered) {
                    panic!("[stream no_opt={no_opt}] {sql}\n{m}");
                }
            }
        }
    }
    names.unwrap()
}

#[derive(Debug, PartialEq)]
enum Fails {
    /// error text contains `unsupported` (state ⛔)
    Unsupported,
    /// any other error (state ❌: the user is not told the feature is unsupported)
    Generic,
}

/// A rejected feature: must error on the default and `--no-opt` paths with identical text (no rows).
/// Returns the class and the error text.
fn rejection(d: &Path, sql: &str) -> (Fails, String) {
    let run = |o: &engine::Options| match engine::run_collect_opts(d, sql, o) {
        Ok((_, rows)) => panic!("{sql}: expected an error, got {} rows: {rows:?}", rows.len()),
        Err(e) => e.0,
    };
    let e = run(&engine::Options::default());
    let e_ref = run(&engine::Options { no_opt: true, ..Default::default() });
    assert_eq!(e, e_ref, "{sql}: default and --no-opt error text differ");
    (if e.contains("unsupported") { Fails::Unsupported } else { Fails::Generic }, e)
}

fn rejected_in(d: &Path, sql: &str, want: Fails) -> String {
    let (got, e) = rejection(d, sql);
    assert_eq!(got, want, "{sql}: error class changed; text: `{e}`");
    e
}

fn rejected(sql: &str, want: Fails) -> String {
    rejected_in(&data_dir(), sql, want)
}

/// Checks a whole table of probes and reports every row whose class differs, not just the first.
fn check_all(rows: &[(&str, &str, Fails)]) {
    let d = data_dir();
    let mut bad = Vec::new();
    for (id, sql, want) in rows {
        let (got, e) = rejection(&d, sql);
        eprintln!("{id} [{got:?}]: {sql}\n    -> {e}");
        if got != *want {
            bad.push(format!("{id}: {sql}: expected {want:?}, got {got:?}: `{e}`"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

// ----------------------------------------------------------------------------- 🟡 rows: supported, must equal SQLite

#[test]
fn typ10_string_literal_with_doubled_quote() {
    supported("SELECT id FROM t WHERE s = 'o''k'");
    supported("SELECT id, s FROM t WHERE s <> 'o''k' ORDER BY id");
    supported("SELECT 'it''s' AS lit, id FROM t WHERE id < 3 ORDER BY id");
}

/// Task 0.4: the parser accepts a bare `*` and the binder expands it to every column of the FROM clause, in
/// table order. It must equal SQLite's `SELECT *` (names and rows), including after a join.
#[test]
fn prj06_select_star() {
    let names = supported("SELECT * FROM t");
    assert_eq!(names, ["id", "s", "v", "p", "f", "d"]);
    supported("SELECT * FROM t WHERE v > 20 ORDER BY id");
    supported("SELECT * FROM t ORDER BY id LIMIT 3");
    let names = supported("SELECT * FROM t JOIN u ON t.id = u.k ORDER BY t.id, u.w");
    assert_eq!(names, ["id", "s", "v", "p", "f", "d", "k", "w", "kd"]);
    supported("SELECT * FROM t LEFT JOIN w ON t.id = w.m ORDER BY t.id");
    // `*` with other items stays rejected (PRJ-08)
    rejected("SELECT *, id FROM t", Fails::Unsupported);
    rejected("SELECT id, * FROM t", Fails::Unsupported);
    // `*` with GROUP BY is an ordinary non-grouped-column error, as in SQL
    rejected("SELECT * FROM t GROUP BY s", Fails::Generic);
}

#[test]
fn flt01_not_equal_bang() {
    supported("SELECT id FROM t WHERE v != 30");
    supported("SELECT id FROM t WHERE s != 'a' ORDER BY id");
    supported("SELECT id FROM t WHERE p != 1.5 AND d != '2026-01-01'");
}

#[test]
fn jn04_self_join_with_aliases() {
    supported("SELECT a.id, b.id FROM t a JOIN t b ON a.s = b.s ORDER BY 1, 2");
    supported("SELECT a.id, b.id FROM t a LEFT JOIN t b ON a.v = b.id ORDER BY 1");
    supported("SELECT p.a, q.b FROM x p JOIN x q ON p.a = q.b AND p.b = q.a");
    supported("SELECT a.s, COUNT(*) FROM t a JOIN t b ON a.s = b.s GROUP BY a.s");
}

#[test]
fn jn05_three_and_four_way_joins() {
    supported("SELECT t.id, u.w, w.n FROM t JOIN u ON t.id = u.k JOIN w ON u.k = w.m");
    supported("SELECT t.id, u.w, w.n FROM t LEFT JOIN u ON t.id = u.k LEFT JOIN w ON t.id = w.m ORDER BY 1, 2");
    supported("SELECT t.id, u.w, w.n, x.b FROM t JOIN u ON t.id = u.k JOIN w ON t.id = w.m JOIN x ON w.m = x.a");
    supported("SELECT w.n, COUNT(*), SUM(t.v) FROM t JOIN u ON t.id = u.k JOIN w ON t.id = w.m GROUP BY w.n");
    supported("SELECT t.id, w.n FROM t, u, w WHERE t.id = u.k AND u.k = w.m");
}

#[test]
fn jn07_join_keys_of_different_numeric_types() {
    // i64 = decimal(2): 1 = 1.00 matches, 9 vs 9.50 does not
    supported("SELECT t.id, u.w FROM t JOIN u ON t.id = u.kd");
    supported("SELECT u.w, t.id FROM u JOIN t ON u.kd = t.id");
    supported("SELECT t.id, u.w FROM t LEFT JOIN u ON t.id = u.kd ORDER BY 1, 2");
    // decimal(2) = decimal(2) across tables and i64 = f64
    supported("SELECT t.id, u.w FROM t JOIN u ON t.p = u.kd");
    supported("SELECT t.id, u.w FROM t JOIN u ON t.f = u.k");
}

// ----------------------------------------------------------------------------- ❌ rows: SQL syntax not recognised
//
// Each probe pins the class observed at the audit. A row with probes of both classes (AGG-16, CMP-07) is
// marked as mixed in sql-surface.md. The class is the observation, not the goal: ❌ should become ⛔.

#[test]
fn not_recognised_sql_gives_a_generic_error() {
    use Fails::*;
    check_all(&[
        ("TYP-11", "SELECT id FROM t WHERE d = DATE '2026-01-01'", Generic),
        ("PRJ-04", "SELECT id % 2 FROM t", Generic),
        ("PRJ-05", "SELECT s || 'x' FROM t", Generic),
        ("PRJ-07", "SELECT t.* FROM t", Generic),
        ("PRJ-09", "SELECT 1 + 1", Generic),
        ("FLT-09", "SELECT id FROM t WHERE v IS DISTINCT FROM 10", Generic),
        ("AGG-05", "SELECT COUNT(DISTINCT s, v) FROM t", Generic),
        ("AGG-14", "SELECT COUNT(*) FILTER (WHERE v > 10) FROM t", Generic),
        ("AGG-16", "SELECT s, SUM(v) FROM t GROUP BY ROLLUP (s)", Unsupported),
        ("AGG-16", "SELECT s, SUM(v) FROM t GROUP BY GROUPING SETS ((s), ())", Generic),
        ("AGG-16", "SELECT s, SUM(v) FROM t GROUP BY CUBE (s)", Unsupported),
        ("ORD-03", "SELECT id FROM t ORDER BY v NULLS LAST", Generic),
        ("CMP-01", "SELECT q.id FROM (SELECT id FROM t) q", Generic),
        ("CMP-04", "SELECT id, (SELECT MAX(v) FROM t) FROM t", Generic),
        ("CMP-07", "SELECT id, ROW_NUMBER() OVER (ORDER BY id) FROM t", Unsupported),
        ("CMP-07", "SELECT id, SUM(v) OVER () FROM t", Generic),
        ("INT-01", "DESCRIBE t", Generic),
        ("INT-02", "EXPLAIN SELECT id FROM t", Generic),
        ("SRC-08", "SELECT COUNT(*) FROM 't*.csv'", Generic),
    ]);
}

/// The schema-level ❌ rows: types that `schema.toml` does not accept, and a directory without `schema.toml`.
#[test]
fn schema_level_rows_are_errors() {
    let mk = |name: &str, schema: Option<&str>| {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("surface-audit-schema").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("t.csv"), "id,b\n1,true\n2,2026-01-01T10:00:00Z\n").unwrap();
        if let Some(s) = schema {
            std::fs::write(d.join("schema.toml"), s).unwrap();
        }
        d
    };
    // TYP-06 bool column type, TYP-07 timestamp column type
    for (id, ty) in [("TYP-06", "bool"), ("TYP-07", "timestamp")] {
        let d = mk(ty, Some(&format!("[t]\nid = \"i64\"\nb = \"{ty}\"\n")));
        let e = rejected_in(&d, "SELECT id, b FROM t", Fails::Generic);
        eprintln!("{id}: column type {ty}\n    -> {e}");
    }
    // SRC-03 no schema.toml, no inference
    let d = mk("noschema", None);
    let e = rejected_in(&d, "SELECT id FROM t", Fails::Generic);
    assert!(e.contains("schema.toml"), "{e}");
    eprintln!("SRC-03: no schema.toml\n    -> {e}");
}

// ----------------------------------------------------------------------------- ⛔ rows: rejected by name

/// The rows marked ⛔ before the audit, re-checked so that the table cannot drift. Most say `unsupported`; the
/// probes pinned `Generic` (FLT-04 `NOT IN`, FLT-07, FLT-08, ORD-05 `LIMIT m, n`) were ⛔ in the table but the
/// audit found a generic error, so sql-surface.md now marks them ❌.
#[test]
fn rejected_by_name() {
    use Fails::*;
    check_all(&[
        ("TYP-08", "SELECT CAST(id AS str) FROM t", Unsupported),
        ("PRJ-08", "SELECT *, id FROM t", Unsupported),
        ("FLT-04", "SELECT id FROM t WHERE id IN (1, 2)", Unsupported),
        ("FLT-04", "SELECT id FROM t WHERE id NOT IN (1, 2)", Generic),
        ("FLT-05", "SELECT id FROM t WHERE id BETWEEN 1 AND 3", Unsupported),
        ("FLT-06", "SELECT id FROM t WHERE s LIKE 'a%'", Unsupported),
        ("FLT-07", "SELECT id FROM t WHERE s ILIKE 'a%'", Generic),
        ("FLT-08", "SELECT id FROM t WHERE (v > 1) = (id > 1)", Generic),
        ("CND-01", "SELECT CASE WHEN v > 10 THEN 1 ELSE 0 END FROM t", Unsupported),
        ("CND-02", "SELECT CASE v WHEN 10 THEN 1 END FROM t", Unsupported),
        ("CND-03", "SELECT COALESCE(v, 0) FROM t", Unsupported),
        ("CND-04", "SELECT NULLIF(v, 10) FROM t", Unsupported),
        ("CND-05", "SELECT GREATEST(v, id) FROM t", Unsupported),
        ("JN-08", "SELECT t.id FROM t RIGHT JOIN u ON t.id = u.k", Unsupported),
        ("JN-08", "SELECT t.id FROM t FULL JOIN u ON t.id = u.k", Unsupported),
        ("JN-09", "SELECT t.id FROM t CROSS JOIN u", Unsupported),
        ("JN-09", "SELECT t.id FROM t, u", Unsupported),
        ("JN-10", "SELECT t.id FROM t JOIN u USING (id)", Unsupported),
        ("JN-10", "SELECT t.id FROM t NATURAL JOIN u", Unsupported),
        ("JN-11", "SELECT t.id FROM t JOIN u ON t.id < u.k", Unsupported),
        ("AGG-03", "SELECT DISTINCT s FROM t", Unsupported),
        ("AGG-04", "SELECT SUM(DISTINCT v) FROM t", Unsupported),
        ("AGG-04", "SELECT AVG(DISTINCT v) FROM t", Unsupported),
        ("AGG-10", "SELECT STDDEV(v) FROM t", Unsupported),
        ("AGG-10", "SELECT VAR_SAMP(v) FROM t", Unsupported),
        ("AGG-11", "SELECT STRING_AGG(s, ',') FROM t", Unsupported),
        ("AGG-11", "SELECT GROUP_CONCAT(s) FROM t", Unsupported),
        ("AGG-12", "SELECT ANY_VALUE(s) FROM t", Unsupported),
        ("AGG-12", "SELECT ARG_MAX(s, v) FROM t", Unsupported),
        ("AGG-13", "SELECT MEDIAN(v) FROM t", Unsupported),
        ("AGG-15", "SELECT APPROX_COUNT_DISTINCT(v) FROM t", Unsupported),
        ("ORD-05", "SELECT id FROM t ORDER BY id LIMIT 2 OFFSET 1", Unsupported),
        ("ORD-05", "SELECT id FROM t ORDER BY id LIMIT 1, 2", Generic),
        ("CMP-02", "SELECT id FROM t WHERE id IN (SELECT k FROM u)", Unsupported),
        ("CMP-03", "SELECT id FROM t WHERE EXISTS (SELECT k FROM u WHERE u.k = t.id)", Unsupported),
        ("CMP-05", "WITH q AS (SELECT id FROM t) SELECT id FROM q", Unsupported),
        ("CMP-06", "SELECT id FROM t UNION SELECT k FROM u", Unsupported),
        ("CMP-06", "SELECT id FROM t UNION ALL SELECT k FROM u", Unsupported),
        ("CMP-06", "SELECT id FROM t INTERSECT SELECT k FROM u", Unsupported),
        ("CMP-06", "SELECT id FROM t EXCEPT SELECT k FROM u", Unsupported),
        // FN-01..FN-06: no scalar functions exist; each is rejected by name (ERR-04)
        ("FN-01", "SELECT ROUND(f, 1) FROM t", Unsupported),
        ("FN-01", "SELECT ABS(v) FROM t", Unsupported),
        ("FN-02", "SELECT LOWER(s) FROM t", Unsupported),
        ("FN-02", "SELECT LENGTH(s) FROM t", Unsupported),
        ("FN-03", "SELECT FLOOR(f) FROM t", Unsupported),
        ("FN-04", "SELECT SUBSTR(s, 1, 1) FROM t", Unsupported),
        ("FN-05", "SELECT DATE_TRUNC('month', d) FROM t", Unsupported),
        ("FN-06", "SELECT REGEXP_REPLACE(s, 'a', 'b') FROM t", Unsupported),
    ]);
}

// ----------------------------------------------------------------------------- the probe can fail

/// Falsifier for the harness itself: a query that differs from the oracle by one row must fail the probe.
#[test]
#[should_panic(expected = "row count")]
fn supported_probe_detects_a_wrong_answer() {
    supported_vs("SELECT id FROM t", "SELECT id FROM t WHERE id > 1");
}

/// Falsifier for value comparison: same row count, one value off by 1e-6 relative.
#[test]
#[should_panic(expected = "no SQLite counterpart")]
fn supported_probe_detects_a_wrong_value() {
    supported_vs("SELECT f FROM t WHERE f IS NOT NULL", "SELECT f * 1.000001 FROM t WHERE f IS NOT NULL");
}

/// SRC-09 and SRC-10: the documented stream limits fail with a clear, specific error, but not with the
/// `unsupported` form, so by the audit rule they are ❌ until the text names them as unsupported.
#[test]
fn src09_src10_stream_limits_are_named_but_not_unsupported() {
    let d = data_dir();
    let csv = std::fs::read(d.join("t.csv")).unwrap();
    for (id, sql, want) in [
        ("SRC-09", "SELECT a.id FROM t a JOIN t b ON a.id = b.id", "can be scanned only once"),
        ("SRC-10", "SELECT u.w FROM u LEFT JOIN t ON u.k = t.id", "cannot be the right side of a LEFT JOIN"),
    ] {
        for no_opt in [false, true] {
            let o = engine::Options { no_opt, ..Default::default() };
            let e = match engine::run_collect_stream(&d, sql, &o, "t", Box::new(std::io::Cursor::new(csv.clone()))) {
                Ok((_, rows)) => panic!("{id}: expected an error, got {rows:?}"),
                Err(e) => e.0,
            };
            eprintln!("{id}: {sql}\n    -> {e}");
            assert!(e.contains(want) && !e.contains("unsupported"), "{id}: `{e}`");
        }
    }
}
