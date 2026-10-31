//! `--progress`: one live status line on stderr (~10 Hz), only when stderr is a terminal.
//! Bytes in are counted by wrapping stdin; rows in/out come from `engine::progress`. Nothing here touches stdout.
use std::io::{IsTerminal, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

static BYTES_IN: AtomicU64 = AtomicU64::new(0);

/// Pass-through reader that counts the bytes it hands out.
pub struct Counting<R>(pub R);

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read(buf)?;
        BYTES_IN.fetch_add(n as u64, Relaxed);
        Ok(n)
    }
}

/// Current resident set size in bytes of this process, if the platform tells us.
fn rss_bytes() -> Option<u64> {
    if let Ok(s) = std::fs::read_to_string("/proc/self/statm") {
        let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
        return Some(pages * 4096);
    }
    // macOS and others: `ps` is the portable fallback (same call the demo uses).
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    let kib: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kib * 1024)
}

fn human(n: u64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.2} GB", f / 1e9)
    } else {
        format!("{:.1} MB", f / 1e6)
    }
}

fn line(t0: Instant, last: &mut (Instant, u64), show_bytes: bool) -> String {
    let now = Instant::now();
    let b = BYTES_IN.load(Relaxed);
    let dt = now.duration_since(last.0).as_secs_f64().max(1e-6);
    let mbs = (b.saturating_sub(last.1)) as f64 / 1e6 / dt;
    *last = (now, b);
    let rss = rss_bytes().map(|r| format!("{:.0} MiB", r as f64 / 1048576.0)).unwrap_or_else(|| "?".into());
    let bytes = if show_bytes { format!("in {} · {:.0} MB/s · ", human(b), mbs) } else { String::new() };
    format!(
        "[qe] {:>6.1}s · {}{} rows in · {} rows out · RSS {}",
        t0.elapsed().as_secs_f64(),
        bytes,
        engine::progress::rows_in(),
        engine::progress::rows_out(),
        rss
    )
}

pub struct Progress {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Progress {
    /// Starts the status line, or returns None when stderr is not a terminal.
    pub fn start(show_bytes: bool) -> Option<Progress> {
        // QE_PROGRESS_FORCE exists only so tests can exercise the status line without a pty.
        if !std::io::stderr().is_terminal() && std::env::var_os("QE_PROGRESS_FORCE").is_none() {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let handle = std::thread::spawn(move || {
            let t0 = Instant::now();
            let mut last = (t0, 0u64);
            let mut err = std::io::stderr();
            while !s.load(Relaxed) {
                let l = line(t0, &mut last, show_bytes);
                let _ = write!(err, "\r\x1b[K{l}");
                let _ = err.flush();
                std::thread::sleep(Duration::from_millis(100));
            }
            let l = line(t0, &mut last, show_bytes);
            let _ = write!(err, "\r\x1b[K{l}\n");
        });
        Some(Progress { stop, handle })
    }

    pub fn finish(self) {
        self.stop.store(true, Relaxed);
        let _ = self.handle.join();
    }
}
