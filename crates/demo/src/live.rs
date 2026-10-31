//! Live runs: spawn an engine, feed it the input file through its stdin, sample it every 100 ms.
use crate::data::{self, RowCounter};
use crate::model::{End, Sample};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Finished {
    wall_ms: u64,
    ok: bool,
    rows: u64,
    digest: String,
}

pub struct Run {
    pid: u32,
    t0: Instant,
    bytes: Arc<AtomicU64>,
    temp: PathBuf,
    out: PathBuf,
    rows: RowCounter,
    last_rss: u64,
    peak_rss: u64,
    peak_temp: u64,
    fin: Arc<Mutex<Option<Finished>>>,
    stop_feed: Arc<AtomicBool>,
}

pub struct Spec<'a> {
    pub program: String,
    pub args: Vec<String>,
    pub input: &'a Path,
    pub work: &'a Path, // per-engine scratch dir: <work>/tmp (TMPDIR / temp_directory) and <work>/out
}

impl Run {
    /// All runs share `t0` so their time axes line up. `args` may contain `{TEMP}` and `{OUT}`.
    pub fn start(spec: &Spec, t0: Instant) -> Result<Run, String> {
        let temp = spec.work.join("tmp");
        let out = spec.work.join("out.csv");
        let _ = std::fs::remove_dir_all(spec.work);
        std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
        let args: Vec<String> = spec.args.iter().map(|a| a.replace("{TEMP}", temp.to_str().unwrap()).replace("{OUT}", out.to_str().unwrap())).collect();
        let outf = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        let mut child = Command::new(&spec.program)
            .args(&args)
            .env("TMPDIR", &temp)
            .stdin(Stdio::piped())
            .stdout(outf)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", spec.program))?;
        let pid = child.id();
        let bytes = Arc::new(AtomicU64::new(0));
        let stop_feed = Arc::new(AtomicBool::new(false));
        // Feeder: the same file, byte for byte, into the engine's stdin. Pipe back-pressure paces it, so
        // `bytes` is (within one 64 KiB pipe buffer) what the engine has consumed.
        let mut stdin = child.stdin.take().unwrap();
        let (b2, stop2, input) = (bytes.clone(), stop_feed.clone(), spec.input.to_path_buf());
        std::thread::spawn(move || {
            let Ok(mut f) = std::fs::File::open(input) else { return };
            let mut buf = vec![0u8; 1 << 20];
            while !stop2.load(Relaxed) {
                match f.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if stdin.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        b2.fetch_add(n as u64, Relaxed);
                    }
                }
            }
        });
        // Waiter: exact wall time, then the order-independent output digest.
        let fin = Arc::new(Mutex::new(None));
        let (fin2, out2) = (fin.clone(), out.clone());
        std::thread::spawn(move || {
            let ok = child.wait().map(|s| s.success()).unwrap_or(false);
            let wall_ms = t0.elapsed().as_millis() as u64;
            let (rows, digest) = data::digest_file(&out2).unwrap_or((0, String::new()));
            *fin2.lock().unwrap() = Some(Finished { wall_ms, ok, rows, digest });
        });
        Ok(Run { pid, t0, bytes, temp, out, rows: RowCounter::default(), last_rss: 0, peak_rss: 0, peak_temp: 0, fin, stop_feed })
    }

    fn rss_kib(&self) -> Option<u64> {
        let o = Command::new("ps").args(["-o", "rss=", "-p", &self.pid.to_string()]).output().ok()?;
        String::from_utf8_lossy(&o.stdout).trim().parse().ok()
    }

    /// Take one measurement (called every ~100 ms until `finished()`).
    pub fn sample(&mut self) -> Sample {
        if let Some(r) = self.rss_kib() {
            self.last_rss = r;
            self.peak_rss = self.peak_rss.max(r);
        }
        let temp = data::du(&self.temp);
        self.peak_temp = self.peak_temp.max(temp);
        Sample {
            t_ms: self.t0.elapsed().as_millis() as u64,
            bytes: self.bytes.load(Relaxed),
            rss_kib: self.last_rss,
            temp,
            rows: self.rows.poll(&self.out),
        }
    }

    pub fn finished(&self) -> Option<End> {
        let g = self.fin.lock().unwrap();
        let f = g.as_ref()?;
        Some(End { t_ms: f.wall_ms, ok: f.ok, peak_rss_kib: self.peak_rss, peak_temp: self.peak_temp, rows: f.rows, digest: f.digest.clone() })
    }

    pub fn kill(&self) {
        self.stop_feed.store(true, Relaxed);
        let _ = Command::new("kill").arg(self.pid.to_string()).stderr(Stdio::null()).status();
    }

    pub fn cleanup(&self, work: &Path) {
        let _ = std::fs::remove_dir_all(work);
    }
}
