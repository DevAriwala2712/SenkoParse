//! Regression files in `tests/regression/`. Today: `demo_candidates.sql` (production-readiness.md §2.3), the query
//! shapes that may have been the failed demo aggregation. Each statement must fail with exactly the error text
//! recorded in the file (`expect-error`), or return exactly the recorded rows (`expect-rows`), on the default and `--no-opt` paths. When a candidate becomes supported,
//! its expectation changes to the oracle's answer in the same PR.
use std::path::{Path, PathBuf};

struct Case {
    id: String,
    data: String,
    /// `Err(text)` = expected error; `Ok(rows)` = expected rows, as `fmt_row` prints them, sorted.
    expect: Result<Vec<String>, String>,
    sql: String,
}

fn parse(text: &str) -> Vec<Case> {
    let mut out = Vec::new();
    let (mut id, mut data, mut expect, mut sql) = (None, None, None, String::new());
    for line in text.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("-- id:") {
            id = Some(v.trim().to_string());
        } else if let Some(v) = l.strip_prefix("-- data:") {
            data = Some(v.trim().to_string());
        } else if let Some(v) = l.strip_prefix("-- expect-error:") {
            expect = Some(Err(v.trim().to_string()));
        } else if let Some(v) = l.strip_prefix("-- expect-rows:") {
            let mut rows: Vec<String> = v.split('|').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
            rows.sort();
            expect = Some(Ok(rows));
        } else if l.starts_with("--") || (l.is_empty() && sql.is_empty()) {
            continue;
        } else {
            sql.push_str(line);
            sql.push('\n');
            if l.ends_with(';') {
                let id = id.take().expect("statement without `-- id:`");
                out.push(Case {
                    data: data.take().unwrap_or_else(|| panic!("{id}: no `-- data:`")),
                    expect: expect.take().unwrap_or_else(|| panic!("{id}: no `-- expect-error:` or `-- expect-rows:`")),
                    sql: std::mem::take(&mut sql).trim().to_string(),
                    id,
                });
            }
        }
    }
    assert!(sql.trim().is_empty(), "unterminated statement: {sql}");
    out
}

fn data_dir(kind: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("regression-data").join(kind);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("t.csv"), "col,x,a,b\na,1,1,1\nb,-2,1,2\na,3,2,1\n,,,\n").unwrap();
    match kind {
        "default" => {
            std::fs::write(d.join("schema.toml"), "[t]\ncol = \"str\"\nx = \"i64\"\na = \"i64\"\nb = \"i64\"\n")
                .unwrap()
        }
        "no-schema" => {
            let _ = std::fs::remove_file(d.join("schema.toml"));
        }
        k => panic!("unknown data kind {k}"),
    }
    d
}

fn fmt_row(r: &[engine::types::Value]) -> String {
    use engine::types::Value as V;
    r.iter()
        .map(|v| match v {
            V::Null => "NULL".to_string(),
            V::I64(x) => x.to_string(),
            V::F64(x) => x.to_string(),
            V::Str(s) => s.clone(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The outcome as the file records it: `Err(text)`, or `Ok(rows)` formatted and sorted.
fn outcome(d: &Path, sql: &str, no_opt: bool) -> Result<Vec<String>, String> {
    match engine::run_collect_opts(d, sql, &engine::Options { no_opt, ..Default::default() }) {
        Ok((_, rows)) => {
            let mut v: Vec<String> = rows.iter().map(|r| fmt_row(r)).collect();
            v.sort();
            Ok(v)
        }
        Err(e) => Err(e.0),
    }
}

#[test]
fn demo_candidates_fail_with_the_recorded_text() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/regression/demo_candidates.sql");
    let cases = parse(&std::fs::read_to_string(&path).unwrap());
    assert!(cases.len() >= 15, "only {} cases parsed", cases.len());
    let mut bad = Vec::new();
    for c in &cases {
        let d = data_dir(&c.data);
        let got = outcome(&d, &c.sql, false);
        let got_ref = outcome(&d, &c.sql, true);
        let want = c.expect.clone().map_err(|e| e.replace("{data}", &d.display().to_string()));
        eprintln!("{}: {}\n    -> {got:?}", c.id, c.sql);
        if got != want || got_ref != got {
            bad.push(format!(
                "{}: {}\n    want:     {want:?}\n    default:  {got:?}\n    --no-opt: {got_ref:?}",
                c.id, c.sql
            ));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
