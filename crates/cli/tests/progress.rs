//! `--progress` must never change stdout, and must print nothing unless stderr is a terminal.
use std::io::Write;
use std::process::{Command, Stdio};

fn data_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("qe-progress-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("schema.toml"), "[t]\nid = \"i64\"\nname = \"str\"\n").unwrap();
    d
}

fn run(args: &[&str], envs: &[(&str, &str)]) -> (Vec<u8>, Vec<u8>) {
    let d = data_dir();
    let mut csv = String::from("id,name\n");
    for i in 0..50_000 {
        csv.push_str(&format!("{i},\"n,{}\"\n", i % 7));
    }
    let mut c = Command::new(env!("CARGO_BIN_EXE_qe"));
    c.arg("--data").arg(&d).args(["--stdin", "t"]).args(args).arg("SELECT id, name FROM t WHERE id < 10 OR id > 40000");
    for (k, v) in envs {
        c.env(k, v);
    }
    let mut ch = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut si = ch.stdin.take().unwrap();
    let h = std::thread::spawn(move || si.write_all(csv.as_bytes()).unwrap());
    let o = ch.wait_with_output().unwrap();
    let _ = h.join();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    (o.stdout, o.stderr)
}

#[test]
fn progress_leaves_stdout_byte_identical() {
    let (plain, plain_err) = run(&[], &[]);
    let (forced, forced_err) = run(&["--progress"], &[("QE_PROGRESS_FORCE", "1")]);
    assert!(plain_err.is_empty());
    assert!(!plain.is_empty());
    assert_eq!(plain, forced, "--progress changed stdout");
    let e = String::from_utf8_lossy(&forced_err);
    assert!(e.contains("[qe]") && e.contains("rows in") && e.contains("RSS"), "no status line: {e:?}");
}

#[test]
fn progress_is_silent_without_a_terminal() {
    let (plain, _) = run(&[], &[]);
    let (out, err) = run(&["--progress"], &[]);
    assert_eq!(plain, out);
    assert!(err.is_empty(), "status line printed to a non-terminal: {:?}", String::from_utf8_lossy(&err));
}
