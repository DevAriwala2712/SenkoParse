//! `qe --serve`: a tiny std-only web UI on 127.0.0.1 (no extra dependencies, no TLS, no auth).
//!
//! One page (SQL workbench: editor, schema explorer, results grid, run stats, history) plus a few JSON
//! endpoints. Queries run through `shell::run`, the same function the interactive shell uses.
//!
//! Endpoints: `GET /api/tables`, `GET /api/queries`, `GET /api/databases`, `GET /api/progress`,
//! `POST /api/datadir` (body = directory containing schema.toml), `POST /api/query?max=N` (body = SQL;
//! reply carries rows, total and run stats: time, sampled peak RSS, referenced table bytes, threads).
//!
//! Safety notes: the listener binds 127.0.0.1 only. Because any web page in the user's browser can send
//! requests to localhost, every request must carry a `Host` of `127.0.0.1:PORT` or `localhost:PORT`
//! (blocks DNS rebinding) and any `Origin` header must match it (blocks cross-site POSTs). Only SELECT
//! is parsed by the engine, and it reads only the tables in the data directory.
use crate::shell::{self, Session};
use engine::storage::Catalog;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("serve_page.html");
/// The repository logo (docs/img/logo.png), downscaled to 256 px wide, served as the header logo and favicon.
const LOGO: &[u8] = include_bytes!("serve_logo.png");
const MAX_BODY: usize = 1 << 20;
/// Default and hard cap for rows sent to the browser per result; the row count shown is always the true total.
const MAX_ROWS: usize = 1000;
const MAX_ROWS_CAP: usize = 200_000;

/// Live state of the running query, read by `/api/progress`. Queries run one at a time (see `gate`).
static RUNNING: AtomicBool = AtomicBool::new(false);
static CUR_RSS: AtomicU64 = AtomicU64::new(0);
static PEAK_RSS: AtomicU64 = AtomicU64::new(0);
static ROWS_BASE: AtomicU64 = AtomicU64::new(0);
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);

/// Everything a request handler needs. `data` is switchable from the UI; each query reads it once at start.
struct Srv {
    tmpl: Session,
    data: Mutex<PathBuf>,
    gate: Mutex<()>,
    port: u16,
}

impl Srv {
    fn session(&self) -> Session {
        let data = self.data.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Session { data, opts: self.tmpl.opts.clone(), mode: self.tmpl.mode, limit: self.tmpl.limit, timer: false }
    }
}

fn sample_rss() {
    if let Some(r) = crate::progress::rss_bytes() {
        CUR_RSS.store(r, Relaxed);
        PEAK_RSS.fetch_max(r, Relaxed);
    }
}

pub fn serve(sess: Session, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    eprintln!("qe: serving http://127.0.0.1:{port}/  (data: {}; Ctrl-C to stop)", sess.data.display());
    // Queries use all cores; `gate` runs one at a time so a second tab cannot thrash the machine.
    let srv = Arc::new(Srv { data: Mutex::new(sess.data.clone()), tmpl: sess, gate: Mutex::new(()), port });
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let srv = srv.clone();
        std::thread::spawn(move || {
            let _ = handle(conn, &srv);
        });
    }
    Ok(())
}

struct Req {
    method: String,
    path: String,
    host: String,
    origin: Option<String>,
    body: Vec<u8>,
}

