//! Parallel-scan properties: exact chunk splitting, thread-count independence, canonical errors.
use engine::types::Value;
use engine::Options;
use std::path::PathBuf;

fn dir(name: &str, csv: &str, schema: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("par-data").join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("t.csv"), csv).unwrap();
    std::fs::write(d.join("schema.toml"), schema).unwrap();
    d
}

fn run(d: &PathBuf, sql: &str, o: Options) -> Result<Vec<Vec<Value>>, String> {
    engine::run_collect_opts(d, sql, &o).map(|r| r.1).map_err(|e| e.0)
}

fn opts(threads: usize, chunk: usize) -> Options {
    Options { threads: Some(threads), chunk_bytes: Some(chunk), ..Default::default() }
}

const T3: &str = "[t]\nid = \"i64\"\ns = \"str\"\nv = \"i64\"\n";

/// A chunk boundary may fall on every byte offset of a file full of quoted commas, doubled quotes,
/// embedded LF and CRLF, and the result must equal the single-chunk reference.
#[test]
fn every_chunk_size_gives_the_reference_result() {
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
    let d = dir("sweep", &csv, T3);
    let queries = [
        "SELECT id, s, v FROM t ORDER BY id",
        "SELECT COUNT(*), SUM(v), COUNT(s) FROM t",
        "SELECT s, COUNT(*), SUM(v) FROM t GROUP BY s ORDER BY 1",
        "SELECT id FROM t WHERE s IS NULL ORDER BY 1",
    ];
    let reference = Options { no_opt: true, ..Default::default() };
    for q in queries {
        let want = run(&d, q, reference.clone()).unwrap();
        for chunk in (1..=70).chain([97, 128, 255, 1000, 100_000]) {
            for threads in [1, 4] {
                let got = run(&d, q, opts(threads, chunk)).unwrap_or_else(|e| panic!("chunk {chunk}: {q}: {e}"));
                assert_eq!(got, want, "chunk_bytes={chunk} threads={threads}: {q}");
            }
        }
    }
}

/// Float sums must be bit-identical for any thread count (chunking depends only on chunk_bytes).
#[test]
fn float_results_do_not_depend_on_thread_count() {
    // deterministic pseudo-random values with wide magnitude spread, so summation order matters
    let mut x: u64 = 0x1234_5678_9ABC_DEF1;
    let mut csv = String::from("id,s,v\n");
    let mut sch = String::from("[t]\nid = \"i64\"\ns = \"str\"\nv = \"f64\"\n");
    sch.push('\n');
    for i in 0..30000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let v = ((x % 2_000_001) as f64 - 1_000_000.0) * 10f64.powi((x >> 40) as i32 % 9 - 4);
        csv.push_str(&format!("{i},g{},{v}\n", x % 37));
    }
    let d = dir("det", &csv, &sch);
    let queries = [
        "SELECT SUM(v), AVG(v), MIN(v), MAX(v), COUNT(*) FROM t",
        "SELECT s, SUM(v), AVG(v) FROM t GROUP BY s ORDER BY 1",
        "SELECT s, SUM(v) FROM t GROUP BY s", // unordered: group order must also be stable
    ];
    for q in queries {
        let base = run(&d, q, opts(1, 4096)).unwrap();
        for threads in [2, 3, 5, 8, 11] {
            let got = run(&d, q, opts(threads, 4096)).unwrap();
            assert_eq!(got, base, "threads={threads}: {q} (must be bit-identical)");
        }
    }
}

/// An error found in a late chunk is reported with the reference path's text (row number included).
#[test]
fn errors_in_parallel_chunks_use_reference_text() {
    let mut csv = String::from("id,s,v\n");
    for i in 0..500 {
        csv.push_str(&format!("{i},a,{i}\n"));
    }
    csv.push_str("500,a,notanumber\n");
    for i in 501..900 {
        csv.push_str(&format!("{i},a,{i}\n"));
    }
    let d = dir("err", &csv, T3);
    let reference = run(&d, "SELECT SUM(v) FROM t", Options { no_opt: true, ..Default::default() }).unwrap_err();
    assert!(reference.contains("row 502"), "{reference}");
    for chunk in [64, 1000, 5000] {
        assert_eq!(run(&d, "SELECT SUM(v) FROM t", opts(4, chunk)).unwrap_err(), reference);
    }
    // a stray quote inside an unquoted field is rejected consistently
    let d = dir("err2", "id,s,v\n1,ab\"c,2\n", T3);
    let e = run(&d, "SELECT COUNT(*) FROM t WHERE s IS NOT NULL", opts(2, 5)).unwrap_err();
    assert!(e.contains("unexpected quote"), "{e}");
}

