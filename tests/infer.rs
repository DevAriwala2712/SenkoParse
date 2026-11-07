//! Schema inference (ADR-0007, production-readiness.md task 1.1).
//! Claim: a table with no `schema.toml` entry gets types from a sample of its file, and then answers exactly as
//! if those types had been declared. Falsifiers: different rows from the declared twin on any path; a later
//! value that does not fit is silently NULLed or widened instead of failing with the hint; `schema.toml` loses.
use engine::types::Value;
use engine::Options;
use std::path::{Path, PathBuf};

fn dir(name: &str, files: &[(&str, &str)], schema: Option<&str>) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("infer-data").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for (n, c) in files {
        std::fs::write(d.join(n), c).unwrap();
    }
    if let Some(s) = schema {
        std::fs::write(d.join("schema.toml"), s).unwrap();
    }
    d
}

fn opts() -> Vec<Options> {
    vec![
        Options::default(),
        Options { no_opt: true, ..Default::default() },
        Options { threads: Some(3), chunk_bytes: Some(7), ..Default::default() },
    ]
}

fn run(d: &Path, sql: &str, o: &Options) -> Result<Vec<Vec<Value>>, String> {
    engine::run_collect_opts(d, sql, o).map(|r| r.1).map_err(|e| e.0)
}

fn sorted(r: Vec<Vec<Value>>) -> Vec<String> {
    let mut v: Vec<String> = r.into_iter().map(|x| format!("{x:?}")).collect();
    v.sort();
    v
}

const SALES: &str = "Region,Amount,Qty,Day,Note\r\n\
north,10.5,3,2024-01-02,\"a, b\"\r\n\
south,7,,2024-01-03,\"line\nbreak\"\r\n\
north,,5,,\r\n\
,2.25,1,2024-02-01,x\r\n";

#[test]
fn inferred_equals_declared() {
    let inf = dir("eq-inf", &[("sales.csv", SALES)], None);
    let dec = dir(
        "eq-dec",
        &[("sales.csv", SALES)],
        Some("[sales]\nregion = \"str\"\namount = \"f64\"\nqty = \"i64\"\nday = \"date\"\nnote = \"str\"\n"),
    );
    let queries = [
        "SELECT * FROM sales",
        "SELECT region, SUM(amount), COUNT(qty), MAX(day) FROM sales GROUP BY region",
        "SELECT note FROM sales WHERE qty > 2 ORDER BY note",
        "SELECT COUNT(*) FROM sales WHERE day IS NULL",
    ];
    for q in queries {
        for o in opts() {
            let a = run(&inf, q, &o).unwrap_or_else(|e| panic!("{q}: {e}"));
            let b = run(&dec, q, &o).unwrap();
            assert_eq!(sorted(a), sorted(b), "{q}");
        }
    }
}

#[test]
fn schema_toml_wins_and_both_sources_mix() {
    let d = dir(
        "mix",
        &[("a.csv", "id,v\n1,007\n2,8\n"), ("b.csv", "id,w\n1,x\n2,y\n")],
        Some("[a]\nid = \"i64\"\nv = \"str\"\n"),
    );
    for o in opts() {
        let r = run(&d, "SELECT a.v, b.w FROM a JOIN b ON a.id = b.id ORDER BY a.v", &o).unwrap();
        assert_eq!(r[0], vec![Value::Str("007".into()), Value::Str("x".into())]);
    }
}

#[test]
fn late_conflict_fails_with_hint() {
    // 1 MiB of integers, then a value that is not one: the sample never sees it.
    let mut csv = String::from("k,n\n");
    let mut i = 0u64;
    while csv.len() < (1 << 20) + 100 {
        csv.push_str(&format!("{},{}\n", i % 7, i));
        i += 1;
    }
    csv.push_str("3,oops\n");
    let d = dir("late", &[("t.csv", &csv)], None);
    for o in opts() {
        let e = run(&d, "SELECT SUM(n) FROM t", &o).unwrap_err();
        assert!(e.contains("invalid i64 value 'oops'") && e.contains("inferred") && e.contains("schema.toml"), "{e}");
        // The column is not read, so the bad value does not matter (same as declared columns).
        assert!(run(&d, "SELECT COUNT(*) FROM t", &o).is_ok());
    }
}

#[test]
fn file_names_and_errors() {
    let d = dir(
        "names",
        &[("my-data.csv", "a\n1\n"), ("Upper.CSV", "a\n2\n"), ("empty.txt", "x"), ("ok_1.csv", "a\n3\n")],
        None,
    );
    let o = Options::default();
    assert_eq!(run(&d, "SELECT a FROM upper", &o).unwrap(), vec![vec![Value::I64(2)]]);
    assert_eq!(run(&d, "SELECT a FROM ok_1", &o).unwrap(), vec![vec![Value::I64(3)]]);
    assert!(run(&d, "SELECT a FROM t", &o).unwrap_err().contains("unknown table 't'"));
    // A malformed CSV with no schema entry fails only the queries that use it, naming the file.
    let bad = dir("bad", &[("good.csv", "a\n1\n"), ("broken.csv", "a,a\n1,2\n")], None);
    assert_eq!(run(&bad, "SELECT a FROM good", &o).unwrap(), vec![vec![Value::I64(1)]]);
    let e = run(&bad, "SELECT a FROM broken", &o).unwrap_err();
    assert!(e.contains("broken.csv") && e.contains("duplicate header column 'a'"), "{e}");
}

#[test]
fn stream_uses_inferred_schema_of_same_named_file() {
    let d = dir("stream", &[("t.csv", "k,v\n1,2.5\n")], None);
    let data = "k,v\n1,2.5\n2,4\n";
    let r = engine::run_collect_stream(
        &d,
        "SELECT SUM(v) FROM t",
        &Options::default(),
        "t",
        Box::new(std::io::Cursor::new(data.as_bytes().to_vec())),
    )
    .unwrap()
    .1;
    assert_eq!(r, vec![vec![Value::F64(6.5)]]);
}
