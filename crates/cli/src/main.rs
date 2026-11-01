mod progress;
mod serve;
mod shell;
use std::io::{BufWriter, IsTerminal};
use std::path::PathBuf;

fn main() {
    let mut data = PathBuf::from("data");
    let mut sql: Option<String> = None;
    let mut opts = engine::Options::default();
    let mut stdin_table: Option<String> = None;
    let mut show_progress = false;
    let mut file: Option<String> = None;
    let mut table_mode = false;
    let mut force_shell = false;
    let mut serve = false;
    let mut port: u16 = 8080;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data" => data = PathBuf::from(args.next().unwrap_or_else(|| die("--data needs a directory"))),
            "--no-opt" => opts.no_opt = true,
            "--threads" => opts.threads = Some(num(args.next(), "--threads")),
            "--chunk-bytes" => opts.chunk_bytes = Some(num(args.next(), "--chunk-bytes")),
            "--stdin" => stdin_table = Some(args.next().unwrap_or_else(|| die("--stdin needs a table name"))),
            "--progress" => show_progress = true,
            "--table" => table_mode = true,
            "--serve" => serve = true,
            "--port" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| die("--port needs a port number")),
            "--shell" | "-i" => force_shell = true,
            "--file" | "-f" => file = Some(args.next().unwrap_or_else(|| die("--file needs a path"))),
            "--cache" => die("--cache is not implemented yet (M6)"),
            "-h" | "--help" => {
                eprintln!("usage: qe [--data DIR] [options] \"SELECT ...\"      run one query, CSV on stdout\n       qe [--data DIR] [options]                   interactive shell (when stdin is a terminal)\n       qe [--data DIR] --file queries.sql --table   run every statement in a file\noptions: --table (pretty table instead of CSV)  --no-opt  --threads N  --chunk-bytes N  --stdin TABLE  --progress\n         --serve [--port N]  local web UI on 127.0.0.1 (default port 8080)\nshell:   .help lists commands, .queries shows the nine benchmark queries (queries/benchmark.sql)");
                return;
            }
            _ if a.starts_with("--") => die(&format!("unknown option {a}")),
            _ => sql = Some(a),
        }
    }
    if serve {
        if stdin_table.is_some() {
            die("--serve cannot be combined with --stdin");
        }
        let sess = shell::Session::new(data, opts, shell::Mode::Table);
        if let Err(e) = serve::serve(sess, port) {
            die(&format!("cannot serve on 127.0.0.1:{port}: {e}"));
        }
        return;
    }
    if stdin_table.is_none() && (file.is_some() || sql.is_none() || force_shell) {
        let mode = if table_mode || (sql.is_none() && file.is_none() && std::io::stdin().is_terminal()) {
            shell::Mode::Table
        } else {
            shell::Mode::Csv
        };
        let sess = shell::Session::new(data, opts, mode);
        if let Some(f) = file {
            let text = std::fs::read_to_string(&f).unwrap_or_else(|e| die(&format!("cannot read {f}: {e}")));
            if !shell::run_script(&sess, &text) {
                std::process::exit(1);
            }
        } else {
            shell::repl(sess);
        }
        return;
    }
    let sql = sql.unwrap_or_else(|| die("missing SQL query"));
    if table_mode && stdin_table.is_none() {
        let sess = shell::Session::new(data, opts, shell::Mode::Table);
        if !shell::execute(&sess, &sql, None) {
            std::process::exit(1);
        }
        return;
    }
    let prog = if show_progress { progress::Progress::start(stdin_table.is_some()) } else { None };
    let result = match &stdin_table {
        None => engine::prepare(&data, &sql, &opts),
        Some(t) => engine::prepare_stream(&data, &sql, &opts, t, Box::new(progress::Counting(std::io::stdin()))),
    }
    .and_then(|mut p| {
        let out = std::io::stdout().lock();
        let mut w = BufWriter::with_capacity(1 << 20, out);
        engine::output::write_csv_opts(&p.names, &mut *p.root, &mut w, stdin_table.is_some())
    });
    if let Some(p) = prog {
        p.finish();
    }
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn num(v: Option<String>, flag: &str) -> usize {
    v.and_then(|v| v.parse().ok()).unwrap_or_else(|| die(&format!("{flag} needs a positive integer")))
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(2);
}