/// Single integer-like group keys (i64, decimal, date): thousands of groups (forces table growth),
/// NULL keys, and unordered output whose group order must equal the reference (first-seen order).
#[test]
fn integer_like_group_keys_match_reference_including_order() {
    let mut x: u64 = 0xDEAD_BEEF_1234_5678;
    let mut csv = String::from("id,g,p,d,v\n");
    for i in 0..6000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let g = if x % 11 == 0 { String::new() } else { ((x >> 8) % 3000).to_string() }; // NULLs + ~3000 groups
        let p = if x % 13 == 0 { String::new() } else { format!("{}.{:02}", (x >> 20) % 50, (x >> 30) % 100) };
        let d = if x % 17 == 0 { String::new() } else { format!("2021-{:02}-{:02}", 1 + (x >> 12) % 12, 1 + (x >> 24) % 28) };
        let v = if x % 7 == 0 { String::new() } else { ((x >> 33) % 1000).to_string() };
        csv.push_str(&format!("{i},{g},{p},{d},{v}\n"));
    }
    let schema = "[t]\nid = \"i64\"\ng = \"i64\"\np = \"decimal(2)\"\nd = \"date\"\nv = \"i64\"\n";
    let d = dir("intkeys", &csv, schema);
    let queries = [
        "SELECT g, COUNT(*), SUM(v), MIN(v), MAX(v), COUNT(DISTINCT id) FROM t GROUP BY g",
        "SELECT p, COUNT(*), SUM(v), AVG(v) FROM t GROUP BY p",
        "SELECT d, COUNT(*), SUM(v) FROM t GROUP BY d",
        "SELECT g, COUNT(*) FROM t WHERE v IS NOT NULL GROUP BY g HAVING COUNT(*) > 1",
        "SELECT id, COUNT(*) FROM t GROUP BY id", // 6000 distinct keys
    ];
    let reference = Options { no_opt: true, ..Default::default() };
    for q in queries {
        let want = run(&d, q, reference.clone()).unwrap();
        for (threads, chunk) in [(1, 100_000_000), (1, 3000), (4, 3000), (4, 400), (7, 77)] {
            let got = run(&d, q, opts(threads, chunk)).unwrap_or_else(|e| panic!("{q}: {e}"));
            assert_eq!(got, want, "threads={threads} chunk={chunk}: {q} (rows and group order must match)");
        }
    }
}

fn sorted(mut r: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    r.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    r
}

