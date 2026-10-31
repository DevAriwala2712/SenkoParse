//! Hand-written edge cases for the CSV layer, NULL semantics and error reporting.
use engine::types::Value;
use std::path::PathBuf;

fn dir(name: &str, files: &[(&str, &str)], schema: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("edge-data").join(name);
    std::fs::create_dir_all(&d).unwrap();
    for (n, c) in files {
        std::fs::write(d.join(n), c).unwrap();
    }
    std::fs::write(d.join("schema.toml"), schema).unwrap();
    d
}

fn q(d: &PathBuf, sql: &str) -> Vec<Vec<Value>> {
    engine::run_collect(d, sql).unwrap_or_else(|e| panic!("{sql}: {e}")).1
}
fn err(d: &PathBuf, sql: &str) -> String {
    match engine::run_collect(d, sql) {
        Ok(_) => panic!("expected error for: {sql}"),
        Err(e) => e.0,
    }
}
fn s(x: &str) -> Value {
    Value::Str(x.into())
}

const T: &str = "[t]\nid = \"i64\"\ns = \"str\"\nv = \"i64\"\n";

#[test]
fn quoting_newlines_crlf_and_missing_final_newline() {
    // quoted comma, doubled quote, embedded LF, embedded CRLF, CRLF row ends, no final newline
    let csv = "id,s,v\r\n1,\"a,b\",10\r\n2,\"say \"\"hi\"\"\",\r\n3,\"line1\nline2\",30\r\n4,\"x\r\ny\",40\r\n5,,50";
    let d = dir("quoting", &[("t.csv", csv)], T);
    let r = q(&d, "SELECT id, s FROM t ORDER BY id");
    assert_eq!(r[0][1], s("a,b"));
    assert_eq!(r[1][1], s("say \"hi\""));
    assert_eq!(r[2][1], s("line1\nline2"));
    assert_eq!(r[3][1], s("x\r\ny"));
    assert_eq!(r[4][1], Value::Null); // empty field = NULL
    assert_eq!(q(&d, "SELECT COUNT(*), COUNT(v), SUM(v) FROM t")[0], vec![Value::I64(5), Value::I64(4), Value::I64(130)]);
}

#[test]
fn null_semantics_in_aggregates_and_groups() {
    let csv = "id,s,v\n1,a,\n2,a,\n3,,5\n4,,\n";
    let d = dir("nulls", &[("t.csv", csv)], T);
    // SUM of all-NULL group is NULL; NULL keys form one group; AVG ignores NULLs
    let r = q(&d, "SELECT s, SUM(v), COUNT(*), COUNT(v), AVG(v) FROM t GROUP BY s ORDER BY 1");
    assert_eq!(r[0], vec![Value::Null, Value::I64(5), Value::I64(2), Value::I64(1), Value::F64(5.0)]);
    assert_eq!(r[1], vec![s("a"), Value::Null, Value::I64(2), Value::I64(0), Value::Null]);
}

#[test]
fn null_keys_never_join() {
    let a = "id,s,v\n1,x,1\n,y,2\n";
    let b = "k,w\n1,p\n,q\n";
    let d = dir("nulljoin", &[("t.csv", a), ("u.csv", b)], &format!("{T}\n[u]\nk = \"i64\"\nw = \"str\"\n"));
    assert_eq!(q(&d, "SELECT t.s, u.w FROM t JOIN u ON t.id = u.k").len(), 1);
    let r = q(&d, "SELECT t.s, u.w FROM t LEFT JOIN u ON t.id = u.k ORDER BY 1");
    assert_eq!(r, vec![vec![s("x"), s("p")], vec![s("y"), Value::Null]]);
}

#[test]
fn empty_table_aggregates() {
    let d = dir("empty", &[("t.csv", "id,s,v\n")], T);
    assert_eq!(q(&d, "SELECT COUNT(*), SUM(v), AVG(v), MIN(v) FROM t")[0], vec![Value::I64(0), Value::Null, Value::Null, Value::Null]);
    assert!(q(&d, "SELECT s, COUNT(*) FROM t GROUP BY s").is_empty());
}

