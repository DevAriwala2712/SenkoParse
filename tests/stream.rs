//! `--stdin` (bounded-memory stream scan) must give the same answers and the same error text as reading the
//! same data by file name, for any read sizes and any block size.
mod common;
use common::Dribble;
use engine::types::Value;
use engine::Options;
use std::io::Read;
use std::path::PathBuf;

type Rows = Vec<Vec<Value>>;

fn dir(name: &str, files: &[(&str, &str)], schema: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("stream-data").join(name);
    std::fs::create_dir_all(&d).unwrap();
    for (n, c) in files {
        std::fs::write(d.join(format!("{n}.csv")), c).unwrap();
    }
    std::fs::write(d.join("schema.toml"), schema).unwrap();
    d
}

fn file_run(d: &PathBuf, sql: &str, o: Options) -> Result<Rows, String> {
    engine::run_collect_opts(d, sql, &o).map(|r| r.1).map_err(|e| e.0)
}

fn stream_run(d: &PathBuf, sql: &str, table: &str, input: Box<dyn Read + Send>, o: Options) -> Result<Rows, String> {
    engine::run_collect_stream(d, sql, &o, table, input).map(|r| r.1).map_err(|e| e.0)
}

/// File-mode error text mentions the file path; stream mode says `<stdin>`.
fn unlabel(d: &PathBuf, table: &str, e: String) -> String {
    e.replace(&d.join(format!("{table}.csv")).display().to_string(), "<stdin>")
}

fn sorted(mut r: Rows) -> Vec<String> {
    let mut v: Vec<String> = r.drain(..).map(|x| format!("{x:?}")).collect();
    v.sort();
    v
}

const T3: &str = "[t]\nid = \"i64\"\ns = \"str\"\nv = \"i64\"\n";

/// Stream `csv` as table `t` of `d` through every read pattern and block size; each must equal file mode.
fn check_all(d: &PathBuf, csv: &str, sql: &str, order_matters: bool) {
    let want = file_run(d, sql, Options { no_opt: true, ..Default::default() }).map_err(|e| unlabel(d, "t", e));
    let fast = file_run(d, sql, Options::default()).map_err(|e| unlabel(d, "t", e));
    assert_eq!(fast, want, "file mode default vs --no-opt: {sql}");
    let same = |got: &Result<Rows, String>, what: &str| match (got, &want) {
        (Ok(g), Ok(w)) if order_matters => assert_eq!(g, w, "{what}: {sql}"),
        (Ok(g), Ok(w)) => assert_eq!(sorted(g.clone()), sorted(w.clone()), "{what}: {sql}"),
        (g, w) => assert_eq!(g, w, "{what}: {sql}"),
    };
    let blocks = (1..=70usize).chain([97, 128, 255, 1000, 16 << 20]);
    for (i, block) in blocks.enumerate() {
        for no_opt in [false, true] {
            let o = Options { chunk_bytes: Some(block), no_opt, ..Default::default() };
            let got = stream_run(d, sql, "t", Box::new(Dribble::new(csv.as_bytes(), i as u64 + 1)), o.clone());
            same(&got, &format!("dribble, block {block}, no_opt {no_opt}"));
            if block % 7 == 1 {
                let got = stream_run(d, sql, "t", Box::new(Dribble::bytewise(csv.as_bytes())), o);
                same(&got, &format!("1 byte per read, block {block}, no_opt {no_opt}"));
            }
        }
    }
}

fn tricky_csv() -> String {
    let mut csv = String::from("id,s,v\r\n");
    for i in 0..40 {
        let s = match i % 5 {
            0 => "plain".to_string(),
            1 => "\"a,b\"".to_string(),
            2 => "\"line1\nline2\"".to_string(),
            3 => "\"say \"\"hi\"\"\r\nthere\"".to_string(),
            _ => "\"\"".to_string(), // empty quoted = NULL
        };
        csv.push_str(&format!("{i},{s},{}\r\n", i * 3));
    }
    csv.push_str("99,\"tail\nend\",1"); // no final newline, quoted newline in the last row
    csv
}