/// Partition-wise merge (forced with `agg_partition_groups = 0`): same groups and values as the reference
/// (order may differ: partition order), identical for every thread count, NULL / decimal / date keys included.
#[test]
fn partitioned_merge_matches_reference_and_is_thread_independent() {
    let mut x: u64 = 0xABCD_EF01_2345_6789;
    let mut csv = String::from("id,g,p,d,v,f\n");
    for i in 0..8000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let g = if x % 9 == 0 { String::new() } else { ((x >> 8) % 4000).to_string() };
        let p = if x % 13 == 0 { String::new() } else { format!("{}.{:02}", (x >> 20) % 40, (x >> 30) % 100) };
        let d = if x % 17 == 0 { String::new() } else { format!("2022-{:02}-{:02}", 1 + (x >> 12) % 12, 1 + (x >> 24) % 28) };
        let v = if x % 7 == 0 { String::new() } else { ((x >> 33) % 1000).to_string() };
        let f = ((x >> 11) % 100000) as f64 / 7.0 * if x % 2 == 0 { 1e6 } else { 1e-3 };
        csv.push_str(&format!("{i},{g},{p},{d},{v},{f}\n"));
    }
    let schema = "[t]\nid = \"i64\"\ng = \"i64\"\np = \"decimal(2)\"\nd = \"date\"\nv = \"i64\"\nf = \"f64\"\n";
    let d = dir("partmerge", &csv, schema);
    let queries = [
        "SELECT g, COUNT(*), SUM(v), MIN(v), MAX(v), COUNT(DISTINCT id), AVG(v) FROM t GROUP BY g",
        "SELECT p, COUNT(*), SUM(v) FROM t GROUP BY p",
        "SELECT d, COUNT(*), SUM(v), MAX(f) FROM t GROUP BY d",
        "SELECT id, SUM(f) FROM t GROUP BY id",
        "SELECT g, SUM(f), AVG(f) FROM t GROUP BY g",
    ];
    let forced = |threads: usize, chunk: usize| Options {
        threads: Some(threads),
        chunk_bytes: Some(chunk),
        agg_partition_groups: Some(0),
        ..Default::default()
    };
    for q in queries {
        let want = sorted(run(&d, q, Options { no_opt: true, ..Default::default() }).unwrap());
        let base = run(&d, q, forced(1, 700)).unwrap_or_else(|e| panic!("{q}: {e}"));
        for threads in [1, 2, 5, 11] {
            let got = run(&d, q, forced(threads, 700)).unwrap();
            assert_eq!(got, base, "threads={threads}: {q} (must be identical, including order and float bits)");
        }
        // vs reference: same groups; integer results exact, float results within 1e-9 relative
        let got = sorted(base);
        assert_eq!(got.len(), want.len(), "{q}");
        for (a, b) in got.iter().zip(&want) {
            for (x, y) in a.iter().zip(b) {
                match (x, y) {
                    (Value::F64(x), Value::F64(y)) => assert!((x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0), "{q}: {x} vs {y}"),
                    _ => assert_eq!(x, y, "{q}"),
                }
            }
        }
    }
}

/// EXP-007: each worker reuses one aggregation table across many chunks. With hundreds of chunks and 1, 2 or 4
/// workers, the chunks-per-worker split differs wildly, so any state left over from a previous chunk (stale keys,
/// NULL group, hash index, DISTINCT sets) or any order-dependence would show up. Results, including float sums
/// and the order of unordered groups, must be bit-identical for 1, 2 and 4 threads, on the pairwise-tree merge
/// and on the forced partitioned merge; exact (integer) aggregates must also equal the single-chunk result.
#[test]
fn table_reuse_gives_identical_results_for_1_2_and_4_threads() {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut csv = String::from("k,s,w,v,n\n");
    let sch = "[t]\nk = \"i64\"\ns = \"str\"\nw = \"i64\"\nv = \"f64\"\nn = \"i64\"\n";
    for _ in 0..40000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let k = if x % 11 == 0 { String::new() } else { (x % 3001).to_string() }; // ~3000 int groups + NULL group
        let s = if (x >> 8) % 13 == 0 { String::new() } else { format!("g{}", (x >> 12) % 29) }; // 29 str groups + NULL
        let w = (x >> 20) % 7;
        let v = ((x % 2_000_001) as f64 - 1_000_000.0) * 10f64.powi((x >> 40) as i32 % 9 - 4);
        let n = (x >> 30) % 1000;
        csv.push_str(&format!("{k},{s},{w},{v},{n}\n"));
    }
    let d = dir("reuse", &csv, sch);
    let float_queries = [
        "SELECT k, COUNT(*), SUM(v), AVG(v), MIN(v), MAX(v) FROM t GROUP BY k", // int key, unordered
        "SELECT s, SUM(v), AVG(v) FROM t GROUP BY s",                           // string key, unordered
        "SELECT s, w, SUM(v), COUNT(DISTINCT k) FROM t GROUP BY s, w",          // composite key + DISTINCT
        "SELECT SUM(v), AVG(v), COUNT(*) FROM t",                               // global aggregate
    ];
    let exact_queries = [
        "SELECT k, COUNT(*), SUM(n), MIN(s), MAX(s), COUNT(DISTINCT w) FROM t GROUP BY k",
        "SELECT s, COUNT(*), SUM(n), COUNT(DISTINCT k) FROM t GROUP BY s",
        "SELECT w, COUNT(*), SUM(n) FROM t GROUP BY w",
    ];
    let forced = |threads: usize| Options {
        threads: Some(threads),
        chunk_bytes: Some(600),
        agg_partition_groups: Some(0),
        ..Default::default()
    };
    for q in float_queries.iter().chain(exact_queries.iter()) {
        let base = run(&d, q, opts(1, 600)).unwrap();
        let base_forced = run(&d, q, forced(1)).unwrap();
        for threads in [2, 4] {
            assert_eq!(run(&d, q, opts(threads, 600)).unwrap(), base, "tree merge, threads={threads}: {q}");
            assert_eq!(run(&d, q, forced(threads)).unwrap(), base_forced, "partitioned merge, threads={threads}: {q}");
        }
    }
    let canon = |mut r: Vec<Vec<Value>>| {
        r.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        r
    };
    for q in exact_queries {
        let one_chunk = canon(run(&d, q, opts(1, 100_000_000)).unwrap());
        let reference = canon(run(&d, q, Options { no_opt: true, ..Default::default() }).unwrap());
        assert_eq!(one_chunk, reference, "{q}");
        for threads in [1, 2, 4] {
            assert_eq!(canon(run(&d, q, opts(threads, 600)).unwrap()), reference, "threads={threads} vs reference: {q}");
        }
    }
}

