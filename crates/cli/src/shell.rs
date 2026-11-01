//! Interactive shell and pretty table output for `qe` (std only, no extra dependencies).
//!
//! `qe --data DIR` with a terminal on stdin opens a `qe>` prompt. SQL ends with `;`, lines starting with `.`
//! are shell commands (`.help` lists them). Results print as a MySQL-style table with a row count and timing.
use engine::storage::Catalog;
use engine::types::{fmt_date, fmt_dec, Ty, Value};
use engine::Options;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

/// The nine benchmark queries, compiled in so `.queries` and `.q N` work from any directory.
const BENCHMARK_SQL: &str = include_str!("../../../queries/benchmark.sql");

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Table,
    Csv,
}

pub struct Session {
    pub data: PathBuf,
    pub opts: Options,
    pub mode: Mode,
    /// Rows shown per result in table mode; `None` = all rows.
    pub limit: Option<usize>,
    pub timer: bool,
}

impl Session {
    pub fn new(data: PathBuf, opts: Options, mode: Mode) -> Session {
        Session { data, opts, mode, limit: Some(20), timer: true }
    }
}

pub struct Named {
    pub name: String,
    pub sql: String,
}

/// Split a script into statements at `;` outside single quotes. A `-- comment` line directly above a
/// statement becomes its name. Statements are returned without the trailing `;`.
pub fn split_statements(text: &str) -> Vec<Named> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut name = String::new();
    let mut in_str = false;
    for line in text.lines() {
        let t = line.trim();
        if !in_str && cur.trim().is_empty() {
            if let Some(c) = t.strip_prefix("--") {
                let c = c.trim();
                if !c.is_empty() {
                    name = c.to_string();
                }
                continue;
            }
        }
        for ch in line.chars() {
            if ch == '\'' {
                in_str = !in_str;
            }
            if ch == ';' && !in_str {
                if !cur.trim().is_empty() {
                    out.push(Named { name: std::mem::take(&mut name), sql: cur.trim().to_string() });
                }
                cur.clear();
            } else {
                cur.push(ch);
            }
        }
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        out.push(Named { name, sql: cur.trim().to_string() });
    }
    out
}

pub fn benchmark_queries() -> Vec<Named> {
    split_statements(BENCHMARK_SQL)
}

// ---------------------------------------------------------------------------------------------- running

pub struct Shown {
    pub names: Vec<String>,
    pub numeric: Vec<bool>,
    pub rows: Vec<Vec<String>>,
    pub total: usize,
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
        Value::I64(i) => i.to_string(),
        Value::F64(f) => f.to_string(),
        Value::Dec(x, s) => {
            let mut b = Vec::new();
            fmt_dec(*x, *s, &mut b);
            String::from_utf8_lossy(&b).into_owned()
        }
        Value::Date(d) => {
            let mut b = Vec::new();
            fmt_date(*d, &mut b);
            String::from_utf8_lossy(&b).into_owned()
        }
        Value::Str(s) => s.clone(),
    }
}

/// Run a query and keep only the first `keep` rows (all if `None`); the rest are only counted.
pub fn run(s: &Session, sql: &str, keep: Option<usize>) -> engine::Result<Shown> {
    let mut p = engine::prepare(&s.data, sql, &s.opts)?;
    let numeric = p.types.iter().map(|t| matches!(t, Ty::I64 | Ty::F64 | Ty::Dec(_))).collect();
    let mut rows = Vec::new();
    let mut total = 0usize;
    while let Some(b) = p.root.next()? {
        for i in 0..b.len {
            if keep.map_or(true, |k| rows.len() < k) {
                rows.push(b.cols.iter().map(|c| cell(&c.value(i))).collect());
            }
        }
        total += b.len;
        engine::progress::add_rows_out(b.len);
    }
    Ok(Shown { names: p.names, numeric, rows, total })
}

fn truncate(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    if flat.chars().count() <= max {
        flat
    } else {
        let mut t: String = flat.chars().take(max.saturating_sub(3)).collect();
        t.push_str("...");
        t
    }
}

fn print_table(sh: &Shown, out: &mut impl Write) {
    const MAXW: usize = 48;
    let n = sh.names.len();
    let rows: Vec<Vec<String>> = sh.rows.iter().map(|r| r.iter().map(|c| truncate(c, MAXW)).collect()).collect();
    let mut w: Vec<usize> = sh.names.iter().map(|h| h.chars().count()).collect();
    for r in &rows {
        for (j, c) in r.iter().enumerate() {
            w[j] = w[j].max(c.chars().count());
        }
    }
    let rule: String = {
        let mut s = String::from("+");
        for x in &w {
            s.push_str(&"-".repeat(x + 2));
            s.push('+');
        }
        s
    };
    let _ = writeln!(out, "{rule}");
    let mut line = String::from("|");
    for j in 0..n {
        line.push_str(&format!(" {:<width$} |", sh.names[j], width = w[j]));
    }
    let _ = writeln!(out, "{line}");
    let _ = writeln!(out, "{rule}");
    for r in &rows {
        let mut line = String::from("|");
        for j in 0..n {
            if sh.numeric[j] {
                line.push_str(&format!(" {:>width$} |", r[j], width = w[j]));
            } else {
                line.push_str(&format!(" {:<width$} |", r[j], width = w[j]));
            }
        }
        let _ = writeln!(out, "{line}");
    }
    if !rows.is_empty() {
        let _ = writeln!(out, "{rule}");
    }
}

