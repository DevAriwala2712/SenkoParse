//! Regression files in `tests/regression/`. Today: `demo_candidates.sql` (production-readiness.md §2.3), the query
//! shapes that may have been the failed demo aggregation. Each statement must fail, with no rows, with exactly
//! the error text recorded in the file, on the default and `--no-opt` paths. When a candidate becomes supported,
//! its expectation changes to the oracle's answer in the same PR.
use std::path::{Path, PathBuf};

struct Case {
    id: String,
    data: String,
    expect: String,
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
            expect = Some(v.trim().to_string());
        } else if l.starts_with("--") || (l.is_empty() && sql.is_empty()) {
            continue;
        } else {
            sql.push_str(line);
            sql.push('\n');
            if l.ends_with(';') {
                let id = id.take().expect("statement without `-- id:`");
                out.push(Case {
                    data: data.take().unwrap_or_else(|| panic!("{id}: no `-- data:`")),
                    expect: expect.take().unwrap_or_else(|| panic!("{id}: no `-- expect-error:`")),
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

fn error_of(d: &Path, sql: &str, no_opt: bool) -> String {
    match engine::run_collect_opts(d, sql, &engine::Options { no_opt, ..Default::default() }) {
        Ok((_, rows)) => format!("<no error: {} rows>", rows.len()),
        Err(e) => e.0,
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
        let got = error_of(&d, &c.sql, false);
        let got_ref = error_of(&d, &c.sql, true);
        let want = c.expect.replace("{data}", &d.display().to_string());
        eprintln!("{}: {}\n    -> {got}", c.id, c.sql);
        if got != want || got_ref != got {
            bad.push(format!(
                "{}: {}\n    want:     {want}\n    default:  {got}\n    --no-opt: {got_ref}",
                c.id, c.sql
            ));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