fn read_req(c: &mut TcpStream) -> Result<Req, &'static str> {
    c.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 16 * 1024 {
            return Err("headers too large");
        }
        match c.read(&mut tmp) {
            Ok(0) | Err(_) => return Err("incomplete request"),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("").to_string();
    let path = first.next().unwrap_or("").to_string();
    let (mut host, mut origin, mut len) = (String::new(), None, 0usize);
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let v = v.trim();
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v.to_string(),
                "origin" => origin = Some(v.to_string()),
                "content-length" => len = v.parse().map_err(|_| "bad content-length")?,
                _ => {}
            }
        }
    }
    if len > MAX_BODY {
        return Err("body too large");
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match c.read(&mut tmp) {
            Ok(0) | Err(_) => return Err("incomplete body"),
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    body.truncate(len);
    Ok(Req { method, path, host, origin, body })
}

fn respond(c: &mut TcpStream, status: &str, ctype: &str, body: &str) -> std::io::Result<()> {
    respond_bytes(c, status, ctype, body.as_bytes())
}

fn respond_bytes(c: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        c,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    c.write_all(body)
}

fn handle(mut c: TcpStream, srv: &Srv) -> std::io::Result<()> {
    let req = match read_req(&mut c) {
        Ok(r) => r,
        Err(e) => return respond(&mut c, "400 Bad Request", "text/plain", e),
    };
    let port = srv.port;
    let ok_hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !ok_hosts.contains(&req.host) {
        return respond(&mut c, "403 Forbidden", "text/plain", "bad Host header");
    }
    if let Some(o) = &req.origin {
        if !ok_hosts.iter().any(|h| *o == format!("http://{h}")) {
            return respond(&mut c, "403 Forbidden", "text/plain", "bad Origin header");
        }
    }
    let json = "application/json";
    let (path, query) = req.path.split_once('?').unwrap_or((req.path.as_str(), ""));
    match (req.method.as_str(), path) {
        ("GET", "/") => respond(&mut c, "200 OK", "text/html; charset=utf-8", PAGE),
        ("GET", "/logo.png") => respond_bytes(&mut c, "200 OK", "image/png", LOGO),
        ("GET", "/api/tables") => respond(&mut c, "200 OK", json, &tables_json(&srv.session())),
        ("GET", "/api/queries") => respond(&mut c, "200 OK", json, &queries_json()),
        ("GET", "/api/databases") => respond(&mut c, "200 OK", json, &databases_json(srv)),
        ("GET", "/api/progress") => respond(&mut c, "200 OK", json, &progress_json()),
        ("POST", "/api/datadir") => {
            let want = String::from_utf8_lossy(&req.body).trim().to_string();
            let out = match set_datadir(srv, &want) {
                Ok(()) => tables_json(&srv.session()),
                Err(e) => format!(r#"{{"error":{}}}"#, jstr(&e)),
            };
            respond(&mut c, "200 OK", json, &out)
        }
        ("POST", "/api/query") => {
            let max = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("max="))
                .and_then(|v| v.parse::<usize>().ok())
                .map_or(MAX_ROWS, |m| m.clamp(1, MAX_ROWS_CAP));
            let sql = String::from_utf8_lossy(&req.body).into_owned();
            let sql = sql.trim().trim_end_matches(';').trim();
            if sql.is_empty() {
                return respond(&mut c, "200 OK", json, r#"{"error":"empty query"}"#);
            }
            let out = run_query(srv, sql, max);
            respond(&mut c, "200 OK", json, &out)
        }
        _ => respond(&mut c, "404 Not Found", "text/plain", "not found"),
    }
}

fn run_query(srv: &Srv, sql: &str, max: usize) -> String {
    let sess = srv.session();
    let _g = srv.gate.lock().unwrap_or_else(|e| e.into_inner());
    // Live counters for /api/progress. Result rows are counted as they are produced (baseline subtracted).
    ROWS_BASE.store(engine::progress::rows_out(), Relaxed);
    PEAK_RSS.store(0, Relaxed);
    sample_rss();
    let rss_start = CUR_RSS.load(Relaxed);
    *STARTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    RUNNING.store(true, Relaxed);
    let sampler = std::thread::spawn(|| {
        while RUNNING.load(Relaxed) {
            sample_rss();
            for _ in 0..5 {
                if !RUNNING.load(Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    });
    let t0 = Instant::now();
    let res = shell::run(&sess, sql, Some(max));
    let secs = t0.elapsed().as_secs_f64();
    sample_rss();
    RUNNING.store(false, Relaxed);
    let _ = sampler.join();
    match res {
        Ok(sh) => {
            let (tables, bytes) = referenced_tables(&sess, sql);
            let threads = sess.opts.threads.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
            let names: Vec<String> = sh.names.iter().map(|n| jstr(n)).collect();
            let numeric: Vec<&str> = sh.numeric.iter().map(|b| if *b { "true" } else { "false" }).collect();
            let rows: Vec<String> =
                sh.rows.iter().map(|r| format!("[{}]", r.iter().map(|v| jstr(v)).collect::<Vec<_>>().join(","))).collect();
            let tl: Vec<String> = tables.iter().map(|t| jstr(t)).collect();
            format!(
                r#"{{"names":[{}],"numeric":[{}],"rows":[{}],"total":{},"secs":{secs:.6},"stats":{{"rss_start":{rss_start},"peak_rss":{},"table_bytes":{bytes},"threads":{threads},"tables":[{}],"max":{max}}}}}"#,
                names.join(","),
                numeric.join(","),
                rows.join(","),
                sh.total,
                PEAK_RSS.load(Relaxed),
                tl.join(",")
            )
        }
        Err(e) => format!(r#"{{"error":{},"secs":{secs:.6}}}"#, jstr(&e.to_string())),
    }
}

/// Tables whose name appears as a whole word in the SQL, and the sum of their CSV file sizes. The engine
/// reads only the referenced columns, so this is an upper bound on bytes scanned, not a measurement.
fn referenced_tables(s: &Session, sql: &str) -> (Vec<String>, u64) {
    let Ok(cat) = Catalog::load(&s.data) else { return (vec![], 0) };
    let lower = sql.to_ascii_lowercase();
    let words: std::collections::HashSet<&str> =
        lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
    let mut names: Vec<String> = cat.tables.keys().filter(|n| words.contains(n.as_str())).cloned().collect();
    names.sort();
    let bytes = names.iter().map(|n| std::fs::metadata(&cat.tables[n].path).map(|m| m.len()).unwrap_or(0)).sum();
    (names, bytes)
}

fn progress_json() -> String {
    let running = RUNNING.load(Relaxed);
    let elapsed = STARTED.lock().unwrap_or_else(|e| e.into_inner()).map_or(0.0, |t| t.elapsed().as_secs_f64());
    let rows = engine::progress::rows_out().saturating_sub(ROWS_BASE.load(Relaxed));
    format!(
        r#"{{"running":{running},"elapsed":{elapsed:.3},"rss":{},"peak_rss":{},"rows_out":{rows}}}"#,
        CUR_RSS.load(Relaxed),
        PEAK_RSS.load(Relaxed)
    )
}

fn has_schema(p: &std::path::Path) -> bool {
    p.join("schema.toml").is_file()
}

fn set_datadir(srv: &Srv, want: &str) -> Result<(), String> {
    if want.is_empty() {
        return Err("empty path".into());
    }
    let p = std::fs::canonicalize(want).map_err(|e| format!("cannot open {want}: {e}"))?;
    if !p.is_dir() || !has_schema(&p) {
        return Err(format!("{} has no schema.toml", p.display()));
    }
    Catalog::load(&p).map_err(|e| e.to_string())?;
    *srv.data.lock().unwrap_or_else(|e| e.into_inner()) = p;
    Ok(())
}

/// Directories that look like databases (contain schema.toml): the current one, its siblings, and the
/// children of the working directory. One level only; nothing is read beyond a directory listing.
fn databases_json(srv: &Srv) -> String {
    let cur = std::fs::canonicalize(&*srv.data.lock().unwrap_or_else(|e| e.into_inner())).unwrap_or_default();
    let mut found: Vec<PathBuf> = vec![cur.clone()];
    let mut roots: Vec<PathBuf> = vec![];
    if let Some(par) = cur.parent() {
        roots.push(par.to_path_buf());
    }
    if let Ok(cwd) = std::env::current_dir() {
        if has_schema(&cwd) {
            found.push(cwd.clone());
        }
        roots.push(cwd);
    }
    for r in roots {
        let Ok(rd) = std::fs::read_dir(&r) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && has_schema(&p) {
                if let Ok(p) = std::fs::canonicalize(&p) {
                    found.push(p);
                }
            }
        }
    }
    found.sort();
    found.dedup();
    let items: Vec<String> = found
        .iter()
        .map(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
            format!(r#"{{"name":{},"path":{},"current":{}}}"#, jstr(&n), jstr(&p.display().to_string()), *p == cur)
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn tables_json(s: &Session) -> String {
    match Catalog::load(&s.data) {
        Ok(cat) => {
            let mut names: Vec<_> = cat.tables.keys().cloned().collect();
            names.sort();
            let items: Vec<String> = names
                .iter()
                .map(|n| {
                    let t = &cat.tables[n];
                    let size = std::fs::metadata(&t.path).map(|m| m.len()).unwrap_or(0);
                    let cols: Vec<String> =
                        t.cols.iter().map(|(c, ty)| format!(r#"{{"name":{},"type":{}}}"#, jstr(c), jstr(&format!("{ty:?}")))).collect();
                    format!(r#"{{"name":{},"size":{},"bytes":{size},"cols":[{}]}}"#, jstr(n), jstr(&shell::human(size)), cols.join(","))
                })
                .collect();
            // The "database" is the data directory: one schema.toml plus its CSV files.
            let abs = std::fs::canonicalize(&s.data).unwrap_or_else(|_| s.data.clone());
            let name = abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| abs.display().to_string());
            format!(
                r#"{{"database":{},"path":{},"tables":[{}]}}"#,
                jstr(&name),
                jstr(&abs.display().to_string()),
                items.join(",")
            )
        }
        Err(e) => format!(r#"{{"error":{}}}"#, jstr(&e.to_string())),
    }
}

fn queries_json() -> String {
    let items: Vec<String> = shell::benchmark_queries()
        .iter()
        .enumerate()
        .map(|(i, q)| format!(r#"{{"n":{},"name":{},"sql":{}}}"#, i + 1, jstr(&q.name), jstr(&q.sql)))
        .collect();
    format!("[{}]", items.join(","))
}

/// JSON string literal with escaping.
fn jstr(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