fn print_csv(sh: &Shown, out: &mut impl Write) {
    fn f(s: &str) -> String {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }
    let _ = writeln!(out, "{}", sh.names.iter().map(|s| f(s)).collect::<Vec<_>>().join(","));
    for r in &sh.rows {
        let _ = writeln!(out, "{}", r.iter().map(|s| if s == "NULL" { String::new() } else { f(s) }).collect::<Vec<_>>().join(","));
    }
}

/// Execute one statement and print it. Returns false on error (already reported on stderr).
pub fn execute(s: &Session, sql: &str, label: Option<&str>) -> bool {
    let mut out = std::io::stdout().lock();
    if let Some(l) = label {
        let _ = writeln!(out, "-- {l}");
    }
    let keep = if s.mode == Mode::Table { s.limit } else { None };
    let t0 = Instant::now();
    match run(s, sql, keep) {
        Ok(sh) => {
            let secs = t0.elapsed().as_secs_f64();
            match s.mode {
                Mode::Table => print_table(&sh, &mut out),
                Mode::Csv => print_csv(&sh, &mut out),
            }
            if s.mode == Mode::Table || s.timer {
                let shown = sh.rows.len();
                let mut msg = if sh.total == 0 {
                    "Empty set".to_string()
                } else if sh.total == 1 {
                    "1 row in set".to_string()
                } else if shown < sh.total {
                    format!("{shown} of {} rows shown", sh.total)
                } else {
                    format!("{} rows in set", sh.total)
                };
                if s.timer {
                    msg.push_str(&format!(" ({secs:.3} sec)"));
                }
                if shown < sh.total {
                    msg.push_str("   [.limit all shows every row]");
                }
                let target: &mut dyn Write = if s.mode == Mode::Csv { &mut std::io::stderr() } else { &mut out };
                let _ = writeln!(target, "{msg}\n");
            }
            true
        }
        Err(e) => {
            let _ = out.flush();
            eprintln!("error: {e}\n");
            false
        }
    }
}

/// Run every statement of a script in order. Returns false if any failed.
pub fn run_script(s: &Session, text: &str) -> bool {
    let mut ok = true;
    for st in split_statements(text) {
        let label = if st.name.is_empty() { None } else { Some(st.name.as_str()) };
        ok &= execute(s, &st.sql, label);
    }
    ok
}

// ---------------------------------------------------------------------------------------------- shell

fn print_tables(s: &Session) {
    match Catalog::load(&s.data) {
        Ok(c) => {
            let mut names: Vec<_> = c.tables.keys().cloned().collect();
            names.sort();
            for n in names {
                let t = &c.tables[&n];
                let size = std::fs::metadata(&t.path).map(|m| m.len()).unwrap_or(0);
                println!("  {n:<12} {:>2} columns   {}", t.cols.len(), human(size));
            }
            println!();
        }
        Err(e) => eprintln!("error: {e}\n"),
    }
}

fn print_schema(s: &Session, which: Option<&str>) {
    match Catalog::load(&s.data) {
        Ok(c) => {
            let mut names: Vec<_> = c.tables.keys().cloned().collect();
            names.sort();
            let want = which.map(|w| w.to_ascii_lowercase());
            let mut found = false;
            for n in names {
                if want.as_ref().map_or(false, |w| *w != n) {
                    continue;
                }
                found = true;
                let t = &c.tables[&n];
                println!("{n}  ({})", t.path.display());
                for (col, ty) in &t.cols {
                    println!("  {col:<10} {ty:?}");
                }
                println!();
            }
            if !found {
                eprintln!("error: no such table\n");
            }
        }
        Err(e) => eprintln!("error: {e}\n"),
    }
}

pub fn human(b: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1000.0 && i < 3 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.2} {}", U[i]) }
}

fn help() {
    println!(
        "End SQL with ';'.  Shell commands:
  .tables              list tables and file sizes
  .schema [TABLE]      show columns and types
  .queries             list the nine benchmark queries
  .q N                 run benchmark query N
  .read FILE           run every statement in a .sql file
  .mode table|csv      result format (table is the default in the shell)
  .limit N|all         rows shown per result (default 20)
  .timer on|off        show elapsed time
  .threads N|auto      worker threads (results never depend on this)
  .chunk BYTES|auto    scan chunk size (results never depend on this)
  .noopt on|off        single-threaded reference path
  .help                this text
  .quit                leave (also Ctrl-D)
"
    );
}

