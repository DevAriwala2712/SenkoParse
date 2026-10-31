use std::io::BufWriter;
use std::path::PathBuf;

fn main() {
    let mut data = PathBuf::from("data");
    let mut sql: Option<String> = None;
    let mut opts = engine::Options::default();
    let mut stdin_table: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data" => data = PathBuf::from(args.next().unwrap_or_else(|| die("--data needs a directory"))),
            "--no-opt" => opts.no_opt = true,
            "--threads" => opts.threads = Some(num(args.next(), "--threads")),
            "--chunk-bytes" => opts.chunk_bytes = Some(num(args.next(), "--chunk-bytes")),
            "--stdin" => stdin_table = Some(args.next().unwrap_or_else(|| die("--stdin needs a table name"))),
            "--cache" => die("--cache is not implemented yet (M6)"),
            "-h" | "--help" => {
                eprintln!("usage: qe [--data DIR] [--no-opt] [--threads N] [--chunk-bytes N] [--stdin TABLE] \"SELECT ...\"");
                return;
            }
            _ if a.starts_with("--") => die(&format!("unknown option {a}")),
            _ => sql = Some(a),
        }
    }
    let sql = sql.unwrap_or_else(|| die("missing SQL query"));
    let result = match &stdin_table {
        None => engine::prepare(&data, &sql, &opts),
        Some(t) => engine::prepare_stream(&data, &sql, &opts, t, Box::new(std::io::stdin())),
    }
    .and_then(|mut p| {
        let out = std::io::stdout().lock();
        let mut w = BufWriter::with_capacity(1 << 20, out);
        engine::output::write_csv_opts(&p.names, &mut *p.root, &mut w, stdin_table.is_some())
    });
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