#[test]
fn tricky_csv_every_block_size_and_read_pattern() {
    let csv = tricky_csv();
    let d = dir("tricky", &[("t", &csv)], T3);
    for q in [
        "SELECT id, s, v FROM t",
        "SELECT COUNT(*), SUM(v), COUNT(s) FROM t",
        "SELECT s, COUNT(*), SUM(v) FROM t GROUP BY s",
        "SELECT id FROM t WHERE s IS NULL",
        "SELECT id, s FROM t WHERE v > 20 AND s IS NOT NULL LIMIT 4",
        "SELECT s, SUM(v) FROM t GROUP BY s HAVING COUNT(*) > 2 ORDER BY 1",
    ] {
        check_all(&d, &csv, q, true);
    }
}

#[test]
fn odd_inputs_match_file_mode() {
    let cases: Vec<(&str, String)> = vec![
        ("header_only", "id,s,v\n".into()),
        ("header_only_no_nl", "id,s,v".into()),
        ("empty", String::new()),
        ("bom", "\u{feff}id,s,v\n1,a,2\n2,\"b\nc\",3\n".into()),
        ("blank_lines", "id,s,v\n\n1,a,2\n\r\n\n2,b,3\n\n".into()),
        ("crlf_only", "id,s,v\r\n1,a,2\r\n2,b,3\r\n".into()),
        ("no_final_newline", "id,s,v\n1,a,2\n2,b,3".into()),
        ("ragged_row", "id,s,v\n1,a,2\n2,b\n3,c,4\n".into()),
        ("stray_quote", "id,s,v\n1,a,2\n2,b\"x,3\n3,\"c\nd\",4\n".into()),
        ("unterminated", "id,s,v\n1,a,2\n2,\"b,3\n3,c,4\n".into()),
    ];
    for (name, csv) in cases {
        let d = dir(&format!("odd-{name}"), &[("t", &csv)], T3);
        for q in ["SELECT id, s, v FROM t", "SELECT COUNT(*), SUM(v) FROM t", "SELECT s, COUNT(*) FROM t GROUP BY s"] {
            check_all(&d, &csv, q, true);
        }
    }
}

#[test]
fn bad_value_late_in_the_stream_reports_the_file_mode_row() {
    let mut csv = String::from("id,s,v\n");
    for i in 0..3000 {
        csv.push_str(&format!("{i},\"a\nb\",{i}\n"));
    }
    csv.push_str("3000,a,notanumber\n");
    for i in 3001..3500 {
        csv.push_str(&format!("{i},a,{i}\n"));
    }
    let d = dir("late-error", &[("t", &csv)], T3);
    let want = file_run(&d, "SELECT SUM(v) FROM t", Options { no_opt: true, ..Default::default() }).unwrap_err();
    assert!(want.contains("row 3002"), "{want}");
    for block in [1, 50, 4096, 16 << 20] {
        for no_opt in [false, true] {
            let o = Options { chunk_bytes: Some(block), no_opt, ..Default::default() };
            let got = stream_run(&d, "SELECT SUM(v) FROM t", "t", Box::new(Dribble::new(csv.as_bytes(), 9)), o).unwrap_err();
            assert_eq!(got, unlabel(&d, "t", want.clone()).to_string(), "block {block}");
        }
    }
}

const JOIN_SCHEMA: &str = "[t]\nid = \"i64\"\nk = \"i64\"\nv = \"i64\"\n\n[dim]\nk = \"i64\"\nname = \"str\"\n";