/// EXP-009 / ADR-0004: COUNT(DISTINCT) sets live in per-worker tables (one per concurrently running chunk task) and
/// are unioned at the end, while every other aggregate keeps the fixed per-chunk merge order. With about a thousand
/// chunks and 1, 2 or 4 threads the rows reach the worker tables in a different split each run, so a wrong union, a
/// lost worker table or DISTINCT states landing in the wrong output column would change a count. Results, including
/// `f64` sums in the same query and the order of unordered groups, must be bit-identical for 1, 2 and 4 threads on the
/// tree merge and the forced partitioned merge, and equal (as multisets) to the serial `--no-opt` path.
#[test]
fn distinct_per_worker_gives_identical_results_for_1_2_and_4_threads() {
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let mut csv = String::from("k,s,d,ds,v\n");
    let sch = "[t]\nk = \"i64\"\ns = \"str\"\nd = \"i64\"\nds = \"str\"\nv = \"f64\"\n";
    for _ in 0..40000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let k = if x % 17 == 0 { String::new() } else { (x % 1201).to_string() }; // ~1200 int groups + NULL group
        let s = if (x >> 8) % 13 == 0 { String::new() } else { format!("g{}", (x >> 12) % 23) }; // 23 str groups + NULL
        let d = if (x >> 16) % 9 == 0 { String::new() } else { (((x >> 20) % 5000) as i64 - 2500).to_string() }; // NULLs, negatives
        let ds = if (x >> 24) % 7 == 0 { String::new() } else { format!("v{}", (x >> 28) % 3000) };
        let v = ((x % 2_000_001) as f64 - 1_000_000.0) * 10f64.powi((x >> 40) as i32 % 9 - 4);
        csv.push_str(&format!("{k},{s},{d},{ds},{v}\n"));
    }
    let d = dir("distinct-worker", &csv, sch);
    let queries = [
        // DISTINCT between float aggregates: output columns must come back in query order
        "SELECT s, SUM(v), COUNT(DISTINCT d), AVG(v), COUNT(DISTINCT ds), MIN(v) FROM t GROUP BY s",
        "SELECT k, COUNT(DISTINCT d), SUM(v), COUNT(*) FROM t GROUP BY k",
        "SELECT s, k, COUNT(DISTINCT ds), SUM(v) FROM t GROUP BY s, k",
        "SELECT COUNT(DISTINCT d), SUM(v), COUNT(DISTINCT ds), COUNT(DISTINCT k) FROM t", // global
        "SELECT s, COUNT(DISTINCT d), COUNT(DISTINCT k) FROM t GROUP BY s",               // DISTINCT only
        "SELECT k, COUNT(DISTINCT v) FROM t GROUP BY k",                                  // DISTINCT over f64
    ];
    let forced = |threads: usize| Options {
        threads: Some(threads),
        chunk_bytes: Some(600),
        agg_partition_groups: Some(0),
        ..Default::default()
    };
    let canon = |mut r: Vec<Vec<Value>>| {
        r.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        r
    };
    // Drop the float aggregates' columns (SUM/AVG/MIN of v) so the rest can be compared with the serial path exactly.
    let exact_cols = |q: &str, r: Vec<Vec<Value>>| -> Vec<Vec<Value>> {
        let sel = q.split(" FROM ").next().unwrap().trim_start_matches("SELECT ");
        let keep: Vec<bool> = sel.split(", ").map(|c| !c.contains("(v)")).collect();
        r.into_iter().map(|row| row.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(v, _)| v).collect()).collect()
    };
    for q in queries {
        let base = run(&d, q, opts(1, 600)).unwrap();
        let base_forced = run(&d, q, forced(1)).unwrap();
        for threads in [2, 4] {
            assert_eq!(run(&d, q, opts(threads, 600)).unwrap(), base, "tree merge, threads={threads}: {q}");
            assert_eq!(run(&d, q, forced(threads)).unwrap(), base_forced, "partitioned merge, threads={threads}: {q}");
        }
        let reference = canon(exact_cols(q, run(&d, q, Options { no_opt: true, ..Default::default() }).unwrap()));
        for threads in [1, 2, 4] {
            for (name, o) in [("tree", opts(threads, 600)), ("partitioned", forced(threads))] {
                let got = canon(exact_cols(q, run(&d, q, o).unwrap()));
                assert_eq!(got, reference, "{name} merge, threads={threads} vs --no-opt: {q}");
            }
        }
    }
}

