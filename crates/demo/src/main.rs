//! qe-demo: live terminal dashboard, SenkoParse vs DuckDB on the same piped CSV. std + ANSI only.
//! Everything shown is measured now (live) or read back from a recording (replay). Nothing is simulated.
mod data;
mod json;
mod live;
mod model;
mod term;
mod ui;

use model::*;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

const COLS: [&str; 8] = ["id", "center_id", "user_id", "session_id", "device_id", "trace_id", "user_agent", "ts"];
const QE_ACCENT: &str = "38;5;43";
const DUCK_ACCENT: &str = "38;5;214";

fn die(msg: &str) -> ! {
    eprintln!("qe-demo: {msg}");
    std::process::exit(2);
}

struct Args {
    size: String,
    replay: Option<PathBuf>,
    record: Option<PathBuf>,
    data: PathBuf,
    qe: Option<PathBuf>,
    py: Option<String>,
    no_duckdb: bool,
}

fn parse_args() -> Args {
    let mut a = Args { size: "5x".into(), replay: None, record: None, data: PathBuf::from(".demo-data"), qe: None, py: std::env::var("DUCKDB_PY").ok(), no_duckdb: false };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        let mut val = |f: &str| it.next().unwrap_or_else(|| die(&format!("{f} needs a value")));
        match x.as_str() {
            "--size" => a.size = val("--size"),
            "--live" => a.replay = None,
            "--replay" => a.replay = Some(val("--replay").into()),
            "--record" => a.record = Some(val("--record").into()),
            "--data" => a.data = val("--data").into(),
            "--qe" => a.qe = Some(val("--qe").into()),
            "--duckdb-python" => a.py = Some(val("--duckdb-python")),
            "--no-duckdb" => a.no_duckdb = true,
            "-h" | "--help" => {
                println!("usage: qe-demo [--size 1x|2x|5x|10x] [--live | --replay FILE] [--record FILE] [--data DIR] [--qe PATH] [--duckdb-python PY] [--no-duckdb]\n  --replay accepts a recording made with --record (full time series) or bench/exp005 raw.jsonl (recorded summary only).");
                std::process::exit(0);
            }
            _ => die(&format!("unknown option {x} (try --help)")),
        }
    }
    if !["1x", "2x", "5x", "10x"].contains(&a.size.as_str()) {
        die("--size must be 1x, 2x, 5x or 10x");
    }
    a
}

fn find_root() -> PathBuf {
    let mut cands = vec![std::env::current_dir().unwrap_or_default()];
    if let Ok(e) = std::env::current_exe() {
        cands.extend(e.ancestors().skip(1).map(|p| p.to_path_buf()));
    }
    cands.into_iter().find(|d| d.join("bench/duckdb_run.py").exists()).unwrap_or_else(|| die("run from the repository (bench/duckdb_run.py not found)"))
}

fn query() -> String {
    format!("SELECT {} FROM a JOIN ids i ON a.center_id = i.id", COLS.iter().map(|c| format!("a.{c}")).collect::<Vec<_>>().join(", "))
}

fn pick_python(a: &Args, root: &Path) -> Option<String> {
    if a.no_duckdb {
        return None;
    }
    let mut cands: Vec<String> = a.py.iter().cloned().collect();
    cands.push(root.join(".demo-venv/bin/python").to_string_lossy().into_owned());
    cands.push("python3".into());
    cands.into_iter().find(|p| data::duckdb_version(p).is_some())
}

fn ensure_data(root: &Path, dir: &Path, size: &str) {
    let f = dir.join(format!("a_{size}.csv"));
    if f.exists() && dir.join("ids.csv").exists() && dir.join("schema.toml").exists() {
        return;
    }
    let k = size.trim_end_matches('x');
    eprintln!("qe-demo: generating EXP-005 data into {} (one-time; deterministic, seed 42)…", dir.display());
    let st = std::process::Command::new("python3")
        .arg(root.join("bench/gen_stream_data.py"))
        .args(["--out", dir.to_str().unwrap(), "--sizes", k])
        .status();
    if !st.is_ok_and(|s| s.success()) || !f.exists() {
        die("data generation failed (needs python3)");
    }
}

fn read_machine_from_raw(raw: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(raw.with_file_name("exp005-stream-output.txt")).ok()?;
    let m = text.lines().find(|l| l.starts_with("machine:"))?.trim_start_matches("machine: ").to_string();
    let d = text.lines().nth(1).unwrap_or("").to_string();
    Some((m, d))
}

