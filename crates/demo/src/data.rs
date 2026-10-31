//! Output verification (order-independent row hash), recorded EXP-005 lookup, machine/DuckDB facts, dir sizes.
use crate::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

/// Allocated bytes under `p` (st_blocks * 512, like `du`; sparse and unlinked files count as the OS says).
pub fn du(p: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    for e in rd.flatten() {
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        total += if m.is_dir() { du(&e.path()) } else { m.blocks() * 512 };
    }
    total
}

/// Split RFC 4180 records (quote-aware) and call `f(fields)`. Rows are normalised to their parsed fields, so
/// two engines that quote differently but write the same values compare equal.
fn for_each_record(bytes: &[u8], mut f: impl FnMut(&[Vec<u8>])) {
    let (mut fields, mut cur): (Vec<Vec<u8>>, Vec<u8>) = (vec![], vec![]);
    let (mut inq, mut i) = (false, 0);
    while i < bytes.len() {
        let c = bytes[i];
        if inq {
            if c == b'"' {
                if bytes.get(i + 1) == Some(&b'"') {
                    cur.push(b'"');
                    i += 1;
                } else {
                    inq = false;
                }
            } else {
                cur.push(c);
            }
        } else {
            match c {
                b'"' => inq = true,
                b',' => fields.push(std::mem::take(&mut cur)),
                b'\r' => {}
                b'\n' => {
                    fields.push(std::mem::take(&mut cur));
                    f(&fields);
                    fields.clear();
                }
                _ => cur.push(c),
            }
        }
        i += 1;
    }
    if !cur.is_empty() || !fields.is_empty() {
        fields.push(cur);
        f(&fields);
    }
}

/// (data rows excluding header, hash of the sorted per-row hashes).
pub fn digest_file(p: &Path) -> Option<(u64, String)> {
    let bytes = std::fs::read(p).ok()?;
    let mut hs: Vec<u64> = vec![];
    for_each_record(&bytes, |f| {
        let mut h = DefaultHasher::new();
        f.hash(&mut h);
        hs.push(h.finish());
    });
    if hs.is_empty() {
        return None;
    }
    hs.remove(0); // header
    let rows = hs.len() as u64;
    hs.sort_unstable();
    let mut h = DefaultHasher::new();
    hs.hash(&mut h);
    Some((rows, format!("{:016x}", h.finish())))
}

/// Incremental record counter for a growing output file (counts newlines outside quotes).
#[derive(Default)]
pub struct RowCounter {
    pub off: u64,
    pub newlines: u64,
    inq: bool,
}

impl RowCounter {
    pub fn poll(&mut self, p: &Path) -> u64 {
        use std::io::{Read, Seek, SeekFrom};
        if let Ok(mut f) = std::fs::File::open(p) {
            if f.seek(SeekFrom::Start(self.off)).is_ok() {
                let mut buf = vec![0u8; 1 << 22];
                if let Ok(n) = f.read(&mut buf) {
                    for &b in &buf[..n] {
                        if b == b'"' {
                            self.inq = !self.inq;
                        } else if b == b'\n' && !self.inq {
                            self.newlines += 1;
                        }
                    }
                    self.off += n as u64;
                }
            }
        }
        self.newlines.saturating_sub(1)
    }
}

/// Recorded EXP-005-stream summary for one engine and size: (wall_s, rss_mib, temp_mib, out_rows), medians of the runs.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub wall_s: f64,
    pub rss_mib: f64,
    pub temp_mib: f64,
    pub out_rows: u64,
    pub runs: usize,
}

pub fn recorded(raw: &Path, engine: &str, size: &str) -> Option<Recorded> {
    let text = std::fs::read_to_string(raw).ok()?;
    let rows: Vec<json::Obj> = text
        .lines()
        .filter_map(json::parse_obj)
        .filter(|o| json::text(o, "engine").as_deref() == Some(engine) && json::text(o, "size").as_deref() == Some(size))
        .collect();
    if rows.is_empty() {
        return None;
    }
    let med = |k: &str| {
        let mut v: Vec<f64> = rows.iter().filter_map(|o| json::num(o, k)).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    Some(Recorded {
        wall_s: med("wall_s"),
        rss_mib: med("rss_mib"),
        temp_mib: med("temp_mib"),
        out_rows: med("out_rows") as u64,
        runs: rows.len(),
    })
}

fn sh(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

pub fn machine() -> String {
    let os = format!("{} {}", sh("uname", &["-s"]).unwrap_or_default(), sh("uname", &["-m"]).unwrap_or_default());
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| t.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split(':').nth(1)).map(|s| s.trim().to_string()))
        .or_else(|| sh("sysctl", &["-n", "machdep.cpu.brand_string"]))
        .unwrap_or_else(|| "unknown CPU".into());
    let ram_kib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| t.lines().find(|l| l.starts_with("MemTotal")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()))
        .or_else(|| sh("sysctl", &["-n", "hw.memsize"]).and_then(|v| v.parse::<u64>().ok()).map(|b| b / 1024));
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    let ram = ram_kib.map(|k| format!("{:.1} GiB RAM", k as f64 / 1048576.0)).unwrap_or_else(|| "? RAM".into());
    format!("{os} · {cpu} · {cores} cores · {ram}")
}

pub fn duckdb_version(py: &str) -> Option<String> {
    sh(py, &["-c", "import duckdb;print(duckdb.__version__)"])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_of(s: &str) -> (u64, String) {
        let p = std::env::temp_dir().join(format!("qe-demo-digest-{}-{}", std::process::id(), s.len()));
        std::fs::write(&p, s).unwrap();
        let d = digest_file(&p).unwrap();
        let _ = std::fs::remove_file(p);
        d
    }

    #[test]
    fn digest_ignores_row_order_and_quoting_but_not_values() {
        let a = digest_of("a,b\n1,\"x,y\"\n2,z\n");
        let b = digest_of("a,b\r\n2,z\r\n1,\"x,y\"\r\n");
        let c = digest_of("a,b\n1,x,y\n2,z\n");
        let d = digest_of("a,b\n1,\"x,y\"\n2,w\n");
        assert_eq!(a, b);
        assert_eq!(a.0, 2);
        assert_ne!(a.1, c.1);
        assert_ne!(a.1, d.1);
    }

    #[test]
    fn row_counter_respects_quoted_newlines() {
        let p = std::env::temp_dir().join(format!("qe-demo-rows-{}", std::process::id()));
        std::fs::write(&p, "h\n\"a\nb\"\nc\n").unwrap();
        let mut rc = RowCounter::default();
        assert_eq!(rc.poll(&p), 2);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn recorded_medians_come_from_the_raw_file() {
        let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/experiments/data/exp005-stream-raw.jsonl");
        let r = recorded(&raw, "duckdb pipe", "5x").unwrap();
        assert_eq!(r.runs, 3);
        assert_eq!(r.out_rows, 274930);
        assert!(r.temp_mib > 1400.0);
    }
}