fn command(s: &mut Session, line: &str) -> bool {
    let mut it = line.split_whitespace();
    let cmd = it.next().unwrap_or("");
    let arg = it.next();
    match cmd {
        ".quit" | ".exit" | ".q!" => return false,
        ".help" | ".h" => help(),
        ".tables" => print_tables(s),
        ".schema" => print_schema(s, arg),
        ".queries" => {
            for (i, q) in benchmark_queries().iter().enumerate() {
                println!("  Q{}  {}\n      {}\n", i + 1, q.name, q.sql);
            }
        }
        ".q" => {
            let qs = benchmark_queries();
            match arg.and_then(|a| a.trim_start_matches(['Q', 'q']).parse::<usize>().ok()) {
                Some(n) if n >= 1 && n <= qs.len() => {
                    println!("{}", qs[n - 1].sql);
                    execute(s, &qs[n - 1].sql, None);
                }
                _ => eprintln!("usage: .q N   (N = 1..{}; see .queries)\n", qs.len()),
            }
        }
        ".read" => match arg {
            Some(f) => match std::fs::read_to_string(f) {
                Ok(t) => {
                    run_script(s, &t);
                }
                Err(e) => eprintln!("error: cannot read {f}: {e}\n"),
            },
            None => eprintln!("usage: .read FILE\n"),
        },
        ".mode" => match arg {
            Some("table") => s.mode = Mode::Table,
            Some("csv") => s.mode = Mode::Csv,
            _ => eprintln!("usage: .mode table|csv\n"),
        },
        ".limit" => match arg {
            Some("all") => s.limit = None,
            Some(n) => match n.parse::<usize>() {
                Ok(n) if n > 0 => s.limit = Some(n),
                _ => eprintln!("usage: .limit N|all\n"),
            },
            None => eprintln!("usage: .limit N|all\n"),
        },
        ".timer" => match arg {
            Some("on") => s.timer = true,
            Some("off") => s.timer = false,
            _ => eprintln!("usage: .timer on|off\n"),
        },
        ".threads" => match arg {
            Some("auto") => s.opts.threads = None,
            Some(n) => match n.parse::<usize>() {
                Ok(n) if n > 0 => s.opts.threads = Some(n),
                _ => eprintln!("usage: .threads N|auto\n"),
            },
            None => eprintln!("usage: .threads N|auto\n"),
        },
        ".chunk" => match arg {
            Some("auto") => s.opts.chunk_bytes = None,
            Some(n) => match n.parse::<usize>() {
                Ok(n) if n > 0 => s.opts.chunk_bytes = Some(n),
                _ => eprintln!("usage: .chunk BYTES|auto\n"),
            },
            None => eprintln!("usage: .chunk BYTES|auto\n"),
        },
        ".noopt" => match arg {
            Some("on") => s.opts.no_opt = true,
            Some("off") => s.opts.no_opt = false,
            _ => eprintln!("usage: .noopt on|off\n"),
        },
        other => eprintln!("unknown command {other} (try .help)\n"),
    }
    true
}

fn banner(s: &Session) {
    let n = Catalog::load(&s.data).map(|c| c.tables.len()).unwrap_or(0);
    println!("SenkoParse qe {} - SQL over CSV files, no database library", env!("CARGO_PKG_VERSION"));
    println!(
        "data: {}  ({n} tables)   threads: {}   limit: {}",
        s.data.display(),
        s.opts.threads.map_or("all".to_string(), |t| t.to_string()),
        s.limit.map_or("all".to_string(), |l| l.to_string())
    );
    println!("Type .tables, .queries, .help. End SQL with ';'.\n");
}

/// Read SQL and shell commands from stdin until EOF or `.quit`. Prompts only when stdin is a terminal.
pub fn repl(mut s: Session) {
    let tty = std::io::stdin().is_terminal();
    if tty {
        banner(&s);
    }
    let stdin = std::io::stdin();
    let mut buf = String::new();
    loop {
        if tty {
            print!("{}", if buf.trim().is_empty() { "qe> " } else { "...> " });
            let _ = std::io::stdout().flush();
        }
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => {
                if !buf.trim().is_empty() {
                    execute(&s, buf.trim(), None);
                }
                if tty {
                    println!();
                }
                return;
            }
            Ok(_) => {}
        }
        let t = line.trim();
        if buf.trim().is_empty() {
            if t.is_empty() || t.starts_with("--") {
                continue;
            }
            if t.starts_with('.') {
                if !command(&mut s, t) {
                    return;
                }
                continue;
            }
        }
        buf.push_str(&line);
        let mut in_str = false;
        let mut done = false;
        for ch in buf.chars() {
            if ch == '\'' {
                in_str = !in_str;
            }
        }
        if !in_str && buf.trim_end().ends_with(';') {
            done = true;
        }
        if done {
            let st = split_statements(&buf);
            buf.clear();
            for q in st {
                execute(&s, &q.sql, None);
            }
        }
    }
}