#[test]
fn joins_with_the_stream_as_probe_side() {
    let mut csv = String::from("id,k,v\n");
    for i in 0..400 {
        // some keys have no dim row, some are NULL
        let k = if i % 13 == 0 { String::new() } else { (i % 17).to_string() };
        csv.push_str(&format!("{i},{k},{}\n", i * 2));
    }
    let mut dim = String::from("k,name\n");
    for k in 0..12 {
        dim.push_str(&format!("{k},\"n,{k}\"\n"));
    }
    let d = dir("joins", &[("t", &csv), ("dim", &dim)], JOIN_SCHEMA);
    let queries = [
        // stream on the left of an INNER join
        "SELECT t.id, dim.name FROM t JOIN dim ON t.k = dim.k",
        // stream on the right of an INNER join (the planner must still keep it as the probe side)
        "SELECT t.id, dim.name FROM dim JOIN t ON dim.k = t.k",
        // LEFT JOIN with the stream on the left
        "SELECT t.id, dim.name FROM t LEFT JOIN dim ON t.k = dim.k",
        "SELECT dim.name, COUNT(*), SUM(t.v) FROM t JOIN dim ON t.k = dim.k GROUP BY dim.name",
        "SELECT t.id FROM t LEFT JOIN dim ON t.k = dim.k WHERE dim.k IS NULL",
    ];
    for q in queries {
        let want = file_run(&d, q, Options { no_opt: true, ..Default::default() }).unwrap();
        for (i, block) in [1usize, 13, 64, 1000, 16 << 20].into_iter().enumerate() {
            for no_opt in [false, true] {
                let o = Options { chunk_bytes: Some(block), no_opt, ..Default::default() };
                let got = stream_run(&d, q, "t", Box::new(Dribble::new(csv.as_bytes(), i as u64 + 3)), o).unwrap();
                assert_eq!(sorted(got), sorted(want.clone()), "block {block} no_opt {no_opt}: {q}");
            }
        }
    }
}

#[test]
fn documented_stream_errors_are_clear() {
    let csv = "id,k,v\n1,1,1\n";
    let dim = "k,name\n1,a\n";
    let d = dir("errors", &[("t", csv), ("dim", dim)], JOIN_SCHEMA);
    let run = |q: &str, no_opt: bool| {
        stream_run(&d, q, "t", Box::new(std::io::Cursor::new(csv.as_bytes().to_vec())), Options { no_opt, ..Default::default() })
            .unwrap_err()
    };
    for no_opt in [false, true] {
        let e = run("SELECT dim.name FROM dim LEFT JOIN t ON dim.k = t.k", no_opt);
        assert!(e.contains("cannot be the right side of a LEFT JOIN"), "{e}");
        let e = run("SELECT a.id FROM t a JOIN t b ON a.id = b.id", no_opt);
        assert!(e.contains("appears 2 times") && e.contains("only once"), "{e}");
        let e = run("SELECT name FROM dim", no_opt);
        assert!(e.contains("is not used by the query"), "{e}");
    }
    let e = engine::run_collect_stream(&d, "SELECT 1 FROM t", &Options::default(), "nosuch", Box::new(std::io::empty()))
        .map(|_| ())
        .unwrap_err();
    assert!(e.0.contains("unknown table"), "{}", e.0);
}

/// An endless input must not hang or grow: LIMIT stops reading.
#[test]
fn limit_stops_an_endless_stream() {
    struct Endless(u64, Vec<u8>, usize);
    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.len() == self.2 {
                self.1 = format!("{},x,{}\n", self.0, self.0 * 2).into_bytes();
                self.0 += 1;
                self.2 = 0;
            }
            let n = buf.len().min(self.1.len() - self.2);
            buf[..n].copy_from_slice(&self.1[self.2..self.2 + n]);
            self.2 += n;
            Ok(n)
        }
    }
    let d = dir("endless", &[("t", "id,s,v\n")], T3);
    for no_opt in [false, true] {
        let (tx, rx) = std::sync::mpsc::channel();
        let d = d.clone();
        let input: Box<dyn Read + Send> = Box::new(std::io::Cursor::new(b"id,s,v\n".to_vec()).chain(Endless(0, vec![], 0)));
        std::thread::spawn(move || {
            let o = Options { no_opt, chunk_bytes: Some(4096), ..Default::default() };
            tx.send(stream_run(&d, "SELECT id FROM t LIMIT 3", "t", input, o)).unwrap();
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("LIMIT over an endless stream hung");
        assert_eq!(got.unwrap().len(), 3);
    }
}