#[test]
fn decimals_are_exact_and_avg_is_not_integer_division() {
    let d = dir("dec", &[("t.csv", "id,p\n1,0.10\n2,0.20\n3,-0.30\n4,1\n")], "[t]\nid = \"i64\"\np = \"decimal(2)\"\n");
    // 0.1 + 0.2 - 0.3 + 1 is exactly 1.00 with scaled integers
    assert_eq!(q(&d, "SELECT SUM(p) FROM t")[0][0], Value::Dec(100, 2));
    assert_eq!(q(&d, "SELECT AVG(id) FROM t")[0][0], Value::F64(2.5));
    assert_eq!(q(&d, "SELECT id / 2 FROM t WHERE id = 3")[0][0], Value::F64(1.5));
}

#[test]
fn sum_overflow_is_an_error() {
    let big = i64::MAX.to_string();
    let d = dir("ovf", &[("t.csv", &format!("id,s,v\n1,a,{big}\n2,a,{big}\n"))], T);
    assert!(err(&d, "SELECT SUM(v) FROM t").contains("overflow"));
    // the i128 accumulator makes cancelling sums fine
    let d2 = dir("ovf2", &[("t.csv", &format!("id,s,v\n1,a,{big}\n2,a,{big}\n3,a,-{big}\n"))], T);
    assert_eq!(q(&d2, "SELECT SUM(v) FROM t")[0][0], Value::I64(i64::MAX));
}

#[test]
fn errors_are_loud_and_specific() {
    let d = dir("errs", &[("t.csv", "id,s,v\n1,a,2\n2,b\n")], T);
    assert!(err(&d, "SELECT id FROM t").contains("expected 3 fields"));
    let d = dir("errs2", &[("t.csv", "id,s,v\n1,a,xyz\n")], T);
    assert!(err(&d, "SELECT v FROM t").contains("invalid i64"));
    // columns that are not referenced are not parsed, so a bad value there is not an error (projection pushdown)
    assert_eq!(q(&d, "SELECT COUNT(*) FROM t")[0][0], Value::I64(1));
    let d = dir("errs3", &[("t.csv", "id,s,v\n1,\"abc,2\n")], T);
    assert!(err(&d, "SELECT id FROM t").contains("unterminated"));
    let d = dir("errs4", &[("t.csv", "id,s,v\n1,a,2\n")], T);
    for (sql, want) in [
        ("SELECT id FROM t WHERE s LIKE 'a%'", "LIKE"),
        ("SELECT id FROM t RIGHT JOIN t u ON t.id = u.id", "RIGHT"),
        ("SELECT nope FROM t", "unknown column"),
        ("SELECT id FROM missing", "unknown table"),
        ("SELECT s, COUNT(*) FROM t", "GROUP BY"),
        ("SELECT id FROM t a JOIN t b ON a.id = b.id", "ambiguous"),
        ("SELECT SUM(s) FROM t", "not defined"),
        ("SELECT DISTINCT id FROM t", "DISTINCT"),
    ] {
        let e = err(&d, sql);
        assert!(e.contains(want), "{sql}: got `{e}`, wanted `{want}`");
    }
}