/// EXP-008: the integer-key join builds its table in parallel (64 hash partitions once the build side has 65,536 rows
/// or more). Results, the row order of unordered output, and `f64` sums must be bit-identical for 1, 2 and 4 threads,
/// with duplicate build keys, NULL keys, negative and sparse keys, inner and LEFT joins; and equal to the serial
/// `--no-opt` byte-key join as a multiset.
#[test]
fn integer_join_results_do_not_depend_on_thread_count() {
    let mut x: u64 = 0xA5A5_1234_DEAD_BEEF;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut key = |r: u64| -> String {
        match r % 100 {
            0..=3 => String::new(), // NULL
            4..=29 => ((r >> 8) % 5000).to_string(),
            30..=49 => (-(((r >> 8) % 5000) as i64)).to_string(),
            50..=69 => (((r >> 8) % 3000) << 40).to_string(), // sparse
            _ => ((r >> 8) % 20_000).to_string(),
        }
    };
    let mut dcsv = String::from("k,w\n");
    for _ in 0..70_000 {
        let r = next();
        dcsv.push_str(&format!("{},{}\n", key(r), (r >> 40) % 1000));
    }
    let mut fcsv = String::from("id,k,v\n");
    for i in 0..30_000 {
        let r = next();
        let v = ((r % 2_000_001) as f64 - 1_000_000.0) * 10f64.powi((r >> 40) as i32 % 9 - 4);
        fcsv.push_str(&format!("{i},{},{v}\n", key(r.rotate_left(17))));
    }
    let d = dir("intjoin", &dcsv, "[d]\nk = \"i64\"\nw = \"i64\"\n\n[f]\nid = \"i64\"\nk = \"i64\"\nv = \"f64\"\n");
    std::fs::write(d.join("f.csv"), &fcsv).unwrap();
    std::fs::rename(d.join("t.csv"), d.join("d.csv")).unwrap();
    let queries = [
        "SELECT COUNT(*), SUM(f.v), AVG(f.v), SUM(d.w) FROM f JOIN d ON f.k = d.k",
        "SELECT COUNT(*), SUM(f.v), COUNT(d.w) FROM f LEFT JOIN d ON f.k = d.k",
        "SELECT f.id, d.w FROM f JOIN d ON f.k = d.k", // unordered: output order must be stable too
        "SELECT d.w, f.id FROM d JOIN f ON f.k = d.k",
        "SELECT f.id, d.w FROM f LEFT JOIN d ON f.k = d.k",
        "SELECT d.w, COUNT(*), SUM(f.v) FROM f JOIN d ON f.k = d.k GROUP BY d.w",
    ];
    let o = |threads: usize| opts(threads, 64 * 1024);
    for q in queries {
        let base = run(&d, q, o(1)).unwrap();
        for threads in [2, 4] {
            assert_eq!(run(&d, q, o(threads)).unwrap(), base, "threads={threads}: {q} (must be bit-identical, incl. row order)");
        }
        if !q.contains("AVG") && !q.contains("SUM(f.v)") {
            let canon = |mut r: Vec<Vec<Value>>| {
                r.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
                r
            };
            let reference = run(&d, q, Options { no_opt: true, ..Default::default() }).unwrap();
            assert_eq!(canon(base), canon(reference), "vs --no-opt byte-key join: {q}");
        }
    }
}
