//! Phase 0 audit of the CLI-only rows of docs/sql-surface.md (the SQL rows are in `tests/surface_audit.rs`).
//! Each probe runs the real `qe` binary and pins how the missing feature fails today: a non-zero exit, nothing
//! on stdout, and the error text below.
use std::io::Write;
use std::process::{Command, Stdio};

fn data_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("qe-surface-audit-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("schema.toml"), "[t]\nid = \"i64\"\nname = \"str\"\n").unwrap();
    std::fs::write(d.join("t.csv"), "id,name\n1,a\n2,b\n").unwrap();
    d
}

/// Runs `qe --data <dir> args...` with `stdin` piped in; returns (exit ok, stdout, stderr).
fn qe(args: &[&str], stdin: &[u8]) -> (bool, String, String) {
    let mut ch = Command::new(env!("CARGO_BIN_EXE_qe"))
        .arg("--data")
        .arg(data_dir())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut si = ch.stdin.take().unwrap();
    let input = stdin.to_vec();
    let h = std::thread::spawn(move || {
        let _ = si.write_all(&input);
    });
    let o = ch.wait_with_output().unwrap();
    let _ = h.join();
    (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn fails_with(id: &str, args: &[&str], stdin: &[u8], want: &str) {
    let (ok, out, err) = qe(args, stdin);
    eprintln!("{id}: qe {}\n    -> {}", args.join(" "), err.trim());
    assert!(!ok, "{id}: expected failure, got success with stdout {out:?}");
    assert!(out.is_empty(), "{id}: rows on stdout despite the error: {out:?}");
    assert!(err.contains(want), "{id}: stderr {err:?} does not contain {want:?}");
}

#[test]
fn cli_only_rows_fail_today() {
    let q = "SELECT id FROM t";
    // SRC-04: `--table` is the pretty-print flag today. `--table t=other.csv` does NOT fail: `t=other.csv` is taken
    // as the SQL text and then silently replaced by the next positional argument (BUG-0001 below). The query runs
    // against t.csv in --data, prints a pretty table, and the requested file is ignored.
    let (ok, out, err) = qe(&["--table", "t=other.csv", q], b"");
    eprintln!("SRC-04: qe --table t=other.csv {q}\n    -> ok={ok}, stderr {err:?}");
    assert!(ok && out.contains("2 rows in set"), "SRC-04 behaviour changed: ok={ok} stdout={out:?} stderr={err:?}");
    // SRC-05, SRC-07, OUT-03, INT-01, INT-03: flags that do not exist
    fails_with("SRC-05", &["--schema", "s.toml", q], b"", "unknown option --schema");
    fails_with("SRC-07", &["--fifo", "u=/tmp/x", q], b"", "unknown option --fifo");
    fails_with("OUT-03", &["--format", "json", q], b"", "unknown option --format");
    fails_with("INT-01", &["--schema-only", q], b"", "unknown option --schema-only");
    fails_with("INT-03", &["--stats", q], b"", "unknown option --stats");
    // SRC-06: gzip bytes on stdin are read as CSV text, not decompressed
    let gz: &[u8] = &[0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, b'\n', b'1', b',', b'a', b'\n'];
    let (ok, out, err) = qe(&["--stdin", "t", q], gz);
    eprintln!("SRC-06: gzip on --stdin -> ok={ok} stdout={out:?} stderr={}", err.trim());
    assert!(!ok && out.is_empty(), "SRC-06: gzip input was not rejected: stdout {out:?}");
}

/// ERR-02: errors carry a byte offset for syntax errors only; no error class and no `line:col` marker.
#[test]
fn err02_errors_have_no_class_or_line_col() {
    let (ok, _, err) = qe(&["SELECT id FROM t WHERE"], b"");
    assert!(!ok);
    assert!(err.contains("at offset"), "{err}");
    assert!(!err.contains("-->"), "{err}");
    let (ok, _, err) = qe(&["SELECT nope FROM t"], b"");
    assert!(!ok);
    assert!(!err.contains("offset") && !err.contains("-->"), "{err}");
}

/// BUG-0001 (found by the Phase 0 audit, not fixed in Phase 0): a second positional argument silently replaces the
/// first, so `qe "SELECT a ..." "SELECT b ..."` runs only the second query and exits 0. Expected: an error that
/// names the extra argument. Remove `#[ignore]` when fixed.
#[test]
#[ignore = "BUG-0001: extra positional arguments are silently dropped"]
fn bug_0001_extra_positional_argument_is_an_error() {
    let (ok, out, err) = qe(&["SELECT id FROM t WHERE id = 1", "SELECT id FROM t WHERE id = 2"], b"");
    assert!(!ok, "two SQL arguments were accepted; stdout {out:?}");
    assert!(out.is_empty(), "{out:?}");
    assert!(err.contains("SELECT id FROM t WHERE id = 1"), "error should name the dropped argument: {err:?}");
}