/// The default path and the --no-opt reference path must agree (and fail identically) on awkward files,
/// including quote-free files (fast path) and files whose only quote is on the last line.
#[test]
fn default_and_reference_paths_agree_on_tricky_files() {
    let files: &[(&str, &str)] = &[
        ("lf", "id,s,v\n1,a,10\n2,b,20\n3,c,30\n"),
        ("crlf", "id,s,v\r\n1,a,10\r\n2,b,20\r\n"),
        ("no_final_newline", "id,s,v\n1,a,10\n2,b,20"),
        ("blank_lines", "id,s,v\n1,a,10\n\n2,b,20\n\r\n3,c,30\n\n"),
        ("empty_fields", "id,s,v\n1,,\n,b,\n,,3\n"),
        ("quote_on_last_line_only", "id,s,v\n1,a,10\n2,b,20\n3,\"c,d\",30\n"),
        ("embedded_newline_last", "id,s,v\n1,a,10\n2,\"x\ny\",20"),
        ("ragged_short", "id,s,v\n1,a,10\n2,b\n3,c,30\n"),
        ("ragged_long", "id,s,v\n1,a,10\n2,b,20,99\n"),
        ("bad_number_unread_col", "id,s,v\n1,a,zz\n"),
        ("bad_number_read_col", "id,s,v\nq,a,1\n"),
        ("stray_cr_at_eof", "id,s,v\n1,a,10\r"),
        ("header_only", "id,s,v\n"),
    ];
    let queries = [
        "SELECT COUNT(*) FROM t",
        "SELECT SUM(id) FROM t",
        "SELECT SUM(v) FROM t",
        "SELECT s, COUNT(*), SUM(v) FROM t GROUP BY s ORDER BY 1",
        "SELECT id, s, v FROM t ORDER BY 1, 2, 3",
        "SELECT MIN(s), MAX(s) FROM t",
    ];
    for (name, csv) in files {
        let d = dir(&format!("agree-{name}"), &[("t.csv", csv)], T);
        for sql in queries {
            let a = engine::run_collect_opts(&d, sql, &engine::Options::default());
            let b = engine::run_collect_opts(&d, sql, &engine::Options { no_opt: true, ..Default::default() });
            // tiny chunks + threads: results and error text must still equal the reference
            let c = engine::run_collect_opts(
                &d,
                sql,
                &engine::Options { threads: Some(3), chunk_bytes: Some(7), ..Default::default() },
            );
            match (&b, &c) {
                (Ok(x), Ok(y)) => assert_eq!(x.1, y.1, "{name} (chunked): {sql}"),
                (Err(x), Err(y)) => assert_eq!(x.0, y.0, "{name} (chunked): {sql}: error text differs"),
                (x, y) => panic!("{name} (chunked): {sql}: reference={x:?} chunked={y:?}"),
            }
            match (a, b) {
                (Ok(x), Ok(y)) => assert_eq!(x.1, y.1, "{name}: {sql}"),
                (Err(x), Err(y)) => assert_eq!(x.0, y.0, "{name}: {sql}: error text differs"),
                (x, y) => panic!("{name}: {sql}: default={x:?} reference={y:?}"),
            }
        }
    }
}

#[test]
fn output_formatting_of_numbers_and_dates() {
    use engine::types::{fmt_date, fmt_dec};
    let dec = |v: i64, s: u8| {
        let mut b = Vec::new();
        fmt_dec(v, s, &mut b);
        String::from_utf8(b).unwrap()
    };
    assert_eq!(dec(0, 2), "0.00");
    assert_eq!(dec(5, 2), "0.05");
    assert_eq!(dec(-5, 2), "-0.05");
    assert_eq!(dec(123456, 2), "1234.56");
    assert_eq!(dec(-100, 2), "-1.00");
    assert_eq!(dec(7, 0), "7");
    assert_eq!(dec(i64::MIN, 0), "-9223372036854775808");
    assert_eq!(dec(i64::MIN, 18), "-9.223372036854775808");
    let date = |d: i32| {
        let mut b = Vec::new();
        fmt_date(d, &mut b);
        String::from_utf8(b).unwrap()
    };
    assert_eq!(date(0), "1970-01-01");
    assert_eq!(date(18262), "2020-01-01");
    assert_eq!(date(-1), "1969-12-31");
    // end to end through the CSV writer: extreme integers, negative zero-ish floats
    let d = dir("fmt", &[("t.csv", "id,s,v\n-9223372036854775808,a,9223372036854775807\n0,b,-1\n")], T);
    let mut p = engine::prepare(&d, "SELECT id, v FROM t ORDER BY 1", &engine::Options::default()).unwrap();
    let mut out = Vec::new();
    engine::output::write_csv(&p.names, &mut *p.root, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "id,v\n-9223372036854775808,9223372036854775807\n0,-1\n");
}