fn main() {
    let a = parse_args();
    let root = find_root();
    let raw_default = root.join("docs/experiments/data/exp005-stream-raw.jsonl");
    let color = std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) && std::io::stdout().is_terminal();
    let interactive = std::io::stdout().is_terminal() && std::fs::File::open("/dev/tty").is_ok();

    let mut qe = Eng::new("qe", "SenkoParse", QE_ACCENT, Source::Live);
    let mut duck = Eng::new("duckdb", "DuckDB (piped)", DUCK_ACCENT, Source::Live);
    let mut meta = Meta {
        mode: "LIVE",
        note: String::new(),
        size: a.size.clone(),
        total_bytes: 0,
        machine: data::machine(),
        duckdb: String::new(),
        qe_settings: "--threads 1 (4 MiB blocks)".into(),
        query: "SELECT a.* FROM a JOIN ids i ON a.center_id = i.id  [8 cols; ids: 50 rows]".into(),
    };
    // Replay data, if any: (qe samples, qe end, duck samples, duck end)
    let mut replay: Option<Recording> = None;
    let mut reveal_clock = false;
    let mut runs: Vec<(live::Run, std::path::PathBuf)> = vec![];
    let mut work_root = PathBuf::new();
    let (mut have_duck, mut py) = (false, None);

    if let Some(path) = &a.replay {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(&format!("cannot read {}: {e}", path.display())));
        meta.mode = "REPLAY";
        if let Some(rec) = model::load_recording(&text) {
            meta.size = rec.meta.size.clone();
            meta.total_bytes = rec.meta.total_bytes;
            meta.machine = rec.meta.machine.clone();
            meta.duckdb = rec.meta.duckdb.clone();
            meta.qe_settings = rec.meta.qe_settings.clone();
            meta.query = rec.meta.query.clone();
            meta.note = "REPLAY: every number was measured live when this run was recorded".to_string();
            qe.source = Source::Replay;
            duck.source = if rec.duck.0.is_empty() { Source::Missing("no DuckDB data in this recording".into()) } else { Source::Replay };
            replay = Some(rec);
        } else {
            // EXP-005 raw.jsonl: summaries only. Be explicit that nothing in between is known.
            let q = data::recorded(path, "qe pipe", &a.size).unwrap_or_else(|| die("no qe rows for this size in the recording"));
            let d = data::recorded(path, "duckdb pipe", &a.size);
            let mb = text.lines().filter_map(json::parse_obj).find(|o| json::text(o, "size").as_deref() == Some(a.size.as_str())).and_then(|o| json::num(&o, "input_mb")).unwrap_or(0.0);
            meta.total_bytes = (mb * 1e6) as u64;
            if let Some((m, d)) = read_machine_from_raw(path) {
                meta.machine = m;
                meta.duckdb = d.split("duckdb ").nth(1).and_then(|s| s.split(';').next()).map(|v| format!("{v} (memory_limit=256MB, threads=1)")).unwrap_or_default();
            }
            meta.note = "REPLAY of EXP-005 summary: totals were recorded, curves in between were not".into();
            qe.source = Source::Summary(q);
            duck.source = d.map(Source::Summary).unwrap_or(Source::Missing("no DuckDB rows for this size".into()));
            reveal_clock = true;
        }
    } else {
        ensure_data(&root, &a.data, &a.size);
        let input = a.data.join(format!("a_{}.csv", a.size));
        meta.total_bytes = std::fs::metadata(&input).map(|m| m.len()).unwrap_or(0);
        py = pick_python(&a, &root);
        have_duck = py.is_some();
        match &py {
            Some(p) => meta.duckdb = format!("{} (memory_limit=256MB, threads=1)", data::duckdb_version(p).unwrap_or_default()),
            None => {
                meta.duckdb = "not installed here; DuckDB panel shows recorded numbers (2.0.0, memory_limit=256MB, threads=1)".into();
                duck.source = match data::recorded(&raw_default, "duckdb pipe", &a.size) {
                    Some(r) => Source::Summary(r),
                    None => Source::Missing("DuckDB not installed and no recording".into()),
                };
            }
        }
        let qe_bin = a.qe.clone().unwrap_or_else(|| root.join("target/release/qe"));
        if !qe_bin.exists() {
            die(&format!("{} not found; run `cargo build --release` (or ./demo.sh)", qe_bin.display()));
        }
        work_root = std::env::temp_dir().join(format!("qe-demo-{}", std::process::id()));
        let t0 = Instant::now();
        let data_abs = std::fs::canonicalize(&a.data).unwrap_or(a.data.clone());
        let qspec = live::Spec {
            program: qe_bin.to_string_lossy().into_owned(),
            args: ["--data", data_abs.to_str().unwrap(), "--stdin", "a", "--threads", "1", &query()].iter().map(|s| s.to_string()).collect(),
            input: &input,
            work: &work_root.join("qe"),
        };
        let r = live::Run::start(&qspec, t0).unwrap_or_else(|e| die(&e));
        runs.push((r, work_root.join("qe")));
        if let Some(p) = &py {
            let dspec = live::Spec {
                program: p.clone(),
                args: vec![root.join("bench/duckdb_run.py").to_string_lossy().into_owned(), "pipe".into(), data_abs.to_string_lossy().into_owned(), "".into(), "{TEMP}".into(), "{OUT}".into()],
                input: &input,
                work: &work_root.join("duckdb"),
            };
            let r = live::Run::start(&dspec, t0).unwrap_or_else(|e| die(&e));
            runs.push((r, work_root.join("duckdb")));
        }
    }

    let mut rec_out = a.record.as_ref().map(|p| std::fs::File::create(p).unwrap_or_else(|e| die(&format!("--record: {e}"))));
    if let Some(f) = rec_out.as_mut() {
        let _ = writeln!(f, "{}", model_meta(&meta));
    }

    let term = if interactive { Some(term::Term::enter()) } else { None };
    let quit = term.as_ref().map(|t| t.quit_flag());
    let (mut w, mut _h) = term::size();
    let mut last_size = Instant::now();
    let start = Instant::now();
    let tick = Duration::from_millis(100);
    let mut next = Instant::now();
    let mut rcur = (0usize, 0usize); // replay cursors
    let mut last_plain = 0u64;
    let mut finished_at: Option<Instant> = None;

    loop {
        let now_ms = start.elapsed().as_millis() as u64;
        if let Some(rec) = &replay {
            while rcur.0 < rec.qe.0.len() && rec.qe.0[rcur.0].t_ms <= now_ms {
                qe.samples.push(rec.qe.0[rcur.0].clone());
                rcur.0 += 1;
            }
            while rcur.1 < rec.duck.0.len() && rec.duck.0[rcur.1].t_ms <= now_ms {
                duck.samples.push(rec.duck.0[rcur.1].clone());
                rcur.1 += 1;
            }
            if qe.end.is_none() && rec.qe.1.as_ref().is_some_and(|e| e.t_ms <= now_ms) {
                qe.end = rec.qe.1.clone();
            }
            if duck.end.is_none() && rec.duck.1.as_ref().is_some_and(|e| e.t_ms <= now_ms) {
                duck.end = rec.duck.1.clone();
            }
        } else if !runs.is_empty() {
            for (i, (r, _)) in runs.iter_mut().enumerate() {
                let e = if i == 0 { &mut qe } else { &mut duck };
                if e.end.is_some() {
                    continue;
                }
                let s = r.sample();
                if let Some(f) = rec_out.as_mut() {
                    let _ = writeln!(f, "{}", sample_json(e.key, &s));
                }
                e.samples.push(s);
                if let Some(end) = r.finished() {
                    if let Some(f) = rec_out.as_mut() {
                        let _ = writeln!(f, "{}", end_json(e.key, &end));
                    }
                    e.end = Some(end);
                }
            }
        }
        let _ = have_duck;
        let sum_done = reveal_clock && [&qe, &duck].iter().all(|e| match &e.source { Source::Summary(r) => now_ms >= (r.wall_s * 1000.0) as u64, _ => true });
        let all_done = qe.done() && duck.done() && (!reveal_clock || sum_done);
        let view = ui::View { qe: &qe, duck: &duck, meta: &meta, now_ms, color, reveal_clock };
        if interactive {
            if last_size.elapsed() > Duration::from_secs(1) {
                (w, _h) = term::size();
                last_size = Instant::now();
            }
            let lines = ui::render(&view, w);
            let mut s = String::from("\x1b[?2026h\x1b[H");
            s.push_str(&lines.join("\x1b[K\r\n"));
            s.push_str("\x1b[K\x1b[J\x1b[?2026l");
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(s.as_bytes());
            let _ = o.flush();
        } else if now_ms / 1000 > last_plain || all_done {
            last_plain = now_ms / 1000;
            if !all_done {
                println!("{}", ui::status_line(&view));
            }
        }
        if all_done && finished_at.is_none() {
            finished_at = Some(Instant::now());
        }
        let quit_now = quit.as_ref().is_some_and(|q| q.load(Relaxed));
        // Interactive: stay on the end card until a key is pressed. Otherwise leave right away.
        if quit_now || (all_done && !interactive) {
            break;
        }
        next += tick;
        let d = next.saturating_duration_since(Instant::now());
        std::thread::sleep(d);
    }

    // Leave: stop children, clean temp, restore the terminal, then print the final frame to the normal screen.
    for (r, dir) in &runs {
        r.kill();
        r.cleanup(dir);
    }
    let _ = std::fs::remove_dir_all(&work_root);
    drop(term);
    let now_ms = start.elapsed().as_millis() as u64;
    let view = ui::View { qe: &qe, duck: &duck, meta: &meta, now_ms, color, reveal_clock };
    let (fw, _) = term::size();
    let fw = if interactive { fw } else { 100 };
    println!("{}", ui::render(&view, fw).join("\n"));
    let _ = py;
}

fn model_meta(m: &Meta) -> String {
    model::meta_json(m)
}
